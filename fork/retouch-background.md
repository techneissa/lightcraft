# Portrait retouching: Background

List: the Portrait tab's Background section: Blur grows with depth (an iPhone Portrait photo's depth map, else the distance from the person) with no halo at the hair, Swap puts a colour, a picture file or a library photo behind the person with the hair's own colours at its edge, Clean Backdrop smooths creases, scuffs and banding on studio paper; one setting for the photo (issue #141). Branch `feat/retouch-background`.

## What it does

The Background feature of portrait retouching (`fork/retouch-foundation.md`), on the people parse's Entire Person
masks (with the hair edge Settings ▸ Faces ▸ Hair Edges chooses, `fork/hair-edges.md`). Three sliders, 0…100, off at 0,
one setting for the photo (a face picked in the tab does not get its own):

- **Blur**: a lens-like blur of everything that is not the person. The radius grows with how far behind the person each
  pixel is: from the photo's depth map when the file has one (an iPhone Portrait HEIC; the person's depth is the median
  under the Entire Person mask, the far depth the map's 2nd percentile), else from the distance to the person (35 % of
  the radius right next to them, all of it over 40 % of the person's size). At 100 the farthest background gets a disc
  of 3 % of the photo's long side. Each blur is of the background alone (weighted by how much of each pixel is
  background), so the person's colour never spreads out: no halo round the hair.
- **Swap**: what is behind the person becomes a colour (four swatches, or any colour), a picture file, or a photo of
  the view as it is developed (Use a Photo… shows them), scaled to cover the frame and blurred with the background when
  Blur is on. Swap is how much of it shows. The picture loads once, off the UI thread, with a row "Loading Background
  Picture"; an export loads it first.
- **Clean Backdrop**: the background smoothed by a Gaussian of 2.5 % of the long side, so creases, scuffs and banding go;
  a pixel that differs from the smoothed backdrop by more than a crease does (20 to 45 % of its brightness, in any
  colour channel) is kept, so a light stand, a chair or a coloured object stays. A dither of about one 8-bit step
  (never more than twice what was smoothed away, so flat paper stays flat) keeps the export from banding again.

The person is drawn over the new background with their edge's own colours (blur-fusion foreground estimation, as a
mask's Decontaminate, `fork/decontaminate.md`), so no old background stays between strands. Pixels wholly inside the
person never change. Small holes in the person's mask (gaps between curls, at most 3 % of the mask across) and faint
patches far from any background (an earring the matte gave a few percent of background) count as the person: before,
they showed the new background as rings inside the hair.

How (`crates/retouch/src/background/`): the matte, its edge's colours, the blur's shares, the background blurred at four
radii (1/8, 1/4, 1/2 and all of the radius, each a disc on blocks of a third of it) and the cleaned background are made
once per picture and kept for the newest two (a hash of the pixels, the masks, the view and the depth map); a slider
drag only mixes them per pixel. A zoomed loupe window is grown by the reach (`background::reach`, about 10 % of the long
side with every slider on): the blocks are anchored to the whole render, the distances and depth statistics come from
the whole photo, so a window draws what the whole render draws inside it (within 1 of 255, tested in the pipeline).

How it does (2026-11-09; masks from the people parse with BiRefNet Matting edges; three of techneissa's A7 V photos and
three CC0 Unsplash photos from Wikimedia Commons):
- Blur: sofa, wall and a corrugated wall blur smoothly up to the hair and shoulders with no light or dark rim.
- Swap: curly hair keeps its strands over a grey or a park photo; the parse's own errors show (on one A7 V photo it
  counts part of the sofa as the person).
- Clean Backdrop: on studio paper the JPEG's banding rings and blotches go and a fine grain stays; on a room wall it
  smooths the wall and keeps the shelf and its coloured objects.
- Time (Apple M4 Pro, release, a 2048 × 1365 photo, the Mac at load about 10; one run each, the fastest of 20 renders
  with the work kept): Blur 100 6.5 ms, Swap 100 1.7 ms, Clean Backdrop 100 3.7 ms, Swap with a picture and Blur 7.4 ms,
  all three 9.2 ms. Making the work kept takes 70 to 95 ms once per picture (after white balance or another change
  before the step). The people parse with BiRefNet Matting edges is 8 to 13 s per photo, once (`fork/hair-edges.md`).

## Branch

`feat/retouch-background`

## Own files

