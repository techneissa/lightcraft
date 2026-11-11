# Portrait retouching: Skin

List: the Portrait tab's Skin section: Smoothness evens the skin and keeps its pores, Texture softens or strengthens the pores, Shine lowers oily highlights; on each face's Facial Skin mask, sizes in fractions of the face. Branch `feat/retouch-skin`.

## What it does

The Skin feature of portrait retouching (`fork/retouch-foundation.md`), on the part masks of the people parse
(`fork/retouch-masks.md`). Three sliders, off at 0, for every face or the picked one:

- **Smoothness** (0…100) evens the skin: bumps, small marks, fine unevenness and blotches go, while the pores, fine
  hairs and grain stay (about 90 % kept), and so do edges: eyes, brows, lips, nostrils, lash lines, folds, glasses,
  stubble and the face's own shading. Every step shows: low values even gentle unevenness almost fully and leave
  stronger marks, higher values reach marks of more contrast too. Of the cheek's unevenness between 0.2 % and 3 % of
  the face on portraits p1–p4 of the audit in #195, 25 takes away 20–31 %, 50 38–58 % (a plainly visible, natural
  retouch), 75 53–77 % and 100 61–85 % (a strong retouch that keeps the pores); on the low-key p5, whose cheek's
  unevenness at that size is mostly its shading, 7 / 15 / 24 / 29 %.
- **Texture** (−100…100) softens the pores (−100 takes most of their contrast away) or makes them crisper (+100
  doubles their contrast).
- **Shine** (0…100) lowers oily highlights: skin brighter than the skin right around it and paler than it (the
  light's colour on the skin's) is brought toward that skin's light and colour; a quarter of its excess stays at
  100, so the skin keeps some sheen. A lit cheek or side of the face, broad and in the skin's own colour, stays. 50
  takes away about half of a highlight's excess, 100 three quarters. On a black-and-white photo brightness alone
  finds the shine.

How (`crates/retouch/src/skin.rs`, `skin/grid.rs`): frequency separation inside the Facial Skin mask, in log
luminance and colour ratios (each channel over luminance: calm where a channel is near 0, as blue is on warm skin
in low light). The pores are what is finer than a blur of 0.2 % of the face; Smoothness keeps them and Texture
scales them. The rest goes through a bilateral grid (Chen, Paris and Durand, 2007): the skin gathered into cells of
about 1.6 % of the face and bins of light, blurred by 4 % of the face in space and by up to 0.4 in log light, and read
back at each pixel's own place and light. A pixel is averaged only with skin of about its own brightness, so a
bump or a blotch is evened right up to a nostril or a fold without crossing it, and with no halo. 15 % of a wider
grid (7 % of the face) is mixed in for larger blotches. The slider sets how much of the even skin is taken
(`1 − (1 − s)^1.6`: two thirds at 50) and the difference of light it evens across (0.16 just above 0, 0.4 at 100). A colour change larger than about 8 % of a channel's share
is held back, because the grid tells edges by light only (a lit nose wing beside a cheek of another colour keeps
its colour). Smoothness fades in over 1 % of the face from the mask's edge, so there is no seam against hair, beard
or background. Every blur reads the skin only (weighted by the
mask), the change is weighted by the mask, and pixels outside it never change. The grid's size is a fixed fraction
of the face, so a draft and an export make the same change.

Shine (`skin/shine.rs`, issue #209): the skin around each pixel is a blur over 5 % of the face that counts only skin
within about the usual spread of light of that place (a first blur gives the usual light and its spread), so the
shine itself, a beard or a shadow does not pull it up or down. A pixel 1.08× brighter than it starts to count, 1.5×
counts fully, scaled down to a quarter when it is as saturated as that skin; that weight is blurred over 0.8 % of the
face, so where a pale highlight meets skin of a deeper colour no line is drawn. The pixel's pore-free light and colour
move toward the skin around it by the weight times the slider (`1 − (1 − s)^1.5`, up to three quarters), and the
pores' contrast there is calmed by up to 40 %. On the audit's portraits the earlier version darkened lit cheeks and
noses and turned them pink or purple (it took grey light away where the skin was brighter than a 10 % blur), and left
real shine, which has the light's colour, often a little blue, with a lilac cast.

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

`crates/retouch/src/skin.rs`, `crates/retouch/src/skin/grid.rs`, `crates/retouch/src/skin/shine.rs`, `crates/retouch/examples/skin.rs`, `crates/retouch/tests/skin.rs`,
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
far less; Shine bringing a bluish spot on skin lit from one side toward the skin's colour, 50 a plain step toward
100, the lit side left alone (issue #209: the earlier version left the spot lilac); a flat grey photo staying flat; no halo next to dark hair at the mask's edge; a face cut by the edge
retouched inside the photo; a draft and an export four times its size making the same change (within 15 %), and
Texture on a 250 px face, its pores half a pixel wide, against a 1000 px export (fine blurs under 2 px use exact
Gaussian taps); a face with its own settings being the only one retouched; a face too big for full-size work
retouched reduced; the Skin section's three sliders and a click setting Smoothness.
