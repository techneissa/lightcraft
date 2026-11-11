# Select People: masks of people's parts

List: Select People masks: Masking ▸ Person / Background / Custom and a picker on the photo (All or one face, then Entire Person, Entire Face, Facial / Body Skin, Hair, Eyebrows, Eyes (Whites), Iris, Lips, Teeth, Clothes and its kinds, Background; several with ⌘-click), each an ordinary mask drawn from the people parse (part of issue #79). Branch `feat/select-people`.

## What it does

- **Masking ▸ Create New Mask** starts with a choice of **Person**, **Background** and **Custom**. Person adds an
  Entire Person mask of everyone (of the face picked last on this photo), Background a mask of everything that is
  not a person; Custom shows the usual tiles (Object, Describe, Subject, Brush…). Under Person and Background a line
  says what to do, that people are being found, or why there are none (with Settings… when the models are not
  downloaded). The choice is not saved: the panel opens on Custom.
- **The picker on the photo**: the people picker of the Portrait tab (`fork/retouch-foundation.md`) at the image
  view's top-right corner, while Person or Background is chosen or a Select People mask is selected. Row one: All
  and a round chip per person (their name from People on hover, else "Person 2"). Row two, under the chips and
  left-aligned with them: a dropdown naming the parts picked ("Entire Person", "Hair + Lips", "3 Parts"). Its menu
  groups the parts: Person (Entire Person, Entire Face (facial skin, eyebrows, eye whites, irises, lips and teeth
  together), Facial Skin, Body Skin), Face (Hair, Eyebrows, Eyes (Whites), Iris, Lips, Teeth), Clothes (Clothes (all of
  them), Top, Bottom, Dress, Shoes, Hat, Bag), Background, a check mark on each one the selected mask has and a
  small dot after the name of each one that has a mask of its own (for the face shown). A click picks one part and
  closes the menu, ⌘-click adds or removes one (the last one stays) and keeps it open; ↑/↓ and ↩ work in it, Esc
  closes it. A chip picks whose. It folds into its "⋯" button (kept in the saved `openFlyouts` as
  `selectPeople.folded`, an existing `ui.json` key), hides with the tool overlay (View ▸ Tool Overlay ▸ Never) and
  in the full-screen preview.
- **One mask per part pick** (`selectPeople.pick`), so one photo can carry several masks with different edits on
  different parts (Hair with Exposure, Lips with Saturation). A mask counts as a part's mask when one of its Select
  People parts is exactly that part and face; other parts (a brush, say) do not matter (they get the dot, and a
  click selects it). A plain click on a part: when such a mask is on the photo, it is selected (not an edit, no undo
  step; an empty mask the picker made and had selected is deleted in the same undo step); else a new mask for it is
  made and selected (one undo step) and the mask selected before keeps its edits. The exception that keeps the
  list clean: a selected Select People mask the picker made and nobody edited is re-targeted instead of left
  empty. "Unedited" is strict: visible, one part only (named by nobody, mode Add, not inverted), generated name,
  the effects a new mask starts with (Recolour and Amount included), no Edge, Refine Edges, Decontaminate or Invert.
  ⌘-click adds or removes the part in the selected mask (how one mask covers several parts); it is refused with
  "Hair already has a mask" when the result would be exactly the parts and face of another mask. A face chip
  changes whose the selected mask is, unless a mask of those parts and that face exists, then that one is selected
  (the same empty-mask rule); with no mask selected the chip makes an Entire Person mask. A selected mask whose face
  is not in the photo shows no chip; a plain click then makes the new mask for everyone. Masking ▸ Person and
  Background always add a mask. `selectPeople.pick` checks its input: `how` must be a string (part, add or face),
  `add` needs `parts` (the whole new list), `face: null` with `add` keeps the mask's face (`"all"` is everyone's),
  an unknown part or a face past the people is an error.