`crates/retouch/src/background/` (`mod.rs`, `draw.rs`, `falloff.rs`, `grid.rs`, `holes.rs`, `matte.rs`, `prepare.rs`),
`crates/retouch/examples/background.rs`, `crates/retouch/tests/background.rs`, `crates/engine/src/retouch_background.rs`,
`crates/engine/src/cmd/retouch_background.rs`, `crates/engine/src/tests/retouch_background.rs`, `crates/heif/src/depth.rs`,
`crates/ui-egui/src/panels/portrait/background.rs`, `crates/ui-egui/src/tests/portrait_background.rs`,
`fork/retouch-background.md`, `fork/tests.d/retouch-background.txt`

## Hooks in official files

- `crates/pipeline/src/lib.rs` (`plan`): a windowed render grows by `retouch_step::reach` while Background is on.
- `crates/heif/src/lib.rs`: `mod depth`, `pub use depth::{DepthMap, decode_depth}`. `crates/codecs/src/heif.rs`:
  `decode_depth` (none without the `heif` feature); `crates/codecs/src/lib.rs`: its `pub use`.
- `crates/engine/src/lib.rs`, `crates/engine/src/cmd.rs`, `crates/engine/src/tests.rs`, `crates/ui-egui/src/tests.rs`:
  `mod` lines and the command specs. `crates/ui-egui/src/menus/ui_commands.rs`: `portrait.backgroundPicture`.
- `crates/catalog`: `fork_format.rs` (`VERSION` 7), `journal.rs` (the fork 7 row), `tests_format_version.rs`,
  `tests_fork_format.rs`; `fork/catalog-format.md`.
- Fork files of the foundation: `crates/retouch/src/feature.rs` (`View::place`, `Scene::backdrop` and `Scene::depth`),
  `crates/retouch/src/input.rs` (`Input::backdrop`, `Input::depth`, `with_backdrop`, `with_depth`, in the fingerprint
  only when there), `crates/pipeline/src/retouch_step.rs` (`reach`, the render's place, a window test),
  `crates/engine/src/retouch_input.rs` (`FaceStore::backdrops`; renders and exports get the picture and the depth map
  with the faces), `crates/ui-egui/src/panels/portrait.rs` (`portrait.backgroundPicture` in `command`).
- The stub's `retouch.background.amount` is now `retouch.background.blur`, `.swap` and `.clean`. The stub had no effect,
  so no saved edit changes; a saved `amount` is dropped when read.

## Tests

`fork/tests.d/retouch-background.txt`. They cover: every slider at 0 gives back the original bits; the person never
changes; a face without an Entire Person mask leaves the photo alone; no faces, 50 faces, faces past the edge, 1 × 1 and
empty photos; Blur stronger far from the person and following a depth map; no halo next to a bright person (Blur and
Clean); Swap's colour behind the person and at most 30 % of the old background's blue left in the strands; a Swap
picture covering the frame, waiting while it loads, blurred with Blur; Clean taking 70 % of a crease away and keeping a
light stand and a grain; a flat grey photo staying flat; a draft and an export four times its size making the same
change (within 15 %); a window drawing what the whole render draws (in the crate, and through the pipeline within 1 of
255); small gaps inside the person not swapped; the work kept for the next slider move; the command, a library photo
loaded off the UI thread and reaching the render, an export loading it first; an iPhone depth image read from a HEIC
(with the `heif` feature) and reaching the render while Blur is on; fork format 7; the section's sliders writing the
photo's setting whichever face is picked, and Swap With taking a swatch, a file and a photo of the view.

Pictures: `cargo run -p lightcraft-retouch --release --example background -- in.jpg out.png --mask person.png [--blur N]
[--swap N] [--clean N] [--colour #RRGGBB | --picture file] [--depth depth.png] [--repeat N]` (`--repeat` prints the
fastest slider time); person masks from `cargo run --release -p lightcraft-engine --features sam --example hair_edges`.

## What's New entry

### Portrait Background: Blur, Swap and Clean Backdrop
- Blur softens the background more the farther it is behind the person, from an iPhone Portrait photo's depth map or from the distance to the person, with no halo round the hair.
- Swap puts a colour, a picture or another photo behind the person; strands of hair keep their own colour at the edge.
- Clean Backdrop smooths creases, scuffs and banding on studio paper and keeps what stands in front of it.

## Parity rows

| LR-EDIT-PORTRAIT-BACKGROUND | Portrait Background: Blur, Swap, Clean Backdrop | — | 🟡 | `cmd:retouch.background`, `cmd:portrait.backgroundPicture`, `ctl:retouch.background.blur`, `ctl:retouch.background.swap`, `ctl:retouch.background.clean`, `crates/retouch/src/background/mod.rs`, `crates/engine/src/retouch_background.rs` | an extra (the reference app has no portrait background tools): on the people parse's Entire Person masks with the chosen hair edge; depth from an iPhone HEIC's depth map (needs the `heif` feature) or the distance to the person; CPU, slider drags on kept work; not compared against Evoto |
