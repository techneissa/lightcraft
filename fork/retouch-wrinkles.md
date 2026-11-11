# Portrait retouching: Wrinkles

List: the Portrait tab's Wrinkles section: Forehead, Crow's Feet, Smile Lines and Neck take away the deep wrinkles in each area and keep the pores and the face's shape; areas from the face mesh, sizes in fractions of the face. Branches `feat/retouch-wrinkles` (v1), `feat/wrinkles-v2` (v2).

## Wrinkles v2: removal from a stored change

v1 (below) only lifts a crease by part of its depth, so deep wrinkles stayed. v2 takes them away:

- **Once per photo**, in the face search next to Blemishes' spots and Stray Hair's strands (`crates/engine/src/retouch_input/wrinkles.rs`,
  `crates/retouch/src/wrinkles/search.rs`), for each face with the mesh and a Facial Skin mask, on a crop of the areas
  where the face is at most 1280 px across (crop at most 2560 px; #201: at 768 px a 3 px crease was under a pixel wide):
  0. **The pixels** are a floating-point crop of the original with the default settings (no retouching), rendered at the
     size the search works at (`Cropper::crop_float`, `OutputDepth::F32Linear`, a window of the reduced render), not the 8-bit
     parse picture, which quantises the shadows (a wrinkle in dark skin is a step or two deep: the same faint line reads as a
     wrinkle in the 8-bit copy and not in the float one, tested). When the original can't be cut, the 8-bit picture is the
     fallback. The thresholds (ridge 0.025 / 0.05 log light) were re-checked on float crops of both CC0 faces and three private
     test portraits.
  1. **The skin**: Facial Skin, plus the skin seen through and beside a pair of glasses (`glasses.rs`): the parse puts the
     lenses and the skin beside them in Others, so crow's feet behind glasses had no skin. Inside Others, a pixel with the
     skin's colour and light is skin; the rest (non-skin colour, or far lighter: a reflection) is the frame, opened by 0.4 %
     of the face (thinner dark lines are folds) and kept out with a 0.6 % margin.
  2. **Where a wrinkle may be**: that skin eroded (0.6 %), inside an area, not an ear, a nostril, the face's edge or an eye
     (`exclude.rs`: a band round each eye and a disc at each corner, so the lash line and the eye's dark corner stay), not a
     hair (`hairlike.rs`: thin, long, bright and greyer than the skin; a lit skin ridge is thicker and is skin), and not
     far darker than the skin around (a lash line, a deep shadow).
  3. **The lines** (`zones.rs`, `ridge.rs`): each pixel's zone is the region that weighs most there, with its widths (out of
     eight from 0.12 to 2 % of the face) and its lines' direction: the forehead across (and up and down in a band round the
     middle: frown lines), crow's feet and cheeks any way, the nose-to-mouth fold along itself (softened to 60 %, never
     taken away), the neck across. A Hessian valley filter at those widths, damped for round shapes (a freckle, a pore) and
     averaged along the line; the crest kept by non-maximum suppression across the line, then hysteresis with each zone's
     shortest line (3 % of the face on the forehead and neck, 2.5 % elsewhere). Round pigment spots (browner than the skin
     around: freckles, age spots) are found too and left as they are, except on a line's core.
  4. **The change** (`flatten.rs`): near the lines, each skin pixel's median along the line (13 samples), then the weighted
     median of those across the line (a Gaussian of 1.6 times the zone's widest line or 4 times the line's own width): the
     skin the pixel would have without the lines. Change = that less its own, smoothed 2 px along the line, scaled by how
     near a line it is, the zone's keep and the allowed part. Creases come up and the lit ridges between them come down, so a
     lined forehead does not turn paler than the skin around it; pores and grain stay. A freckle or a dark pit inside a
     crease takes the crease's change (it comes up with it and stays a freckle, not a dark dot). Kept in `Face::wrinkles`
     (`wrinkles::Fix`: a grid over the crop, normalised like the masks, 16-bit cells in 16 × 16 tiles, only the changed
     tiles stored), in the face store and the input's fingerprint. No new saved field.
  - **Approaches weighed** (#201): a two-sided fill along each cross-section from the skin on both sides (weak, blocky
    edges), the old banded heal with LaMa's low band (removed only 14–16 % of a forehead's line depth even at its own size,
    and brightened), and evening across the lines (kept: the pictures were the cleanest). LaMa is no longer called.
