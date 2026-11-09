# Portrait retouching: the shared base

List: the base every portrait feature plugs into: `crates/retouch`, the Retouch settings with per-face settings, one pipeline step, the faces of each photo, `retouch.*` commands and the Portrait tab of the tool strip with a face picker (issue #80, part of #79). Branch `feat/80-retouch-foundation`.

## What it does

Nothing changes in a photo yet: every feature is an empty stub. What is there for the features of #79 to fill:

- **`crates/retouch`** (`lightcraft-retouch`, layer 1, a leaf crate): the settings, the input contract and the
  step. One module per feature (`blemish`, `skin`, `wrinkles`, `skin_tone`, `eyes`, `teeth_lips`, `glare`, `hair`,
  `reshape`, `background`; clothes recolouring is a local-mask effect instead, issue #83), each with its settings struct, its sliders, a no-op `apply`, an
  example program (`cargo run -p lightcraft-retouch --example skin -- in.jpg out.jpg --amount 50`) and a test file.
  A feature's issue edits only its own module, example, tests and section of the Portrait tab. `crates/retouch/README.md` has the
  contract, the rules (sizes are fractions of the face; off is byte for byte; no panic), the speed target every
  feature lane builds to and reports (sliders are GPU steps on cached masks; models run once per photo, ahead of
  time), what running ahead of time still needs, and the render order.
- **Settings**: the develop settings' `fork_retouch` block: settings for every face, and per-face settings keyed by
  the face's person (People names) or, for an unnamed face, by its box on the photo before the user's rotation
  (Rotate Left / Right and flips keep them with their face; a lens correction or transform changed afterwards can
  still move an unnamed face's box away from its settings), matched the same whatever order the faces are found in;
  settings made for a person never go to someone else. Not written while it is the default; a build
  without it, or a malformed block, reads it as off. Stored in the catalog and in the sidecar's `lc:settings`; an
  app that rewrites the sidecar and keeps `lc:settings` (Lightroom) keeps it. Copy / Paste, Sync and presets carry
  it as the **Portrait** group. It holds one photo's face edits, like Masking and Remove: it is not in the default
  copy groups and Auto Sync leaves it alone; ticked by hand it is copied (copying an unretouched photo's Portrait
  group turns retouching off).
- **Controls**: every value is a develop control in section Portrait: `retouch.<feature>.<field>` (every face) and
  `retouchFace.<i>.<feature>.<field>` (the `i`-th face with its own settings). `develop.set`, `develop.controls`,
  the control channel and MCP reach them. Today each feature has one, `Amount` (0…100), with no effect.
- **The step** (`crates/pipeline/src/retouch_step.rs`): in the scene-linear stage after white balance, spots and red
  eye, before noise reduction, tone, colour, masks, detail and effects; retouch stage, then background stage; the
  geometry slot (face reshape, next to Transform) is reserved, not run. Skipped while the block is off or the
  Portrait tab's eye is off; the scene-linear stage's key then hashes a 0 for it (a new element of that key, so
  in-memory stage caches made before this change are not reused; nothing on disk changes). On the GPU, a retouched photo's scene-linear stage
  runs on the CPU.
- **Faces** (`crates/engine/src/retouch_input.rs`): a `FaceProvider` finds them on a 1600-pixel render of the photo
  before its crop, without retouching; the default is YuNet (Settings ▸ Faces: boxes and five landmarks, no masks),
  so features that need masks change nothing until the people parser (#81) installs its provider
  (`FaceStore::set_provider`). Found once per photo on a worker thread with the activity row "Finding Faces", kept
  in memory for the 64 photos used last, with a round thumbnail per face; a missing or unreadable face model is kept
  as the photo's reason (shown, never read again each frame) until a face model is installed or removed, and the search then runs; a face's person is the named People region it
  overlaps. Renders never wait (a preview without faces is redrawn once they are found: their fingerprint is in the
  render keys). A background export sets up without waiting and finds the faces on its own worker, under its
  activity row, before rendering; synchronous renders and exports find them first.
- **Commands**: `retouch.faces {id?}` (the faces found, with `own`, the index of a face's own settings),
  `retouch.detect {id?, again?}`, `retouch.set {control, value, face?, id?}` (with `face`, that face gets its own
  settings, copied from every face's), `retouch.resetFace {face}`, `retouch.reset {feature?, face?}`. UI commands
  `panel.portrait` (Window ▸ Portrait, shortcut T) and `portrait.section {section, open?}`.
- **Portrait tab** (`RightPanel::Portrait`, `panels/portrait.rs`): its own icon in the tool strip (a person, after
  Masking, before Red Eye; widget `icon:portrait`; the group between the strip's dividers has five icons now, and an
  order saved before it gets Portrait after Masking), not a section of Edit. A title row whose eye turns all
  retouching off and on for the photo (`develop.sectionEnabled {section: "portrait"}`), a line "Editing: All Faces"
  (or the picked face: its name, else "Face 2"), why there are no faces when there are none, then a collapsible
  section per feature, empty: Skin, Blemishes, Wrinkles, Eyes, Teeth and Lips, Skin Tone, Face Reshape, Glasses
  Glare, Background, Stray Hair, and Portrait Presets (no feature module yet). Sections start closed; open ones are
  kept in the saved `openSections` as `portrait.<key>` (a key already there, so no new `ui.json` key; Edit's Solo
  Mode leaves them alone). `ui.json` saves the open Portrait tab as Edit: a build without the tab cannot read
  `right: "portrait"` and would drop the whole file. Opening the tab finds the photo's faces; so does opening the photo, early, when parsing ahead is on (`fork/parse-ahead.md`, the early check: only "nobody" is kept). **A photo with no people** (the face search finished and found no face, or the early check saw none while nothing was searched: `FacesState::no_people`, a *hint*, which only the icon and the tab read; the face search, renders and exports ignore it, so a photo the early check missed is still searched and retouched; unknown, running or failed searches are not "no people"): the tool strip's Portrait icon is drawn dimmed in its place, still clickable, with the hover text "Portrait: No People in This Photo"; the tab (opened by the icon, T or Window ▸ Portrait) shows the title row, the line "No People in This Photo" and the button "Search Again" (`retouch.search {again: true}`: the real search in the background, one "Finding Faces" row; "Finding Faces…" meanwhile; when it finds a face the tab fills and the icon is normal), with no sections, "Editing" line or face picker, and does not search by itself; switching photos follows. The photo's stored Portrait settings are not touched. `retouch.faces` says `noPeople` and `nobodyHint`, and `face` commands (`retouch.set`, `resetFace`, `reset`, `refine.*`) answer "no face N: the photo has no people" once a search found nobody (`retouch.refine.list` with a face past the last still reads the strokes for every face when the photo has faces) (issue #167). Sliders in a feature's section change the picked
  face. Select People's parts stay in the Masking panel.
- **Face picker on the loupe** (`portrait/overlay.rs`), while the Portrait tab is open: the people picker
  (`panels/detail/people_picker.rs`), a semi-opaque dark card at the image view's top-right corner, 12 points from
  its edges: the same place for every photo, zoom and aspect ratio, below the "Camera preview" badge when that is
  at the same corner. Row one: All and each face as a round thumbnail (its name on hover), the picked one ringed;
  one at a time (a per-face slider writes one face's own settings). Row two, for Select People masks: a list of
  parts (several with ⌘-click), shown only when a caller gives items; Portrait gives none yet. The card folds into
  an overflow button ("⋯") at the same corner and opens from it; folded or not is kept in the saved
  `openFlyouts` (`portrait.facesFolded`). Zooming and panning move the photo under it; it takes its own clicks and
  hides with the tool overlay (View ▸ Tool Overlay ▸ Never, Shift+H) and in the full-screen preview. The choice
  is per photo and not saved.

Mac only, like every fork feature; the web build compiles it but finds no faces.

## Branch

`feat/80-retouch-foundation`

## Own files

`crates/retouch/` (`Cargo.toml`, `README.md`, `src/{lib,control,feature,input,overrides,run,settings}.rs`, one
`src/<feature>.rs`, `examples/<feature>.rs` and `tests/<feature>.rs` per feature, `examples/common/mod.rs`,
`tests/common/mod.rs`, `tests/foundation.rs`), `crates/develop/src/retouch.rs`, `crates/pipeline/src/retouch_step.rs`,
`crates/engine/src/retouch_input.rs`, `crates/engine/src/retouch_input/tests.rs`, `crates/engine/src/cmd/retouch.rs`,
`crates/ui-egui/src/panels/portrait.rs`, `crates/ui-egui/src/panels/portrait/<feature>.rs`,
`crates/ui-egui/src/panels/portrait/presets.rs`, `crates/ui-egui/src/panels/portrait/overlay.rs`, `crates/ui-egui/src/panels/detail/people_picker.rs` and its
`people_picker/tests.rs`,
`crates/ui-egui/src/tests/portrait.rs`, `crates/mcp/src/tests_retouch.rs`, `fork/retouch-foundation.md`,
`fork/tests.d/retouch-foundation.txt`

## Hooks in official files

- `Cargo.toml`, `Cargo.lock`: the workspace dependency `lightcraft-retouch`. `xtask/src/layers.rs`: `retouch` is
  layer 1, below `develop`.
- `crates/develop/Cargo.toml` (the dependency); `src/settings.rs` (field `DevelopSettings::retouch`, serialised as
  `fork_retouch`, and its default); `src/lib.rs` (`pub mod retouch`, `to_json_for_merge` states the block,
  `reset_section` resets it); `src/controls.rs` (`Section::Portrait`, and `get` / `set` / `find` / `in_section`
  fall back to the retouch controls); `src/presets.rs` (`SettingsGroup::Portrait`, left out of `default_copy`).
- `crates/pipeline/src/lib.rs`: `pub mod retouch_step`, `SourceInfo::retouch`, the step's key in `pre_nr_key`, the
  step in `lin_cpu` and `lin_needs_cpu`. `crates/gpu/src/render.rs` (`linear`): a retouched photo's scene-linear
  stage on the CPU.
- `crates/engine/src/lib.rs` (`pub mod retouch_input`, `Session::retouch_faces`, Auto Sync's delta leaves
  `fork_retouch` out); `src/export/prepare.rs` (`RenderWork::faces`: a prepared export finds the faces when it
  runs); `src/cmd.rs` (`mod retouch`, its
  specs); `src/cmd/query.rs` (`develop.controls` lists the retouch controls); `src/media/session.rs` (`build_job`,
  `variant_job`, `view_key` and `quick_view_job` add the faces and their salt; `render_now` and `export_job` find
  the faces first); `src/media/source.rs` (`info_or` keeps the faces); `SourceInfo` literals in
  `src/files/load.rs` and `src/smart.rs`.
- `crates/ui-egui/src/panels.rs` (`mod portrait`); `src/state/view.rs` (`RightPanel::Portrait`, an edit tool);
  `src/state.rs` (`right` saved through `portrait::save_right`); `src/panels/right.rs` (the tab drawn); `src/panels/detail.rs` (`mod people_picker`; the face picker drawn over the
  view, after the navigator; the tab renders in Develop mode); `src/panels/strip.rs` (the icon, the second divider
  one place lower); `src/panels/tool_strip_order.rs` (`Tool::Portrait`; an old saved order gets it after Masking)
  and its `tests.rs` (the usual order with Portrait); `src/menus/ui_commands.rs` (`panel.portrait`,
  `portrait.section`), `src/menus/menubar.rs` (Window ▸ Portrait), `src/menus/run.rs` (`panel.portrait`, the
  `portrait.*` commands); `src/keybinds/context.rs` (the icon's shortcut menu), `src/keybinds/when/keys.rs` (Tool is
  Portrait); `src/tests.rs` (`mod portrait`).
- `crates/mcp/src/lib.rs` (`mod tests_retouch`).

Portrait has no hook in `panels/edit.rs`, so the Edit panel's arranged sections (`fork/section-arrange.md`) are as
`custom` has them. The tool strip's order (`fork/reorder-strip-sections.md`) has one more icon.

## Tests

`fork/tests.d/retouch-foundation.txt`:

```bash
cargo test -p lightcraft-retouch
cargo test -p lightcraft-develop --lib -- retouch
cargo test -p lightcraft-pipeline --lib -- retouch_step
cargo test -p lightcraft-engine --lib -- retouch_input
cargo test -p lightcraft-mcp --lib -- tests_retouch
cargo test -p lightcraft-ui-egui --lib -- portrait tool_strip_order
```

They cover: the all-off block renders byte for byte like none (CPU, 8-bit and float) with the same stage keys; the
block through JSON, the sidecar (also rewritten by another app), the catalog, copy / paste, sync and presets; a face's
own settings following its person when the faces come back in another order, and its face through a rotation; the
default copy and Auto Sync leaving Portrait alone; a background export finding faces when it runs, not when it is
set up; a missing model not kept as a failure; the face cache forgetting the photo used least recently; MCP reading and setting a value; the
Portrait tab (its strip icon and shortcut, a section per feature and Portrait Presets opening and closing, the
eye, the face picker only while it is open, an old saved strip order); the face picker keeping to the view's corner for photos of other shapes and at 100 %,
picking, folding into the overflow button and hiding with the tool overlay; the picker's parts row (⌘-click
multi-select, with a dummy list) and its place below the camera-preview badge; the Portrait icon dimmed but clickable, the tab one line with Search Again, T opening it and the face commands failing clearly on a photo without people (`tests/portrait_no_people.rs`); no panic with no faces, 50 faces, a failing provider or a 1 × 1 photo.
No test needs a model: the engine and UI tests install a fake provider.

## Parity rows

| LR-EDIT-PORTRAIT | Portrait retouching | — | 🟡 | `cmd:retouch.faces`, `cmd:retouch.detect`, `cmd:retouch.set`, `cmd:retouch.resetFace`, `cmd:retouch.reset`, `ctl:retouch.*`, `crates/retouch/src/lib.rs`, `crates/pipeline/src/retouch_step.rs`, `crates/engine/src/retouch_input.rs`, `crates/ui-egui/src/panels/portrait.rs`, `cmd:panel.portrait` | an extra (the reference app has no portrait retouching): the base only (issue #80). Settings for every face and per face (kept by person or place), one pipeline step, faces from YuNet, the Portrait tab (tool strip, shortcut T) with a face picker and Portrait Presets; every feature (skin, blemishes, wrinkles, skin tone, eyes, teeth and lips, glare, stray hair, reshape, background) is a stub that changes nothing yet. See `fork/retouch-foundation.md` |
