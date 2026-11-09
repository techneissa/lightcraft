# Portrait retouching: Wrinkles

List: the Portrait tab's Wrinkles section: Forehead, Crow's Feet, Smile Lines and Neck take away the deep wrinkles in each area and keep the pores and the face's shape; areas from the face mesh, sizes in fractions of the face. Branches `feat/retouch-wrinkles` (v1), `feat/wrinkles-v2` (v2).

## Wrinkles v2: removal from a stored change

v1 (below) only lifts a crease by part of its depth, so deep wrinkles stayed. v2 takes them away:

- **Once per photo**, in the face search next to Blemishes' spots and Stray Hair's strands (`crates/engine/src/retouch_input/wrinkles.rs`,
  `crates/retouch/src/wrinkles/search.rs`), for each face with the mesh and a Facial Skin mask, on a crop of the four areas at
  most 768 px (reduced by a whole factor when bigger):
  1. **Where a wrinkle may be**: eroded skin (0.6 % of the face) inside an area, and not an ear, a nostril or the face's edge
     (`exclude.rs`, from the mesh's outline and nostril points), and not a hair across the skin (`hairlike.rs`: a thin
     bright line that is greyer than the skin; a skin-coloured lit ridge is skin).
  2. **The mask** (`ridge.rs`): valleys of the log luminance at four widths (0.25–1.8 % of the face; a Hessian ridge filter),
     crests only, kept when they connect to a strong response and are long enough, grown 0.35 line widths, soft edge 0.35 %.
  3. **The replacement** (`heal.rs`): LaMa fills the mask (the engine's `lama_fill` runner, crop at 768 px, little context)
     and only its low band is used (blurred to twice the fine blur); without LaMa the **banded heal** rebuilds the low band
     from the skin around the mask (normalised convolution at growing widths). The mid band (the crease and its flanks) is not
     put back. The **fine band** (pores, sigma 0.2 % of the face, at least 2 px) comes back from clean skin nearby (first of 14
     shifts whose source is outside the mask, with skin all around it weighted by the Facial Skin mask), held to 3 times the
     skin's own typical fine band so a stray hair or speck is not copied whole.
  4. **The change** (log light per channel, new over old) is kept in `Face::wrinkles` (`wrinkles::Fix`: a grid over the crop,
     normalised like the masks, 16-bit cells, trimmed to where there is a change), in the face store and the input's
     fingerprint. No new saved field: it lives with the faces, not in the settings.
- **A row** `retouch.wrinkles` ("Smoothing Wrinkles") in the activity stack while it runs; its ✕ stops it (the faces then keep
  no change, and Wrinkles shows v1's result). LaMa's own row is not added (the search's row is held by the fill's run).
- **A slider tick** scales the stored change (`fix.rs`): in each area, slider/100 times the area's weight (v1's `areas.rs`,
  the strongest where areas overlap) is how much of the change is applied, as a gain per channel on the pixels the render has.
  Slider 0 keeps every bit; 100 gives the stored change inside the area. No model runs. A face whose search has not run (or was
  cancelled, or has no mesh) shows v1's result; a face whose search found no wrinkles has an empty change and is left alone.
- **Zoom**: the change is read at the pixel by its normalised place, the weight lattice is anchored to the face's corner, so a
  zoomed window equals the whole render (tested); a draft and an export make the same change.
- The Refine brush hook `wrinkles::area_weight` is unchanged: it still gives one weight per area.

Time (release, a test Mac with other work running): the face search per photo, with the parse known, 0.3–1.5 s with LaMa (about
0.3 s without it, banded heal) for one face including Blemishes' and Stray Hair's searches; the wrinkles part alone 0.3–0.6 s
with the banded heal on a 1936 px draft. A slider tick (the retouch step, a 1936 px draft, all four sliders at 100): 4.0 ms on a
head-and-shoulders portrait, 7.6 ms on a tall frame (v1: 34 ms and 12 ms); Forehead alone 1.1 ms.

Not done: lit ridges beside a deep crease stay as pale streaks (taking them too gave a flat, wet look); a face crop over 768 px
is searched reduced, so pores at an export's size come from the photo itself; stubble in Smile Lines can be softened (the parse
has no beard class); faces found before LaMa was installed keep the banded heal until the faces are searched again.

## What it does (v1, kept for a face without a search)

The Wrinkles feature of portrait retouching (`fork/retouch-foundation.md`), on the face mesh and the part masks of the
people parse (`fork/retouch-masks.md`). Four sliders, 0…100, off at 0, for every face or the picked one:

- **Forehead**: from just above the brows up past the top of the face mesh, cut by Facial Skin (so the hairline).
- **Crow's Feet**: a fan beyond each eye's outer corner, starting a little past the corner so the lash line stays.
- **Smile Lines**: a band from each side of the nose down past the mouth's corner, outside the lips.
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

v2: `crates/retouch/src/wrinkles/{search,ridge,heal,hairlike,exclude,fix}.rs`, `crates/retouch/tests/wrinkles_v2.rs`,
`crates/engine/src/retouch_input/wrinkles.rs`, `crates/engine/examples/wrinkles_v2.rs`.

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

v2 (`crates/retouch/tests/wrinkles_v2.rs`, the unit tests in `crates/retouch/src/wrinkles/`, `crates/engine/src/retouch_input/wrinkles.rs`):
slider 0 and the other areas' sliders leave every bit; at 100 a pixel has the stored change and at 50 half of it; Forehead 100
takes away most of the lines' depth and the pores come back inside them; a window equals the whole render inside it; a draft and
an export four times its size leave the same share; a filler's fill and the banded heal both remove the lines (no filler = the
banded heal; a cancelled search keeps nothing and v1 shows); faces without the mesh or the skin mask get none, a face without
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
