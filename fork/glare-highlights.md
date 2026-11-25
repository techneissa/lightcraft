# Glare highlights and reflection areas

List: Remove ▸ Reflections gets a Highlights mode (glare found by brightness and filled with LaMa, painting to add or erase) and an Area (a box drawn on the photo or a Masks-panel mask) that any reflection model runs on at full detail. Branch `feat/74-glare-highlights`.

## What it does

The Reflections section of the Remove panel has two new rows under Apply:

- **Mode**: Reflections (a reflection model separates a picture reflected in glass, as before) or **Highlights**
  (glare and shine are found and filled). Issue #74.
- **Area**: Whole Photo, **Box** (drag a box on the photo) or any mask of the Masks panel (a brush, Describe
  "glasses", Object…). Issue #73.

**Area with a reflection model.** The model gets the area and a margin of a quarter of its longer side around it, at
the panel's Quality (its long side at 384 / 768 / 1024 pixels, never more than twice its own size), instead of the
whole photo shrunk to that size. Its change is kept inside the area: a box fades in over 8 % of its shorter side, a
mask uses its own soft alpha. Nothing outside the area changes. A small area on a large photo runs faster than the
whole photo (a glasses box on a 1920 px photo: 21 s against 27 s with DIRS-MuGI at Standard) and keeps more detail.

**Highlights.** No open model exists for glasses glare or skin shine (survey: #100), and the reflection models can't
remove glare by design: a blown-out spot has nothing behind it to separate. Highlights finds glare and fills it:

- Finding (`crates/engine/src/reflections/glare.rs`): bright pixels (near the area's brightest, and above a floor
  near white when the whole photo is searched), low in colour, brighter than their surroundings (a box mean that holds
  a spot's own light down), grown over their softer edge; then spots too small (catchlights, under 0.3 % of the long
  edge across) or too large (8 % of the photo, 35 % of a chosen area) go, as do spots no brighter than the ring around
  them (pieces of a white wall). **Threshold** (default 50) makes it stricter, **Expand** (default 30) grows the
  result before a feathered edge. Inside a chosen area the floor is lower, so a reflection on dark sunglasses counts.
- **Paint Glare** / **Erase Glare** paint on the photo with the Remove brush's size (a share of the whole photo's
  long edge, so a crop does not change it; the circle on the photo shows it): strokes add to or take from the
  found glare, and each stroke finds and fills again. **Clear** drops them.
- Filling (`highlights.rs`): the glare is grouped into clusters, each filled in a crop of up to 1536 px with **LaMa**
  (the Remove panel's model, read from `ArtCraft/models/lama`; its own activity row and cache), or **Classic** (the
  patch fill) when LaMa is missing or chosen. Under a veil of light the photo's own fine detail is kept (only the smooth
  part comes from the fill); where the photo is clipped the fill is used as it is.

Both run on a worker thread with the "Reflection Removal" row in the activity stack. ✕ stops the run, its LaMa fill
included, and nothing is saved. The panel's choices (mode, area, strokes) belong to the photo shown: changing photos
takes the new photo's back from its saved result, open section or not, and a stroke that waits for a run only runs if
its photo is still shown. A box's border fades in for Highlights too. Glare clusters are filled at sizes that together
fit one correction (12.6 MP). The result is a tiled
correction (`LCREFL02`, `crates/engine/src/reflections/tiles.rs`): each tile is a box in fractions of the original
photo and a grid of the change, so the preview, thumbnails and exports apply the same thing at any size, and the
correction stays on the same content when the crop or rotation changes. Old whole-photo corrections (`LCREFL01`) read
as one tile. Amount works for both modes; Protect Sky, Detail Protection and Limit to Area show in Reflections mode.

New whole-photo reflection results start at Amount 100 % (was 35 %): with Detail Protection 60 % only about a fifth of
the model's change showed, which read as "does nothing" (measured on shop-window-day with DSRNet-S and DIRS-MuGI).

Several boxes (#363): the area is up to 32 boxes or one mask. Boxes that overlap, touch or lie within 1 % of the long
edge of each other are searched as one, with the gap between them (the soft edge is at their outside, not where they
meet), others each at full detail; a reflection model runs once per box. The Reflections section has a Box / Paint Glare /
Erase Glare tool row and a list of the boxes (a click or the arrows select, ✕ or Delete removes, Clear All). With the
Box tool in hand (Shift+B; B is the Masking brush) a click selects a box, a drag of the selected box moves it, its
handles resize, a drag anywhere else draws one more box (with a mask area, the first box takes its place), Delete removes, Esc puts the tool down. In Highlights mode each finished change finds and fills
the glare again; when no box is left what the boxes' result changed comes back (in either mode, its other choices
kept), and boxes with no glare in them fill nothing. The
panel's boxes and strokes follow the saved result: undo and redo bring boxes back (and drop the selection), Reset
clears them. An edit made while a run goes still runs after you move to another photo. The web build greys all of this
out. A file's boxes with no inside or off the photo are dropped on reading.

