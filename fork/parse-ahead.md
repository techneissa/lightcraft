# People parts ahead of time

List: people parsing runs ahead of time in the background for photos likely to be retouched (the photo open in Edit and, once you edit, the photos ahead of it; photos with portrait settings; picked or starred photos with faces while idle; nothing while culling), so opening a photo finds its part masks ready in milliseconds; paused while you work, on low battery and in Low Power Mode; Settings ▸ Performance ▸ People Parts (issues #125, #200). Branch `perf/masks-ahead-of-time`.

## What it does

People parsing (`fork/people-parsing.md`) takes seconds a photo. This queue runs it before the photo is needed, into
people parsing's own disk cache, so a request finds the parse there and no model runs. The requests are `parsing.run`, `parsing.mask` and portrait
retouching's face search (`fork/retouch-masks.md`); Select People does not ask for a parse yet (see When other
branches merge).

- **Which photos, in order** (issue #200: only photos likely to be retouched): the photo open in the Edit view (the
  loupe with an Edit, Crop, Remove, Masking, Red Eye or Portrait panel) once it has stayed open 4 s (stepping on
  sooner parses nothing, and a run going for a photo stepped past ends; a portrait tool opened sooner asks for the
  parse itself), and, once the user has edited there since
  coming to it (an edit replaces the open photo's develop settings) or the open photo has portrait settings, the 4
  photos ahead of it in the direction of travel, nearest first; none behind (stepped past already). Then photos with
  portrait settings, whose drawing needs the parse (newest first, also while the user is around). Then, while the user
  is idle, photos with faces they kept: picked, or one star or more, and not rejected (newest first). Nothing else: a
  new import, a photo with faces nobody picked or starred, a rejected photo. While culling (any other view: the
  grid, the loupe without an Edit panel, Compare, Survey) nothing near the open photo is parsed and the early check
  for faces (below) waits; leaving the Edit view forgets the edits, so coming back parses the open photo alone until
  the next edit; an edit, or portrait settings on the open photo, count as staying (no 4 s wait). While the open
  photo waits, `state` is `paused` (the app keeps pumping) and no row shows. A neighbour without faces is parsed only while the face scan has not looked at it yet. A photo is
  parsed once per model set and People edge, and once per app start (a failure is not retried until the next start; a
  photo whose file or faces change is done again).
- **Never in the way**: one photo at a time on one worker thread, so the GPU turn is given up between photos. Nothing
  new starts while a slider is dragged, for 1.5 s after the user last dragged, typed or scrolled (also in a window
  without the keyboard), while the loupe waits for its render (`prep::foreground_busy`), while an export, print, book
  or slideshow output runs (an `export` row in the activity stack), or while another parse (a request) runs; a model
  run going then is stopped (unless a request waits for that very photo) and run again once the user lets go. The
  worker waits again before each step (the render, the face detection, the model run), since a render can't be
  stopped once it runs. The idle sweep runs only at the face scan's "normal" pace (the user idle for 3 s, or working
  in another app).
- **Requests go first**: a request for another photo stops the queue's model run at once, so a synchronous
  `parsing.run` on the UI thread never waits for it (the queue runs that photo again later). A request for the photo
  the queue is parsing joins its run; if that run is stopped (✕, the setting), the request runs the photo itself. A
  stopped queue run keeps no result and leaves no message in `parsing.status`.
- **Light on the UI thread**: the pump never loads YuNet (it reads only whether the file is there, and its size and
  time); the worker loads it. The worker also reads, lists and writes the index beside the cache. The queue is
  rebuilt only when what it reads of the catalog changes (faces and other metadata, dates, files, photos added or
  removed, ratings, flags, develop settings: `Catalog::aspects_since`), at most every 3 s, never while the work is held
  back; labels and analysis leave it as it is. The app asks for a frame only while there is work the pace allows, now and then while it is held back, and
  once when the user will count as idle while only photos for an idle user wait.
- **Power**: nothing in Low Power Mode or on battery below 40 % (`pmset`, read at most once a minute on its own thread).
  On the Mac nothing starts before the first reading is in (`checkingPower`).
