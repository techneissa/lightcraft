# Performance: idle memory and CPU

List: The app lets go of AI models and caches it is not using (one rule for all of them), hands freed raw buffers back to macOS, and draws no frames while nothing changes. Issue #304, branch `perf/304-idle-memory`.

## What it does

- **One idle rule** (`crates/engine/src/idle.rs`). A holder registers once with a function that lets its memory go and marks each use (`Tracker::using` around a run, never unloaded meanwhile; `Tracker::touch` where there is no run to bracket). One thread (`lc-idle`), started by the first holder, sleeps until the nearest deadline and calls the function when `MODEL_IDLE` (60 s) has passed since the last use, `BACKGROUND_IDLE` (15 s) while the window is in the background. With nothing loaded it sleeps for good: nothing is polled. The function says it freed something, had nothing, or found the holder in use (asked again after a whole idle time). The decisions are in `Registry::sweep`, which takes the time as an argument.
- **Holders on it:** Relight's depth model (about 3.4 GB of GPU buffers once it ran; clock starts when a depth run ends) and Relight's kept fields (all but the newest go after no other photo's field was asked for), Learn My Edits' image model, the denoise models, the people-parsing set, BiRefNet Matting, LaMa (candle; the Core ML copy stays, see `lama_fill/coreml.rs`). They were three 10-second polling watcher threads, two models with no unload at all, and SAM 3; the SAM 3 worker keeps its own `recv_timeout` on the same `MODEL_IDLE` (was 10 minutes) and sleeps for good while nothing is loaded. A model reloads on its next use.
- **Heap pages after big work.** A decode that ends (`WorkGate` permit dropped) notes work; 4 s without more (`TRIM_QUIET`) calls `memory::release()`. On macOS 27 `malloc_zone_pressure_relief` returns nothing for freed large blocks (they stay in the footprint, up to 2 GB), so the desktop app restarts itself once with `MallocLargeCache=0` (`apps/lightcraft/src/alloc_release.rs`; libmalloc reads it only at process start; `LIGHTCRAFT_KEEP_MALLOC_CACHE=1` turns it off), which frees them at once.
- **Background window** (`crates/ui-egui/src/background.rs`). No keyboard focus, minimized or hidden counts as the background. After 20 s there the full-size originals (`MediaCache::release_originals`), the renderer's stage caches, the full-size picture and the GPU's recycled buffers go (`Renderer::release_for_background`), once; coming back lets the next frame take what it needs. The textures on screen and the preview cache stay.
- **No frames for nothing.** `fs_cached` (the folder tree's file-system answers) wakes the window only when its answer changed (it woke it after every listing and spawned `lc-fs-list` threads again and again); the screen's colour profile is read every 10 s and not while hidden (was every second); face recognition with nothing to do looks again every 30 s (was 5 s). `ui.inspect` → `perf.repaintCauses` lists who asked for the last frame (`file:line`).
- **Cheaper frames.** `macraw::supports` (asked for each look-ahead photo on every frame) remembers its answer by camera model; `PeopleParsing::installed_lately` remembers what the models folder holds for 3 s (a finished download clears it) where `model_set()` and the parse-ahead pump checked the file system on every frame.

## Branch

`perf/304-idle-memory`

## Own files

`crates/engine/src/idle.rs`, `crates/ui-egui/src/background.rs`, `crates/ui-egui/src/render/background.rs`, `fork/idle-memory.md`, `fork/tests.d/idle-memory.txt`

## Hooks in official files

`crates/engine/src/lib.rs` (`pub mod idle`), `memory.rs` (a permit's drop notes work), `media/cache.rs` (`release_originals`), `relight.rs`, `learn.rs`, `denoise/products.rs`, `people_parsing.rs` (+ `ahead.rs`, `download.rs`), `hair_edges.rs` + `hair_edges/model.rs`, `lama_fill.rs`, `segment/worker.rs`, `crates/macraw/src/macos.rs`, `crates/ui-egui/src/` (`lib.rs`, `app.rs`, `app/frame.rs`, `app/prefs.rs`, `render.rs`, `control.rs`, `dialogs/faces.rs`, `dialogs/import/body.rs`, `panels/left/local.rs`), `apps/lightcraft/src/` (`main.rs`, `alloc_release.rs`)

## Tests

`fork/tests.d/idle-memory.txt`: a model goes after its idle time and loads again on the next use; a run in progress is never unloaded and the clock starts when it ends; a holder found in use is asked again after a whole idle time; the background shortens the time; the allocator's pages are asked back once the work is quiet; Relight keeps only the newest field and does not touch a store in use.
