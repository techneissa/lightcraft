# Portrait presets that follow the person

List: Save Portrait Preset from one face's retouch settings (optionally the whole look); applying it changes only the same person's face on each photo (by name, or by face recognition ranked against every named person); an All Faces preset goes to any face; Apply to Every Photo of a person from the Portrait tab and the People view, as one undo step, in the background past a handful of photos (issue #106). Branch `feat/106-portrait-presets`.

## What it does

- **Save** (Portrait tab ▸ Portrait Presets): saves the picked face's settings under a name, in the Presets panel's
  **Portrait** group. Its row there says whom it follows. With a face picked the preset follows that person: their
  name and an id that stays with the preset, and the face's recognition embedding when the face index has one.
  With All picked it holds the settings for every face and follows nobody: an **All Faces** preset, given to every
  face of the photos it is applied to; Apply to Every Photo is not offered for it.
- **Face settings only by default.** Save the Whole Look adds the photo's other
  develop settings of the default copy groups go in too. Never local masks, spots, red eye or the per-face block.
- **What a preset carries of a face** (`carry`): per feature, the fields that are numbers (sliders), switches or
  lists of up to four numbers (a colour), so Teeth and Lips, Skin Tone, Blemishes, Face Reshape and every later
  slider or switch are carried with no change here. Lists of points or shapes (Refine brush strokes, spots) and any
  field whose name says stroke, spot, mark, brush, refine or mask are never carried. Applying keeps the face's own
  values of what a preset does not carry.
- **Apply** (`preset.apply`, the Presets panel, the section's Apply): the preset's face settings become the same
  person's face's own settings; every other face keeps what it had. The person is found among the photo's face
  regions in the catalog: a face named after them; else an unnamed face recognised as them, ranked against every
  named person's faces (and the preset's own embedding) and taken only when it passes the model's match threshold
  (0.45 without one) and leads the next person by 0.06, as name suggestions do. A photo's only face takes it only
  when it can't be compared (no embedding on the face, and none of the person's) and isn't named as someone else,
  and only in an explicit apply to photos the user picked. A photo without the person is reported as skipped; the
  preset's other settings still apply. The hover preview uses the same matching as the click.
- **Apply to Every Photo of a person** (`retouch.applyToPerson`): from the picked face (its own settings) or a
  person's page in People (Apply Portrait Preset). Every photo where they are named or recognised gets it, as **one
  undo step**; a photo's only face never counts unless it is them. Over 24 photos with faces the matching runs on a
  worker with an activity row ("Applying portrait settings", ✕ stops it with nothing changed), and the result is
  committed when it is done. The commit is one batch, rolled back whole if it fails; a photo that can't take it (too
  many faces with their own settings) is listed, and the toast says how many.
- **Renaming** a person (`faces.renamePerson`, also a merge) moves their presets to the new name with the same id; a
  person given the old name afterwards is someone new and does not get them. Undoing the rename leaves the presets
  on the new name.
- **Leaving this Mac**: an exported file (`preset.export`) has neither the face's embedding nor the person, so it
  imports as an All Faces preset. Settings Sync keeps the person (name and id) for the user's other Macs, never the
  embedding. The local preset store keeps both.
- **Speed**: applying sets values only. Faces are matched on the catalog's face regions and the embeddings the
  background face scan made ahead of time; no model runs and no pixel is read. The library's named people are
  gathered once and kept until a name, a photo or an embedding changes, with each photo's pick. The masks come from
  the people parse (cached, or parsed ahead: `fork/parse-ahead.md`) when the photo next renders.
- **Commands** (and MCP tools): `retouch.savePreset {name, face?, id?, group?, others?, groups?}`, `preset.apply`
  (result: `portrait: {applied, skipped: [{id, why}], anyFace}`), `retouch.applyToPerson {name? | face? + id? |
  preset, wait?}`, `retouch.applyStatus {wait?, timeoutMs?, cancel?}`.
- No new develop settings field and no catalog format change: the face part lives in the preset's own JSON
  (`fork_portrait`), which a build without this feature ignores (it then applies the rest of the preset).

