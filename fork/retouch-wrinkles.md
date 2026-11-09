# Portrait retouching: Wrinkles

List: the Portrait tab's Wrinkles section: Forehead, Crow's Feet, Smile Lines and Neck soften the lines in each area and keep the pores and the face's shape; areas from the face mesh, sizes in fractions of the face. Branch `feat/retouch-wrinkles`.

## What it does

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
- **Zoom rule**: the feature declares `REACH`, how far around a pixel it reads (about a fifth of the face). A zoomed
  window is grown by it around every face that can reach into it (`crates/pipeline/src/retouch_step.rs`,
  `window_for_reads`, called where `plan` grows windows for spots). No statistic comes from the window's pixels; the
  coarse blurs' cells and the weight's lattice are counted from the face's corner. A window holding part of a face
  equals the whole render inside it.
- **Speed**: the coarse blurs run on cells of at least 2.5 per sigma, read back bilinearly; the wide margin only
  feeds them, and the fine blur and the change run inside the area. Each skin mask is sampled once per face for all
  its areas, both sides of a face share one box, and an area's weight is measured on a lattice (0.4 % of the face)
  and read bilinearly. On the CPU, on the parse's cached masks; no model runs while a slider moves.

Speed (Apple M4 Pro, release, one face, all four sliders at 60, fastest of 20; the machine under load ~9 from other
builds):

| | Draft (1936 px) | Loupe (2560 px) |
| --- | --- | --- |
| Head-and-shoulders portraits | 7.0–8.0 ms | 8.4–11.9 ms |
| A face filling the frame | 20 ms | 32 ms |

## Branch

`feat/retouch-wrinkles`

## Own files

`crates/retouch/src/wrinkles.rs`, `crates/retouch/src/wrinkles/areas.rs`, `crates/retouch/src/wrinkles/lines.rs`,
`crates/retouch/examples/wrinkles.rs`, `crates/retouch/tests/wrinkles.rs`, `crates/ui-egui/src/panels/portrait/wrinkles.rs`,
`fork/retouch-wrinkles.md`, `fork/tests.d/retouch-wrinkles.txt`

## Hooks in official files

- `crates/pipeline/src/lib.rs` (`plan`): one call to `retouch_step::window_for_reads` where windows grow for spots,
  joint colour noise reduction and decontamination.

In fork files:

- `crates/retouch/src/feature.rs`: `Feature::REACH` (0 by default) and `AROUND_FACE`. `crates/retouch/src/settings.rs`:
  `Features::reach` and `Retouch::reach`. `crates/retouch/README.md`: the zoom rule, `REACH`, the Done-means line for
  it, and the `face_input` example.
- `crates/pipeline/src/retouch_step.rs`: `window_for_reads` and its test.
- `crates/parsing/examples/face_input.rs`: the same file as on `feat/retouch-eyes` (what the app's step gets for
  each face, for the retouch examples); identical, so the two branches merge without a conflict.
- `crates/ui-egui/src/tests/portrait.rs`: a test of the Wrinkles section.
- The stub's control `retouch.wrinkles.amount` is now four controls. The stub had no effect and nothing referred to
  it, so no saved edit changes; a saved `amount` is dropped when settings are read.

## Tests

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
settings the only one retouched; the Wrinkles section's four sliders and a click setting Forehead.