- **The areas are wider than v1's** (same four controls and ids, `areas.rs`): Forehead also covers between the brows (the
  frown lines down to the bridge of the nose); Crow's Feet also covers a thin crescent under each eye; Smile Lines (label "Cheeks
  and Smile Lines", id `retouch.wrinkles.smileLines`) is the nose-to-mouth band plus each cheek from under the eye to the jaw
  plus the chin below the lower lip; Neck is unchanged. The old Wrinkles (a face without a search) use the same areas.
  **No double work with Eyes** (Dark Circles and Eye Bags, `fork/retouch-eyes.md`): Wrinkles runs first and takes away narrow
  creases (the ridge mask, a few pixels wide); Eyes then works on the lifted skin at another scale (Eye Bags flattens the shading
  between 0.025 and 0.45 half eye widths and keeps what is finer, Dark Circles corrects the colour under the eye), so the two
  never act on the same band.
- **A row** `retouch.wrinkles` ("Smoothing Wrinkles") in the activity stack while it runs; its ✕ stops it. LaMa's own row is not
  added (the search's row is held by the fill's run). **A cancelled search drops every face's change** (the ones of faces it
  had finished too) and the photo's faces are kept as "to search again" (`Found::retry`): Wrinkles shows v1's result, and the
  photo is searched again after 30 s by a render or the Portrait tab opening on it, and at once by an export (an export must
  match what the preview will show). Neither is a failure, and the other searches' results (spots, strands) stay.
- **The key** the faces are kept under still includes whether LaMa is installed (`LamaFill::term`), though the search no
  longer uses it (left for a follow-up that removes the plumbing).
- **A slider tick** scales the stored change (`fix.rs`): in each area, slider/100 times the area's weight (`areas.rs`, the
  strongest where areas overlap) is how much of the change is applied, as an additive change in log light (a gain per channel)
  on the pixels the render has. Slider 0 keeps every bit; 100 gives the stored change inside the area. No model runs. The
  stored change was measured on the picture before Skin (which runs first and may have softened a crease already), so the lift
  is bounded: a pixel is not lifted past the skin around it (a blur of 2 % of the face over the pixels the change leaves alone)
  plus 4 %. A face whose search has not run (cancelled, or no mesh) shows v1's result; a face whose search found no wrinkles has
  an empty change and is left alone. The change is read between cells bilinearly but never where the cell the point lies in has
  none, so no lift reaches past the mask.
