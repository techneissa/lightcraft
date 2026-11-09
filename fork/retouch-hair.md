# Portrait retouching: Stray Hair

List: the Portrait tab's Stray Hair section: Amount cleans flyaways at the hair's outer edge and stray strands across the face, per face, off by default; strands are found once per photo with the people parse; beards, eyebrows, lashes, clothes and the hair itself are left alone. Branches `feat/retouch-hair` (v1), `feat/stray-hair-v2` (v2).

## What it does

One slider in the Portrait tab's Stray Hair section, **Amount**, 0 to 100, off at 0, for every face or the picked one
(`retouch.hair.amount`; per face `retouchFace.<i>.hair.amount`). It finds thin, high-contrast, line-like strands once
per photo and, in every render, fills each from what is beside it.

**The search, once per photo** (`hair::with_strands_on_crops`), as Blemishes finds its spots: the face search runs it
with the people parse (`crates/engine/src/retouch_input/parts.rs`, `found_with`), on its worker and under its "Finding
Faces" row in the activity stack. **Off for now** (`parts::FULL_SIZE_CROPS`, a constant, no UI): the search runs on the
parse's picture (the photo before any retouching, 2048 px, in linear light, turned like the masks), as before the
rework, until a noise-aware or model-based detector exists (see the findings below). The code for the other way is
kept and tested: per face it searches a **crop of the head cut from the full-size photo** (not the 2048 px parse
picture, where a strand is thinner than a pixel): the pixels the face's grid covers and a margin to read
beside them (`PAD`), rendered like the parse picture (default settings, linear light, turned like the masks) as a window
of the original (`parts/crop.rs`, `Cropper`), reduced to at most 3072 px along its longer side (`MAX_CROP`). Strand
width stays a face fraction and the result stays in the photo's normalised frame, with the crop's pixels as the
picture the directions and distances are measured in (`Strands::picture`: the photo's size at the crop's reduction).
A photo whose size is not the parse picture's shape, or a head that can't be cut, falls back to the parse picture. What it finds is kept with each face (`Face::strands`): a grid of cells over the searched area in the
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
- Faces smaller than 120 px in the searched picture are not searched (their strands are thinner than a pixel there);
  on the full-size crop that is a face of 120 px of the original, not of the 2048 px picture.