- **The Portrait icon's early check** (`parse_ahead/peek.rs`, issue #167): when a photo opens, it finds out whether
  the photo has any face, so the Portrait icon is dimmed for a photo of nobody before the tab is opened
  (`fork/retouch-foundation.md`). The answer is a hint, kept apart from the face search's results
  (`FaceStore::hint_nobody`, keyed like them) and read only by the icon and the tab: the face search, renders and
  exports ignore it, so a photo YuNet misses but the people parse finds is still searched and retouched, and the tab's
  "Search Again" (`retouch.search`) runs the real search, whose answer replaces the hint. A photo with faces keeps
  nothing, and a photo with face regions in the catalog is left alone. The answer comes from the parse the index
  already knows for the photo (its people; no model), else from a render of the photo at 640 pixels and only the face
  detector (YuNet, or the installed provider): not the parse, not the face mesh; the YuNet it loads is the session's
  (`face_detect::detector_at` keeps it, the session's own call then offers it the GPU), so the net is built once. One
  photo at a time on a worker of its own that waits before each step while the user drags, types or scrolls, while the
  loupe waits for its render and while an export runs, and stops when the user moves to another photo (the same photo
  is checked again when it is open again); a photo is checked once per key, and a pump that has nothing to do for the
  open photo and the catalog's revision skips it without hashing the photo. It does not wait for a parse going, needs
  no parse models, and stands still when the setting is off, in Low Power Mode and on battery below 40 %, as the queue
  does. Threads have no priority knob here (no `unsafe`): "low priority" is the holds, one at a time and the small
  render.
- **The setting**: Settings ▸ Performance ▸ People Parts ▸ Masks, "Find people's parts ahead of time", on by default
  (`ui.json` `settings.parseAhead`, kept on each Mac, not synced). Off stops the photo going.
- **One row**: "Finding people parts ahead" with a count (n of m) in the activity stack while there is work; the
  parse itself has no row of its own. Its ✕ stops the photo going and empties the queue until another photo is opened.
- **A parsed photo's request** takes milliseconds: an index beside the cache (`ahead-index.json`) files each parse's
  key by what the parse depends on (the file's content, the render version, the face regions and their names, the
  installed YuNet's file, and the version the parse is filed under: the model set's, plus `+matting1` or `+vision1`
  when the People edge is BiRefNet Matting or Apple Vision), so `parsing.run` and `parsing.mask` read the parse
  without rendering the photo or finding its faces first (`parsing.mask` then renders the photo only to draw the
  mask's edges). One function gives that version (`PeopleParsing::version_of`: `Choice::version`, so
  `PeopleParsing::parse_as` and the retouch face store's key, and `PeopleParsing::ahead_version`, so the identity and
  `crate::parse_ahead::ready`, all call it), and a request files the key its parse was made under, so changing the
  People edge never reads a parse made for the other. A photo whose identity is not listed, or whose parse has left
  the cache, is parsed as before.
- **Portrait retouching** asks the same way: its face search takes what the index knows of the photo
  (`crate::parse_ahead::known`, on the thread that sets the search up, reading nothing but the index in memory) and,
  on its worker, reads that parse (`Known::read`), even when the picture and faces it found would key it a hair
  apart; else it parses with `parse_as` and files the key (`Known::file`), so `parsing.mask` and the queue then find
  it. The worker still renders the photo and runs YuNet (loaded there, never on the UI thread), for the face picker's
  thumbnails and each face's five landmarks; no people-parsing model runs for a photo parsed before.
- **Cache budget**: people parsing's 512 MB cap now removes the parses used least recently (each lookup marks the file
  as used). The idle sweep stops at 90 % of the cap, so it never pushes out parses for its own; the neighbours and new
  imports may. A parse file is 100–230 KB, so the cap holds a few thousand photos.
- **Commands**: `parsing.ahead.pump {pace, backward?, culling?, enabled?, wait?, timeoutMs?}` (the app calls it every frame,
  with `culling` outside the Edit view;
  `wait` keeps going until the queue is done, for 3 s at most, since it holds the UI thread: an agent calls again while
  the state is `working`, `paused` or `loading`) and `parsing.ahead.status` → `{enabled, culling, editing, state, active,
  queued, left, queue, working, parsed, cached, skipped, failed, power, indexFile}` (`editing`: the user has edited
  since coming to the Edit view) (`left`: the queued photos the pace allows now);
  `state` is one of `off`, `battery`, `lowPower`, `checkingPower`, `noModels`, `noCache`, `stopped`, `loading` (the
  worker is reading the index), `paused`, `working`, `done`, `idle`.
- **Never crashes**: a damaged or hostile index is started afresh (keys that could leave the folder are refused); a
  failed render, detection or run counts as failed; a panic in the worker is a failure, not a crash.

How it does (6 private test raws with people, copied to a scratch library; a test Mac, SegFormer set, four runs
while other builds were running; 2026-11-09):
- Parsed ahead: the open photo and its 4 neighbours in 11–50 s (2–10 s a photo, the first including loading the
  models), the sixth at the idle pace in 2–10 s. The parses a request makes afterwards have the same cache keys.
- Opening each of the 6 afterwards: no model run, 0.1–6 ms each. Before, a request took about 1 s even when its parse
  was cached: it rendered the photo and found its faces to make the cache key.
- After a restart: all 6 skipped without a render, and opening each read its parse from disk in 0.1–4 ms.
- A parse file is 100–230 KB (132 bytes for a photo with nobody in it).

## What one parse costs (issue #200)

