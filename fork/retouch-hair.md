# Portrait retouching: Stray Hair

List: the Portrait tab's Stray Hair section: Amount cleans flyaways at the hair's outer edge and stray strands across the face, per face, off by default; beards, eyebrows, lashes and the hair itself are left alone. Branch `feat/retouch-hair`.

## What it does

One slider in the Portrait tab's Stray Hair section, **Amount**, 0 to 100, off at 0, for every face or the picked one
(`retouch.hair.amount`; per face `retouchFace.<i>.hair.amount`). It finds thin, high-contrast, line-like strands and
fills each from what is beside it:

- **Where** (`crates/retouch/src/hair/region.rs`, on a coarse grid anchored to the face): the main hair mass is the
  Hair mask opened at 2 % of the face's size (thin strands drop out, the mass stays). Strands are looked for in a band
  up to half a face outside it (the Hair matte's soft fringe and past it, so the chosen hair edge, Current, BiRefNet
  Matting or Apple Vision, decides where the hair ends) and over Facial Skin (closed, so strands it was cut around are
  inside) and Body Skin. Never in the mass, on eyebrows, eyes and lashes, lips and teeth, glasses and jewellery
  (Others), or clothes (the person's pixels that are neither hair nor skin). Strands fade in over 0.4–3 % of the face
  from the mass, so a strand leaving it is not cut off at its root. The search stays within 1.2 faces of the face's
  box to each side, 1 above and 1.6 below.
- **Detect** (`hair/detect.rs`, on a grid of 400 cells across the face, no finer than a pixel): the Hessian of the
  luminance's cube root, blurred at a strand's width (0.25 % of the face), finds lines; a line is kept when it stands
  out from both sides alike (a step, such as the face's or the hair's outline, does not), runs on along its own
  direction, and stands out from the texture around it (measured off the lines, so a cloud of flyaways against a sky
  is not a texture but a beard's mottle, foliage or a knit is). On skin, a strand must also have the hair's polarity
  (dark strands for hair darker than the skin), lit skin on both sides (the skin's 75th percentile tone, measured
  over the face's grid), contrast above a crease's, and not sit in a beard (much of the skin around darker than the
  lit skin).
- **Repair** (`hair/repair.rs`, per pixel): each strand pixel takes the colour beside the strand: on each side across
  it, the median of three distances (so a sharpening halo or a neighbouring strand does not set it), averaged along
  the strand; the side more like the strand counts little. A pixel changes by how much it differs from its fill, so
  the skin's and the sky's fine texture beside a strand stays, and the light halo sharpening leaves beside a dark
  strand goes with it.
- **Speed** (`hair/cache.rs`): the fills are kept by a hash of every pixel the search reads, the masks, the face's
  place and the view's mapping, so the slider only blends kept fills; a render never depends on what was rendered
  before. A change before the step (exposure, Skin) searches again.
- **Zoom**: `Feature::REACH` 0.55 of the face (the band and what the fills read past it); grids anchored to the face's
  box; reference tones from the face's whole grid.
- The Refine brush (#99) adds or takes away its strokes in `hair::region::weights`.

How it does, on four of techneissa's portraits and three CC0 portraits from Wikimedia Commons (pictures in the PR and
in `$WORKSPACE/assets/lightcraft/shots/portrait/hair-stray/`):

- A flyaway cloud against a pale sky ([Long-haired woman laughing](https://commons.wikimedia.org/wiki/File:Long-haired_woman_laughing_(Unsplash).jpg)):
  most strands go; at 1:1 the densest crossings leave faint blotches.
- Windblown hair across a face against foliage ([Windblown](https://commons.wikimedia.org/wiki/File:Windblown_(Unsplash).jpg)):
  loose strands against the sky go; the lock across the mouth stays (it is hair, not a stray strand); some backlit
  strands over the hands are cut into pieces.
- A busy garden ([Woman in a flower garden](https://commons.wikimedia.org/wiki/File:Woman_in_a_flower_garden_(Unsplash).jpg)):
  small changes in the out-of-focus flowers, not visible at full view.
- Curly hair, beards, stubble, a balding crown, clear glasses: left alone apart from a few isolated hairs.

Time (Apple M4 Pro, other work running, load average 14–27; the example's step alone, one face, faces from the
`face_input` example): the search once per photo 89–390 ms at a draft's 1936 px, the slider on kept fills 2.2–6.6 ms.
No model runs.

## Branch

`feat/retouch-hair`

## Own files

`crates/retouch/src/hair.rs`, `crates/retouch/src/hair/{cache,detect,grid,region,repair}.rs`,
`crates/retouch/examples/hair.rs`, `crates/retouch/tests/hair.rs`, `crates/ui-egui/src/panels/portrait/hair.rs`,
`fork/retouch-hair.md`, `fork/tests.d/retouch-hair.txt`

## Hooks in official files

None. In fork files: `crates/pipeline/src/retouch_step.rs` (a zoom test on a flyaway), `crates/ui-egui/src/tests/portrait.rs`
(a test of the section).

## Tests

`fork/tests.d/retouch-hair.txt`. They cover: off and Amount 0 giving the original bits; a strand across the forehead
and a flyaway against the sky going (under 30 % left), half the Amount taking about half; only pixels within a few
pixels of a strand changing, never the hair mass, the eyebrows or the skin beside (a dark halo formed before the
side weighting was fixed); a beard left alone; a face without Hair and Facial Skin left alone, and one with Facial
Skin only cleaned on the face alone; a flat grey photo; a draft and an export four times its size agreeing; a window
holding the read area giving the whole frame's pixels; the same pixels giving the same result, and a photo without
strands nearly untouched; no faces, 50 faces, faces past the edge, a 1 × 1 and an empty photo. In the pipeline: a
zoomed window on a flyaway outside the masks' area equals the whole frame's render (200 levels apart with no reach).
In the UI: the section's Amount set by a click. Pictures: `cargo run -p lightcraft-retouch --release --example hair --
in.jpg out.jpg --faces dir/faces.json --amount 100 --changes changes.png` (faces from the parsing crate's
`face_input` example).

## What's New entry

### Stray Hair in the Portrait Tab
The Portrait tab's Stray Hair section cleans flyaways around the hair and stray strands across the face, for every
face or the one you pick. Amount sets how much; beards, eyebrows, lashes and the hair itself are left alone.

## Parity rows

| LR-EDIT-PORTRAIT-HAIR | Portrait: Stray Hair (flyaways and strands across the face) | — | 🟡 | `ctl:retouch.hair.amount`, `crates/retouch/src/hair.rs`, `crates/ui-egui/src/panels/portrait/hair.rs` | an extra (the reference app has no portrait retouching). Needs the people-parts set for masks; dense flyaway clouds leave faint blotches at 1:1; no Refine brush yet (#99). See `fork/retouch-hair.md` |