- **An ordinary mask**: the parts become a `MaskShape::People` part with a stored selection, like Subject's: one grid
  over the selection (64 to 1024 cells a side, as fine as the parse's own planes) and up to four finer grids over the
  people it draws too coarsely (small faces in a group), marked `people:<set>`. Every mask slider, Edge, Refine
  Edges, Invert, Recolour and the mask effects work on it, and a render never needs the model, so preview and export
  draw the same mask. Entire Person and Background come from the parse's Entire Person, and Hair gains the strands
  its edge finds, so they have BiRefNet Matting or Apple Vision edges when Settings ▸ Faces ▸ Hair Edges chooses one
  for people (`fork/hair-edges.md`); a selection with any of the three is marked `+matting` or `+vision` and not
  filtered again at any size. Other grids are filtered with the window of the grid that gave each pixel its logit
  (a small face's finer grid keeps its edge). Entire Person with Background selects the whole photo. A finer grid
  that would be no finer than the main one is not stored. Names: "Hair Mask 1", "Hair + Lips Mask 1", "Hair + 2 More Mask 1"; a generated
  name follows the parts when they change, a name the user gave stays. Rotate Left / Right moves the grids with the
  photo. A click that leaves the parts, the face and the parse as they are changes nothing (no undo step).
- **Faces**: a mask keeps its face as portrait retouching does: by the person's name when the face is named in
  People, else by its box on the photo before the user's rotation; found again by name, else by the closest box
  centre within half the box, never as someone named otherwise. A face named or renamed in People since its people
  were found makes them be found again (the names are part of the key they are kept under). When the photo's people
  don't have the mask's face, the picker picks no chip and says "Face not in this photo".
- **Another photo** (Paste Settings, Paste from Previous, Sync Settings, Auto Sync, a preset): each Select People part
  that reaches a photo is drawn again from that photo's own people, its face found there by name or place (everyone's
  stays everyone's), in the background in the app (`fork/adaptive-masks.md`). A part the photo already had (same mask,
  parts, face and rotation) keeps its own grids. When the face is not there, or its people can't be found, the part
  selects nothing on that photo.
- **People are found** once per photo off the main thread: the photo is rendered as the parse sees it and its
  faces found (a row "Finding people"), then the people parse runs with its own row or comes from its cache
  (`fork/people-parsing.md`). The search starts when the Masking panel opens on the photo (with Person or Background
  chosen or a Select People mask selected) or on a click, never from a frame. A mask asked for before then is made
  when they are found, also after showing another photo meanwhile (one per photo). A command (`selectPeople.add` /
  `set`, Paste) during a search waits for it and uses its result; only the search started last files its result. The newest eight photos'
  people stay in memory. Nothing runs while a slider moves: a mask's grids are drawn once, when its parts or face
  are picked.
- **Commands**: `selectPeople.people {id?, wait?}`, `selectPeople.add {parts?, face?, name?}`,
  `selectPeople.set {id?, component?, parts?, face?}`, `selectPeople.pick {parts, face?, how?: part|add|face}` (what a
  click in the picker does, above; MCP and the control channel reach them).
- **Never crashes**: no people (no person part; Background is the whole photo), 50 people, a face past the photo's
  edge, damaged planes, a person who is not there and unknown parts are tested; a failed parse is a message.

How it does (a CC0 photo from Wikimedia Commons and private test photos, 2026-11-09): faces, hair, lips and
irises land on the right places on single portraits and groups of three; clothes and hats follow the parse. What
the parse gets wrong shows in the mask: on the CC0 couple, the parse's own Clothes Top and Entire Person have a
thin vertical seam where one person's crop ends (also in `parsing.mask`), and on a group photo parts of a turned-away
person's shirt count as background.

Mac only, like every fork feature.

## Branch

`feat/select-people`

## Own files

`crates/engine/src/select_people.rs`, `crates/engine/src/select_people/{grid,parts,tests,tests_round,tests_pick}.rs`,
`crates/engine/src/cmd/select_people.rs`, `crates/ui-egui/src/panels/select_people.rs`,
`crates/ui-egui/src/panels/select_people/{tests,tests_round,tests_per_part}.rs`, `crates/pipeline/src/masks/{people,tests_people}.rs`,
`fork/select-people.md`, `fork/tests.d/select-people.txt`

## Hooks in official files

- `crates/develop/src/settings.rs`: `MaskShape::People` gains `face`, `seg`, `detail` and `edge` (all optional: an
  older file reads as before, another build reads the part as the old People shape); `MaskShape::people` makes one
  without them; `MaskShape::seg` returns its selection; `visit_shape` / `visit_shape_mut` reach its grids, so a
  snapshot keeps each once in the shared grid table. Saved as fork catalog format 5 (`crates/catalog`:
  `fork_format::VERSION`, the format table in `journal.rs`, `tests_format_version.rs`, `tests_fork_format.rs`).
- `crates/pipeline/src/masks.rs`: a People part with a stored selection samples it like Subject (and the source's
  mattes do not replace it), and its grids are filtered by `masks/people.rs`. `crates/pipeline/src/masks/key.rs`: its alpha key hashes the selection.
  `crates/pipeline/src/mask_space.rs`: Rotate Left / Right resamples it. `masks/tests.rs`, `masks/tests_cache.rs`:
  `MaskShape::people(..)`.
- `crates/engine/src/lib.rs`: `pub mod select_people`, field `Session::select_people`. Paste, Sync, Auto Sync and
  presets draw Select People parts from each photo's people through `fork/adaptive-masks.md` (`crate::adaptive`). `crates/engine/src/cmd.rs`:
  `mod select_people`, its specs.
- `crates/ui-egui/src/panels.rs` (`pub mod select_people`), `crates/ui-egui/src/panels/masking.rs` (the mode row
  under Create New Mask, the tiles only under Custom, the Edge slider for a Select People part),
  `crates/ui-egui/src/panels/detail.rs` (the picker drawn after the Portrait one).
- `apps/lightcraft-cli/src/main.rs`: `snapshot` sets up the people-parsing models as the app does.
- `docs/parity-checklist.md`: the LR-MASK-PEOPLE row, changed in place.
- Fork files of other features: `crates/engine/src/cmd/parsing.rs` (`hints_for`, the faces as parse hints, shared
  so both make the same parse cache key), `crates/engine/src/people_parsing.rs` (`set_runner` also under
  `test-hooks`), `crates/engine/src/tests/decontaminate.rs` (`MaskShape::people(..)`),
  `crates/ui-egui/src/panels/detail/people_picker.rs` (`Picker::note`, a line under the faces; a `face` past the
  chips picks none) and its
  callers (`portrait/overlay.rs`, `people_picker/tests.rs`); the parts dropdown in `people_picker/parts_menu.rs`
  (`PartItem::group` for its headers, `PartItem::marked` for the dot; the dropdown on the second row).
- `crates/ui-egui/src/widgets.rs` (`register_opener`, `open_menu_for`) and `crates/ui-egui/src/app/control.rs`
  (`resolve_pending_gestures`): `ui.clickWidget` on a row of the closed parts menu opens it first.

## Tests

`fork/tests.d/select-people.txt`:

```bash
cargo test -p lightcraft-engine --lib select_people
cargo test -p lightcraft-ui-egui --lib -- select_people people_picker
cargo test -p lightcraft-pipeline --lib masks
```

They cover (`tests_round` the review round's: Paste, Sync and Auto Sync on a second photo, a face not there, the
edge marks, Entire Person with Background, a command waiting for the running search, a renamed face, a click that
changes nothing, no copy of the main grid, a finer grid's own filter window, and in the picker the missing face,
a pending mask after another photo, one set of face textures, the search starting on opening; `tests_pick` and
`tests_per_part` the one-mask-per-part rules, in the engine and through the picker: a plain click after edits makes a
second mask and leaves the first one's edits, a click on a part that has a mask selects it (also with other parts in
it), the empty mask deleted with it, ⌘-click adds to the selected mask or is refused as a copy, the unedited-mask
exception and each edit that stops it, a missing face, the input checks, the face chip, the dot, the dropdown under
the chips): each part makes a mask of its own place, several parts one mask, one face or everyone (and one person's
background holding the others), no people, 50 people (at most four finer grids), a face past the edge, damaged
planes, a turned photo, faces found again by name or place, the commands (add, change, generated names, undo, what
is refused), preview and export drawing the same mask, the mask after closing and reopening the library, and in the
UI: Person, Background and Custom, the picker making and changing masks, ⌘-click, folding, hiding with the tool
overlay, and a photo without people. A fake runner stands in for the parser: no test needs a model.
Pictures: `lightcraft-cli snapshot --script <script>.jsonl -o out.png PHOTO` with `ui.clickWidget` on
`button:maskMode-person`, `selectPeople:parts` (the dropdown), `selectPeople:part:<id>` (opens the menu when it is
closed; `"cmd": true` adds) and `selectPeople:face:<n>`.

## What's New entry

### Select People Masks
- Masking ▸ Create New Mask now starts with Person, Background and Custom. Person and Background make a mask of people or of everything around them; Custom has the usual tools.
- A picker on the photo chooses whose (everyone or one face) and which parts: Entire Person, Entire Face, Facial and Body Skin, Hair, Eyebrows, Eyes, Iris, Lips, Teeth, Clothes and its kinds, Background. Each part you pick gets a mask of its own with its own edits (Hair with Exposure, Lips with Saturation); picking a part that has a mask selects it. ⌘-click adds more parts to the same mask.
- These are ordinary masks: every mask slider, Refine Edges and Recolour work on them, and exports match the preview.