Speed (a test Mac, release, under load from other builds; `cargo run --release -p lightcraft-engine --example
portrait_preset_speed -- OUT_DIR`, median of 7): `preset.apply` to 1 photo 0.15 ms, to 50 photos 2.4 ms;
`retouch.applyToPerson` over a library of 2 060 photos, 2 000 with Ann: 1.4 ms on the UI thread to start, 17 ms on
the worker, 18 ms on the UI thread to commit the one undo step.

## Branch

`feat/106-portrait-presets`

## Own files

`crates/engine/src/portrait_presets.rs`, `crates/engine/src/portrait_presets/{matching,person}.rs`,
`crates/engine/src/cmd/portrait_presets.rs`, `crates/engine/src/tests/portrait_presets.rs`,
`crates/engine/examples/portrait_preset_speed.rs`, `crates/ui-egui/src/tests/portrait_presets.rs`,
`fork/portrait-presets.md`, `fork/tests.d/portrait-presets.txt`

## Hooks in official files

- `crates/engine/src/cmd/develop.rs` (`preset.apply`: the face part through `PresetFaces`, its `portrait` result),
  `crates/engine/src/presets.rs` (`Session::preset_applied`: the face part in the hover preview; `to_lcpreset`
  without the face's identity, `to_lcpreset_synced` for Settings Sync), `crates/engine/src/lib.rs` (the module, the
  `Session::portrait_presets` field), `crates/engine/src/cmd.rs` (the module, the specs),
  `crates/engine/src/cmd/face_recognize.rs` (`faces.renamePerson` moves presets along), the `mod` lines of
  `crates/engine/src/tests.rs` and `crates/ui-egui/src/tests.rs`, `crates/ui-egui/src/panels/person.rs` (the Apply
  Portrait Preset menu), `crates/ui-egui/src/panels/presets.rs` (the row's tag),
  `crates/ui-egui/src/app/frame.rs` (the per-frame pump), `crates/ui-egui/src/settings_sync/rules.rs` (presets synced
  with `to_lcpreset_synced`), `docs/parity-checklist.md` (the `LR-EDIT-PORTRAIT-PRESETS` row).
- Fork files: `crates/ui-egui/src/panels/portrait/presets.rs` (the section), `crates/ui-egui/src/panels/portrait.rs`
  (`apply_preset_menu`, `pump`, `preset_tag`), `crates/ui-egui/src/panels/portrait/overlay.rs` (`title_case`
  shared), `crates/retouch/README.md` (what a preset carries).

## Tests

`fork/tests.d/portrait-presets.txt`. They cover: a preset saved from Ann's face, applied to a group photo of Ann and
Bob the other way round, changing only Ann's face (and a photo with two others skipped); Apply to Every Photo of Ann
reaching the photos she is named on and leaving Bob alone, an unnamed lone face and a photo without faces, as one
undo step; the picked face's own settings to the person's other photos; an unnamed face matched by embedding, two
strangers skipped, a lone face that can't be compared taking it and a lone stranger scoring 0.1 turned down; a face
0.50 like Ann and 0.90 like Bob not taken for Ann; an exported file with neither embedding nor person, importing as
an All Faces preset, and Settings Sync keeping the person without the embedding; an All Faces preset going to every
face and Apply to Every Photo refused for it; a rename keeping the preset and a new person of the old name not
getting it; every feature carried, other settings only when asked, never masks, spots, red eye or the Portrait
block; strokes, spots and mask fields dropped while switches and colours stay; a malformed face part with no panic;
the hover preview equal to the click with a face only the library's named faces recognise; a photo that can't take
it reported while the others change; past 24 photos a worker with an activity row, one undo step, one run at a
time, and a cancel; in the app, Save Portrait Preset from a picked face, Apply to Every Photo of Ann, the People
view's menu, and the toast's wording.

## What's New entry

### Portrait Presets That Follow the Person
- Save a face's portrait settings as a preset from the Portrait tab's Portrait Presets section; it appears in the
  Presets panel's Portrait group with the name of the person it follows. Tick Save the Whole Look to
  save the whole look.
- Applying it changes only the same person's face on each photo, found by name or by face recognition; other people
  in the photo are left alone. A preset saved from All Faces goes to every face.
- Apply to Every Photo of a person, from the Portrait tab or from their page in People, gives the look to every
  photo they are on in one step you can undo.
