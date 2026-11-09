# Portrait retouching: Skin Tone

List: the Portrait tab's Skin Tone section: Even Tone evens blotchy skin colour, Redness lowers red patches, Face Light lightens or darkens the face with a soft edge; on each face's skin masks, sizes in fractions of the face. Branch `feat/retouch-skin-tone` (issue #133, part of #79).

## What it does

The Skin Tone feature of portrait retouching (`fork/retouch-foundation.md`), on the part masks of the people parse
(`fork/retouch-masks.md`). Three sliders, off at 0, for every face or the picked one:

- **Even Tone** (0…100): blotchy colour in Facial Skin and Body Skin evens out (cheeks, chin, forehead patches);
  the skin keeps its pores and its natural gradients, and a beard, nostril shadows or the edge of the lips stay.
- **Redness** (0…100): red patches (cheeks, nose, acne redness) take the colour of the skin around them, up to three
  quarters of the way; their brightness stays.
- **Face Light** (−100…+100): the face (Facial Skin with its eyebrows, eyes, lips and teeth) lighter or darker
  against the rest of the photo, up to 0.6 stops, with a soft edge that falls to nothing at the face's border.

How (`crates/retouch/src/skin_tone.rs`): colour is measured on a grid of cells, 128 across the face (at least a
pixel), over the box each skin mask can touch; each cell is the mask-weighted mean of the skin's pixels, and each
blur divides by the blurred mask, so hair, eyes, lips and the background never bleed in (no halo at the mask's edge).
Facial Skin and Body Skin are each measured against themselves (a neck or a hand is often paler than the face). Even
Tone takes away the difference between the colour at 1.2 % of the face and at 12 %, in log chroma, plus a third of
the brightness difference; differences much larger than a blotch fade out of the correction. Redness compares the
colour at 0.8 % of the face with the skin at 25 %: a spot redder (log red over green) by 0.02 starts to move, by 0.1
moves fully. The result is a smooth gain per cell, read back bilinearly and multiplied in, weighted by the mask:
texture finer than a cell keeps its contrast, and a pixel outside the mask keeps its bits. The wide blurs run on cells
four times larger. Face Light blurs the face's area on a coarser grid (48 cells across the face) and lights where the
blur is above half, ramping to full at 0.92.

Per draft render with every slider on (Apple M4 Pro, release, the machine under load 15–35; one face, its Facial
Skin and Body Skin masks from the parse): draft (1936 px long edge) 2.8–4.2 ms on head-and-shoulders portraits,
8.6 ms with three people; loupe (2560 px) 4.8–6.7 ms and 11.5 ms. The step runs on the CPU (on the GPU, a retouched
photo's scene-linear stage already runs there), on the parse's cached masks; no model runs while a slider moves. No
GPU version: the CPU step is within the draft budget.

The stand-in `retouch.skinTone.amount` of `fork/retouch-masks.md` is gone (`retouch.skinTone.faceLight` replaces it in
the tests that used it); it was never on `custom`, so no saved edit changes.

## Branch

`feat/retouch-skin-tone`

## Own files

`crates/retouch/src/skin_tone.rs`, `crates/retouch/src/skin_tone/grid.rs`, `crates/retouch/examples/skin_tone.rs`,
`crates/retouch/tests/skin_tone.rs`, `crates/ui-egui/src/panels/portrait/skin_tone.rs`, `fork/retouch-skin-tone.md`,
`fork/tests.d/retouch-skin-tone.txt`

## Hooks in official files

No product code outside the own files. Docs and tests only:

- `crates/retouch/README.md` (the skin tone example reading masks), `fork/retouch-masks.md` (the proof is
  replaced), `crates/ui-egui/src/tests/portrait.rs` (the section's test), and the control id
  `retouch.skinTone.faceLight` in the tests of `crates/pipeline/src/retouch_step.rs`, `tests_spots_keep.rs`,
  `crates/engine/src/retouch_input/tests.rs`, `parts/tests.rs`, `crates/retouch/tests/foundation.rs` and
  `frames.rs`. The warp test in `retouch_step.rs` asks 95 % of the patch to be lit, not 97 %: Face Light's soft edge
  leaves the patch's rim barely lit.

## Tests

`fork/tests.d/retouch-skin-tone.txt`. They cover: off and every slider at 0 changing no bit; Even Tone halving
blotches with the texture kept within 5 %; Redness lowering a red patch's excess red by more than half with its
brightness kept and the skin away from it unchanged; Face Light lighting the middle and the eyes by 0.6 stops,
falling off towards the edge over at least three pixels, nothing outside the face, −100 darkening by as much; every
slider on with three faces (one past the photo's edge, one Body Skin only) leaving every pixel outside the masks
bit for bit; a face without masks (YuNet boxes only) left alone; no halo next to dark hair; a flat grey photo staying flat under Even Tone and Redness; a 1 × 1 photo;
no faces, 50 faces, faces past the edge and an empty photo; a draft and an export four times its size agreeing after
scaling (mean difference under 1 %); the grid's cells, rows and bilinear reads; the Portrait tab's Skin Tone section
with its three sliders.

## What's New entry

### Portrait Skin Tone
- The Portrait tab's Skin Tone section evens blotchy skin colour (Even Tone), lowers red patches (Redness) and lightens or darkens the face against the rest of the photo with a soft edge (Face Light), for every face or one. It works on the skin masks found by people parsing; pores and natural gradients stay, and nothing outside the skin changes.
