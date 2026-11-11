# Portrait retouching: Eyes

List: the Portrait tab's Eyes section: Whites, Iris, Dark Circles and Eye Bags sliders, per face, on the people parse's eye masks and the face mesh. Branch `feat/retouch-eyes`.

## What it does

Four sliders in the Portrait tab's Eyes section, 0 to 100, off at 0, for every face or the picked one
(`retouch.eyes.whites`, `retouch.eyes.iris`, `retouch.eyes.darkCircles`, `retouch.eyes.eyeBags`; per face
`retouchFace.<i>.eyes.<field>`):

- **Whites** (issue #202): the eye whites get brighter by a gain in stops, so their shading keeps its ratios: the
  corners and the shadow under the upper lid stay darker and the eye stays round. The gain at 100 is 1 stop plus
  half the way (in stops) from the whites' level to 0.85 of the face's lit skin, within 1–1.8 stops: whites in
  shadow (deep-set eyes, a low-key photo) get more, bright ones less. A soft shoulder from 0.45 (linear) toward 0.86
  keeps them under paper white, the brightest channel rolls off from 0.7 toward 0.92 so the warm tint never clips,
  and nothing gets darker. Veins and blotches (darker than the white around them at 0.05 half eye widths) lose 60 % of
  their depth (as a power), and 75 % of the colour goes, toward a neutral that keeps 35 % of the lit skin's colour,
  so a red or yellow cast turns a clean white that doesn't look grey-lilac beside warm skin. Left alone: lashes and
  the lid's edge (a pixel under 0.12–0.3 of the whites' level, read on a slightly blurred copy so grain in a dark,
  noisy white is lifted with the white around it); by the lids, any pixel as saturated as the lit skin (the lid's
  rim); and toward the inner corner (from 0.4 to 0.6 across the eye) the caruncle and the pink fold beside it:
  tissue whose colour, read at 0.05 half eye widths so thin veins don't count, is 1.25–1.7 times redder (green over
  red) than the lit white in the middle of the eye, or under 0.25–0.45 of the whites' level. The change also fades
  out toward both corners (whole from −0.7 to 0.6 across the eye, gone at −1.02 and 0.88).
- **Iris** (issue #202): the iris's own pattern comes out: its fibres' contrast (the band between 0.02 and 0.05 of
  the iris's radius, so pixel grain is not raised) and a little clarity (0.05 to 0.2 radii), each measured against
  the iris alone, so the pupil, the sclera and a catchlight don't halo into it. Each iris is raised toward the same
  spread of detail (0.08 for the fibres, 0.2 for clarity; the mean size of its detail against its surroundings), up
  to 3 times the usual raise, so a dark iris, whose pattern is small against its level, shows it at 50 and more at
  100; the noise floor is a share of that spread. A pixel changes by 0.7–1.8× at most, rolled off softly. It gets
  brighter between the pupil and the limbal ring (up to 1.2 stops for a dark iris, 0.85 for a light one, half that
  for an iris with no colour, which would only turn grey), the lower part most (where light enters) and the upper
  part 40 % of it, and gains 20 % of its own colour. The pupil (its radius read from the iris's profile out from the
  centre, else 0.36 of the iris) and the limbal ring stay as they are, and so does anything much brighter than the
  iris (a catchlight, a reflection) or much darker (a lid's shadow, a lash), and a lid's wet rim near the lids. The
  circle is the face mesh's, else the Iris mask's centre and width.
- Both are cut to the eye's opening between the lids when the face has the 478 mesh points (`eyes/tone/opening.rs`:
  the mesh's eye contour from the eye landmarks). Whites fades out 0.03 half eye widths past the upper lid's outline
  and starts 0.16 inside the lower one's (Iris: at the outline and 0.12 inside the lower one): the lower lid's pale
  wet rim lies inside the mesh's outline, on a squinting eye well inside it. The sliders' 0…1 goes through a power
  of 0.8, so 25 and 50 show clearly.
- **Dark Circles**: under each eye, the skin darker than the skin just outside the circle is lifted toward that
  skin's lightness, the slider's share of the way in lightness (L*): 25 closes a quarter of the gap, 50 half, 100 all
  of it, and no value lifts past it. That level is measured on the original pixels, in a ring below the circle and
  beside the eye's corners: the lightness a quarter of the ring is darker than (a lit, shiny cheekbone below the
  circle does not raise it), and never above the ring's skin near the place (the darker side of a side-lit face is
  lifted to its own surroundings). The hue turns toward the ring's skin near that level (the cheekbone's cooler,
  paler colour would make the circle ashy), sooner than the light. Each pixel is lifted from a fine level (40 cells
  per half eye width) that follows a circle's sharp edge, so no line is left along it; dark spots up to 0.05 half
  eye widths (freckles) are closed over in that level and stay texture. The texture against it is kept, its grain
  scaled down to the ring's when a shadow's is stronger. A deep shadow (under 0.15–0.35 of the level) has lost its
  texture and colour to the camera's noise and compression: as it is lifted it takes the texture and colour grain of
  the clean skin 0.7 half eye widths below it, as a healing brush would, and the ring's colour, so it never turns
  into a flat grey patch. The crescent stays at full weight past where a circle ends; skin as light as the ring does
  not move, so the lift ends with the circle.
