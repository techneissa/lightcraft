# Portrait retouching: Face Reshape

List: the Portrait tab's Face Reshape section: Slim Face, Jaw, Chin, Double Chin, Nose, Forehead and Eye Size reshape each face along its face mesh, a warp before the lens corrections that masks and the other Portrait features follow (issue #142). Branch `feat/retouch-reshape`.

## What it does

The Face Reshape feature of portrait retouching (`fork/retouch-foundation.md`), on the face mesh of the people parse
(`fork/retouch-masks.md`). Seven sliders, −100…+100, none at 0, for every face or the picked one:

- **Slim Face**: + draws the cheeks and the jaw's sides in, − widens them.
- **Jaw**: + widens the jaw at its corners, − narrows it.
- **Chin**: + lengthens the chin, − shortens it.
- **Double Chin**: + lifts the soft area under the chin toward the jaw line, − lets it down.
- **Nose**: + widens the nose's wings, − narrows them.
- **Forehead**: + raises the top of the forehead, − lowers it.
- **Eye Size**: + enlarges the eyes, − makes them smaller.

How (`crates/retouch/src/reshape/field.rs`): each slider moves a few face-mesh points (the cheek contour, the jaw's
corners, the chin, a band under it, the nose's wings, the top of the forehead) by a fraction of the face's width or
height, or scales each eye about its centre. Each move spreads through a Wendland bump (smooth, zero past a radius
that is a fraction of the face), so pixels away from the face never move and a straight line there stays straight.
The moves are summed on a grid of cells a fortieth of the face and read back bilinearly. Each cell is checked to stay
a convex quadrilateral the same way round, with at least a quarter of its area at every corner; when one would not,
that face's moves are scaled down until all hold. So the warp never folds. A move across the photo's edge fades to
nothing at it (over 15 % of the face), so the border maps onto itself: a face cut by the edge never reads from past
it, and the warp can always be inverted there. A face without the mesh (YuNet alone) is left alone.

Where (`crates/pipeline/src/reshape_step.rs`): the warp is part of the frame's `Warp`, on the photo before the lens
corrections: the photo is reshaped first, then corrected and transformed. Everything that reads the photo through
the warp follows the reshaped face: the render's one resample, local masks drawn on the photo (Select People's among
them), the Portrait features' faces, landmarks and part masks, and the raw's local tone map. Spots, and old masks
kept after the warp, stay where they are drawn, as under Upright. Per-face settings are matched through the lens and
transform only. Upright and Auto straighten analyse the photo without the reshape (their analysis is cached by the
optics alone). The GPU's warp kernel reads the reshape's grid (`Field::gpu_table`, `sample_warp` in `geom.wgsl`);
on the CPU, with no lens correction or transform, the resample is the plain one, redone over the faces' areas only.
Face Reshape alone leaves the per-pixel retouch step off, so the GPU keeps the scene-linear stage.

Speed: GPU draft render (1936 px) of a private 2400 × 1600 test portrait whose face fills a third of the frame, on a
test Mac under load 10–15, fastest of 15 per case. No Portrait setting: 9.0–9.8 ms. Face Reshape alone (Slim Face
and Eye Size on): 9.0–9.5 ms, so the reshape adds nothing measurable. Another Portrait setting on (it moves the
scene-linear stage to the CPU, a separate cost): 16.0 ms, and 15.3–23 ms with the reshape on top (noise). Building
the warp: 0.07 ms. No model runs while a slider moves; the mesh comes from the cached parse.

Not done: the loupe's mask outlines and the face picker are drawn without the reshape, so an outline near a reshaped
face sits a few pixels off what it covers; spots do not follow a reshape made after them.

## Branch

`feat/retouch-reshape`

## Own files

`crates/retouch/src/reshape.rs`, `crates/retouch/src/reshape/field.rs`, `crates/retouch/examples/reshape.rs`,
`crates/retouch/tests/reshape.rs`, `crates/pipeline/src/reshape_step.rs`, `crates/pipeline/src/reshape_step/tests.rs`,
`crates/gpu/tests/reshape.rs`, `crates/gpu/src/render/tests_reshape.rs`, `crates/ui-egui/src/panels/portrait/reshape.rs`, `fork/retouch-reshape.md`,
`fork/tests.d/retouch-reshape.txt`