**What the section says** (`panels/portrait/hair.rs`, and `retouch.faces`' `strands` per face): "Searched N Faces · M
Strands" (a strand: a run of at least 20 cells of the search grid above half a weight, `count_strands`); why not:
"Faces Not Found Yet" (no faces yet), "Face Parts Not Found: Settings…" (the faces came from YuNet, without the people
parse: its models are not installed, so there is nothing to search with; the button opens Settings ▸ Faces), "Face Too
Small", "Face Parts Not Found" (a face with neither a Hair nor a Facial Skin mask). `Strands::{searched, count,
face_px, ms, crop_ms}` hold it; `retouch.faces` gives `strands: {status: searched|tooSmall|noParts|notSearched, count,
ms, cropMs, facePx}` (the time is not in the fingerprint).

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

## Stray Hair v2 (`feat/stray-hair-v2`): the strands outside the hair are taken away with Apple Vision and LaMa

v1 (below) fills each strand from its sides and found sensor grain and background lines at full size. v2 finds the
strands outside the hair mass with a person matte and takes them away with a learned fill, **once per photo** in the
face search (`crates/engine/src/retouch_input/strand_removal.rs`, `crates/retouch/src/hair/removal.rs`), and the
Amount slider only scales what was kept. On a Mac with Apple Vision (macOS 14 or later) and the LaMa model installed;
otherwise nothing of this runs and Stray Hair is v1 alone (non-macOS builds compile the same, `coreml`'s stub has no Vision).

1. **The crop**: the face's search grid (box and masks grown by the hair band) plus a read margin, cut from the
   **full-size** photo (`parts/crop.rs`, `Cropper`, as v1's crop search) and reduced to at most 3072 px
   (`hair::MAX_CROP`). `FULL_SIZE_CROPS` still only gates v1's use of the crops (`Crops::v1`).
2. **The matte**: Apple Vision's person segmentation (Accurate) on the crop (`lightcraft_coreml::vision`), 0.1–0.4 s.
3. **The strand mask** (`hair/removal/strand_mask.rs`, on the crop at most 3072 px, `removal::MAX_WORK`, sizes as fractions
   of the face): the solid = the matte over 0.85, opened at 1.2 % of the face and its gaps closed at 3 %; its rim (1.2 %)
   is never looked at. **Strands are the picture's thin lines** (what stands out of the cube-root luminance by more than it
   is wide, dark or light: closing and opening at 0.4 %) **where the matte sees the person at all** (over 0.03), outside
   the solid, not over Facial Skin, glasses and jewellery (Others) or clothes, and not where lines crowd (a tuft is hair:
   over half the area at 2 % blur). A piece counts when it is long (6 % of the face), stands out by 0.07 and lies more than
   5 % from the solid; it then grows 0.3 % back to its root and width.
   *What did not work*: the matte's own thin features (its top-hat) find only a grey cloud where flyaways are faint; the
   parse's Hair part takes in the flyaways, so it can't define the solid (the first made nothing, the second ate the
   hair's edge); looser settings (long strands from 3 %, lines from 0.04, 3 % from the solid) removed dense fringe and
   left flat patches on the edge of dark locks. The settings kept take few strands and leave the cloud of faint ones.
4. **The removal**: LaMa (the engine's runner, `LamaFill::fill_strands`, crop at 1024 px, little context, hole grown 1 px) fills the mask; only its
   low band is used (blurred 1.5 × the fine blur), held within 0.2 (log light) of the plain background around (matte
   under 0.1), as a learned fill made pale patches in a tangle of hair; the background's fine band (grain, 0.2 % of the
   face, at least 2 px) is put back from clean pixels of the same tone nearby (first of 14 shifts, held to 3 × the typical
   fine band).
5. **What is kept** (`Strands::removal`, `hair::Removal`, in the face store and the input's fingerprint): per pixel of the
   crop a coverage (the mask grown 1 px and softened) and the **change** (log of new over old light), 16-bit, only for the
   16 × 16 tiles with a strand, plus copies at 1/2, 1/4 and 1/8 (the change of each block's mean light, so a draft
   takes only the strand's share of its pixel). *Not the background's colour*: the step runs on the photo as edited and
   the search's picture is not edited, so a colour put there showed as pale patches on a raw. A ratio carries across, but
   not far: a render finer than the search's picture (an export above the original's size) takes out less, because
   its pixels hold more of the strand than the search's.
6. **A render** (`Removal::apply`): a pixel in the coverage takes the stored change, `amount` of the way in light, on the
   blocks of pixels the occupied tiles can touch; never past the lightest and darkest pixel within 1.5 % of the face
   round it (a change worked out on the unedited picture can overshoot); no model, no search. Amount 0 keeps every bit; a draft, an export and a zoomed window
   make the same change (the pixel is read at its normalised place; the copy is picked by the render's face size).
7. **v1 stays** for the strands across the face's skin, and for every strand when v2 did not run for the face
   (no Vision, no LaMa, no matte, a failed fill, a face under 160 px of the original, a cancelled search): for a face
   with a change `region::weights(…, skin_only)` drops the band outside the hair.
8. **The readout** counts v1's and v2's strands ("Searched N Faces · M Strands"); `retouch.faces` adds `removed`,
   `removeMs`, `removeCropMs`. The search shows a row `retouch.strands` ("Removing Stray Hair") in the activity stack,
   held with LaMa's run; its ✕ stops it.

How it does (a test Mac, release, one face each; pictures in the PR for the two CC0 portraits, private ones not):

| Photo | Strands v2 took | v2 search (+ crop) | Stray Hair's apply alone (1920 px) |
|---|---|---|---|
| [Portrait with black hair](https://commons.wikimedia.org/wiki/File:Portrait_with_black_hair,_3080644.jpg) (CC0), 5340 × 3336 | 7 of 9 | 699 ms (+ 154) | 0.75 ms |
| [Seated woman with blonde hair](https://commons.wikimedia.org/wiki/File:Seated_woman_with_blonde_hair-3177506.jpg) (CC0), 3891 × 3264 | 1 of 2 | 838 ms (+ 307) | 0.73 ms |
| 4 private portraits (33 MP raws, curls, stubble, short dark hair; 5 faces) | 0 | 109–574 ms (+ 460–690) | – |

- On the black-haired portrait it takes the long, isolated strands away and leaves the cloud of faint ones and the
  dense fringe. At 100 % one long strand taken from beside a lock leaves a grey smear, and a few small flat patches
  appear on a lock's edge. On the blonde portrait one light strand over the dark background goes cleanly.
- The private portraits have few separate strands: none passes the length and contrast gates, so Stray Hair there is v1
  alone, as before.
- A slider tick of the whole draft render (Stray Hair or Skin) is 70–530 ms on these photos with or without Stray Hair:
  the render dominates. Stray Hair's own share is 0.4–1.4 ms (the ≤ 30 ms target holds).
- The search per face, with the parse known: 0.7–1.2 s on the matte, LaMa (1024 px) and the change, plus the crop's 0.15–0.7 s.



## Step 1 of the rework (`feat/stray-hair-v2-step1`): measured on 12 private test portraits (crop search kept off)

Test Mac, 33 MP raws, 13 faces (the photos are men with short dark hair and beards: few real flyaways). Search per
face before (the parse picture, 2048 px) → after (a full-size head crop); strands = the readout's count.

| Face (px in the picture searched) | Strands before → after | Search ms before → after (+ crop ms) |
|---|---|---|
| 634 → 2171 | 17 → 22 | 172 → 122 (+763) |
| 576 → 1972 | 5 → 5 | 242 → 106 (+729) |
| 505 → 1728 | 13 → 12 | 311 → 122 (+616) |
| 849 → 2905 | 16 → 12 | 235 → 90 (+806) |
| 766 → 2620 | 1 → 2 | 857 → 93 (+707) |
| 763 → 2612 | 15 → 14 | 687 → 87 (+602) |
| 417 → 1426 | 2 → 2 | 1165 → 122 (+624) |
| 361 → 1236 | 20 → 44 | 283 → 110 (+639) |
| 344 → 1179 | 19 → 39 | 1082 → 110 (+584) |
| 212 → 725 (padel hall, ISO noise) | 12 → 277 | 172 → 101 (+632) |
| 254 → 870 | 23 → 210 | 368 → 110 (+592) |
| 67 → 230 (too small before) | – → 226 | – → 76 (+554) |
| 122 → 417 | 4 → 282 | 66 → 119 (+651) |

What it shows:
- **Silent no-op: holds.** Without the people-parse models the faces come from YuNet, `strands` is `None` and Stray Hair
  does nothing with no message. Now the section says so. With the models installed (here) every face is searched.
- **Gates multiply: does not hold.** Counting cells per face (a temporary counter, not kept): of cells that pass the
  contrast and line gates at 0.5 (hundreds to ~12 000 of ~200 000 to 390 000 searched), 0 to 37 end below a full
  weight because the product is small, 0 are cut by the density gate, and the straight-run gate cuts 5 to 25 %.
  The gate that rejects is the contrast floor (`SKIN_CONTRAST` 0.05, or 1.5 × the texture): of ~100 000 line-like cells
  only ~1 000 stand out enough. No threshold changed: no photo here showed strands rejected by a gate that I could
  attribute to one (the flyaways visible at 100 % are thin curls, 1 to 2 px wide, that were mostly not found on either
  picture).
- **Decision: the full-size crop search is off** (`FULL_SIZE_CROPS`) until a noise-aware or model-based detector. The
  table below is with it on. **The full-size crop finds more, and not only strands.** On the two clean faces few strands change; on noisy,
  small faces in a dark hall the count goes up 15 to 70 times: sensor grain and background lines (court bars, wall
  stripes) pass the gates at full size, and a beard's stubble and an ear's rim were already repaired on the parse
  picture too. The cell size was left at 400 across the face: 700 took about a second a face and counted grain.
- **Speed (crop search on).** The search is 76–122 ms a face on the crop (it was 66–1165 ms on the parse picture), plus 550–810 ms to
  render the head's window from the decoded original (once per face; the original decodes once per photo). A face
  stays under 1 s.
- Pictures (private test portraits, so only in `$WORKSPACE/assets/lightcraft/shots/portrait/hair-stray/v2-step1/`).

## Branch

`feat/retouch-hair`

## Own files

`crates/retouch/src/hair.rs`, `crates/retouch/src/hair/{detect,grid,region,repair}.rs`,
`crates/retouch/examples/hair.rs`, `crates/retouch/tests/hair.rs`, `crates/ui-egui/src/panels/portrait/hair.rs`,
`crates/engine/src/retouch_input/parts/crop.rs`, `crates/retouch/src/hair/removal.rs`, `crates/retouch/src/hair/removal/{strand_mask,replace,change}.rs`,
`crates/retouch/tests/hair_removal.rs`, `crates/engine/src/retouch_input/strand_removal.rs`, `crates/engine/examples/stray_hair_v2.rs`,
`fork/retouch-hair.md`, `fork/tests.d/retouch-hair.txt`

## Hooks in official files

- `apps/lightcraft-cli/src/main.rs` (`render`): the session has the people-parts models, so `lightcraft-cli render`
  renders a portrait retouch as the app does.
- `docs/parity-checklist.md`: the `LR-EDIT-PORTRAIT` row says Stray Hair works; the `LR-EDIT-PORTRAIT-STRAY-HAIR` row.

In fork files:
- `crates/retouch/src/input.rs`: `Face::strands`, `Strands`, in the fingerprint.
- `crates/engine/src/retouch_input/parts.rs`: `with_spots_and_strands` in `found_with` (the parse's faces get their
  spots from one linear picture and their strands from full-size crops, `Crops`); `crates/engine/src/retouch_input.rs`:
  the finder's `Cropper`; `crates/engine/src/cmd/retouch.rs`: `strands` in `retouch.faces`; `crates/engine/src/retouch_input.rs`: `strands: None` for YuNet's faces and
  the strands in the store's byte count; `crates/engine/src/retouch_input/parts/tests.rs`: two tests.
- v2: `crates/retouch/src/input.rs`: `Strands::removal` (in the fingerprint when there); `crates/retouch/src/hair.rs`: `apply` runs
  it, `keep` merges it and its count, `search(…, skin_only)`; `hair/region.rs`: `weights(…, skin_only)`;
  `crates/engine/src/lama_fill.rs`: the `Use`/`Crop`/`Job` split (as Wrinkles v2's) and `fill_strands`;
  `crates/engine/src/retouch_input.rs` and `parts.rs`: `StrandRemoval` in the finder, `Crops::v1`, `found_with(…, removal)`;
  `crates/engine/src/cmd/retouch.rs`: `removed`, `removeMs`, `removeCropMs`; `crates/engine/Cargo.toml`: `image` for the example;
  `crates/pipeline/src/retouch_step.rs`: `read_box` includes the change's area.
- `crates/pipeline/src/retouch_step.rs`: a zoom test on a flyaway. `crates/ui-egui/src/tests/portrait.rs`: a test of
  the section.

## Tests

v2 (`crates/retouch/tests/hair_removal.rs`, the unit tests of `hair/removal/`, `crates/engine/src/retouch_input/strand_removal.rs`):
a thin strand off a matte's head is kept and the head, its rim, a thick lock, a speck, skin and glasses are not; the
flyaway goes (under a quarter left) and half the Amount leaves about half; Amount 0 gives the original bits; only pixels
at the strand change, never the head, hair mass or face; a draft (a quarter of the size) and an export four times it
remove the same strand; a render finer than the grid replaces the strand and touches nothing beside it; a window equals
the whole render; with no matte, no fill or a stop the face keeps no change and v1 finds its strands; a face with a
change is left to v1 over its skin only; the search shows its row, ✕ stops it, no LaMa model means not available; odd
faces, crops and mattes do not panic.

v1 (`fork/tests.d/retouch-hair.txt`). They cover: off and Amount 0 giving the original bits; a strand across the forehead
and a flyaway against the sky going (under 30 % left), half the Amount taking about half; only pixels within a few
pixels of a strand changing, never the hair mass, the eyebrows or the skin beside (no dark halo at a strand's side or
past its tip); a beard left alone; a face without Hair and Facial Skin left alone, and one with Facial Skin only
cleaned on the face alone; a flat grey photo; a draft and an export four times its size repairing the strands one
search found; a window holding the read area giving the whole frame's pixels; a render never searching (faces not
searched yet: no change) and the same input giving the same pixels; a brighter render (a change before the step)
still losing the strands with no new search; a new parse with another Hair mask finding other strands; a face too
small to search left alone; a flyaway running into a sweater repaired with the sweater's pixels and stored cells
untouched, and the strands' grown edge stopping where nothing may change (it reached two cells in before the cut); no
faces, 50 faces, faces past the edge, a 1 × 1 and an empty photo. A head crop cut from a full-size photo finds the strands, in the photo's frame, with the count; a face too small or without parts says why; a crop that fails leaves nothing; a crop is reduced to the cap; the engine searches the crops (also turned) and falls back to the parse picture for a photo of another shape. In the engine: the parse's faces get their strands,
in the photo's frame, also turned; a face without Hair or Facial Skin is not searched. In the pipeline: a zoomed
window whose edge lies on a flyaway equals the whole frame's render (2 levels apart with no reach). In the UI: the
section's Amount set by a click, and its readout's words. Pictures: `cargo run -p lightcraft-retouch --release --example hair -- in.jpg
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
