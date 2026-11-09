# Portrait retouching: Eyes

List: the Portrait tab's Eyes section: Whites, Iris, Dark Circles and Eye Bags sliders, per face, on the people parse's eye masks and the face mesh. Branch `feat/retouch-eyes`.

## What it does

Four sliders in the Portrait tab's Eyes section, 0 to 100, off at 0, for every face or the picked one
(`retouch.eyes.whites`, `retouch.eyes.iris`, `retouch.eyes.darkCircles`, `retouch.eyes.eyeBags`; per face
`retouchFace.<i>.eyes.<field>`):

- **Whites**: the Eye Whites mask gets brighter (darker spots such as veins a little more) and loses most of its
  colour, so a yellow or red cast goes and veins stay as faint grey lines. Lashes and the lid's edge inside the mask
  (much darker than the whites) are left alone.
- **Iris**: the Iris mask gets brighter and its texture sharper (luminance only; noise under a threshold is left as
  it is, and a catchlight's dark rim is held back). Cut to the face mesh's iris circles when the face has them.
- **Dark Circles**: under each eye, the skin's colour, blurred, is taken toward the cheek's mean colour below it:
  lifting only (at most 1.6×) and turning the colour part of the way, so make-up stays make-up.
- **Eye Bags**: under each eye, the shading between a fine and a coarse scale (the bag's bulge and the crease below
  it) is flattened; pores and fine lines are kept.

Where: Whites and Iris on their part masks (a face without them, closed eyes, is left alone). Dark Circles and Eye
Bags in crescents under the lower lids, placed from the 478 face mesh points (corners, lower lid, iris) or, without
the mesh, from YuNet's two eye points, and limited to skin: the Facial Skin mask less Others, plus, through glasses
(the parser puts the lenses in Others and takes them out of Facial Skin), any pixel of a lens whose colour is the
face's skin colour. Dark lashes, liner and frames are left out of every blur and every change, so nothing halos at
their edges. A face without Facial Skin, or without both eye points, gets no under-eye change.

Sizes are fractions of the eye's width (from the landmarks, kept within 3–16 % of the face's size). The under-eye
gains are worked out on a grid in each eye's own frame (16 cells per half eye width), so a draft, the loupe and an
export work them out alike, and a close-up costs no more than a small face; the gains are then laid on the pixels.

Each effect is a per-pixel gain times one weight per area (`eyes::Area`, made in `eyes::weights`), the place the
Refine brush (#99) will scale or extend an area.

Speed (the Eyes step alone, all four sliders at 100, best of 20 runs, Apple M4 Pro under load 12–20, faces on the
whole photo): a portrait at draft 1936 px 1.7–4.3 ms, loupe 2560 px 2.8–5.2 ms; a 10 MP close-up whose face fills
the frame 13.0 / 20.1 ms. A slider drag runs no model: it samples the cached masks. No GPU version is needed for the
16 ms draft budget. In the app, faces from the people parse are mapped through the lens and transform warp per
sample, which these numbers leave out.

The parsing crate's `face_input` example writes what the retouch step gets for each face (box, YuNet's five then the
478 mesh points, the model's part masks merged as the app merges them); `examples/eyes.rs --faces dir/faces.json`
tries the sliders on a picture with them.

## Branch

`feat/retouch-eyes`

## Own files

`crates/retouch/src/eyes.rs`, `crates/retouch/src/eyes/{geom,tone,under}.rs`, `crates/retouch/examples/eyes.rs`,
`crates/retouch/tests/eyes.rs`, `crates/ui-egui/src/panels/portrait/eyes.rs`, `crates/parsing/examples/face_input.rs`,
`fork/retouch-eyes.md`, `fork/tests.d/retouch-eyes.txt`

## Hooks in official files

None. In fork files: the foundation's tests used the Eyes stub's `retouch.eyes.amount` as their sample control and
now use `retouch.eyes.whites` (`crates/develop/src/retouch.rs`, `crates/pipeline/src/retouch_step.rs`,
`crates/engine/src/retouch_input/tests.rs`, `crates/mcp/src/tests_retouch.rs`, `crates/retouch/tests/foundation.rs`);
`crates/retouch/README.md` names the `face_input` example.

## Tests

`fork/tests.d/retouch-eyes.txt`. They cover: off and every slider at 0 giving the original pixels bit for bit; each
slider keeping to its own area, and every pixel away from the eyes keeping its bits with all four at 100; whites
brighter and greyer with a vein fading but still there; a brighter iris; dark circles lifted and turned toward the
skin; the bag's shading flattened; no halo beside a dark line (a lash, a frame), which failed at 24 % and then 8 %
before the blurs left dark cells out; a draft and an export four times its size agreeing after scaling; closed eyes;
glasses (skin through the lens lifted, a dark frame unchanged); no faces, a face without masks or landmarks (no face
model), 50 faces, a face cut by the photo's edge, a 1 × 1 and a flat grey photo; the Eyes section showing its four
sliders.

## What's New entry

### Eyes Retouching in the Portrait Tab
The Portrait tab's Eyes section has four sliders, for every face or the one you pick: Whites brightens the eye whites
and takes out their yellow or red, Iris brightens and sharpens the iris, Dark Circles lifts the shadow under each eye
toward your cheek's colour, and Eye Bags softens the bag's shading. Each keeps the skin's texture and works through
clear glasses.

## Parity rows

| LR-EDIT-PORTRAIT-EYES | Portrait: Eyes (Whites, Iris, Dark Circles, Eye Bags) | — | 🟡 | `ctl:retouch.eyes.whites`, `ctl:retouch.eyes.iris`, `ctl:retouch.eyes.darkCircles`, `ctl:retouch.eyes.eyeBags`, `crates/retouch/src/eyes.rs`, `crates/ui-egui/src/panels/portrait/eyes.rs` | an extra (the reference app has no portrait retouching). Needs the people-parts set for masks; no Refine brush yet (#99). See `fork/retouch-eyes.md` |
