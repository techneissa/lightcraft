# Portrait retouching: Skin Tone

List: the Portrait tab's Skin Tone section: Even Tone evens blotchy skin colour, Redness lowers red patches, Face Light lightens or darkens the face with a soft edge; on each face's skin masks, sizes in fractions of the face. Branch `feat/retouch-skin-tone` (issue #133, part of #79).

## What it does

The Skin Tone feature of portrait retouching (`fork/retouch-foundation.md`), on the part masks of the people parse
(`fork/retouch-masks.md`). Three sliders, off at 0, for every face or the picked one:

- **Even Tone** (0…100): blotchy skin in Facial Skin and Body Skin evens out: age and sun spots, darker and lighter
  patches and colour blotches move towards the skin around them. The face keeps its shading (the light across it, the
  nose's sides, the folds from the nose to the mouth, the eyes' and lips' rims, the nostrils), its pores, freckles,
  wrinkles and stubble.
- **Redness** (0…100): red skin calms. A red spot takes the colour of the skin around it; flushed cheeks or a red nose
  move 60 % of the way to the face's own calm skin. Brightness stays, and the skin keeps its warmth (it is not greyed).
- **Face Light** (−100…+100): the face (Facial Skin with its eyebrows, eyes, lips and teeth) lighter or darker
  against the rest of the photo, up to 0.6 stops, with a soft edge that falls to nothing at the face's border.

How (`crates/retouch/src/skin_tone.rs`, the model in `skin_tone/model.rs`, issue #205): colour is measured on a grid
of cells, 160 across the face for Facial Skin and 64 for Body Skin (at least a pixel), over the box each skin mask can
touch; each cell is the mask-weighted mean of the skin's pixels, and each blur divides by the blurred mask, so hair,
eyes, lips and the background never bleed in. In a processed photo a blotch and the face's shading darken the skin
along the same colour direction (the camera's tone curve turns darker skin redder too), so colour cannot tell them
apart; shape and place can:

- The shading is a local plane fitted to the log luminance over 4 % of the face, robustly (a cell more than 0.12 off
  the plane counts for nothing), so it follows the light and the face's slopes but not a blotch.
- What is left at the blotch scale (0.8 % of the face) is evened where it is round. Where it is long and narrow (the
  coherence of its gradients over 1.5 % of the face), within the mesh's zones (eyes, lips' edge, nostrils, the folds
  from the nose's wings to the mouth's corners: `skin_tone/zones.rs`), deeper than 0.6 (log), or in dense fine
  texture (stubble: 2.5 times the face's typical in-cell variance), it is the face's form and stays. 95 % of a dark
  blotch goes at 100, half of a light one, almost none of a highlight.
- A lightened blotch moves along the face's own shading direction (how this photo's skin colour changes with its
  brightness over large scales), so it takes the colour the skin around it has at that brightness, not a greyer one.
- Colour, with the shading's colour taken out, moves 85 % of the way to the skin's colour over 15 % of the face.
- Redness reads colour with a quarter of the shading's colour taken out (the rest keeps shadows warm). A cell redder
  than the skin over 12 % of the face around it goes all the way to it; then, where it is still redder than the
  face's calm skin (the cells between the 10th and 30th percentile of redness), 60 % of the way to that. All the way
  greys the skin: the face's least red skin is also its least saturated.
- Both sliders respond more in the middle: 50 gives 65 % of 100's change.

Approaches tried and dropped (pictures in `$WORKSPACE/assets/lightcraft/shots/even-tone-205/`): taking 85 % of the
1.2 %–12 % band (the issue's suggestion) flattens the face (a beard lit, the nose's and cheeks' shading gone) and
still removes only 41–47 % of that band, as most of it is the face's form; a melanin / haemoglobin split by colour
fails because blotches and shading share one colour direction in these photos; Hessian blob tests and a two-band
split left the age spots half done; Redness pulled fully to the calm skin, or along the face's own red direction,
greyed the skin; a hue turn alone barely showed.

The result is a smooth gain per cell, read back bilinearly and multiplied in, weighted by the mask: texture finer than
a cell keeps its contrast, and a pixel outside the mask keeps its bits. The wide blurs run on cells up to four times
larger and the shading fit on cells twice as large. Face Light blurs the face's area on a coarser grid (48 cells
across the face) and lights where the blur is above half, ramping to full at 0.92.

Per draft render with Even Tone and Redness at 100 (a test Mac, release, fastest of 20, other work running; one face,
its Facial Skin and Body Skin masks and face mesh from the parse; draft 1936 px long edge): 9.9–13.1 ms on the audit
portraits p1, p2, p3 and p7 (the face filling the frame on p1: 12.5 ms), against 3.5–7.6 ms before #205 measured at
the same time. The step runs on the CPU, on the parse's cached masks; no model runs while a slider moves.

The stand-in slider `retouch.skinTone.amount` of `fork/retouch-masks.md` is gone: `retouch.skinTone.faceLight` replaces
it in the tests that used it, and a saved `amount` reads as Face Light (`#[serde(alias)]`; the stand-in gave about
0.59 stop at 100, Face Light +100 gives 0.6).

Zoom: every read stays inside the areas of the skin and face masks (each grid covers a mask's box; the blurs run on
the grid), so `REACH` is 0.02, a pixel of rounding. A zoomed window renders those areas whole
(`window_for_reads`), so the colour of the skin around a spot comes from the whole face, never from the window.

Small faces: a cell is never under a pixel, so a face under 160 px in the draft has coarser cells than its export.
On the synthetic test faces (48–64 px in the draft) the export four times larger differs by 0.5 % on average and
9 % at worst in a blotch; on the audit portraits the draft's change and the full-size export's correlate at 0.998
(p2, a 1119 px face in the draft) and 0.978 (p7, 137 px).

## Branch

`feat/retouch-skin-tone`

## Own files

`crates/retouch/src/skin_tone.rs`, `crates/retouch/src/skin_tone/grid.rs`, `crates/retouch/src/skin_tone/model.rs`,
`crates/retouch/src/skin_tone/zones.rs`, `crates/retouch/examples/skin_tone.rs`,
`crates/retouch/tests/skin_tone.rs`, `crates/ui-egui/src/panels/portrait/skin_tone.rs`, `fork/retouch-skin-tone.md`,
`fork/tests.d/retouch-skin-tone.txt`

## Hooks in official files

No product code outside the own files. Docs and tests only:

- `crates/retouch/README.md` (the skin tone example reading masks), `fork/retouch-masks.md` (the proof is
  replaced), `crates/ui-egui/src/tests/portrait.rs` (the section's test), and the control id
  `retouch.skinTone.faceLight` in the tests of `crates/pipeline/src/retouch_step.rs`, `tests_spots_keep.rs`,
  `crates/engine/src/retouch_input/tests.rs`, `parts/tests.rs`, `crates/retouch/tests/foundation.rs` and
  `frames.rs`. The warp test in `retouch_step.rs` counts the red pixels at least Face Light's edge (`LIGHT_EDGE` of
  the face) inside the patch, 97 % of them lit (fails with the warp mapping off: 75 %), and its zoom test has a
  Skin Tone window over a cheek (fails without the step growing the window: 9 levels off).

## Tests

`fork/tests.d/retouch-skin-tone.txt`. They cover: off and every slider at 0 changing no bit; Even Tone halving
blotches with the texture kept within 5 %; on a face lit from one side, Even Tone and Redness at 100 taking a blotch
35 % darker down to under 40 % of its darkness and a red patch to under 30 % of its excess red, the light across the
face kept within 0.03 (log) (#205); the shading fit following a ramp and ignoring a blotch; a line counted as form
and a disc not; the shading direction falling back to grey on nonsense; the mesh zones' distances; Redness lowering a red patch's excess red by more than half with its
brightness kept and the skin away from it unchanged; Face Light lighting the middle and the eyes by 0.6 stops,
falling off towards the edge over at least three pixels, nothing outside the face, −100 darkening by as much; every
slider on with three faces (one past the photo's edge, one Body Skin only) leaving every pixel outside the masks
bit for bit; a face without masks (YuNet boxes only) left alone; no halo next to dark hair; a flat grey photo staying flat under Even Tone and Redness; a 1 × 1 photo;
no faces, 50 faces, faces past the edge and an empty photo; a draft and an export four times its size agreeing after
scaling (mean difference under 1 %), also for a face under 128 px; a zoomed window over a cheek equal to the whole
frame within one level; a saved `amount` read as Face Light; `PARTS` holding every part read; the grid's cells, rows and bilinear reads; the Portrait tab's Skin Tone section
with its three sliders.

## What's New entry

### Portrait Skin Tone
- The Portrait tab's Skin Tone section evens blotchy skin colour (Even Tone), lowers red patches (Redness) and lightens or darkens the face against the rest of the photo with a soft edge (Face Light), for every face or one. It works on the skin masks found by people parsing; pores and natural gradients stay, and nothing outside the skin changes.
