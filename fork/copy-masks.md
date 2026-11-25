# Copy Masks and Paste Masks

List: Copy Masks keeps all of a photo's masks and Paste Masks puts them on the selected photos, apart from what Copy Settings copied (issue #365). Branch `feat/365-copy-masks`.

## What it does

- **Copy Masks** (`mask.copyAll`) keeps every mask of the shown photo: parts, effect sliders, Amount, names, order. Greyed out on a photo with no masks. Kept while the app runs, not saved.
- **Paste Masks** (`mask.pasteAll {ids?}`, default the selected photos, else the shown one) replaces each photo's masks with the copied ones, one undo step ("Paste Masks"). Greyed out until masks are copied.
- It is Paste Settings of the Masking group alone: AI parts are found again on each photo (`fork/adaptive-masks.md`); brush strokes and gradients land at the same place relative to the photo, whatever its size.
- Its own clipboard (`Session::mask_clipboard`): Copy Masks leaves what Copy Settings copied, and the reverse.
- **Where:** the Edit menu beside Copy and Paste Settings, the photo's right-click menu, and a mask's right-click menu in the Masks list. No keyboard shortcut.

## Branch

`feat/365-copy-masks`

## Own files

`crates/engine/src/cmd/mask_clipboard.rs`, `crates/engine/src/tests/mask_clipboard.rs`, `fork/copy-masks.md`, `fork/tests.d/copy-masks.txt`

## Hooks in official files

- `crates/engine/src/cmd.rs`: `pub mod mask_clipboard;`, its `specs()`. `crates/engine/src/cmd/develop.rs`: `pasted` is `pub(super)`. `crates/engine/src/lib.rs`: `Session::mask_clipboard`. `crates/engine/src/tests.rs`: `mod` line.
- `crates/ui-egui/src/menus/menubar.rs`: the two ids in the Edit layout. `panels/photo_menu.rs`, `photo_menu/items.rs`: items `copyMasks`, `pasteMasks` (an arranged photo menu shows them once added in Settings ▸ Photo Menu). `photo_menu/tests.rs`: both in the added rows. `panels/mask_menus.rs`: both in a mask's menu.

## Known limits

- The Masks list has only a menu per mask, so a photo with no masks is pasted to from the Edit menu or the photo's right-click menu.

## Tests

`fork/tests.d/copy-masks.txt`

## What's New entry

### Copy Masks And Paste Masks
- Edit ▸ Copy Masks keeps all of a photo's masks; Paste Masks puts them on the selected photos, replacing theirs, in one undo step. Both are also in the photo's right-click menu and in a mask's menu.
- Subject, Sky, People and the other AI parts are found again on each photo. What Copy Settings copied stays as it was.

## Parity rows

| LR-MASK-COPYALL | Copy all masks, paste them to other photos | P2 | ✅ | `cmd:mask.copyAll`, `cmd:mask.pasteAll`, `crates/engine/src/cmd/mask_clipboard.rs` | fork extra (issue #365): Edit ▸ Copy Masks / Paste Masks, also in the photo's right-click menu and a mask's menu; pastes to the selected photos, replacing their masks, one undo step, AI parts found again on each; its own clipboard, kept while the app runs |
