# People parts ahead of time

List: people parsing runs ahead of time in the background (the photos around the open one, new imports with faces, then every photo with faces while idle), so opening a photo finds its part masks ready in milliseconds; paused while you work, on low battery and in Low Power Mode; Settings ▸ Performance ▸ People Parts (issue #125). Branch `perf/masks-ahead-of-time`.

## What it does

People parsing (`fork/people-parsing.md`) takes seconds a photo. This queue runs it before the photo is needed, into
people parsing's own disk cache, so a request (opening the photo, a portrait preset, `parsing.run`) finds the parse
there and no model runs.

- **Which photos, in order**: the photo open in the filmstrip and its neighbours (4 ahead in the direction of travel,
  2 behind, nearest first); then photos imported since the library opened that have faces (newest import first); then,
  while the user is idle, every other photo with faces, newest first. A neighbour without faces is parsed only while
  the face scan has not looked at it yet. A photo is parsed once per model set and once per app start (a failure is
  not retried until the next start; a photo whose file or faces change is done again).
- **Never in the way**: one photo at a time on one worker thread, so the GPU turn is given up between photos. Nothing
  new starts while a slider is dragged, for 1.5 s after the user last dragged, typed or scrolled, while the loupe waits
  for its render (`prep::foreground_busy`), or while another parse (a request) runs; a model run going then is stopped
  (unless a request waits for that very photo) and run again once the user lets go. The idle sweep runs only at the
  face scan's "normal" pace (the user idle for 3 s, or working in another app).
- **Power**: nothing in Low Power Mode or on battery below 40 % (`pmset`, read at most once a minute on its own thread).
- **The setting**: Settings ▸ Performance ▸ People Parts ▸ Masks, "Find people's parts ahead of time", on by default
  (`ui.json` `settings.parseAhead`, kept on each Mac, not synced). Off stops the photo going.
- **One row**: "Finding people parts ahead" with a count (n of m) in the activity stack while there is work; the
  parse itself has no row of its own. Its ✕ stops the photo going and empties the queue until another photo is opened.
- **Opening a parsed photo** takes milliseconds: an index beside the cache (`ahead-index.json`) files each parse's key
  by what the parse depends on (the file's content, the render version, the face regions and their names, whether
  YuNet is installed, the model set's version), so a request reads the parse without rendering the photo first. A
  request that parses a photo files it there too. A photo whose identity is not listed, or whose parse has left the
  cache, is parsed as before.
- **Cache budget**: people parsing's 512 MB cap now removes the parses used least recently (each lookup marks the file
  as used). The idle sweep stops at 90 % of the cap, so it never pushes out parses for its own; the neighbours and new
  imports may. A parse file is 100–230 KB, so the cap holds a few thousand photos.
- **Commands**: `parsing.ahead.pump {pace, backward?, enabled?, wait?, timeoutMs?}` (the app calls it every frame;
  `wait` keeps going until the queue is done) and `parsing.ahead.status` → `{enabled, state, active, queued, queue,
  working, parsed, cached, skipped, failed, power, indexFile}`; `state` is one of `off`, `battery`, `lowPower`,
  `noModels`, `noCache`, `stopped`, `paused`, `working`, `done`, `idle`.
- **Never crashes**: a damaged or hostile index is started afresh (keys that could leave the folder are refused); a
  failed render, detection or run counts as failed; a panic in the worker is a failure, not a crash.

How it does (6 of techneissa's A7 V raws with people, copied to a scratch library; M4 Pro, SegFormer set, four runs
while other builds kept the load average between 20 and 56; 2026-11-09):
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
- Fork files of people parsing: `crates/engine/src/people_parsing.rs` (`mod ahead`; `remembered` marks the file used;
  `spawn` asks `shown` for its row), `crates/engine/src/cmd/parsing.rs` (`prepare` split into `prepare_job` /
  `Prepare::run`, the very code of PR #122, so the worker renders and finds faces as a request does; `parse_photo`
  reads a parse the index knows and files the ones it makes).

## When upstream merges

Not an upstream feature. When `feat/103-hair-edges` (PR #113) merges: its matting edges file parses under the set's
version plus `+matting1`. `PeopleParsing::ahead_version` (`people_parsing/ahead.rs`) must then return that same
version (the set's version plus the matting part when BiRefNet Matting is chosen for people), so the index tells
matted from plain parses. Until it does, nothing wrong is shown: a matted parse is never read through the index
(its `model_version` differs), so the photo is parsed by the usual path.

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
