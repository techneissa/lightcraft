# People parts ahead of time

List: people parsing runs ahead of time in the background (the photos around the open one, new imports with faces, then every photo with faces while idle), so opening a photo finds its part masks ready in milliseconds; paused while you work, on low battery and in Low Power Mode; Settings ▸ Performance ▸ People Parts (issue #125). Branch `perf/masks-ahead-of-time`.

## What it does

People parsing (`fork/people-parsing.md`) takes seconds a photo. This queue runs it before the photo is needed, into
people parsing's own disk cache, so a request finds the parse there and no model runs. The requests are `parsing.run`, `parsing.mask` and portrait
retouching's face search (`fork/retouch-masks.md`); Select People does not ask for a parse yet (see When other
branches merge).

- **Which photos, in order**: the photo open in the filmstrip and its neighbours (4 ahead in the direction of travel,
  2 behind, nearest first); then photos imported since the library opened that have faces (newest import first); then,
  while the user is idle, every other photo with faces, newest first. A neighbour without faces is parsed only while
  the face scan has not looked at it yet. A photo is parsed once per model set and People edge, and once per app
  start (a failure is not retried until the next start; a photo whose file or faces change is done again).
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
  removed: `Catalog::aspects_since`), at most every 3 s, never while the work is held back; edits and ratings leave it
  as it is. The app asks for a frame only while there is work the pace allows, now and then while it is held back, and
  once when the user will count as idle while only photos for an idle user wait.
- **Power**: nothing in Low Power Mode or on battery below 40 % (`pmset`, read at most once a minute on its own thread).
  On the Mac nothing starts before the first reading is in (`checkingPower`).
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
- **Commands**: `parsing.ahead.pump {pace, backward?, enabled?, wait?, timeoutMs?}` (the app calls it every frame;
  `wait` keeps going until the queue is done, for 3 s at most, since it holds the UI thread: an agent calls again while
  the state is `working`, `paused` or `loading`) and `parsing.ahead.status` → `{enabled, state, active, queued, left,
  queue, working, parsed, cached, skipped, failed, power, indexFile}` (`left`: the queued photos the pace allows now);
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

## Branch

`perf/masks-ahead-of-time`

## Own files

`crates/engine/src/parse_ahead.rs`, `crates/engine/src/parse_ahead/{queue,index,power,worker}.rs`,
`crates/engine/src/people_parsing/ahead.rs`, `crates/engine/src/cmd/parse_ahead.rs`,
`crates/engine/src/tests/parse_ahead.rs`, `crates/engine/examples/parse_ahead.rs`, `crates/ui-egui/src/parse_ahead.rs`,
`crates/ui-egui/src/tests/parse_ahead.rs`, `fork/parse-ahead.md`, `fork/tests.d/parse-ahead.txt`

## Hooks in official files

- `crates/engine/src/lib.rs`: `pub mod parse_ahead`, field `Session::parse_ahead`. `crates/engine/src/cmd.rs`:
  `mod parse_ahead`, its specs. The `mod` lines of `crates/engine/src/tests.rs`, `crates/ui-egui/src/tests.rs` and
  `crates/ui-egui/src/lib.rs`.
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
- People's part masks (skin, hair, eyes, lips, clothes) are found in the background before you need them: for the photos around the one you have open, for new imports with faces, and, while the computer is idle, for every photo with faces. Opening such a photo finds its masks ready at once.
- It pauses while you edit, and stops on battery below 40 % and in Low Power Mode. Settings ▸ Performance ▸ People Parts turns it off.
