# Object remove

List: Remove an object in one step, in its shape: click it or brush loosely over it (SAM 3), or turn any mask into a Remove spot with "Remove This Area"; LaMa Remove and Content-Aware spots have no source pin; `feat/88-object-remove`.

## What it does

- **Object mode**: the Remove panel shows **Select: Brush | Object** under the tools while Remove or Content-Aware is chosen. In Object mode a click on the photo selects the object under it with SAM 3 and one Remove spot in the object's shape takes it out. ⇧-click adds to the selected object spot, ⌥-click leaves a part out; each is one undo step. A loose stroke over an object snaps to it: its points become SAM 3 clicks, and the selection is cut to within 2.5 brush radii of the stroke. In the app the model works in the background (a "Selecting with SAM 3" row in the activity stack, ✕ stops it); a ⇧ or ⌥ click while one is on its way replaces it with all the clicks.
- **Expand**: a shaped spot reaches past its outline by Expand (0.1–5 % of the long edge, 0.5 % by default), the spot's `size`. The Object mode slider sets it for new spots; with a shaped spot selected, the Size slider reads Expand.
- **Remove This Area**: in a mask's right-click menu. The mask (any kind: Object, Describe, Subject, Sky, Brush, gradients, ranges, the per-person parts when they land) is evaluated as the render evaluates it, on the uncropped photo, and becomes one Remove spot of its shape. The mask stays. A mask that selects nothing, or more than 60 % of the photo, is refused with a message.
- **Fill**: shaped spots fill with LaMa unless Classic is picked. Object mode has its own Fill choice (LaMa by default), separate from the brush's. Classic rebuilds a shaped spot with the content-aware fill over its box and half its size around it.
- **No source where it means nothing**: a Remove spot with the LaMa fill and every shaped spot pick no source and show no source pin; Content-Aware never had one. A Classic Remove spot still heals from its source and keeps its pin, as do Heal and Clone. Switching a spot to LaMa drops its source; switching a Remove spot back to Classic picks one. Refresh Source on a spot without a source says it has none.
- **Find People / Find Wires**: Remove and Remove All now make one spot in each found person's or wire's shape (Remove mode), instead of dozens of round dabs.
- **Without SAM 3**: a click in Object mode opens the SAM 3 download dialog, the panel shows "Download SAM 3 to Select Objects…", and a loose stroke stays a brush-shaped spot.
- **Rows and outlines**: a shaped spot's row says what it is ("Remove 3 · Object", "· Mask", "· Person", "· Wire"). On the photo it shows its outline and a pin in its middle instead of circles; dragging the pin moves the whole shape.
- **Stored with the spot**: `Spot::area` holds the kind, the coverage as a grid (a `SegMask` over the shape's box, in the spot's coordinates) and the clicks. It is left out of the file for stroke spots, so old edits save byte for byte as before. Rendering needs neither the model nor the mask, and is resolution independent: the alpha is the distance in output pixels to the outline, against a radius that scales with the render. The GPU leaves photos with a shaped spot to the CPU, as it does Content-Aware spots, so the two paths give the same picture.
- **Commands**: `spot.object {x, y, add?, subtract?, index?, expand?, fill?, seg?, wait?}`, `spot.objectStroke {points, size?, feather?, opacity?, expand?, fill?, seg?, wait?}`, `spot.fromMask {id?, expand?, fill?}`, all through MCP too. `seg` (a selection in the model's photo coordinates) stands in for SAM 3. `spot.update` refuses Heal or Clone and a source for a shaped spot; `move` moves its shape.

Issue #88 (and #70: a spot in a mask's shape covers most of what ellipse and egg shapes were for).

## Branch

`feat/88-object-remove`

## Own files

`crates/develop/src/spot_area.rs`, `crates/pipeline/src/spot_area.rs`, `crates/pipeline/src/tests_spot_area.rs`, `crates/pipeline/src/find/region.rs`, `crates/gpu/tests/it/shaped_spot.rs`, `crates/engine/src/spot_area.rs`, `crates/engine/src/segment/clicks_request.rs`, `crates/engine/src/cmd/spot_area.rs`, `crates/engine/src/tests/spot_area.rs`, `crates/ui-egui/src/panels/right/remove_object.rs`, `crates/ui-egui/src/panels/detail/object_remove.rs`, `crates/ui-egui/src/tests/object_remove.rs`

## Hooks in official files

- `crates/develop/src/settings.rs` (`Spot::area` at the end), `crates/develop/src/lib.rs` (`mod spot_area`, `SpotArea`).
- `crates/pipeline/src/spots.rs` (`layout` and `window_for_reads` take a shaped spot's box from `spot_area::box_points` with its context pad; a shaped spot is a fill with no heal; `apply_except` takes its alpha from `spot_area::alpha`), `crates/pipeline/src/spots_learned.rs` (the same box and alpha for LaMa), `crates/pipeline/src/lib.rs` (`pub mod spot_area`), `crates/pipeline/src/find/mod.rs` (`Candidate::region`, `pub mod region`), `find/people.rs` and `find/wires.rs` (the region of each candidate).
- `crates/gpu/src/lib.rs` (a shaped spot renders on the CPU), `crates/gpu/tests/it/main.rs` (`mod` line).
- `crates/engine/src/lib.rs` (`pub mod spot_area`, `Session::object_jobs`), `crates/engine/src/segment.rs` (`mod clicks_request`, `segment_poll` applies the Object jobs), `crates/engine/src/cmd.rs` (`mod spot_area` and its specs), `crates/engine/src/cmd/masks.rs` (`spot.add` gives a LaMa spot no source; `spot.update` asks `spot_area::update_source` and moves a shape; `spot.refreshSource` refuses a sourceless spot; `segment` and `pick_source` are `pub(super)`), `crates/engine/src/cmd/find_removals.rs` (a candidate with a region becomes one shaped spot), the tests of `find_removals` and `lama_fill` (expect shaped spots), the `mod` line of `crates/engine/src/tests.rs`.
- `crates/ui-egui/src/state.rs` (`remove_object`, `remove_object_fill`, `remove_expand`), `app.rs` (repaints while Object jobs run), `panels/right.rs` (`mod remove_object`), `panels/right/crop.rs` (the Brush | Object section; the Size slider reads Expand for a shaped spot), `panels/right/remove_fill.rs` (the picker shows and sets Object mode's own fill), `panels/right/spot_rows.rs` (the row names the shape), `panels/mask_menus.rs` (Remove This Area), `panels/detail.rs` (`mod object_remove`), `panels/detail/retouch.rs` (a shaped spot draws its outline; Object mode commits a click or stroke), the `Candidate` literals of two UI tests and the `mod` line of `crates/ui-egui/src/tests.rs`.
- `docs/parity-checklist.md`: LR-REM-DETECT changed in place.

## Tests

`fork/tests.d/object-remove.txt`: the stored shape and old spots saving byte for byte (develop); the shaped spot changes nothing past its outline and Expand, preview and export cover the same area, a window holds a shaped spot, hostile shapes draw nothing (pipeline); CPU and GPU agree (gpu); Remove This Area, clicks and ⇧/⌥ clicks as one undo step each, a stroke snapping near itself, no model, background requests, which spots have a source, found people as shaped spots (engine); Object mode, the download offer, the row name, no source pin, the mask menu entry (UI, headless). The model is never needed: its answers are given as `seg`.

## What's New entry

### Remove an Object in One Step
- The Remove panel has an Object mode: click an object and LightCraft selects it with SAM 3 and removes all of it with one spot in its shape. ⇧-click adds to it, ⌥-click leaves a part out, and a loose stroke over an object snaps to its outline. Expand sets how far past the outline the spot reaches.
- Any mask can become a Remove spot too: right-click it and choose Remove This Area.
- These spots fill with LaMa unless you pick Classic. Find People and Find Wires now remove each find with one spot in its shape.
- Remove spots that use LaMa, and Content-Aware spots, no longer show a source pin: they rebuild the area from what is around it.

## Parity rows

| LR-REM-DETECT | Object detection for remove | P2 | 🟡 | `cmd:spot.object`, `cmd:spot.objectStroke`, `cmd:spot.fromMask`, `cmd:spot.findPeople`, `cmd:spot.findWires`, `cmd:spot.candidates`, `cmd:spot.selectCandidate`, `cmd:spot.removeCandidates`, `crates/pipeline/src/spot_area.rs`, `crates/engine/src/spot_area.rs`, `crates/pipeline/src/find/mod.rs`, `crates/engine/src/find_removals.rs` | Remove panel ▸ Object: click an object (SAM 3; ⇧-click adds, ⌥-click leaves a part out) or brush loosely over it, and one spot in the object's shape takes it out, grown by Expand; a mask's menu ▸ Remove This Area does the same for any mask. These spots have no source and fill with LaMa unless Classic is picked; the shape is stored with the spot, so previews, exports and the GPU path match. Find People and Find Wires outline what they find (SAM 3's "person" and "power line" masks; without the model, rough shapes under faces and a pure-Rust line detector); Remove or Remove All takes each out as one spot in its shape, in one undo step. The main subject is never proposed unless asked. Without SAM 3 a loose stroke stays a brush-shaped spot; no automatic detection of other distraction kinds |
