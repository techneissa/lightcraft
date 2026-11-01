# This fork: LightCraft plus techneissa's own features

The branch **`custom`** is the official LightCraft ([storytold/lightcraft](https://github.com/storytold/lightcraft))
plus the features listed below. The installed app is built from `custom`.

**Mac only.** techneissa uses this fork on macOS and nowhere else (techneissa, 2026-11-01). Its features and fixes
need to work on the Mac only: no Windows, Linux or web (WASM) work, checks, screenshots or fixes for them. A Fork CI
failure that only Linux has (a macOS-only test, a Linux-only path) is not a blocker; list it under "Known failures".
The official code's own platforms are the official project's job.

## Update the app

Click **Update LightCraft** in Applications (or `~/scripts/craft/update-lightcraft.sh`). The steps, and what to do
when it stops: [fork/update.md](fork/update.md).

## GitHub builds the app

Fork build (`fork-build.yml`) and Fork CI (`fork-ci.yml`), the daily update agent, the switched-off official
workflows: [fork/github-builds.md](fork/github-builds.md).

## Where things are

Remotes (`origin` official, `mine` this fork), branches (`main`, `custom`, `feat/…`), the `custom-build` release and the
local folder: [fork/where-things-are.md](fork/where-things-are.md).

Known failures (on this Mac, and on Fork CI only): [fork/known-failures.md](fork/known-failures.md).

## Add a feature

1. `git fetch origin && git switch -c feat/<name> origin/main`
2. Put the code in new files; touch existing files only with small hooks (a `mod` line, a menu entry, a match arm,
   a state field). Never reformat, rename or move official code. This is what keeps updates conflict-free.
   No help line under a setting, toggle or button whose label already says what it does; a needed one is one short
   sentence (techneissa, 2026-10-29: Settings had grown long with lines that repeated their labels).
3. Add tests. Check them as [fork/checks.md](fork/checks.md) says: on this Mac only the touched tests, clippy
   and fmt, never `cargo xtask ci` or `ci --quick`; the full suite runs on GitHub (Fork CI).
4. `git switch custom && git merge --no-ff feat/<name>`, then `git push mine custom feat/<name>`.
5. Write `fork/<slug>.md` with the headings What it does / Branch / Own files / Hooks in official files / Tests
   / When upstream merges (the last only for a feature that took an upstream PR early), add its one line to the list
   below and its test command to `fork/tests.sh`. FORK.md itself never gets a feature's details. Then run Update
   LightCraft to install it.

## The features

- [Keyboard shortcuts, VS Code style](fork/shortcut-editing.md): Two shortcuts per command, two-key chords, Assign Shortcut… on right-click and a wider Keyboard Shortcuts window. Branch `feat/shortcut-editing`.
- [Export Animated GIF](fork/animated-gif-export.md): File ▸ Export Animated GIF, at a quality above ffmpeg. Branch `feat/animated-gif-export`.
- [Selection order numbers](fork/selection-order.md): Numbers 1, 2, 3 on selected photos show the GIF's order. Branch `feat/animated-gif-export`.
- [Draggable Before/After split](fork/draggable-split.md): A draggable Before/After split line and a Split button. Branch `feat/draggable-split`.
- [Command Palette](fork/command-palette.md): A Command Palette on ⌘P (Print has no shortcut). Branch `feat/command-palette`.
- [Edit All N Selected](fork/edit-all-selected.md): Edits apply to every selected photo, with Auto Sync on by default and AI Denoise following it. Branch `fix/edit-all-selected`.
- [Larger file dialogs](fork/larger-file-dialogs.md): Taller, resizable open and save windows on macOS. Branch `feat/larger-file-dialogs`.
- [Arrange the photo menu](fork/photo-menu.md): Settings ▸ Photo Menu arranges the photo's right-click menu. Branch `feat/photo-menu`.
- [Raw view colours](fork/raw-view-colors.md): No camera-JPEG flash in the loupe while a raw renders, and lensfun distortion zoomed to fill the frame. Branch `fix/raw-view-colors`.
- [Auto AI Denoise on open](fork/auto-ai-denoise.md): Switches AI Denoise on once for a raw at or above an ISO. Branch `feat/auto-ai-denoise`.
- [Top bar items](fork/topbar-items.md): Show or hide top-bar items from Settings ▸ Top Bar or a right-click on the bar. Branch `feat/topbar-items`.
- [Camera Matching in the Profile picker](fork/camera-matching-profile.md): Camera Matching / No Camera Matching in the Profile picker, also for cameras without measured matrices. Branch `feat/camera-matching-profile`.
- [A7 V colour fallback](fork/a7v-colour-fallback.md): Dull A7 V photos no longer fall back to raw sensor colour. Branch `fix/a7v-colour-fallback`.
- [Lens profiles: Mark II lenses](fork/a7v-lightness.md): A Mark II lens no longer takes the Mark I lens profile. Branch `fix/a7v-lightness`.
- [Local camera profiles](fork/local-camera-profiles.md): Your own camera profiles appear in the Profile picker. Branch `feat/local-camera-profiles`.
- [Right-click menu presets and backup](fork/photo-menu-presets.md): Three right-click menu presets in one dropdown, with export and import. Branch `feat/photo-menu-presets`.
- [Shortcut conditions (When)](fork/shortcut-when.md): A shortcut can be limited to a context, such as the Develop view. Branch `feat/shortcut-when`.
- [Wider Settings window](fork/settings-wider.md): The Settings window shows 720 pt of page instead of 560. Branch `feat/settings-wider`.
- [macOS Colour](fork/macos-colour.md): macOS's own raw decoder makes a raw's colours; LightCraft keeps tone, white balance and every slider (Mac only). Branch `feat/macos-colour`.
- [Open as Layers in PhotoCraft](fork/open-as-layers.md): Photo ▸ Edit In ▸ Open as Layers in PhotoCraft… for two or more photos. Branch `feat/open-as-layers`.
- [Performance: memory and redraws](fork/memory-redraws.md): Less memory and fewer redraws; the loupe never shows another photo's pixels while a raw renders. Branch `perf/memory-redraws`.
- [Performance: UI freezes](fork/ui-freezes.md): Slow work leaves the UI thread so the window keeps drawing. Branch `perf/ui-freezes`.
- [Performance: XMP sidecar writes](fork/sidecar-writes.md): Automatic XMP sidecar writes run on one background worker. Branch `perf/sidecar-writes`.
- [Smart Previews for editing](fork/smart-previews-for-editing.md): An option to view and edit from Smart Previews even when the original is reachable. Branch `feat/smart-previews-for-editing`.
- [Rejected photos look the same in every view](fork/consistent-colour-across-views.md): Compare and Survey no longer dim a photo flagged as rejected. Branch `fix/consistent-colour-across-views`.
- [Type to search in dropdown lists](fork/dropdown-search.md): Typing in an open dropdown list narrows it. Branch `feat/dropdown-search`.
- [Quick filters in the bottom bar](fork/bottombar-quick-filters.md): A Stars dropdown and Picked / Rejected / Unflagged buttons beside the thumbnail slider. Branch `feat/bottombar-quick-filters`.
- [Delete Permanently asks about the files](fork/delete-permanently-trash.md): Delete Permanently asks whether to move the files to the Trash or only remove them from the library. Branch `fix/delete-permanently-trash`.
- [Delete Rejected Photos, local photos to the Trash](fork/delete-local-rejected.md): Photo ▸ Delete Rejected Photos, and local photos go to the Trash. Branch `fix/delete-local-rejected`.
- [Faster deleting](fork/quick-delete.md): ⌫ in Recently Deleted runs Delete Permanently. Branch `feat/quick-delete`.
- [Shorter Settings](fork/shorter-settings-text.md): Help lines that repeated their labels are removed from Settings. Branch `fix/shorter-settings-text`.
- [Develop sliders on one row](fork/slider-row-redesign.md): Label, groove with ticks and value on one row, Lightroom-style. Branch `claude/slider-row-redesign`.
- [Process V3: Contrast, Whites and Blacks](fork/tone-slider-strength.md): Contrast, Whites and Blacks are clearly visible on every tone path. Branch `claude/tone-slider-strength`.
- [Process V3: Shadows and Highlights](fork/local-tone-draft-parity.md): Shadows and Highlights reach further, and the slider preview matches export. Branch `claude/local-tone-draft-parity`.
- [Slider strength audit](fork/slider-strength-audit.md): A measurement harness and `docs/slider-strength.md` for every remaining Develop slider. Branch `claude/slider-strength-audit`.
- [Shadow and Highlight Clipping toggles](fork/histogram-clipping-toggles.md): The histogram's two triangles each switch their own clipping overlay. Branch `claude/histogram-clipping-toggles`.
- [Edit camera, lens and exposure values in Info](fork/metadata-overrides.md): Edit Camera, Lens, Focal length, Shutter speed, Aperture and ISO for one or many photos. Branch `claude/metadata-overrides`.
- [Brush live adjustment](fork/brush-live-preview.md): Painting with the Brush shows the mask's adjustment under the brush. Branch `claude/brush-live-preview`.
- [⌥-drag slider previews](fork/alt-drag-clipping-preview.md): Holding ⌥ while dragging a slider shows clipping, the sharpening mask or greyscale. Branch `claude/alt-drag-clipping-preview`.
- [Process V3: Sharpening Radius and Vignette](fork/detail-followups.md): Sharpening Radius works, Amount is clearly visible, and the Vignette response is smooth. Branch `claude/detail-followups`.
- [Arrange the Edit panel](fork/section-arrange.md): Right-click a section header to move, hide or reset the Develop sections. Branch `claude/workflow-qol`.
- [Show in Finder after export](fork/export-reveal.md): An Export dialog option that shows the first exported file when the export succeeds. Branch `claude/workflow-qol`.
- [Lens database folder](fork/lens-database-folder.md): Settings ▸ Import ▸ Lens profiles chooses the lensfun folder and says what it found. Branch `claude/workflow-qol`.
- [Masking workflow](fork/masking-workflow.md): Add · Subtract · Intersect and a button per kind of part, pick a part to paint into it, Add / Erase named by their effect, Esc steps back, tiles fill the side bar. Branch `fix/26-masking-rework`.
- [Shared SAM3 model folder](fork/shared-sam3.md): AI masks look for the SAM 3 model in the folder shared with PhotoCraft (`ArtCraft/models/sam3`) first and download there. Branch `feat/16-shared-sam3`.

Every feature also has an entry in `docs/whats-new.md` and a row in `docs/parity-checklist.md`; clashes there are
fixed by the update script (both sides kept).

The tests of every feature, after a merge: run `fork/tests.sh`.

To find which feature owns an official file: `grep -l '<path>' fork/*.md`.
