# Portrait retouching: Stray Hair

List: the Portrait tab's Stray Hair section: Amount cleans flyaways at the hair's outer edge and stray strands across the face, per face, off by default; strands are found once per photo with the people parse; beards, eyebrows, lashes, clothes and the hair itself are left alone. Branch `feat/retouch-hair`.

## What it does

One slider in the Portrait tab's Stray Hair section, **Amount**, 0 to 100, off at 0, for every face or the picked one
(`retouch.hair.amount`; per face `retouchFace.<i>.hair.amount`). It finds thin, high-contrast, line-like strands once
per photo and, in every render, fills each from what is beside it.

**The search, once per photo** (`hair::with_strands`), as Blemishes finds its spots: the face search runs it with the
people parse (`crates/engine/src/retouch_input/parts.rs`, `found`), on the parse's picture (the photo before any
retouching, 2048 px, in linear light, turned like the masks), on its worker and under its "Finding Faces" row in the
activity stack. What it finds is kept with each face (`Face::strands`): a grid of cells over the searched area in the
photo's normalised frame, stored only for the 16 × 16-cell tiles holding a strand (each cell: weight, direction across
the strand, polarity). A render never searches: until the search has run, a face gets no Stray Hair.

- **Where** (`crates/retouch/src/hair/region.rs`, on a coarse grid anchored to the face): the main hair mass is the
  Hair mask opened at 2 % of the face's size (thin strands drop out, the mass stays). Strands are looked for in a band
  up to half a face outside it (the Hair matte's soft fringe and past it, so the chosen hair edge, Current, BiRefNet
  Matting or Apple Vision, decides where the hair ends) and over Facial Skin (closed, so strands it was cut around are
  inside) and Body Skin. Never in the mass, on eyebrows, eyes and lashes, lips and teeth, glasses and jewellery
  (Others), or clothes (the person's pixels that are neither hair nor skin); each of these is grown by a margin
  (clothes by 2 % of the face). Strands fade in over 0.4–3 % of the face from the mass. The search stays within
  1.2 faces of the face's box to each side, 1 above and 1.6 below.
- **Detect** (`hair/detect.rs`, on a grid of 400 cells across the face, no finer than a pixel of the searched picture):
  the Hessian of the luminance's cube root, blurred at a strand's width (0.25 % of the face), finds lines; a line is
  kept when it stands out from both sides alike (a step, such as the face's or the hair's outline, does not), runs on
  along its own direction, and stands out from the texture around it (measured off the lines, so a cloud of flyaways
  against a sky is not a texture but a beard's mottle, foliage or a knit is). On skin, a strand must also have the
  hair's polarity (dark strands for hair darker than the skin), lit skin on both sides (the skin's 75th percentile
  tone, measured over the face's grid), contrast above a crease's, and not sit in a beard. The kept strands are grown
  by two cells and softened (to cover a strand's soft edge and the sharpening halo beside it), then cut again to where
  to look, so their edge never reaches clothes, a part left alone or the hair mass.
- Faces smaller than 120 px in the searched picture are not searched (their strands are thinner than a pixel there).

**The repair, every render** (`hair/repair.rs`), per strand cell, never per pixel of the face: the cell's centre is
mapped into the image through the view, with its tile's local scale for the direction and distances. One fill per
cell: on each side across the strand, the median of three distances (so a sharpening halo or a neighbouring strand
does not set it) and of three points along it (so the strand itself past its tip does not); the side more like the
strand counts little. Each pixel the cell holds moves toward the fill by the strand's weight there and by how much it
differs from the fill: as far as it is like the strand, wholly; the other way (the light halo beside a dark strand)
only by a halo's small step, so skin between a strand's tip and the hair mass never darkens. Where a cell is smaller
than a pixel (a draft of a photo searched at 2048 px), cells are pooled into blocks at least a pixel wide, anchored to
the grid. The Refine brush (#99) will add or take away its strokes in `hair::region::weights` (a search) or mask the
repair in a render.

**What makes the search run again**: a new people parse, which the face search keeps its faces by: another parts set,
another hair edge choice, or another photo version (the file's pixels, which the parse cache is keyed by). A change
before the step (exposure, white balance, Skin, Blemishes) re-runs only the repair, which reads the pixels as they are.

**Zoom**: `Feature::REACH` 0.55 of the face: strands lie within the face's box and the Hair and Facial Skin masks'
areas grown by the band (0.5), and a repair reads 0.05 past them. Every grid is anchored to the face's box or the
photo's frame; reference tones come from the face's whole grid.

How it does, on four private test portraits and three CC0 portraits from Wikimedia Commons (pictures in the PR):

- A flyaway cloud against a pale sky ([Long-haired woman laughing](https://commons.wikimedia.org/wiki/File:Long-haired_woman_laughing_(Unsplash).jpg)):
  most strands go; at 1:1 the densest crossings leave faint blotches.
- Windblown hair across a face against foliage ([Windblown](https://commons.wikimedia.org/wiki/File:Windblown_(Unsplash).jpg)):
  loose strands against the sky go; the lock across the mouth stays (it is hair, not a stray strand); some backlit
  strands over the hands are cut into pieces.
- A busy garden ([Woman in a flower garden](https://commons.wikimedia.org/wiki/File:Woman_in_a_flower_garden_(Unsplash).jpg)):
  small changes in the out-of-focus flowers, not visible at full view.
- Curly hair, beards, stubble, a balding crown, clear glasses: left alone apart from a few isolated hairs.

Speed (on a test Mac with other work running, release, one face, the example's step at a draft's 1936 px): the search,
once per photo on the 2048 px picture, 100–360 ms; the repair (a slider drag, or the Stray Hair share of any other
drag) 2.0–6.4 ms. A Skin drag with Stray Hair on runs Skin and the repair, never the search. No model runs.

## Branch

`feat/retouch-hair`

## Own files

`crates/retouch/src/hair.rs`, `crates/retouch/src/hair/{detect,grid,region,repair}.rs`,
`crates/retouch/examples/hair.rs`, `crates/retouch/tests/hair.rs`, `crates/ui-egui/src/panels/portrait/hair.rs`,
`fork/retouch-hair.md`, `fork/tests.d/retouch-hair.txt`

## Hooks in official files

- `apps/lightcraft-cli/src/main.rs` (`render`): the session has the people-parts models, so `lightcraft-cli render`
  renders a portrait retouch as the app does.
- `docs/parity-checklist.md`: the `LR-EDIT-PORTRAIT` row says Stray Hair works; the `LR-EDIT-PORTRAIT-STRAY-HAIR` row.

In fork files:
- `crates/retouch/src/input.rs`: `Face::strands`, `Strands`, in the fingerprint.
- `crates/engine/src/retouch_input/parts.rs`: `with_spots_and_strands` in `found` (the parse's faces get their spots
  and strands from one linear picture); `crates/engine/src/retouch_input.rs`: `strands: None` for YuNet's faces and
  the strands in the store's byte count; `crates/engine/src/retouch_input/parts/tests.rs`: a test.
- `crates/pipeline/src/retouch_step.rs`: a zoom test on a flyaway. `crates/ui-egui/src/tests/portrait.rs`: a test of
  the section.

## Tests

`fork/tests.d/retouch-hair.txt`. They cover: off and Amount 0 giving the original bits; a strand across the forehead
and a flyaway against the sky going (under 30 % left), half the Amount taking about half; only pixels within a few
pixels of a strand changing, never the hair mass, the eyebrows or the skin beside (no dark halo at a strand's side or
past its tip); a beard left alone; a face without Hair and Facial Skin left alone, and one with Facial Skin only
cleaned on the face alone; a flat grey photo; a draft and an export four times its size repairing the strands one
search found; a window holding the read area giving the whole frame's pixels; a render never searching (faces not
searched yet: no change) and the same input giving the same pixels; a brighter render (a change before the step)
still losing the strands with no new search; a new parse with another Hair mask finding other strands; a face too
small to search left alone; a flyaway running into a sweater repaired with the sweater's pixels and stored cells
untouched, and the strands' grown edge stopping where nothing may change (it reached two cells in before the cut); no
faces, 50 faces, faces past the edge, a 1 × 1 and an empty photo. In the engine: the parse's faces get their strands,
in the photo's frame, also turned; a face without Hair or Facial Skin is not searched. In the pipeline: a zoomed
window whose edge lies on a flyaway equals the whole frame's render (2 levels apart with no reach). In the UI: the
section's Amount set by a click. Pictures: `cargo run -p lightcraft-retouch --release --example hair -- in.jpg
out.jpg --faces dir/faces.json --amount 100 --changes changes.png [--width 1936 --runs 10 --skin 50]` (faces from the
parsing crate's `face_input` example).

## What's New entry

### Stray Hair in the Portrait Tab
The Portrait tab's Stray Hair section cleans flyaways around the hair and stray strands across the face, for every
face or the one you pick. Amount sets how much; beards, eyebrows, lashes, clothes and the hair itself are left alone.

## Parity rows

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-EDIT-PORTRAIT-STRAY-HAIR | Portrait: Stray Hair (flyaways and strands across the face) | — | 🟡 | `ctl:retouch.hair.amount`, `crates/retouch/src/hair.rs`, `crates/ui-egui/src/panels/portrait/hair.rs` | an extra (the reference app has no portrait retouching). Needs the people-parts set for masks; strands are found once per photo with the parse; dense flyaway clouds leave faint blotches at 1:1; no Refine brush yet (#99). See `fork/retouch-hair.md` |