`crates/engine/examples/parse_profile.rs` parses photos as the worker does (`Session::parse_ahead_one`, test hooks
only) and splits each parse at the marks of `lightcraft_parsing::profile` (the render, the face detection, the model
load, each model, the post-processing, the cache write), in wall and CPU time (CPU: the whole process, every thread):

```bash
LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features test-hooks \
    --example parse_profile -- OUT_DIR [--set segformer|bisenet|sapiens2] [--rounds N] PHOTO...
```

A test Mac, other builds running (load average 8–48), 7 CC0 portraits (18–24 MP JPEGs) and 2 CC0 raws (Sony, 24 and
33 MP), models warm (the second round), mean per photo, before the change below. Wall / CPU in ms:

| Step | SegFormer (Core ML) | BiSeNet (candle, mesh on Core ML) | Sapiens2 (candle, B0 and B3 on Core ML) |
| --- | ---: | ---: | ---: |
| Render at 2048 px (JPEG 54–300, raw 600–690 wall) | 224 / 271 | 167 / 242 | 159 / 258 |
| Face detection (YuNet) | 51 / 13 | 39 / 7 | 22 / 7 |
| Clothes parser: resize to its grid | 31 / 31 | 27 / 27 | 28 / 28 |
| Clothes parser: model | 172 / 133 | 700 / 586 | 157 / 118 |
| Sapiens2 | — | — | 1165 / 322 |
| Telling people apart, body planes | 9 / 8 | 8 / 8 | 8 / 8 |
| Face parser, face mesh, face planes | 59 / 58 | 94 / 49 | 48 / 47 |
| Cache write and prune | 1 / 1 | 1 / 1 | 1 / 1 |
| Loading the set (once a minute of use) | 484 / 352 | 318 / 313 | 2211 / 2193 |

- The models cost little on Core ML except for one thing: Core ML plans a model on the GPU again whenever its input
  shape changes from the call before. The clothes parser's canvas follows the photo's shape, so a portrait after a
  landscape (or a 4:3 photo after a 3:2 one) paid about 300 ms of wall and CPU time, more than four runs of the model:
  68 ms and 28 ms CPU in one shape, 98–134 ms and 100–172 ms CPU a call when two shapes alternate
  (`crates/parsing/examples/coreml_shapes.rs`). The face parser and mesh did the same with their batch size. Fixed:
  each model keeps one input shape (`fork/coreml.md`).
- After that, the render is the largest step: the raw decode (macOS's, 570–670 ms for a 24–33 MP Sony raw at the
  2560 px preview size) or the JPEG decode (240–300 ms for 24 MP). The parse of the open photo already reuses the
  decode the loupe made; the neighbours' are decoded for the parse and dropped.
