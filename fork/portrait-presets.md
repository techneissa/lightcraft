# Portrait presets that follow the person

List: Save Portrait Preset from one face's retouch settings (optionally with the photo's other settings); applying it changes only the same person's face on each photo (by name, by face recognition, or a photo's only face); Apply to Every Photo of a person from the Portrait tab and the People view, one undo step per photo (issue #106). Branch `feat/106-portrait-presets`.

## What it does

- **Save**: the Portrait tab's Portrait Presets section saves the picked face's settings (or, with All picked, the
  settings for every face) under a name, in the Presets panel's **Portrait** group. The preset carries the face's
  whole retouch block (every feature, as `lightcraft_retouch::Features` serializes it, so Teeth and Lips, Skin Tone,
  Blemishes, Wrinkles, Background, Reshape and any later feature are carried with no change here) and who the face
  is: the person's name when the face is named, and its recognition embedding when the face index has one (with
  the model it came from). Include Other Settings adds the photo's other develop settings of the default copy groups.
  Spots, red eye, the Portrait group's per-face block and Refine brush strokes are never in it.
- **Apply** (`preset.apply`, the Presets panel, the section's Apply): on each photo the preset's face settings
  become the same person's face's own settings; every other face keeps what it had. The person is found among the
  photo's face regions in the catalog, in this order: a face named after them; else the unnamed face whose
  embedding is most like theirs (the preset's own and up to 512 of their named faces', above the recognition
  model's match threshold, 0.45 without one); else, on a photo with one face, that face unless it is named as
  someone else. A photo without the person is listed as skipped and keeps its faces as they were; the preset's
  other settings still apply. The hover preview shows the same (without the library's named faces, to stay cheap).
- **Apply to Every Photo of a person** (`retouch.applyToPerson`): from the picked face (its own settings, or a
  preset) or from a person's page in the People view (the Apply Portrait Preset menu). Every photo where they are
  named or recognised gets them, as one undo step per photo; a photo's only face counts only when it is them, and
  photos that already have exactly those settings make no step.
- **Speed**: applying sets values only. The face is matched on the catalog's face regions and the embeddings the
  background face scan made ahead of time; no model runs and no pixel is read. The masks come from the people
  parse (cached, or parsed ahead: `fork/parse-ahead.md`) when the photo next renders.
- **Commands** (and MCP tools): `retouch.savePreset {name, face?, id?, group?, others?, groups?}`, `preset.apply`
  (its result has `portrait: {applied, skipped: [{id, why}]}`), `retouch.applyToPerson {name? | face? + id?,
  preset?}`.
- **Keys**: a face that gets the settings is keyed as portrait retouching keys it (`FaceKey`): by its person when
  named, else by its region's box on the photo before the user's rotation, in the people parse's frame
  (`KeyFrame::Photo`).
- No new develop settings field and no catalog format change: the face part lives in the preset's own settings
  JSON (`fork_portrait`), which a build without this feature ignores (it then applies the rest of the preset).

Speed (Apple M4 Pro, release, load average 14–20; library on disk with 60 photos, 50 of them with Ann next to Bob;
`cargo run --release -p lightcraft-engine --example portrait_preset_speed -- OUT_DIR`, median of 7):
`preset.apply` to 1 photo 0.04 ms, to 50 photos 1.8 ms; `retouch.applyToPerson` over the 60, 50 changed (50 undo
steps) 1.5 ms. The catalog writes the steps to its journal later, as it does every edit.

## Branch

`feat/106-portrait-presets`

## Own files

`crates/engine/src/portrait_presets.rs`, `crates/engine/src/cmd/portrait_presets.rs`,
`crates/engine/src/tests/portrait_presets.rs`, `crates/engine/examples/portrait_preset_speed.rs`,
`crates/ui-egui/src/tests/portrait_presets.rs`, `fork/portrait-presets.md`, `fork/tests.d/portrait-presets.txt`

## Hooks in official files

- `crates/engine/src/cmd/develop.rs` (`preset.apply`: the face part through `PresetFaces`, its `portrait` result),
  `crates/engine/src/presets.rs` (`Session::preset_applied`: the face part in the hover preview),
  `crates/engine/src/lib.rs` and `crates/engine/src/cmd.rs` (`mod` lines, the specs), the `mod` lines of
  `crates/engine/src/tests.rs` and `crates/ui-egui/src/tests.rs`, `crates/ui-egui/src/panels/person.rs` (the Apply
  Portrait Preset menu on a person's page).
- Fork files: `crates/ui-egui/src/panels/portrait/presets.rs` (the section's body), `crates/ui-egui/src/panels/portrait.rs`
  (`apply_preset_menu`), `crates/ui-egui/src/panels/portrait/overlay.rs` (`title_case` shared),
  `crates/retouch/README.md` (presets carry `Features`).

## Tests

`fork/tests.d/portrait-presets.txt`. They cover: a preset saved from Ann's face, applied to a group photo of Ann and
Bob the other way round, changing only Ann's face (and a photo with two others skipped); Apply to Every Photo of Ann
reaching the three photos she is named on and leaving Bob alone, an unnamed lone face and a photo without faces,
with one undo step per changed photo; the picked face's own settings to the person's other photos; an unnamed face
matched by embedding on a photo with two unnamed faces, a photo of two strangers skipped, a lone face taking it; the
preset through `preset.export` and `preset.import` into another session, applying the same; every feature carried,
other settings only when asked, never spots, red eye or the Portrait block; a malformed face part with no panic; the
hover preview equal to the click; in the app, Save Portrait Preset from a picked face, Apply to Every Photo of Ann,
and the People view's menu.

## What's New entry

### Portrait Presets That Follow the Person
- Save a face's portrait settings as a preset from the Portrait tab's Portrait Presets section; it appears in the
  Presets panel's Portrait group. Tick Include Other Settings to keep the photo's other settings in it too.
- Applying it changes only the same person's face on each photo, found by name or by face recognition; other people
  in the photo are left alone. On a photo with one face, it goes to that face.
- Apply to Every Photo of a person, from the Portrait tab or from their page in People, gives the look to every
  photo they are on, one undo step per photo.

## Parity rows

| LR-EDIT-PORTRAIT-PRESETS | Portrait presets that follow the person | — | 🟡 | `cmd:retouch.savePreset`, `cmd:retouch.applyToPerson`, `cmd:preset.apply`, `crates/engine/src/portrait_presets.rs`, `crates/ui-egui/src/panels/portrait/presets.rs` | an extra (the reference app has no portrait retouching). Recognition of unnamed faces needs a face recognition model in Settings ▸ Faces. See `fork/portrait-presets.md` |
