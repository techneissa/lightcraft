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
the mesh, from YuNet's two eye points, and limited to skin: the Facial Skin mask and, through glasses (the parser puts
the lenses in Others and takes them out of Facial Skin), the Others mask, either only where a pixel's colour is the
face's skin colour. Dark lashes, liner and frames, and pale ones (white, silver), are left out of every blur and every
change, so nothing halos at their edges. A face without Facial Skin, or without both eye points, gets no under-eye
change; nor does a face whose eyes the landmarks put outside its box. A lid point the mesh throws far away is held
within the eye's size.

Sizes are fractions of the eye's width (from the landmarks, kept within 3–12 % of the face's size). The under-eye
gains are worked out on a grid in each eye's own frame (16 cells per half eye width, at every size), so a draft, the
loupe and an export work them out alike, and a close-up costs no more than a small face; the gains are then laid on
the pixels. Dark Circles only lifts: no channel gets darker.

**Zoom never changes the result** (all retouching, not only Eyes): each feature declares `Feature::REACH`, how far
past its face's box and its masks' areas it reads (Eyes 0.7 of the face's size, Skin 0.02, half the face for a
feature that declares none). A zoomed render whose window touches what a retouched face reads renders all of it and
cuts back to the window (`window_for_reads` in `crates/pipeline/src/retouch_step.rs`). Reference levels come from
the whole face on fixed grids, never from the window's pixels: the whites' level from a 96 × 32 grid over the Eye
Whites mask, the skin's and the cheek's colour from the eye's grid.

Each effect is a per-pixel gain times one weight per area (`eyes::Area`, made in `eyes::weights`), the place the
Refine brush (#99) will scale or extend an area.

Speed (the Eyes step alone, all four sliders at 100, best of 20 runs, a test Mac with other work running, faces on the
whole photo): a portrait at draft 1936 px 2.5–2.9 ms, loupe 2560 px 2.6–3.8 ms; a 10 MP close-up whose face fills
the frame 11.4 / 18.4 ms. A slider drag runs no model: it samples the cached masks. No GPU version is needed for the
16 ms draft budget. In the app, faces from the people parse are mapped through the lens and transform warp per
sample, which these numbers leave out; a zoomed loupe on a retouched face now also renders the face's whole read area.

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

- `crates/pipeline/src/lib.rs` (`plan`): a zoomed window grows by `retouch_step::window_for_reads`, next to the
  decontaminated edges' halo. In fork files:
- `crates/pipeline/src/retouch_step.rs` (`window_for_reads`, its tests); `crates/retouch/src/feature.rs`
  (`Feature::REACH`, `DEFAULT_REACH`), `src/settings.rs` (`FeatureInfo::reach`, `Features::reads`,
  `Retouch::reads`, `Reads`), `src/lib.rs` (exports), `src/skin.rs` (Skin's `PARTS` and `REACH`),
  `README.md` (the zoom rule, reference stats from the whole face, the `face_input` example).
- The foundation's tests used the Eyes stub's `retouch.eyes.amount` as their sample control and now use
  `retouch.eyes.whites` (`crates/develop/src/retouch.rs`, `crates/pipeline/src/retouch_step.rs`,
  `crates/engine/src/retouch_input/tests.rs`, `crates/mcp/src/tests_retouch.rs`, `crates/retouch/tests/foundation.rs`).

## Tests

`fork/tests.d/retouch-eyes.txt`. They cover: off and every slider at 0 giving the original pixels bit for bit; each
slider keeping to its own area, and every pixel away from the eyes keeping its bits with all four at 100; whites
brighter and greyer with a vein fading but still there; a brighter iris; dark circles lifted and turned toward the
skin, never darkening a channel (also under-eye skin as light as the cheek but redder); the bag's shading flattened;
no halo beside a dark line (a lash, a frame); a pale neutral frame not lifted, as Facial Skin or as glasses; a lid
point at 1e6 and 1e20 px (overflowed before); a draft and an export four times its size agreeing after scaling,
tighter inside the crescents; closed eyes; glasses (skin through the lens lifted, a dark frame unchanged); no faces,
a face without masks or landmarks (no face model), 50 faces, a face cut by the photo's edge, a 1 × 1 and a flat grey
photo; Eyes naming its masks; the Eyes section showing its four sliders. In the pipeline: a zoomed window holding part
of one eye (Eyes) or part of a cheek (Skin) equals the whole frame's render there within one 8-bit level (22 and 2
levels apart before), and the window grows only for a retouched face it touches.

## What's New entry

### Eyes Retouching in the Portrait Tab
The Portrait tab's Eyes section has four sliders, for every face or the one you pick: Whites brightens the eye whites
and takes out their yellow or red, Iris brightens and sharpens the iris, Dark Circles lifts the shadow under each eye
toward your cheek's colour, and Eye Bags softens the bag's shading. Each keeps the skin's texture and works through
clear glasses.

## Parity rows

| LR-EDIT-PORTRAIT-EYES | Portrait: Eyes (Whites, Iris, Dark Circles, Eye Bags) | — | 🟡 | `ctl:retouch.eyes.whites`, `ctl:retouch.eyes.iris`, `ctl:retouch.eyes.darkCircles`, `ctl:retouch.eyes.eyeBags`, `crates/retouch/src/eyes.rs`, `crates/ui-egui/src/panels/portrait/eyes.rs` | an extra (the reference app has no portrait retouching). Needs the people-parts set for masks; no Refine brush yet (#99). See `fork/retouch-eyes.md` |
