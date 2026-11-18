# Mask Presets

List: Save Mask as Preset… keeps one mask (its parts, effect sliders and edge settings) in the library, and a click in the Masking panel's Mask Presets list adds it to any photo; AI parts are found again on that photo, gradients and brush strokes land at the same relative place (issue #329). Branch `feat/mask-presets`.

## What it does

- **Save.** A mask's right-click menu has Save Mask as Preset…; it opens the Mask Presets list in the Masking panel with a name field (`TextField`). The button under the list saves the selected mask too. Command: `mask.savePreset {id?, name}` (up to 40 characters, not another preset's, at most 100 presets; a mask that selects nothing yet is refused).
- **What is kept.** Every part (kind, parameters, mode, its own Invert and name, so a mask of several parts and intersections is saved whole), the mask's Invert, effect sliders (`adjust`), Refine Edges and Decontaminate. The preset is a `Mask` as it is serialised, with id 0 (`lightcraft_develop::mask_presets::MaskPreset`).
- **AI parts are recipes.** Subject, Sky, Background, Object (its clicks), Describe (its words) and Select People (its parts, and whose face when the face is named; a face known only by where it was is everyone's) keep what they select and never the selection a model found (`adaptive::strip_found`). Depth Range, Luminance Range and Color Range are kept with their numbers (they are worked out from each photo's pixels and depth when drawn).
- **Brush and gradients are relative.** Places are normalized to the uncropped photo as it is shown (after the user's rotation) and sizes are shares of its long edge, so a preset lands at the same relative place, the same share of the long edge across, on a photo of any size or orientation. An old mask drawn on the corrected frame is put on the photo before saving.
- **Apply.** A click on a row, or `mask.applyPreset {preset, name?}`, adds the mask last in the photo's list and selects it; one undo step (`Mask Preset: <name>`). The mask is named after the preset, numbered when that name is taken ("Pool 2"). Its AI parts wait and are found on this photo by the same machinery as Paste and Sync (`fork/adaptive-masks.md`): in the background in the app (the top-bar ring's "Finding AI masks"), before the command returns in the CLI, MCP and tests. A part whose model is missing, or that finds nothing, selects nothing and the mask is still added. `missing` (why a part can't be found on this computer) is known at once and the app says it in a toast. `foundNothing` (the kinds) is in the result only where the search ends before the command returns (CLI, MCP, tests); in the app the "Finding AI masks" row of the top-bar ring lists the photo with why. Update AI Masks finds it again later.
- **List.** Masking panel ▸ Mask Presets: a row for each preset, its name wrapped (never cut) and the kinds of its parts. A click applies it. The row's menu has Rename… (an inline `TextField`) and Delete Preset. Masks made from a preset keep their settings when it is deleted.
- **Commands:** `mask.presets`, `mask.applyPreset`, `mask.savePreset`, `mask.renamePreset`, `mask.deletePreset`.
- **Storage.** `maskPresets` in the library's `presets.json`, beside `relightPresets` and read the same way: entry by entry, a bad entry (a null name, no parts, a repeated id or name) costs only itself and a copy of the file is kept (`set_aside`) before the next save writes it without it; names cut to 40 characters; what a model found on a hand-edited preset's photo is stripped. A named Select People face keeps its source face box (needed to match it) and the person's name; the source photo's own person index is cleared. Cleared when another library opens. No `ui.json` key and no catalog format change: a mask is an ordinary mask.

## Branch

`feat/mask-presets`

## Own files

`crates/develop/src/mask_presets.rs`, `crates/engine/src/mask_presets.rs`, `crates/engine/src/mask_presets/tests.rs`, `crates/engine/src/cmd/mask_presets.rs`, `crates/ui-egui/src/panels/mask_presets.rs`, `fork/mask-presets.md`, `fork/tests.d/mask-presets.txt`

## Hooks in official files

- `crates/develop/src/lib.rs`: `pub mod mask_presets`. `crates/engine/src/lib.rs`: `pub mod mask_presets`, `Session::mask_presets`. `crates/engine/src/cmd.rs`: `mod mask_presets`, its `specs()`.
- `crates/engine/src/library.rs`: `maskPresets` in `PresetsFile`, `PresetsRead`, `PresetsSaved` (compare), `presets_json`, cleared and read (own block, own `set_aside`) when a library opens.
- `crates/ui-egui/src/panels.rs`: `pub mod mask_presets`. `panels/masking.rs`: the list under the Masks list. `panels/mask_menus.rs`: Save Mask as Preset… in a mask's menu.

## Known limits

- A preset holds one mask. Several masks, and the Develop settings around them, are Develop presets with the Masking group.
- A Select People part with a named face finds that person on the other photo only when the photo's people are named alike.

## Tests

`cargo test -p lightcraft-develop --lib -- mask_presets` · `cargo test -p lightcraft-engine --lib -- mask_presets` (the subject found again on another photo, a radial at the same relative place on a photo of another size and turned, a missing model or an empty search named, one undo step, a bad entry in `presets.json`, another library) · `cargo test -p lightcraft-ui-egui --lib -- masking mask_menus mask_parts settings_sync edit_sections`.

## What's New entry

### Mask Presets
- Right-click a mask and choose Save Mask as Preset… to keep it: its parts, effect sliders and edge settings. The Mask Presets list in the Masking panel adds it to any photo with a click, with rename and delete in a row's menu.
- Subject, Sky, People and the other AI parts are found again on the new photo, never copied from the old one. Brush strokes and gradients land at the same place relative to the photo, whatever its size or orientation.
