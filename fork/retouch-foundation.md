# Portrait retouching: the shared base

List: the base every portrait feature plugs into: `crates/retouch`, the Retouch settings with per-face settings, one pipeline step, the faces of each photo, `retouch.*` commands and the Edit panel's Portrait section with a face picker (issue #80, part of #79). Branch `feat/80-retouch-foundation`.

## What it does

Nothing changes in a photo yet: every feature is an empty stub. What is there for the features of #79 to fill:

- **`crates/retouch`** (`lightcraft-retouch`, layer 1, a leaf crate): the settings, the input contract and the
  step. One module per feature (`blemish`, `skin`, `wrinkles`, `skin_tone`, `eyes`, `teeth_lips`, `glare`, `hair`,
  `reshape`, `background`; clothes recolouring is a local-mask effect instead, issue #83), each with its settings struct, its sliders, a no-op `apply`, an
  example program (`cargo run -p lightcraft-retouch --example skin -- in.jpg out.jpg --amount 50`) and a test file.
  A feature's issue edits only its own module, example, tests and panel section. `crates/retouch/README.md` has the
  contract, the rules (sizes are fractions of the face; off is byte for byte; no panic) and the render order.
- **Settings**: the develop settings' `fork_retouch` block: settings for every face, and per-face settings keyed by
  the face's person (People names) or, for an unnamed face, by its box, matched the same whatever order the faces
  are found in; settings made for a person never go to someone else. Not written while it is the default; a build
  without it, or a malformed block, reads it as off. Stored in the catalog and in the sidecar's `lc:settings`; an
  app that rewrites the sidecar and keeps `lc:settings` (Lightroom) keeps it. Copy / Paste, Sync and presets carry
  it as the **Portrait** group (in the default copy groups; copying an unretouched photo's Portrait group turns
  retouching off).
- **Controls**: every value is a develop control in section Portrait: `retouch.<feature>.<field>` (every face) and
  `retouchFace.<i>.<feature>.<field>` (the `i`-th face with its own settings). `develop.set`, `develop.controls`,
  the control channel and MCP reach them. Today each feature has one, `Amount` (0…100), with no effect.
- **The step** (`crates/pipeline/src/retouch_step.rs`): in the scene-linear stage after white balance, spots and red
  eye, before noise reduction, tone, colour, masks, detail and effects; retouch stage, then background stage; the
  geometry slot (face reshape, next to Transform) is reserved, not run. Skipped, with every cache key unchanged,
  while the block is off or the Portrait section's eye is off. On the GPU, a retouched photo's scene-linear stage
  runs on the CPU.
- **Faces** (`crates/engine/src/retouch_input.rs`): a `FaceProvider` finds them on a 1600-pixel render of the photo
  before its crop, without retouching; the default is YuNet (Settings ▸ Faces: boxes and five landmarks, no masks),
  so features that need masks change nothing until the people parser (#81) installs its provider
  (`FaceStore::set_provider`). Found once per photo on a worker thread with the activity row "Finding Faces", kept
  in memory for the last 64 photos, with a round thumbnail per face; a face's person is the named People region it
  overlaps. Renders never wait (a preview without faces is redrawn once they are found: their fingerprint is in the
  render keys); exports and synchronous renders find them first.
- **Commands**: `retouch.faces {id?}` (the faces found, with `own`, the index of a face's own settings),
  `retouch.detect {id?, again?}`, `retouch.set {control, value, face?, id?}` (with `face`, that face gets its own
  settings, copied from every face's), `retouch.resetFace {face}`, `retouch.reset {feature?, face?}`. UI command
  `section.portrait`.
- **Portrait section** in the Edit panel (last, after Calibration): a face picker (All, then each face as a round
  thumbnail, its name on hover) and a collapsible row per feature, empty. Opening the section finds the photo's
  faces. The picker's choice is per photo and not saved; sliders in a feature's section change the picked face.

Mac only, like every fork feature; the web build compiles it but finds no faces.

## Branch

`feat/80-retouch-foundation`

## Own files

`crates/retouch/` (`Cargo.toml`, `README.md`, `src/{lib,control,feature,input,overrides,run,settings}.rs`, one
`src/<feature>.rs`, `examples/<feature>.rs` and `tests/<feature>.rs` per feature, `examples/common/mod.rs`,
`tests/common/mod.rs`, `tests/foundation.rs`), `crates/develop/src/retouch.rs`, `crates/pipeline/src/retouch_step.rs`,
`crates/engine/src/retouch_input.rs`, `crates/engine/src/retouch_input/tests.rs`, `crates/engine/src/cmd/retouch.rs`,
`crates/ui-egui/src/panels/edit/portrait.rs`, `crates/ui-egui/src/panels/edit/portrait/<feature>.rs`,
`crates/ui-egui/src/tests/portrait.rs`, `crates/mcp/src/tests_retouch.rs`, `fork/retouch-foundation.md`,
`fork/tests.d/retouch-foundation.txt`

## Hooks in official files

- `Cargo.toml`, `Cargo.lock`: the workspace dependency `lightcraft-retouch`. `xtask/src/layers.rs`: `retouch` is
  layer 1, below `develop`.
- `crates/develop/Cargo.toml` (the dependency); `src/settings.rs` (field `DevelopSettings::retouch`, serialised as
  `fork_retouch`, and its default); `src/lib.rs` (`pub mod retouch`, `to_json_for_merge` states the block,
  `reset_section` resets it); `src/controls.rs` (`Section::Portrait`, and `get` / `set` / `find` / `in_section`
  fall back to the retouch controls); `src/presets.rs` (`SettingsGroup::Portrait`).
- `crates/pipeline/src/lib.rs`: `pub mod retouch_step`, `SourceInfo::retouch`, the step's key in `pre_nr_key`, the
  step in `lin_cpu` and `lin_needs_cpu`. `crates/gpu/src/render.rs` (`linear`): a retouched photo's scene-linear
  stage on the CPU.
- `crates/engine/src/lib.rs` (`pub mod retouch_input`, `Session::retouch_faces`); `src/cmd.rs` (`mod retouch`, its
  specs); `src/cmd/query.rs` (`develop.controls` lists the retouch controls); `src/media/session.rs` (`build_job`,
  `variant_job`, `view_key` and `quick_view_job` add the faces and their salt; `render_now` and `export_job` find
  the faces first); `src/media/source.rs` (`info_or` keeps the faces); `SourceInfo` literals in
  `src/files/load.rs` and `src/smart.rs`.
- `crates/ui-egui/src/panels/edit.rs` (`mod portrait`, the section drawn after Calibration),
  `src/menus/ui_commands.rs` (`section.portrait`), `src/tests.rs` (`mod portrait`).
- `crates/mcp/src/lib.rs` (`mod tests_retouch`).

On `custom` the Edit panel's sections are arranged by the user (`fork/section-arrange.md`): the merge keeps the
Portrait section after the arranged ones, before `ui.add_space(40.0)`.

## Tests

`fork/tests.d/retouch-foundation.txt`:

```bash
cargo test -p lightcraft-retouch
cargo test -p lightcraft-develop --lib -- retouch
cargo test -p lightcraft-pipeline --lib -- retouch_step
cargo test -p lightcraft-engine --lib -- retouch_input
cargo test -p lightcraft-mcp --lib -- tests_retouch
cargo test -p lightcraft-ui-egui --lib -- portrait
```

They cover: the all-off block renders byte for byte like none (CPU, 8-bit and float) and keeps every cache key; the
block through JSON, the sidecar (also rewritten by another app), the catalog, copy / paste, sync and presets; a face's
own settings following its person when the faces come back in another order; MCP reading and setting a value; the
Portrait section's rows and face picker; no panic with no faces, 50 faces, a failing provider or a 1 × 1 photo.
No test needs a model: the engine and UI tests install a fake provider.

## Parity rows

| LR-EDIT-PORTRAIT | Portrait retouching | — | 🟡 | `cmd:retouch.faces`, `cmd:retouch.detect`, `cmd:retouch.set`, `cmd:retouch.resetFace`, `cmd:retouch.reset`, `ctl:retouch.*`, `crates/retouch/src/lib.rs`, `crates/pipeline/src/retouch_step.rs`, `crates/engine/src/retouch_input.rs`, `crates/ui-egui/src/panels/edit/portrait.rs` | an extra (the reference app has no portrait retouching): the base only (issue #80). Settings for every face and per face (kept by person or place), one pipeline step, faces from YuNet, the Portrait section with a face picker; every feature (skin, blemishes, wrinkles, skin tone, eyes, teeth and lips, glare, stray hair, reshape, background) is a stub that changes nothing yet. See `fork/retouch-foundation.md` |
