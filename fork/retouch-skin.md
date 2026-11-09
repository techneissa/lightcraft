# Portrait retouching: Skin

List: the Portrait tab's Skin section: Smoothness evens the skin and keeps its pores, Texture strengthens the pores, Shine lowers oily highlights; on each face's Facial Skin mask, sizes in fractions of the face. Branch `feat/retouch-skin`.

## What it does

The Skin feature of portrait retouching (`fork/retouch-foundation.md`), on the part masks of the people parse
(`fork/retouch-masks.md`). Three sliders, 0…100, off at 0, for every face or the picked one:

- **Smoothness** takes away the skin's unevenness: bumps, small marks and fine lines (up to 90 %), blotches and
  uneven colour (up to 60 %). Pores, fine hairs and grain are kept as they are.
- **Texture** strengthens the pores and fine detail, up to 60 % more.
- **Shine** lowers oily highlights: skin brighter than the skin around it and paler than it (white light on the skin's
  colour) is brought down, which gives it back its colour. Light falling on one side of a face keeps its colour, so
  it stays. On a black-and-white photo brightness alone finds the shine, so a strongly lit side is lowered a little
  too.

How (`crates/retouch/src/skin.rs`): frequency separation inside the Facial Skin mask. Gaussian blurs of 0.8 %, 3 %
and 7 % of the face's size split the skin into fine (texture), mid (unevenness), tone (blotches) and base (the face's
shape and light) bands; Smoothness scales the mid and tone bands down, Texture the fine band up, and each change
fades out where a band's contrast is high (a nostril, a lash line), so the face keeps its shape. Shine compares the
smoothed skin with a 10 % blur. Every blur reads the skin only (the picture weighted by the mask, divided by the
blurred mask), so hair, eyes, lips and background never bleed in: no halo at the mask's edge. The change is weighted
by the mask; pixels outside it never change. The blurs run on the mask's box reduced so each is at least 2.5 reduced
pixels wide, and are read back bilinearly.

Speed (a test Mac, release, one face, all three sliders on, fastest of 20, other work running): draft
(1936 px long edge) 3.3–4.7 ms on portraits framed head and shoulders, 13.6 ms on a face filling the frame; loupe
(2560 px) 4.6–7.0 ms and 22 ms. The step runs on the CPU (on the GPU, a retouched photo's scene-linear stage already
runs on the CPU), on the parse's cached masks; no model runs while a slider moves.

A Refine brush per feature comes later (#99): `skin_mask` in `skin.rs` is where its strokes will be applied to the
mask.

## Branch

`feat/retouch-skin`

## Own files

`crates/retouch/src/skin.rs`, `crates/retouch/examples/skin.rs`, `crates/retouch/tests/skin.rs`,
`crates/ui-egui/src/panels/portrait/skin.rs`, `fork/retouch-skin.md`, `fork/tests.d/retouch-skin.txt`

## Hooks in official files

None. In fork files:
- The stub's control `retouch.skin.amount` is now `retouch.skin.smoothness`: renamed in the foundation's tests
  (`crates/retouch/tests/foundation.rs`, `crates/develop/src/retouch.rs`, `crates/engine/src/retouch_input/tests.rs`,
  `crates/mcp/src/tests_retouch.rs`, `crates/pipeline/src/retouch_step.rs`) and in a doc comment of
  `crates/retouch/src/control.rs`. The stub had no effect, so no saved edit changes. There is no alias, on purpose:
  a `retouch.skin.amount` value set through MCP, the control channel or a script is not a control any more
  (`develop.set` reports it unknown), and one in saved settings is dropped when they are read.
- `crates/ui-egui/src/tests/portrait.rs`: a test of the Skin section.
- `crates/retouch/README.md`: how the examples get masks.
- `crates/parsing/examples/parts.rs` also writes each person's masks as grey pictures (`outdir/masks/`), which the
  skin example reads with `--mask`.

## Tests

`fork/tests.d/retouch-skin.txt`. They cover: off and every slider at 0 give back the original bits; pixels outside
every Facial Skin mask keep their bits (three faces, one past the photo's edge); no faces, 50 faces, faces past the
edge, a 1 × 1 and an empty photo; a face without the Facial Skin mask (no model, YuNet boxes only) left alone;
Smoothness taking away at least 65 % of the blotches' energy and keeping the pores' within 15 %; Texture
strengthening the pores and leaving the blotches; Shine lowering a pale highlight and giving it back its colour, and
lowering light in the skin's own colour far less; a flat grey photo staying flat; no halo next to dark hair at the
mask's edge; a face cut by the edge retouched inside the photo; a draft and an export four times its size making the
same change (within 15 %), and Texture on a 60 px face against a 240 px export (fine blurs under 2 px use exact
Gaussian taps; the box blur left a face under about 72 px untouched); a face with its own settings being the only one retouched; the Skin section's three
sliders and a click setting Smoothness.