- **Eye Bags**: under each eye, the shading between a fine (0.025 half eye widths) and a coarse (0.45) scale (the
  bag's bulge and the crease below it) is flattened; pores are kept. A crease it lifts also takes the colour of the
  skin around it, as far as its luminance is taken toward that skin's (a shadow in skin is more saturated than the lit
  skin: lifted alone it would be an orange band, given the lit skin's colour while still dark, a grey one).

Where: Whites and Iris on their part masks (a face without them, closed eyes, is left alone). Dark Circles and Eye
Bags in crescents under the lower lids, placed from the 478 face mesh points (corners, lower lid, iris) or, without
the mesh, from YuNet's two eye points, and limited to skin: the Facial Skin mask and, through glasses (the parser puts
the lenses in Others and takes them out of Facial Skin), the Others mask. On Facial Skin, every colour counts as skin
however dark or off-colour it is (a dark circle is skin; issue #203: the old darkness and hue checks took the circle
itself for lashes and left it alone); through glasses only colours like the skin's (a frame is far darker or off-hue),
and with no plain skin under the eye (the lens covers it all), the skin seen through the lens gives the skin's colour.
Lashes, liner and a frame's edge are kept out by their shape and place, not their darkness: in a cell, a sample far
darker than the cell's other samples; across cells, a line up to two cells wide much darker than a closing of the
skin around it (and the cells beside it, which blur into it); on a pixel, near the lid (up to 0.3–0.5 half eye widths
below it) or through glasses, a line (dark along one of four directions, not a speck of grain) under 0.3–0.55 of the
skin's level around it. Pale, off-hue colours (a white or silver frame, a highlight) are left out too. The eye's masks
and the lid line from the mesh keep the eye out; beside the corners the crescent starts lower, below the lid's dark
rim. So nothing halos at their edges, and lashes keep their pixels. A face without Facial Skin, or without both eye points, gets no under-eye
change; nor does a face whose eyes the landmarks put outside its box. A lid point the mesh throws far away is held
within the eye's size.

**Every step of the slider shows**: Dark Circles closes the circle's lightness gap linearly in L*, which the eye
sees as even steps (with the old lift in stops, 25 / 50 / 75 closed 18 / 38 / 61 % of it). Eye Bags uses the square
root of its slider's value, and its lift of a crease is capped at 2.2× (1.6× clipped deep creases).

Sizes are fractions of the eye's width (from the landmarks, kept within 3–12 % of the face's size). The under-eye
gains are worked out on a grid in each eye's own frame (16 cells per half eye width, at every size), so a draft, the
loupe and an export work them out alike, and a close-up costs no more than a small face; the gains are then laid on
the pixels. Dark Circles only lifts: no channel gets darker.

