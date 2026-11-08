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
- **Paint Glare** / **Erase Glare** paint on the photo with the Remove brush's size: strokes add to or take from the
  found glare, and each stroke finds and fills again. **Clear** drops them.
- Filling (`highlights.rs`): the glare is grouped into clusters, each filled in a crop of up to 1536 px with **LaMa**
  (the Remove panel's model, read from `ArtCraft/models/lama`; its own activity row and cache), or **Classic** (the
  patch fill) when LaMa is missing or chosen. Under a veil of light the photo's own fine detail is kept (only the smooth
  part comes from the fill); where the photo is clipped the fill is used as it is.

Both run on a worker thread with the "Reflection Removal" row in the activity stack (✕ stops). The result is a tiled
correction (`LCREFL02`, `crates/engine/src/reflections/tiles.rs`): each tile is a box in fractions of the original
photo and a grid of the change, so the preview, thumbnails and exports apply the same thing at any size, and the
correction stays on the same content when the crop or rotation changes. Old whole-photo corrections (`LCREFL01`) read
as one tile. Amount works for both modes; Protect Sky, Detail Protection and Limit to Area show in Reflections mode.

New whole-photo reflection results start at Amount 100 % (was 35 %): with Detail Protection 60 % only about a fifth of
the model's change showed, which read as "does nothing" (measured on shop-window-day with DSRNet-S and DIRS-MuGI).

Commands (UI): `reflection.mode {mode: reflections | highlights}`, `reflection.area {area: "whole" | {left, top, right,
bottom} | {mask: id}}` (percent of the original photo before crop or rotation), `reflection.glare {threshold?, expand?,
fill?: lama | classic}`, `reflection.paint {points: [[x, y], …], size?, erase?} | {clear: true}` (percent; size is the
radius in percent of the long edge), `reflection.tool {tool: box | paint | erase | none}`; `reflection.apply` takes
`mode`, `area`, `threshold`, `expand` and `fill` too, and `reflection.status` reports them under `panel`. The settings
saved with the photo (`reflection.mode`, `.area`, `.glare`) are left out of the file while default.

Developer tool: `cargo run --release -p lightcraft-engine --example highlights -- IN OUT [--box L,T,R,B] [--mask M.png]
[--threshold N] [--expand N] [--classic] [--paint X,Y,R] [--erase X,Y,R] [--show CHANGED.png] [--model ID --quality N]`
(`--model` runs a reflection model on the area instead).

UnReflectAnything (#74's first plan) is not ported: its weights are 3.9 GB (over the 300 MB limit), its paper shows
no faces, glasses or products, and it scores below DHAN-SHR on paired highlight tests. Its painted-mask input would fit
this mask later; it stays a possible trial. Licence (for that trial): MIT model card, Meta's DINOv3 licence for the
encoder inside; personal use only in this fork.

## Branch

`feat/74-glare-highlights`

## Own files

`crates/develop/src/reflection_area.rs`, `crates/engine/src/reflections/{area,glare,highlights,tiles}.rs`,
`crates/engine/examples/highlights.rs`, `crates/ui-egui/src/reflections_glare.rs`

## Hooks in official files

- `crates/develop/src/lib.rs` (module and re-exports), `crates/develop/src/settings.rs` (`Reflection::mode`, `area`,
  `glare`).
- `crates/engine/Cargo.toml` and `Cargo.lock` (`lightcraft-inpaint` for the Classic fill), `crates/engine/src/reflections.rs`
  (the modules, tiles in `apply` with `Correct`, Amount 100 % for new results; a fork file from
  `fork/reflection-removal.md`).
- `crates/ui-egui/src/lib.rs` (module, `reflection_glare` field), `app.rs` (its start value), `reflections.rs` (the
  rows, the mode in `start` and `reflection.toggle`, the commands), `menus/run.rs`, `menus/ui_commands.rs` and
  `menus.rs` (five commands), `panels/detail.rs` (`geometry` is `pub(crate)`), `panels/detail/geometry.rs`
  (`CanvasMap::rect` is `pub(crate)`), `panels/detail/retouch.rs` (one call at the start of `remove_overlay`).
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
- Reflections and Highlights can work on an Area: a box drawn on the photo, or any mask (a brush, Describe "glasses").
  The model then sees that part at full detail, and nothing outside it changes.
- Reflection results now start at Amount 100 %.
