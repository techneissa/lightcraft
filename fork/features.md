# The features

Older features, one line each (moved here from FORK.md). A newer feature has no line here: its `fork/<slug>.md` starts
with a `List:` line, and `fork/features.sh` prints this list and those lines together. Never edit this file for a new
feature: it is the hot spot two branches would both append to.

- [Keyboard shortcuts, VS Code style](shortcut-editing.md): Two shortcuts per command, two-key chords, Assign Shortcut… on right-click and a wider Keyboard Shortcuts window. Branch `feat/shortcut-editing`.
- [Export Animated GIF](animated-gif-export.md): File ▸ Export Animated GIF, at a quality above ffmpeg. Branch `feat/animated-gif-export`.
- [Selection order numbers](selection-order.md): Numbers 1, 2, 3 on selected photos show the GIF's order. Branch `feat/animated-gif-export`.
- [Draggable Before/After split](draggable-split.md): A draggable Before/After split line and a Split button. Branch `feat/draggable-split`.
- [Command Palette](command-palette.md): A Command Palette on ⌘P (Print has no shortcut). Branch `feat/command-palette`.
- [Edit All N Selected](edit-all-selected.md): Edits apply to every selected photo, with Auto Sync on by default and AI Denoise following it. Branch `fix/edit-all-selected`.
- [Larger file dialogs](larger-file-dialogs.md): Taller, resizable open and save windows on macOS. Branch `feat/larger-file-dialogs`.
- [Arrange the photo menu](photo-menu.md): Settings ▸ Photo Menu arranges the photo's right-click menu. Branch `feat/photo-menu`.
- [Raw view colours](raw-view-colors.md): No camera-JPEG flash in the loupe while a raw renders, and lensfun distortion zoomed to fill the frame. Branch `fix/raw-view-colors`.
- [Auto AI Denoise on open](auto-ai-denoise.md): Switches AI Denoise on once for a raw at or above an ISO. Branch `feat/auto-ai-denoise`.
- [Top bar items](topbar-items.md): Show or hide top-bar items from Settings ▸ Top Bar or a right-click on the bar. Branch `feat/topbar-items`.
- [Camera Matching in the Profile picker](camera-matching-profile.md): Camera Matching / No Camera Matching in the Profile picker, also for cameras without measured matrices. Branch `feat/camera-matching-profile`.
- [A7 V colour fallback](a7v-colour-fallback.md): Dull A7 V photos no longer fall back to raw sensor colour. Branch `fix/a7v-colour-fallback`.
- [Lens profiles: Mark II lenses](a7v-lightness.md): A Mark II lens no longer takes the Mark I lens profile. Branch `fix/a7v-lightness`.
- [Local camera profiles](local-camera-profiles.md): Your own camera profiles appear in the Profile picker. Branch `feat/local-camera-profiles`.
- [Right-click menu presets and backup](photo-menu-presets.md): Three right-click menu presets in one dropdown, with export and import. Branch `feat/photo-menu-presets`.
- [Shortcut conditions (When)](shortcut-when.md): A shortcut can be limited to a context, such as the Develop view. Branch `feat/shortcut-when`.
- [Wider Settings window](settings-wider.md): The Settings window shows 720 pt of page instead of 560. Branch `feat/settings-wider`.
- [macOS Colour](macos-colour.md): macOS's own raw decoder makes a raw's colours; LightCraft keeps tone, white balance and every slider (Mac only). Branch `feat/macos-colour`.
- [Open as Layers in PhotoCraft](open-as-layers.md): Photo ▸ Edit In ▸ Open as Layers in PhotoCraft… for two or more photos. Branch `feat/open-as-layers`.
- [Performance: memory and redraws](memory-redraws.md): Less memory and fewer redraws; the loupe never shows another photo's pixels while a raw renders. Branch `perf/memory-redraws`.
- [Performance: UI freezes](ui-freezes.md): Slow work leaves the UI thread so the window keeps drawing. Branch `perf/ui-freezes`.
- [Performance: XMP sidecar writes](sidecar-writes.md): Automatic XMP sidecar writes run on one background worker. Branch `perf/sidecar-writes`.
- [Smart Previews for editing](smart-previews-for-editing.md): An option to view and edit from Smart Previews even when the original is reachable. Branch `feat/smart-previews-for-editing`.
- [Rejected photos look the same in every view](consistent-colour-across-views.md): Compare and Survey no longer dim a photo flagged as rejected. Branch `fix/consistent-colour-across-views`.
- [Type to search in dropdown lists](dropdown-search.md): Typing in an open dropdown list narrows it. Branch `feat/dropdown-search`.
- [Quick filters in the bottom bar](bottombar-quick-filters.md): A Stars dropdown and Picked / Rejected / Unflagged buttons beside the thumbnail slider. Branch `feat/bottombar-quick-filters`.
- [Delete Permanently asks about the files](delete-permanently-trash.md): Delete Permanently asks whether to move the files to the Trash or only remove them from the library. Branch `fix/delete-permanently-trash`.
- [Delete Rejected Photos, local photos to the Trash](delete-local-rejected.md): Photo ▸ Delete Rejected Photos, and local photos go to the Trash. Branch `fix/delete-local-rejected`.
- [Faster deleting](quick-delete.md): ⌫ in Recently Deleted runs Delete Permanently. Branch `feat/quick-delete`.
- [Shorter Settings](shorter-settings-text.md): Help lines that repeated their labels are removed from Settings. Branch `fix/shorter-settings-text`.
- [Develop sliders on one row](slider-row-redesign.md): Label, groove with ticks and value on one row, Lightroom-style. Branch `claude/slider-row-redesign`.
- [Process V3: Contrast, Whites and Blacks](tone-slider-strength.md): Contrast, Whites and Blacks are clearly visible on every tone path. Branch `claude/tone-slider-strength`.
- [Process V3: Shadows and Highlights](local-tone-draft-parity.md): Shadows and Highlights reach further, and the slider preview matches export. Branch `claude/local-tone-draft-parity`.
- [Slider strength audit](slider-strength-audit.md): A measurement harness and `docs/slider-strength.md` for every remaining Develop slider. Branch `claude/slider-strength-audit`.
- [Shadow and Highlight Clipping toggles](histogram-clipping-toggles.md): The histogram's two triangles each switch their own clipping overlay. Branch `claude/histogram-clipping-toggles`.
- [Edit camera, lens and exposure values in Info](metadata-overrides.md): Edit Camera, Lens, Focal length, Shutter speed, Aperture and ISO for one or many photos. Branch `claude/metadata-overrides`.
- [Brush live adjustment](brush-live-preview.md): Painting with the Brush shows the mask's adjustment under the brush. Branch `claude/brush-live-preview`.
- [⌥-drag slider previews](alt-drag-clipping-preview.md): Holding ⌥ while dragging a slider shows clipping, the sharpening mask or greyscale. Branch `claude/alt-drag-clipping-preview`.
- [Process V3: Sharpening Radius and Vignette](detail-followups.md): Sharpening Radius works, Amount is clearly visible, and the Vignette response is smooth. Branch `claude/detail-followups`.
- [Arrange the Edit panel](section-arrange.md): Right-click a section header to move, hide or reset the Develop sections. Branch `claude/workflow-qol`.
- [Show in Finder after export](export-reveal.md): An Export dialog option that shows the first exported file when the export succeeds. Branch `claude/workflow-qol`.
- [Lens database folder](lens-database-folder.md): Settings ▸ Import ▸ Lens profiles chooses the lensfun folder and says what it found. Branch `claude/workflow-qol`.
- [Masking workflow](masking-workflow.md): Add · Subtract · Intersect and a button per kind of part, pick a part to paint into it, Add / Erase named by their effect, Esc steps back, tiles fill the side bar. Branch `fix/26-masking-rework`.
- [Shared SAM3 model folder](shared-sam3.md): AI masks look for the SAM 3 model in the folder shared with PhotoCraft (`ArtCraft/models/sam3`) first and download there. Branch `feat/16-shared-sam3`.
- [Book](book.md): File ▸ Book… lays photos out as a photo book (cover, five page templates, Pages and Spread views) and exports a PDF or JPEGs. Branch `feat/23-book`.
- [Find People and Find Wires](find-people-wires.md): Remove panel buttons that outline people and wires and remove them in one click, with SAM 3 or built-in detectors. Branch `feat/24-find-people-wires`.
- [Map view](map-view.md): View ▸ Map with pins, drag to geotag, saved locations, location filter and track logs, from the official Map PRs taken early; starts Offline until you pick a tile server. Branch `feat/4-map-view`.
- [Renaming, one way everywhere](consistent-renaming.md): Rows rename in place with one set of rules; Rename Photos and Rename Folder move their files on a worker. Branch `feat/27-consistent-renaming`.