**Zoom never changes the result** (all retouching, not only Eyes): each feature declares `Feature::REACH`, how far
past its face's box and its masks' areas it reads (Eyes 0.7 of the face's size, Skin 0.02, half the face for a
feature that declares none). A zoomed render whose window touches what a retouched face reads renders all of it and
cuts back to the window (`window_for_reads` in `crates/pipeline/src/retouch_step.rs`). Reference levels come from
the whole face on fixed grids, never from the window's pixels: the whites' level from a 96 × 32 grid over the Eye
Whites mask (where it is at least 0.8), the lit skin's level and colour from a 48 × 48 grid over the Facial Skin
mask, the iris's level, colour and pupil from a polar grid over each iris, the skin's and the cheek's colour from the
eye's grid. Every blur's size is a fraction of the eye's width or the iris's radius.

Each effect is a per-pixel gain times one weight per area (`eyes::Area`, made in `eyes::weights`), the place the
Refine brush (#99) will scale or extend an area.

Speed (the Eyes step alone, all four sliders at 100, best of 20 runs, a test Mac with other work running, faces on the
whole photo): a portrait at draft 1936 px 2.5–2.9 ms, loupe 2560 px 2.6–3.8 ms; a 10 MP close-up whose face fills
the frame 11.4 / 18.4 ms. The under-eye model of issue #203 costs about 2.3× that (measured on a busy Mac against the
code before it): on the CC0 test portraits, draft 3.9–6.6 ms, and 13.9 ms at draft for a close-up whose eyes span a
third of the frame. A slider drag runs no model: it samples the cached masks. No GPU version is needed for the
16 ms draft budget. In the app, faces from the people parse are mapped through the lens and transform warp per
sample, which these numbers leave out; a zoomed loupe on a retouched face now also renders the face's whole read area.
Whites and Iris of issue #202 (both at 100, the same Mac, best of 20): draft 1936 px 6.6–7.0 ms, loupe 2560 px
10.1–10.8 ms (1.6 ms at draft before).

The parsing crate's `face_input` example writes what the retouch step gets for each face (box, YuNet's five then the
478 mesh points, the model's part masks merged as the app merges them); `examples/eyes.rs --faces dir/faces.json`
tries the sliders on a picture with them.

## Branch

`feat/retouch-eyes`

## Own files

`crates/retouch/src/eyes.rs`, `crates/retouch/src/eyes/{geom,tone,under}.rs`, `crates/retouch/src/eyes/tone/{whites,iris,opening}.rs`, `crates/retouch/examples/eyes.rs`,
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
brighter and greyer with a vein fading but still there; a reddish white at 40 % grey lifted to at least 60 % at
Whites 100 with its saturation cut to under half, every lid and skin pixel kept (issue #202: 48 % before); the
iris brighter below than above and its pupil kept (the pupil got brighter before); dark circles lifted and turned toward the
skin, never darkening a channel; a deep purple circle at 0.4 of the cheek's luminance under one-pixel dark lashes lifted
to at least 0.8 of it with its colour's distance halved, every lash pixel kept (issue #203: the circle was left as it
was before); Dark Circles closing a quarter, half and three quarters of the circle's gap in lightness at 25, 50 and
75, all of it at 100, and at no value lifting a skin pixel past the skin around the circle (also under-eye skin as light as the cheek but redder); the bag's shading flattened;
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
and takes out their yellow or red while keeping the eye's shading, Iris brings out the iris's pattern and brightens it
where light enters while its pupil and dark rim stay, Dark Circles lifts the shadow under each eye
toward your cheek's colour, and Eye Bags softens the bag's shading. Each keeps the skin's texture and works through
clear glasses.

## Parity rows

| LR-EDIT-PORTRAIT-EYES | Portrait: Eyes (Whites, Iris, Dark Circles, Eye Bags) | — | 🟡 | `ctl:retouch.eyes.whites`, `ctl:retouch.eyes.iris`, `ctl:retouch.eyes.darkCircles`, `ctl:retouch.eyes.eyeBags`, `crates/retouch/src/eyes.rs`, `crates/ui-egui/src/panels/portrait/eyes.rs` | an extra (the reference app has no portrait retouching). Needs the people-parts set for masks; no Refine brush yet (#99). See `fork/retouch-eyes.md` |
