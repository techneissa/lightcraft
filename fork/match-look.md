# Match Look

List: Photo ▸ Match Look… gives the selected photos the colour and tone of a reference picture (a library photo, an image file, a pasted image or a web address) as ordinary slider values, one undo step. Branch `feat/84-match-look`.

## What it does

Photo ▸ Match Look… opens a small window. Pick the reference: a photo of the view (the strip of thumbnails), **Choose File…**, an image pasted with ⌘V, or an `https://` address. The reference shows small in the window. **Amount** (default 100 %) sets how far the photos move. **Match** fits the selected photos in the background (a "Match Look" row in the activity stack, ✕ stops it); the result lands as one undo step, and the window stays open to try another reference or Amount after Undo.

The result is ordinary slider values, so it stays editable: Temp, Tint, Exposure, Contrast, Highlights, Shadows, Whites, Blacks, Vibrance, Saturation, the Color Mixer bands both photos have, the shadow / midtone / highlight Color Grading wheels, and a master tone curve (only when the photo has none). A reference from outside the library is never imported: a file is read (at most 64 MB), a pasted picture is held in memory, a download (https only, at most 64 MB, 10 s to connect, 20 s without data) is kept only in `~/Library/Caches/LightCraft/match-look` (the 16 newest).

How it fits (`crates/look`): the reference and the photo's 256 px copy are measured in OkLab (lightness quantiles, the cast of shadows / midtones / highlights, colourfulness around each zone's cast, each mixer band's lightness and colour); the sliders move one at a time, broad first, each move kept only when the photo rendered through the real develop pipeline comes closer, with a pull back to where each slider was and a limit on how far each moves (`fork/match-look-followups.md`). A photo matched to itself stays as it is. About 150–250 renders, under a second per photo on the Mac mini (CC0 photos, `--release`).

The fitting takes a list of region pairs (a mask on the photo, a mask on the reference); "whole photo" is the only pair for now, so skin-to-skin and sky-to-sky can plug in with the per-person masks (#81).

The residual **Look** (a colour lookup fitted in OkLab, with its own Amount) for what the sliders cannot reach came later: `fork/match-look-followups.md`.

Commands: `develop.matchLook {reference: {photo} | {path} | {url} | {pasted: true}, ids?, amount?, options?, wait?}` (waits by default: scripts and MCP get the changes back), `develop.matchLookPump` (the app's per-frame check); UI `dialog.matchLook {photo?, path?, url?}`, `matchLook.chooseFile {path?}`, `matchLook.apply {amount?}`. MCP lists `develop.matchLook` like every command.

## Branch

`feat/84-match-look` (from `custom`)

## Own files

`crates/look/` (the fitting, the statistics, reference loading, `examples/match.rs`), `crates/engine/src/cmd/match_look.rs`, `crates/engine/src/tests/match_look.rs`, `crates/ui-egui/src/panels/match_look.rs`, `crates/ui-egui/src/tests/match_look.rs`

## Hooks in official files

`Cargo.toml` and `Cargo.lock` (the workspace member), `xtask/src/layers.rs` (`look` at L3), `crates/engine/Cargo.toml` and `crates/ui-egui/Cargo.toml` (the dependency), engine `cmd.rs` (module and `specs()`), `lib.rs` (the `match_look` field and its start value), `tests.rs` (module); UI `panels.rs` (module), `state.rs` (the `match_look` field and its start value), `app/frame.rs` (one call after the Command Palette, in both frame paths), `menus/run.rs` (dispatch), `menus/ui_commands.rs` (three commands), `tests.rs` (module)

## Tests

Commands in `fork/tests.d/match-look.txt`. Procedural photos only (`lightcraft-scenes`, the demo library): a photo matched to itself keeps its sliders; Temp 7600 / Exposure +0.5 / Saturation +20 come back from the edited render; a warmer reference raises Temp, a darker one lowers Exposure; a bad address, a download past the cap, a corrupt or missing image are errors; one undo step; the background run's activity row; the window end to end. Tune by hand with `cargo run --release -p lightcraft-look --example match -- target.jpg reference.jpg out.jpg --sheet sheet.jpg`.

## What's New entry

### Match Look
- Photo ▸ Match Look… gives the selected photos the colour and tone of a reference: a photo of the view, an image file, a pasted image or a web address. The result is ordinary slider values (white balance, Light, Vibrance and Saturation, Color Mixer, Color Grading, a tone curve), one undo step, with an Amount. A reference from outside the library is never imported.
