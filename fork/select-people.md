# Select People: masks of people's parts

List: Select People masks: Masking ▸ Person / Background / Custom and a picker on the photo (All or one face, then Entire Person, Entire Face, Facial / Body Skin, Hair, Eyebrows, Eyes (Whites), Iris, Lips, Teeth, Clothes and its kinds, Background; several with ⌘-click), each an ordinary mask drawn from the people parse (part of issue #79). Branch `feat/select-people`.

## What it does

- **Masking ▸ Create New Mask** starts with a choice of **Person**, **Background** and **Custom**. Person adds an
  Entire Person mask of everyone (of the face picked last on this photo), Background a mask of everything that is
  not a person; Custom shows the usual tiles (Object, Describe, Subject, Brush…). Under Person and Background a line
  says what to do, that people are being found, or why there are none (with Settings… when the models are not
  downloaded). The choice is not saved: the panel opens on Custom.
- **The picker on the photo**: the people picker of the Portrait tab (`fork/retouch-foundation.md`) at the image
  view's top-right corner, while Person or Background is chosen or a Select People mask is selected. Row one:
  All and a round chip per person (their name from People on hover, else "Person 2"). Then the parts: Entire Person,
  Entire Face (facial skin, eyebrows, eye whites, irises, lips and teeth together), Facial Skin, Body Skin, Hair,
  Eyebrows, Eyes (Whites), Iris, Lips, Teeth, Clothes (all of them), Top, Bottom, Dress, Shoes, Hat, Bag,
  Background. A click picks one part, ⌘-click adds or removes one (the last one stays); a chip picks whose. With a
  Select People mask selected, a click changes that mask (one undo step, "Select People"); without one, it makes a
  new mask. It folds into its "⋯" button (kept in the saved `openFlyouts` as `selectPeople.folded`, an existing
  `ui.json` key), hides with the tool overlay (View ▸ Tool Overlay ▸ Never) and in the full-screen preview.
- **An ordinary mask**: the parts become a `MaskShape::People` part with a stored selection, like Subject's: one grid
  over the selection (64 to 1024 cells a side, as fine as the parse's own planes) and up to four finer grids over the
  people it draws too coarsely (small faces in a group), marked `people:<set>`. Every mask slider, Edge, Refine
  Edges, Invert, Recolour and the mask effects work on it, and a render never needs the model, so preview and export
  draw the same mask. Entire Person and Background come from the parse's Entire Person, so they have BiRefNet Matting
  edges when Settings ▸ Faces ▸ Hair Edges chooses them for people (`fork/hair-edges.md`); such a grid is marked
  `+matting` and not filtered again. Names: "Hair Mask 1", "Hair + Lips Mask 1", "Hair + 2 More Mask 1"; a generated
  name follows the parts when they change, a name the user gave stays. Rotate Left / Right moves the grids with the
  photo.
- **Faces**: a mask keeps its face as portrait retouching does: by the person's name when the face is named in
  People, else by its box on the photo before the user's rotation; found again by name, else by the closest box
  centre within half the box, never as someone named otherwise.
- **People are found** once per photo off the main thread: the photo is rendered as the parse sees it and its
  faces found (a row "Finding people"), then the people parse runs with its own row or comes from its cache
  (`fork/people-parsing.md`). A mask asked for before then is made when they are found. The newest eight photos'
  people stay in memory. Nothing runs while a slider moves: a mask's grids are drawn once, when its parts or face
  are picked.
- **Commands**: `selectPeople.people {id?, wait?}`, `selectPeople.add {parts?, face?, name?}`,
  `selectPeople.set {id?, component?, parts?, face?}` (MCP and the control channel reach them).
- **Never crashes**: no people (no person part; Background is the whole photo), 50 people, a face past the photo's
  edge, damaged planes, a person who is not there and unknown parts are tested; a failed parse is a message.

How it does (a CC0 photo from Wikimedia Commons and techneissa's A7 V photos, 2026-11-09): faces, hair, lips and
irises land on the right places on single portraits and groups of three; clothes and hats follow the parse. What
the parse gets wrong shows in the mask: on the CC0 couple, the parse's own Clothes Top and Entire Person have a
thin vertical seam where one person's crop ends (also in `parsing.mask`), and on a group photo parts of a turned-away
person's shirt count as background.

Mac only, like every fork feature.

## Branch

`feat/select-people`

## Own files

`crates/engine/src/select_people.rs`, `crates/engine/src/select_people/{grid,parts,tests}.rs`,
`crates/engine/src/cmd/select_people.rs`, `crates/ui-egui/src/panels/select_people.rs`,
`crates/ui-egui/src/panels/select_people/tests.rs`, `fork/select-people.md`, `fork/tests.d/select-people.txt`

## Hooks in official files

- `crates/develop/src/settings.rs`: `MaskShape::People` gains `face`, `seg`, `detail` and `edge` (all optional: an
  older file reads as before, another build reads the part as the old People shape); `MaskShape::people` makes one
  without them; `MaskShape::seg` returns its selection.
- `crates/pipeline/src/masks.rs`: a People part with a stored selection samples it like Subject (and the source's
  mattes do not replace it). `crates/pipeline/src/masks/key.rs`: its alpha key hashes the selection.
  `crates/pipeline/src/mask_space.rs`: Rotate Left / Right resamples it. `masks/tests.rs`, `masks/tests_cache.rs`:
  `MaskShape::people(..)`.
- `crates/engine/src/lib.rs`: `pub mod select_people`, field `Session::select_people`. `crates/engine/src/cmd.rs`:
  `mod select_people`, its specs.
- `crates/ui-egui/src/panels.rs` (`pub mod select_people`), `crates/ui-egui/src/panels/masking.rs` (the mode row
  under Create New Mask, the tiles only under Custom, the Edge slider for a Select People part),
  `crates/ui-egui/src/panels/detail.rs` (the picker drawn after the Portrait one).
- `apps/lightcraft-cli/src/main.rs`: `snapshot` sets up the people-parsing models as the app does.
- `docs/parity-checklist.md`: the LR-MASK-PEOPLE row, changed in place.
- Fork files of other features: `crates/engine/src/cmd/parsing.rs` (`hints_for`, the faces as parse hints, shared
  so both make the same parse cache key), `crates/engine/src/people_parsing.rs` (`set_runner` also under
  `test-hooks`), `crates/ui-egui/src/panels/detail/people_picker.rs` (`Picker::note`, a line under the faces) and its
  callers (`portrait/overlay.rs`, `people_picker/tests.rs`).

## Tests

`fork/tests.d/select-people.txt`:

```bash
cargo test -p lightcraft-engine --lib select_people
cargo test -p lightcraft-ui-egui --lib -- select_people people_picker
cargo test -p lightcraft-pipeline --lib masks
```

They cover: each part makes a mask of its own place, several parts one mask, one face or everyone (and one person's
background holding the others), no people, 50 people (at most four finer grids), a face past the edge, damaged
planes, a turned photo, faces found again by name or place, the commands (add, change, generated names, undo, what
is refused), preview and export drawing the same mask, the mask after closing and reopening the library, and in the
UI: Person, Background and Custom, the picker making and changing masks, ⌘-click, folding, hiding with the tool
overlay, and a photo without people. A fake runner stands in for the parser: no test needs a model.
Pictures: `lightcraft-cli snapshot --script <script>.jsonl -o out.png PHOTO` with `ui.clickWidget` on
`button:maskMode-person`, `selectPeople:part:<id>` (`"cmd": true` adds) and `selectPeople:face:<n>`.

## What's New entry

### Select People Masks
- Masking ▸ Create New Mask now starts with Person, Background and Custom. Person and Background make a mask of people or of everything around them; Custom has the usual tools.
- A picker on the photo chooses whose (everyone or one face) and which parts: Entire Person, Entire Face, Facial and Body Skin, Hair, Eyebrows, Eyes, Iris, Lips, Teeth, Clothes and its kinds, Background. ⌘-click adds more parts to the same mask.
- These are ordinary masks: every mask slider, Refine Edges and Recolour work on them, and exports match the preview.