- BiSeNet's clothes parser (SegFormer-B2) ran on candle at 700 ms; Sapiens2 at 1.2 s. Both run on Core ML since #216
  (`fork/coreml.md`, What #216 changed): B2 877 → 122 ms wall, 752 → 42 ms CPU; Sapiens2's step 1352 → 751 ms wall,
  339 → 251 ms CPU.

Culling, measured with `crates/engine/examples/parse_cull.rs` (the 14 photos below, faces found first, a step every
2 s as the arrow keys do, pumped as the app pumps: paused 1.5 s after each key, then light), same Mac, load average
14–25:

| Walk | Before | After |
| --- | --- | --- |
| Outside the Edit view, a step every 2 s | 17 model runs started (12 finished), 6.9 s CPU | none, 0.1 s CPU |
| In the Edit view, no edit, a step every 2 s | 17 started (11 finished), 7.2 s CPU | none, 1.3 s CPU (the early face check on two photos without face regions) |
| In the Edit view, no edit, a step every 6 s | 14 started, 14 finished, 8.4 s CPU | 14 and 14, 9.3 s CPU |

After the change (same Mac and photos plus 5 CC0 group photos of 2 to 26 people, before and after run in turn three
times, load average 25–39), mean per photo over 14 photos, CPU: 565–623 ms → 490–521 ms. By step, CPU: clothes parser
139–157 → 80–83 ms, face parser 63–72 → 48–50 ms, face mesh 29–33 → 14 ms. On the 9 photos without the groups, the
clothes parser went from 191–207 ms wall and 127–134 ms CPU to 113–116 ms and 30–41 ms. Wall times swung with the other
builds (the render alone moved 125–330 ms between runs); CPU time held.

Masks against the parses before (`crates/parsing/examples/parse_agree.rs` on the two cache folders; each part over
everyone at the parse's size, cut at 0.5): at least 99.97 % of pixels the same in every part, IoU at least 0.997 for
every part covering more than 0.5 % of the photo (0.94 for a shoe of a few hundred pixels), the same people, landmarks
within 0.001 px. Two runs of the code before give identical parses. Sapiens2: 99.994 % and IoU 0.9995 at worst;
BiSeNet: identical.

## Branch

`perf/masks-ahead-of-time`

## Own files

`crates/engine/src/parse_ahead.rs`, `crates/engine/src/parse_ahead/{queue,index,peek,power,worker}.rs`,
`crates/engine/src/people_parsing/ahead.rs`, `crates/engine/src/cmd/parse_ahead.rs`,
`crates/engine/src/tests/parse_ahead.rs`, `crates/engine/src/tests/parse_ahead_peek.rs`, `crates/engine/examples/parse_ahead.rs`,
`crates/engine/examples/parse_profile.rs`, `crates/engine/examples/parse_cull.rs`, `crates/ui-egui/src/parse_ahead.rs`,
`crates/ui-egui/src/tests/parse_ahead.rs`, `fork/parse-ahead.md`, `fork/tests.d/parse-ahead.txt`

## Hooks in official files

- `crates/engine/Cargo.toml`: the `parse_profile` (`test-hooks`) and `parse_cull` examples, and `rustix` (their CPU
  clock) as a dev-dependency.
- `crates/engine/src/lib.rs`: `pub mod parse_ahead`, field `Session::parse_ahead`. `crates/engine/src/cmd.rs`:
  `mod parse_ahead`, its specs. The `mod` lines of `crates/engine/src/tests.rs`, `crates/ui-egui/src/tests.rs` and
  `crates/ui-egui/src/lib.rs`.
- `crates/ui-egui/src/full_decode.rs`: `editing` is `pub(crate)` (the pump's `culling`).
- `crates/ui-egui/src/app/frame.rs` (the per-frame pump), `crates/ui-egui/src/dialogs/settings/performance.rs` (the
  switch), `crates/ui-egui/src/state/settings.rs` (`parse_ahead`, on by default),
  `crates/ui-egui/src/settings_sync/rules.rs` (`parseAhead` left out of sync).
- `crates/engine/src/cmd/face_detect.rs`: `detector_path` split out of `detector`; `detector_file` (the installed
  YuNet's file and a stamp of it, from the file system only) and `detector_at` (YuNet loaded on a worker thread).
- Fork files of people parsing: `crates/engine/src/people_parsing.rs` (`mod ahead`; `remembered` marks the file used;
  `spawn` asks `shown` for its row and keeps no result of a stopped queue run; `version_of`, the one version function;
  `parse_as` parses with a `Choice` taken earlier and says the version and key the parse was filed under; a request stops the queue's runs of other photos
  and runs a photo itself when the queue's run it joined stops; a run that finished between the cache lookup and the
  runs' lock is not run again), `crates/engine/src/cmd/parsing.rs` (`prepare` split into `prepare_job` /
  `prepare_ahead` and the steps `Prepare::render` / `Rendered::detect`, so the worker renders and finds faces as a
  request does and waits between the steps; `parse_photo` and `parsing.mask` read a parse the index knows and file
  the ones they make). `crates/engine/src/retouch_input.rs` (`retouch_finder`: `prepare_ahead`, and `known` for the
  search) and `crates/engine/src/retouch_input/parts.rs` (`PartsFinder::find` reads and files through `Known`).

## When other branches merge

Not an upstream feature; everything it touches is in this fork. Hair edges (#113) and Apple Vision edges (#120) are
merged: their parses are filed under `+matting1` / `+vision1`, the version `PeopleParsing::version_of` gives. Portrait
retouching from the people parse (#122) is merged and wired: its face search reads and files through
`crate::parse_ahead::known` (What it does). Select People masks, once they ask for a parse, go the same way:
`crate::parse_ahead::ready` first (no render when the photo was parsed ahead), then `PeopleParsing::parse_as` and
`crate::parse_ahead::remember` with the key it returns; keep the step split (`Prepare::render`, `Rendered::detect`)
and `prepare_ahead` for a worker.

## Tests

```bash
cargo test -p lightcraft-engine --lib parse_ahead
cargo test -p lightcraft-engine --lib people_parsing
cargo test -p lightcraft-ui-egui --lib tests::parse_ahead
```

On real photos (models from the shared folder): `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p
lightcraft-engine --example parse_ahead -- OUT_DIR PHOTO...` (give it copies: it imports them in place).

## What's New entry

### People Parts Ahead of Time
- People's part masks (skin, hair, eyes, lips, clothes) are found in the background before you need them, for the photos you are likely to retouch: the photo open in Edit and, once you edit, the next ones; photos with portrait settings; and, while the computer is idle, picked or starred photos with faces. Opening such a photo finds its masks ready at once. Nothing is done for photos you only step past while culling.
- It pauses while you edit, and stops on battery below 40 % and in Low Power Mode. Settings ▸ Performance ▸ People Parts turns it off.