Boxes are drawn only while the Box tool is in hand (#364); another view, panel or Remove tool, the Find or Object section,
the closed section or a picked spot puts it down.

No light outline (#362): glare's soft edge and bloom reach farther than Expand, and what was left outside the fill read
as a thin light ring. The finder now grows each spot outward while the light still falls and is still brighter than the
surface around it, falling fast (at most 2 % of the long edge; a broad glow is left alone). Results made before this keep their ring until Recalculate. Checked
on a synthetic plate only; the reported car photo is still to be checked.

Older builds: the library format is fork 25 (`fork/catalog-format.md`), so a build from before this refuses the whole
library and leaves it untouched. A sidecar (`lc:settings`) with two or more boxes loses its whole LightCraft edit in
an older build, not only the reflection: such a build reads only the interoperable `crs:` fields.

Commands (UI): `reflection.mode {mode: reflections | highlights}`, `reflection.area {area: "whole" | "boxes" | {left,
top, right, bottom} | [boxes] | {mask: id}}` (percent of the original photo before crop or rotation),
`reflection.box {add: {left, top, right, bottom}} | {update: index, left?, top?, right?, bottom?} | {remove: index} |
{clear: true} | {select: index | null}`, `reflection.glare {threshold?, expand?, fill?: lama | classic}`,
`reflection.paint {points: [[x, y], …], size?, erase?} | {clear: true}` (percent; size is the radius in percent of the
long edge), `reflection.tool {tool: box | paint | erase | none}`, `reflection.boxTool` (Shift+B); `reflection.apply`
takes `mode`, `area`, `threshold`, `expand` and `fill` too. `reflection.status` reports them under `panel`:
`panel.area` is `"whole"`, `{mask: id}` or the list of boxes (empty while none is drawn), and `panel.selected` the
selected box's index or null. The
settings saved with the photo (`reflection.mode`, `.area`, `.glare`) are left out of the file while default; one box is
written as before, several as a list.

Developer tool: `cargo run --release -p lightcraft-engine --example highlights -- IN OUT [--box L,T,R,B] [--mask M.png]
[--threshold N] [--expand N] [--classic] [--paint X,Y,R] [--erase X,Y,R] [--show CHANGED.png] [--model ID --quality N]`
(`--model` runs a reflection model on the area instead).

UnReflectAnything (#74's first plan) is not ported: its weights are 3.9 GB (over the 300 MB limit), its paper shows
no faces, glasses or products, and it scores below DHAN-SHR on paired highlight tests. Its painted-mask input would fit
this mask later; it stays a possible trial. Licence (for that trial): MIT model card, Meta's DINOv3 licence for the
encoder inside; personal use only in this fork.

## Branch

`feat/74-glare-highlights` (from `custom`)

## Own files

`crates/develop/src/reflection_area.rs`, `crates/engine/src/reflections/{area,glare,highlights,tiles}.rs`,
`crates/engine/examples/highlights.rs`, `crates/ui-egui/src/reflections_glare.rs`,
`crates/ui-egui/src/reflections_boxes.rs`

## Hooks in official files

- `crates/develop/src/lib.rs` (module and re-exports), `crates/develop/src/settings.rs` (`Reflection::mode`, `area`,
  `glare`).
- `crates/engine/Cargo.toml` and `Cargo.lock` (`lightcraft-inpaint` for the Classic fill), `crates/engine/src/reflections.rs`
  (the modules, tiles in `apply` with `Correct`, Amount 100 % for new results; a fork file from
  `fork/reflection-removal.md`).
- `crates/ui-egui/src/lib.rs` (modules, `reflection_glare` field), `app.rs` (its start value), `reflections.rs` (the
  rows, the mode in `start` and `reflection.toggle`, the commands), `menus/run.rs` (also Esc), `menus/ui_commands.rs`
  and `menus.rs` (seven commands), `shortcuts.rs` (Delete removes the selected box), `panels/right.rs` and
  `panels/right/remove_compact.rs` (`tile_row` is shared; a Remove tool click puts the Reflections tool down),
  `panels/detail.rs` (`geometry` is `pub(crate)`), `panels/detail/geometry.rs` (`CanvasMap::rect` is `pub(crate)`),
  `app/frame.rs` calls `reflections_glare::release_if_hidden` each frame, and in `panels/detail.rs` the Remove arm of
  `tool_interaction` asks `reflections_glare::overlay` first.
- `panels/right/remove_finds.rs` and `panels/right/remove_object.rs` (using them puts the Reflections tool down).
- `crates/catalog/src/fork_format.rs` (`VERSION` 25), `journal.rs` (the format table), `tests_format_version.rs`.
- `crates/pipeline/src/learned_fill.rs` (`FillRun::with_stop`, so the run's ✕ stops LaMa too; a fork file from
  `fork/lama-fill.md`).
- `docs/ai-reflections.md` (the default Amount).

## Tests

Commands in `fork/tests.d/glare-highlights.txt`. None needs a model or a photo: the glare finder, the tiles, the area
crop and blend, the mask in the source's orientation and the saved settings use synthetic pictures; the fill uses a
fake learned fill and the Classic fill. Tuned by hand with the example on CC0 photos (see the PR).

## What's New entry

### Remove: Glare and Reflection Areas
- Reflections has a Highlights mode: it finds glare and shine (on glasses, skin, glossy things) and fills it with LaMa,
  keeping the photo's texture under a veil of light. Threshold and Expand tune what it finds; Paint Glare and Erase
  Glare correct it on the photo.
- Reflections and Highlights can work on an Area: boxes drawn on the photo, or any mask (a brush, Describe "glasses").
  Boxes can be selected, moved, resized and deleted, on the photo or from their list; Shift+B picks up the Box tool.
  The model then sees that part at full detail, and nothing outside it changes.
- Removed glare no longer leaves a light outline behind.
- Reflection results now start at Amount 100 %.
