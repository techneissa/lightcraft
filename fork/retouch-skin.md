# Portrait retouching: Skin

List: the Portrait tab's Skin section: Smoothness evens the skin and keeps its pores, Texture softens or strengthens the pores, Shine lowers oily highlights; on each face's Facial Skin mask, sizes in fractions of the face. Branch `feat/retouch-skin`.

## What it does

The Skin feature of portrait retouching (`fork/retouch-foundation.md`), on the part masks of the people parse
(`fork/retouch-masks.md`). Three sliders, off at 0, for every face or the picked one:

- **Smoothness** (0…100) evens the skin: bumps, small marks, fine unevenness and blotches go (at 100, 61–85 % of the
  cheek's unevenness between 0.2 % and 3 % of the face on portraits p1–p4 of the audit in #195; 29 % on the low-key
  p5, whose cheek's unevenness at that size is mostly its shading), while the pores, fine hairs
  and grain stay (about 90 % kept), and so do edges: eyes, brows, lips, nostrils, lash lines, folds, glasses, stubble
  and the face's own shading. 50 is a natural everyday amount; 100 is a strong retouch.
- **Texture** (−100…100) softens the pores (−100 takes most of their contrast away) or makes them crisper (+100
  doubles their contrast).
- **Shine** (0…100) lowers oily highlights: skin brighter than the skin around it and paler than it (white light on
  the skin's colour) is brought down, which gives it back its colour. Light falling on one side of a face keeps its
  colour, so it stays. On a black-and-white photo brightness alone finds the shine, so a strongly lit side is
  lowered a little too.

How (`crates/retouch/src/skin.rs`, `skin/grid.rs`): frequency separation inside the Facial Skin mask, in log
luminance and colour ratios (each channel over luminance: calm where a channel is near 0, as blue is on warm skin
in low light). The pores are what is finer than a blur of 0.2 % of the face; Smoothness keeps them and Texture
scales them. The rest goes through a bilateral grid (Chen, Paris and Durand, 2007): the skin gathered into cells of
about 1.6 % of the face and bins of light, blurred by 4 % of the face in space and by 0.4 in log light, and read
back at each pixel's own place and light. A pixel is averaged only with skin of about its own brightness, so a
bump or a blotch is evened right up to a nostril or a fold without crossing it, and with no halo. 15 % of a wider
grid (7 % of the face) is mixed in for larger blotches. A colour change larger than about 8 % of a channel's share
is held back, because the grid tells edges by light only (a lit nose wing beside a cheek of another colour keeps
its colour). Smoothness fades in over 1 % of the face from the mask's edge, so there is no seam against hair, beard
or background. Shine compares the smoothed skin with a 10 % blur. Every blur reads the skin only (weighted by the
mask), the change is weighted by the mask, and pixels outside it never change. The grid's size is a fixed fraction
of the face, so a draft and an export make the same change.

Tried and dropped (issue #206): a guided filter on log luminance with a window variance as the edge test. A
nostril or the face's shadow anywhere in the 3 % window marked the flat cheek beside it as an edge, so the skin
next to every edge stayed uneven, and the low-key p5 hardly changed. A log per colour channel turned the near-zero
blue of warm skin into noise.

Speed (a test Mac under heavy load from other work, release, one face, all three sliders on, fastest of 30, draft
1936 px): about 2× the earlier version's work. 7.8 ms on a head-and-shoulders portrait (earlier 3.5 ms), 22 ms on a
face filling a portrait frame (13 ms), 44 ms on a face filling a landscape frame (20 ms); single-threaded 18 / 71 /
151 ms (8 / 35 / 86 ms). The step runs on the CPU (on the GPU, a retouched photo's scene-linear stage already runs
on the CPU), on the parse's cached masks; no model runs while a slider moves.

A Refine brush per feature comes later (#99): `skin_mask` in `skin.rs` is where its strokes will be applied to the
mask.

## Branch

`feat/retouch-skin`

## Own files

`crates/retouch/src/skin.rs`, `crates/retouch/src/skin/grid.rs`, `crates/retouch/examples/skin.rs`, `crates/retouch/tests/skin.rs`,
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
Smoothness taking away at least 65 % of the blotches' energy and keeping the pores'; Smoothness 100 taking away at
least 60 % of bumps 0.4 % of the face across (between 0.2 % and 3 % of the face in log luminance) and keeping at
least 70 % of the pores (issue #206: the earlier version took 35 %); Texture strengthening the pores and leaving the
blotches; Shine lowering a pale highlight and giving it back its colour, and lowering light in the skin's own colour
far less; a flat grey photo staying flat; no halo next to dark hair at the mask's edge; a face cut by the edge
retouched inside the photo; a draft and an export four times its size making the same change (within 15 %), and
Texture on a 250 px face, its pores half a pixel wide, against a 1000 px export (fine blurs under 2 px use exact
Gaussian taps); a face with its own settings being the only one retouched; a face too big for full-size work
retouched reduced; the Skin section's three sliders and a click setting Smoothness.