## Hooks in official files

- `crates/pipeline/src/optics.rs`: `Warp::reshape`, `Warp::reshaped` (in `corrected_to_source` and `frame`),
  `moves_pixels` counts it, `reshape_only`.
- `crates/pipeline/src/geometry.rs`: `Frame::set_reshape`; `sample_base` redoes only the faces' areas when the
  reshape is the whole warp.
- `crates/pipeline/src/lib.rs`: `mod reshape_step`; `frame_for` adds the reshape.
- `crates/gpu/src/render.rs`: `sample_warp` gets the reshape's grid; the geometry kernels' fourth binding; `mod
  tests_reshape` (`crates/gpu/src/render/tests_reshape.rs`). `crates/gpu/src/ctx.rs`: the `field` binding of
  `geom.wgsl`. `crates/gpu/src/wgsl/geom.wgsl`: `reshaped` in `sample_warp`.
- `crates/pipeline/src/upright.rs`: `analysis_plane` leaves the reshape out.
- `crates/catalog/src/fork_format.rs`, `journal.rs`, `tests_fork_format.rs`, `tests_format_version.rs`: fork catalog
  format 6 (the `reshape` sliders are new saved fields).
- `docs/parity-checklist.md`: the LR-EDIT-PORTRAIT row, in place.

In fork files: `crates/pipeline/src/retouch_step.rs` (`FrameKeyMap` leaves the reshape out; `active` is off when
Face Reshape is the only setting; module docs),
`crates/retouch/src/feature.rs` (`Stage::Geometry` docs), `crates/retouch/README.md` (render order),
`crates/ui-egui/src/tests/portrait.rs` (a test of the section), `crates/parsing/examples/parts.rs` (`people.json`
holds each face's mesh, which the reshape example reads with `--people`).

The stub's control `retouch.reshape.amount` is gone. The stub had no effect, so no saved edit changes; a stored
`amount` is dropped when settings are read.

## Tests

`fork/tests.d/retouch-reshape.txt`. They cover: every slider at 0 (or not a number), the Portrait tab's eye off and no
face give no warp, and the render is byte for byte the render without it; each slider moving its part of the face the
way its name says, the eyes' and mouth's middles staying under the contour sliders; no fold with every slider at ±100
alone and all together (each grid cell, and a fine Jacobian check across the face); moves that would fold scaled down
until they hold; every point away from the face unmoved, bit for bit, in the warp and in the render (a straight line
far from the face stays straight); the warp the same from 600 × 400 to 6000 × 4000; a face with its own settings
moving alone; 50 faces, a face cut by the edge, a collapsed or not-a-number mesh, five landmarks only, a 1 × 1 photo;
a local mask drawn on the photo staying on the content it covers under Slim Face (it misses five times more without
the warp); a landmark moving with Chin; a draft of a small copy and a render four times its size reading from the same
places; a loupe window matching the whole render; fine texture keeping 96 % of its energy inside a face with every
slider at ±100; the warp inverting (`Warp::from_source`) at every point of a face cut by each edge, every slider at
±100, and every landmark finding its place (failed before the edge fade); Upright's analysis image unchanged by a
reshape (failed before); Face Reshape alone leaving the retouch step off; the device's resample matching the CPU's,
alone and with a lens distortion and CA (fails without the kernel change); the whole GPU render matching the CPU's;
fork catalog format 6 saving and reading Face Reshape for every face and per face; the section's seven sliders and a
click setting Slim Face.

Try it on a photo:

```bash
cargo run -p lightcraft-parsing --release --example parts -- in.jpg outdir/
cargo run -p lightcraft-retouch --release --example reshape -- in.jpg out.jpg --people outdir/people.json \
  --set slimFace=60 --set eyeSize=40
```
