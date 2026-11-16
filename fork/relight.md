# Relight

List: Relight, a Develop section with a light you drag on the photo that brightens what is near it in depth and faces it, from the photo's own depth map or an on-device depth model, live while you drag (issue #288). Branch `feat/288-relight`.

## What it does

- **Relight** section in Develop (between Lens Blur and Calibration): a small depth picture of the photo with the light on it, a Near–Far track for the light's depth, **Amount**, **Softness**, **Warmth**, and an **Advanced** flyout with **Darken Surroundings** and **Wrap**. While the section is open and Amount is above 0, the light's handle sits on the photo and can be dragged there; one drag is one undo step.
- **Depth** is found once per photo, off the UI thread, as an activity row ("Finding depth"): the photo's own depth map when it has one (an iPhone Portrait HEIC), else the depth model. Until it is ready the section shows Finding Depth…; without the model it offers Download Depth Model (asks once, then shows progress).
- **The depth model** is Depth Anything 3 Mono Large (code and weights Apache-2.0, about 1.34 GB), run in pure Rust with candle (Metal on macOS). It downloads into the shared models folder (`da3-mono-large/`) and is never in the repository; `LIGHTCRAFT_DEPTH_MIRRORS` adds mirrors. Without the `sam` feature the model is not built in and only files' own depth maps work.
- **The field** (`crates/relight/src/field.rs`): the depth is upsampled to a 1600-pixel guide of the photo by a joint bilateral filter that takes a weighted median across depth steps, so edges take one side (no halo, no stair step). Normals come from a guided-filtered depth; slopes steeper than 80° (depth steps, not surfaces) are left out and the rest is smoothed.
- **The light** (`crates/relight/src/light.rs`): a point at the handle's place on the photo and the track's depth. Each pixel gets light by its 3D distance to the point (Softness widens the falloff) and a gentle facing term (Wrap softens it), normalized to the brightest pixel. It works in scene-linear light, as an exposure before the tone curve. Highlights roll off to a ceiling under white and deep shadows are held back, so noise isn't lifted. Warmth is a colour temperature from about 12000 K to 2800 K, weighted by the light's share of each pixel. Along depth edges the shading is refit against the photo's own luminance.
- **Preview equals export:** the light's place is in normalized photo coordinates (before lens corrections and crop, as a mask's) plus a depth, and the shading is made once per light at field resolution and read by each render at its own pixels, on CPU and GPU.
- **Speed:** a drag makes one shading per position (8–13 ms) and each render's planes (3.4 ms at 2.5 MP).
- **Settings:** `fork_relight` in develop settings (not written while off), fork catalog format 11. Undo, presets and Copy Settings (Relight group), Sync and the control channel carry it: the sliders are controls `relight.amount`, `.softness`, `.warmth`, `.x`, `.y`, `.depth`, `.darken`, `.wrap`.
- **Commands** (and so MCP): `relight.status {id?}` (depth ready, being found, its source, the model's state) and `relight.downloadModel {confirm: true}`.
- Tuning outside the app: `cargo run --release -p lightcraft-depth --example depth -- <model dir> photo.jpg depth.png --dump dump/`, then `cargo run --release -p lightcraft-relight --example relight -- photo.jpg dump/ out [x y depth amount softness warmth darken wrap]`.

## Branch

`feat/288-relight`, from `custom`.

## Own files

`crates/depth/` (new crate: the depth model), `crates/relight/` (new crate: field and light), `crates/develop/src/relight.rs`, `crates/pipeline/src/relight.rs`, `crates/pipeline/src/tests_relight.rs`, `crates/engine/src/relight.rs`, `crates/engine/src/cmd/relight.rs`, `crates/ui-egui/src/panels/edit/relight.rs`, `crates/ui-egui/src/panels/detail/relight.rs`, `fork/relight.md`, `fork/tests.d/relight.txt`

## Hooks in official files

- `Cargo.toml`: the two workspace dependencies. `xtask/src/layers.rs`: their layers (depth L3, relight L1). `NOTICE`: the depth model's code and licence.
- `crates/develop/src/lib.rs`: `pub mod relight;`. `settings.rs`: the `relight` field and its default. `controls.rs`: `Section::Relight` and the relight specs in get, set, find and the section list. `presets.rs`: `SettingsGroup::Relight`.
- `crates/catalog/src/fork_format.rs`: version 11. `journal.rs`: its row. `tests_format_version.rs`: 11. `fork/catalog-format.md`: one sentence.
- `crates/pipeline/src/lib.rs`: `pub mod relight;`, the test `mod` line, `SourceInfo::relight`, `Plan::relight` (set in `plan_kept`, `None` in the colour-noise tile plan), the light's planes added to `prep.masks` after `local::prepare`. `retouch_step.rs`: `relight: None` in its plan. `Cargo.toml`: the relight crate.
- `crates/gpu/src/masks.rs`: the light's planes uploaded after the photo's masks (`upload_relit`).
- `crates/engine/src/lib.rs`: `pub mod relight;`, `Session::relight`. `cmd.rs`: `mod relight;` and its specs. `cmd/query.rs`: `develop.controls` lists the relight specs. `people_parsing.rs`: the models folder for depth. `media/session.rs`: renders, variants and thumbnails take the field and its key; the export waits for it. `media/source.rs`: keeps it in the header. `export/prepare.rs`: `RenderWork::relight`. `files/load.rs`, `smart.rs`: `relight: None`. `Cargo.toml`: the relight crate, and the depth crate under `sam`.
- `crates/ui-egui/src/panels/edit.rs`: the module, the section's closure and match arm. `edit_sections.rs`: `EditSection::Relight`. `edit_sections/tests.rs`: the order lists. `menus/ui_commands.rs`: `section.relight`. `panels/detail.rs`: the module and the on-photo handle's match arm. `headless.rs`: a headless snapshot waits while depth is found.

## Tests

`fork/tests.d/relight.txt`: malformed depth (wrong size, NaN, empty, too small) returns an error; normals of a wall and a floor; a depth step is no surface; disparity maps; a near surface takes more light than a far one; facing; Amount at the peak, the roll-off near white and in the deepest shadow; Darken Surroundings; Warmth; hostile numbers; whole renders (off, no depth yet or the section's eye off change no byte; a near square brightens more than four times the wall; a draft equals a 4× export scaled down, also cropped; the light stays on the same place under a crop); the settings' lenient reading and presets; the engine's store, export wait and download refusal without `confirm`; the section order.

## What's New entry

### Relight
- A new Develop section, Relight, adds a light you place on the photo. Drag its handle and the light reaches what is near it in depth and faces it, live while you drag. Amount, Softness and Warmth shape it; Darken Surroundings and Wrap are under Advanced.
- Depth comes from the photo's own depth map (iPhone Portrait photos) or from an on-device depth model, downloaded once (about 1.3 GB). Saved with the edit and carried by presets, Copy Settings and Sync; the export matches the preview.

## Parity rows

| LR-DEV-RELIGHT | Relight: a depth-aware light dragged on the photo | P2 | 🟡 | `cmd:relight.status`, `cmd:relight.downloadModel`, `crates/relight/src/light.rs`, `crates/ui-egui/src/panels/edit/relight.rs` | beyond Lightroom (no equivalent): a point light placed in the photo's depth (its own map or Depth Anything 3), by 3D distance and facing, in scene-linear light; 🟡 until tried by hand |