- **Zoom**: the change is read at the pixel by its normalised place, the weight lattice is anchored to the face's corner and the
  reference blur reads at most 6 % of the face around (within `Wrinkles::REACH`, which already covers v1's reads), so a zoomed
  window equals the whole render (tested); a draft and an export make the same change (tested pixel for pixel, within 10 %).
  A face whose frame is warped (Face Reshape, lens corrections) is read through the view: tested with a scaling and shifting view.
- The Refine brush (`fork/refine-brush.md`) runs over Wrinkles as over every feature; `wrinkles::area_weight` is unchanged.

Time (release, a test Mac with other work running): the wrinkles search is about 1–1.8 s per face at 1280 px (it was
about 0.8 s with the banded heal at 768 px); on a 1936 px draft 92 ms (was 71 ms). A slider tick (the retouch step on a
1936 × 1290 draft of a face filling the frame, all four sliders at 100, fastest of 20): 12.6 ms (was 10.2 ms), under the
16 ms budget. Memory (peak footprint, the `wrinkles` example with and without `--search`, seven CC0 portraits): the
search adds 50–350 MB per photo, the most for a tall crop with the neck (1119 × 2117 px); a photo with seven faces adds
178 MB (faces are searched one after another). Each full-crop buffer is dropped once used (it added up to 514 MB before);
a face keeps at most 80,000 crest points (the most lined portrait has about 22,000), the deepest first.

Measured (#201, seven CC0 portraits, `crates/retouch/examples/wrinkles_sheet.rs`; line depth = elongated valleys 0.15–0.6 %
of the face; band = the issue's measure, the dark side of the 0.25–3 % band, which also counts spots and mottling): at 100,
the most lined face's forehead 13 → 27 % of line depth (band 16 → 23 %), crow's feet 3 → 21 % (4 → 19 %); crow's feet behind
glasses 0 → 15 %. The area's mean L* stays within 0.3 of slider 0 at every step. The issue's 70 % band target is not met.

Not done: the 70 % band target (#201); faint crow's feet behind glasses come up only a little (below the line threshold); a
faint pale streak on the top line of a deeply lined forehead; stubble in Cheeks and Smile Lines can be softened (the parse
has no beard class); the unused LaMa plumbing for Wrinkles in the engine (`fill_wrinkles`, the key's LaMa term).

## What it does (v1, kept for a face without a search)

The Wrinkles feature of portrait retouching (`fork/retouch-foundation.md`), on the face mesh and the part masks of the
people parse (`fork/retouch-masks.md`). Four sliders, 0…100, off at 0, for every face or the picked one:

- **Forehead**: from just above the brows up past the top of the face mesh, cut by Facial Skin (so the hairline).
- **Crow's Feet**: a fan beyond each eye's outer corner, starting a little past the corner so the lash line stays.
- **Smile Lines**: a band from each side of the nose down past the mouth's corner, outside the lips (v2 widens it, see above).
- **Neck**: under the jaw line, about half a face down, cut by Body Skin.

Each slider lifts the dark creases in its area by up to 90 % of their depth. Pores, fine hairs and grain stay, and so
do the face's shape and light: a broad shadow or a fold of the face is not a line.

How (`crates/retouch/src/wrinkles.rs`, `wrinkles/areas.rs`, `wrinkles/lines.rs`):

- **Areas** are soft shapes built from the face mesh's points (brows, outline, eye corners, nose wings, mouth
  corners, jaw), sizes as fractions of the face. A face without the mesh (YuNet's five points only, or a profile the
  mesh model could not read) is left alone. `wrinkles::area_weight` is where the Refine brush (#99) will add and take
  away strokes: one weight per area.
- **Bands** of each channel's log, inside the area's skin mask: fine (under 0.25 % of the face: pores, kept), lines
  (up to the area's coarse blur: 1.5 % for crow's feet, 3 % forehead and neck, 3.5 % smile lines) and shape (kept).
  A pixel darker than the plain average of the skin around it by more than 2 % starts to count as a crease (fully
  at 7 %), so the skin's own mottling stays. It is lifted to a second average that leaves the creases out, so a
  lifted crease meets the skin beside it with no bright rim. Each channel moves toward the skin around it (by at
  most 0.3 in log), so a crease, darker and redder, does not turn orange when lifted. Half of a crease's fine dark
  core goes with it.
- **No halo**: every blur reads the skin only (the picture weighted by the mask, divided by the blurred mask), so
  hair, eyes, lips, clothes and the background never bleed in, and pixels outside the masks keep their bits.
- **Zoom rule**: every read stays inside the skin masks' boxes, within `Wrinkles::REACH` (about a fifth of the
  face) of each pixel; the step's zoom rule (`crates/pipeline/src/retouch_step.rs`, `window_for_reads`, see
  `crates/retouch/README.md`) renders a face's whole read area around a zoomed window. No statistic comes from the
  window's pixels; the coarse blurs' cells and the weight's lattice are counted from the face's corner. A window
  holding part of a face equals the whole render inside it.
- **A big face**: past `MAX_BOX` pixels (about 67 M, a face filling a very large export) the face's areas are worked
  on reduced by a whole factor and the change carried back as a gain (`crates/retouch/src/reduced.rs`), never
  skipped.
- **Speed**: the coarse blurs run on cells of at least 2.5 per sigma, read back bilinearly; the wide margin only
  feeds them. Each skin mask is sampled once per face (`PartMask::sampler`), and its log and fine blur are built
  once for every area it cuts; pixels where the mask is 0 are skipped; an area's weight is measured on a lattice
  (0.4 % of the face) only inside its bounds and read bilinearly. Areas each start from the face's own pixels, their
  lifts multiplying where two meet. On the CPU, on the parse's cached masks; no model runs while a slider moves.

Speed (release, one face, all four sliders at 60, fastest of 60 runs on a test Mac with other work running; CC0
portraits):

| | Draft (1936 px) | Loupe (2560 px) |
| --- | --- | --- |
| Head-and-shoulders portraits | 6.2–11.8 ms | 6.6–13.1 ms |
| A face filling the frame | 13.9 ms (was 23.9 ms in the same run) | 22.0 ms (was 33.6 ms) |

Single-threaded, the face-filling frame went from 106 ms to 66 ms. The output is the same as before within one 8-bit
level.

## Branch

`feat/retouch-wrinkles`

## Own files

v2: `crates/retouch/src/wrinkles/{search,zones,ridge,flatten,glasses,heal,hairlike,exclude,fix}.rs`,
`crates/retouch/tests/wrinkles_v2.rs`, `crates/retouch/examples/wrinkles_sheet.rs`, `crates/engine/src/retouch_input/wrinkles.rs`,
`crates/engine/examples/wrinkles_v2.rs`.

v1: `crates/retouch/src/wrinkles.rs`, `crates/retouch/src/wrinkles/areas.rs`, `crates/retouch/src/wrinkles/lines.rs`,
`crates/retouch/examples/wrinkles.rs`, `crates/retouch/tests/wrinkles.rs`, `crates/ui-egui/src/panels/portrait/wrinkles.rs`,
`fork/retouch-wrinkles.md`, `fork/tests.d/retouch-wrinkles.txt`

## Hooks in official files

None: the zoomed window's call to `retouch_step::window_for_reads` in `crates/pipeline/src/lib.rs` came with
Eyes (`fork/retouch-eyes.md`).

In fork files:

- `crates/pipeline/src/retouch_step.rs`: `window_for_reads` grows the window until no face's reads touch it
  without lying inside it (faces chained by their reads); tests of a zoomed face, of chained faces, and of seven
  sliders of three features on one face.
- `crates/retouch/src/reduced.rs` (new) and `Scene::through` in `src/feature.rs`: a face too big for full-size
  work, worked on reduced. `src/skin.rs`: Skin uses it too (`Skin::apply_capped`) instead of skipping the face.
- `crates/retouch/src/input.rs`: `PartMask::sampler`, a mask sampled at every pixel with its checks done once.
- `crates/retouch/README.md`: the `REACH` line, chained faces in the zoom rule, a big face never skipped, the
  wrinkles example in the `face_input` paragraph.
- `crates/parsing/examples/face_input.rs`: the same file as on `feat/retouch-eyes`.
- `crates/ui-egui/src/tests/portrait.rs`: a test of the Wrinkles section.
- The stub's control `retouch.wrinkles.amount` is now four controls. The stub had no effect and nothing referred to
  it, so no saved edit changes; a saved `amount` is dropped when settings are read.

## Tests

v2 (`crates/retouch/tests/wrinkles_v2.rs`, the unit tests in `crates/retouch/src/wrinkles/`, `crates/engine/src/retouch_input/wrinkles.rs` and `parts/tests.rs`): the float crop keeps a 16-bit ramp's levels; an 8-bit picture reads a faint line in the shadows as a wrinkle and the float one does not; a cancelled search drops every face and leaves the photo to be searched again (a render after the wait, an export at once); installing LaMa changes the key; Skin 100 with Wrinkles 100 leaves no pixel much brighter than the skin around it (it overshoots by 0.19 without the bound); the widened areas (between the brows, under the eyes, cheek, chin) take their lines away and the other sliders do not; v2 differs from v1 at the same slider; pixels where the stored change is zero keep their bits and no lift passes the mask; the change follows a warped frame; a draft and an export agree pixel for pixel;
slider 0 and the other areas' sliders leave every bit; at 100 a pixel has the stored change and at 50 half of it; Forehead 100
takes away most of the lines' depth and the pores come back inside them; Forehead 100 takes thin 3 px creases away and keeps
the light's gradient and level (#201; failed before at 0.18 of 0.22); the lines even out in light and colour at the clear
skin's level; a spot and a line down the forehead are not found as lines; skin seen through a lens counts and the frame and
its margin don't; a freckle in a crease comes up with it and a big dark patch keeps its pixels; a face keeps its deepest crest points up to the cap; the tiles give back every cell; a window equals the whole render inside it; a draft and
an export four times its size leave the same share; a cancelled search keeps nothing and v1 shows (stopped after
the first, third or fourth check); faces without the mesh or the skin mask get none, a face without
wrinkles an empty change; the exclusions cover a synthetic ear, nostril and face edge and not the forehead, cheek or neck; a hair
is not hair-like unless thin, bright and grey; the search's row is there and its cancel stops it; no panic on odd faces.

v1:

`fork/tests.d/retouch-wrinkles.txt`. They cover: off and every slider at 0 give back the original bits; pixels
outside every mask keep their bits (Facial Skin over half the forehead); a face without the mesh, without the masks,
or with masks and five points left alone; Forehead 100 taking away at least 60 % of the lines' depth and keeping the
pores' energy within 15 %, 50 between 0 and 100; light across the face and a broad shadow left within 1 %, flat grey
staying flat; each slider changing only its own area's rows, the neck only on Body Skin; no darkening or blue tint
next to dark blue-black hair at the mask's edge; a face cut by the photo's edge; a draft and an export four times its
size leaving the same share of the lines (within 15 %); windows holding part of the face (forehead, an eye, a mouth
corner, the neck), grown by the reach, equal to the whole render inside them (fails with the cells counted from the
window); in the pipeline, a zoomed render of a face equal to the whole render inside it, within one 8-bit level
(fails without `window_for_reads`), and no growth far from faces or with the block off; no faces, 50 faces, faces
past the edges, a mesh of one point or of points that are not numbers, 1 × 1 and empty photos; a face with its own
settings the only one retouched; the Wrinkles section's four sliders and a click setting Forehead. A face too big
for full-size work (a small cap in the test) retouched reduced, close to the full-size change, outside the masks
unchanged (Wrinkles and Skin; both failed with the old skip); a zoomed window holding the reads of three faces
chained by their reads (failed with one pass); a mask's sampler equal to `PartMask::sample`.
