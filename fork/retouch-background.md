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
  Blur is on. Swap is how much of it shows. A picture file is copied into the library's `Backgrounds` folder as
  `<sha256>.<ext>` (its file name beside it in `<sha256>.name`), and the photo's settings keep only the hash (a library
  photo: its id), so no sidecar, DNG or catalog carries a path. The picture loads off the UI thread as soon as Swap is
  set or a photo with Swap opens, with a row "Loading background picture"; an export waits for it in its own worker.
  A picture over 64 MP (2^26 pixels, from its header) is refused before it is decoded. A picture that is not in the
  library fails the export with its name, and the section says "Background Picture Missing".
- **Clean Backdrop**: the background smoothed by a Gaussian of 2.5 % of the long side, so creases, scuffs and banding go;
  a pixel that differs from the smoothed backdrop by more than a crease does (20 to 45 % of its brightness, in any
  colour channel) is kept, so a light stand, a chair or a coloured object stays. A dither of about one 8-bit step
  (never more than twice what was smoothed away, so flat paper stays flat) keeps the export from banding again.

The person is drawn over the new background with their edge's own colours (blur-fusion foreground estimation, as a
mask's Decontaminate, `fork/decontaminate.md`), so no old background stays between strands. Pixels wholly inside the
person never change. Small holes in the person's mask that lie in or along their hair (the parse's Hair mask reaches
them; gaps between curls, at most 3 % of the mask across) and faint patches far from any background (an earring the
matte gave a few percent of background) count as the person: before, they showed the new background as rings inside
the hair. A closed pocket of background elsewhere (between an arm and the body) stays background and is swapped.

A portrait preset, and a preset with the Portrait group, carries Blur, Clean Backdrop and a Swap colour, never a Swap
picture or photo (a picture's Swap is left off in the preset). Copying settings to another photo of the library keeps
the picture.

How (`crates/retouch/src/background/`): the pipeline makes an analysis image, the whole frame before the step at 1600
pixels on its long side (or the render's size when smaller), once per source and per setting before the step (white
balance, spots, red eye, defringe, local tone map), kept for the newest two (`retouch_step::analysis`). The matte, its
edge's two colours, the blur's shares, the background blurred at four radii (1/8, 1/4, 1/2 and all of the radius, each
a disc on blocks of a third of it) and the cleaned background are made from it once and kept inside it (the newest two
sets, by the analysis image's key, the faces' input with their masks, depth map and Swap picture, and where the frame
lies in the photo; no render's pixels are hashed). A render reads them at its pixel's point of the analysis image; a
slider drag only mixes them per pixel. So a zoomed loupe window renders only its own pixels (round 1 grew it by about
10 % of the long side on every side with every slider on; now 0), draws what the whole render draws inside it (within 1
of 255, tested in the pipeline), and a pan reuses the kept work. Background lists the Entire Person and Hair masks (so
the face search keeps them) without growing a zoomed window round them (`Feature::READS_AROUND`).

The depth map is used only when its shape matches the photo's within 1 %; otherwise Blur uses the distance from the
person (logged once per file). Swap pictures are kept by content (the hash; a library photo's version), the newest four
within 384 MB; a failed load is tried again when its source changes (the file appears, the photo is edited) or in a
new session. Depth maps are kept by the file's bytes.

How it does (2026-11-09; masks from the people parse with BiRefNet Matting edges; six portraits, among them three CC0
Unsplash photos from Wikimedia Commons):
- Blur: sofa, wall and a corrugated wall blur smoothly up to the hair and shoulders with no light or dark rim.
- Swap: curly hair keeps its strands over a grey or a park photo, with the parse's Hair mask as in the app; the
  parse's own errors show (on one photo it counts part of the sofa as the person).
- Clean Backdrop: on studio paper the JPEG's banding rings and blotches go and a fine grain stays; on a room wall it
  smooths the wall and keeps the shelf and its coloured objects.
- Time (release, a 1920 × 1280 photo, other work running; the fastest of 20 renders with the work kept, the analysis
  image the photo itself): Blur 100 8.8 ms, Swap 100 1.9 ms, Clean Backdrop 100 3.5 ms, Swap with a picture and Blur 60
  9.0 ms, all three 2.0 ms (the background is all swapped, so nothing behind it is read). Making the kept work takes 140
  to 180 ms once per analysis image (after white balance or another change before the step). The people parse with
  BiRefNet Matting edges is 8 to 13 s per photo, once (`fork/hair-edges.md`).

## Branch

`feat/retouch-background`

## Own files

`crates/retouch/src/background/` (`mod.rs`, `draw.rs`, `falloff.rs`, `grid.rs`, `holes.rs`, `matte.rs`, `prepare.rs`),
`crates/retouch/examples/background.rs`, `crates/retouch/tests/background.rs`, `crates/engine/src/retouch_background.rs`,
`crates/engine/src/cmd/retouch_background.rs`, `crates/engine/src/tests/retouch_background.rs`, `crates/heif/src/depth.rs`,
`crates/ui-egui/src/panels/portrait/background.rs`, `crates/ui-egui/src/tests/portrait_background.rs`,
`fork/retouch-background.md`, `fork/tests.d/retouch-background.txt`

## Hooks in official files

- `crates/pipeline/src/lib.rs`: `Plan::retouch_analysis`, set at the end of `plan` by `retouch_step::analysis` (`None`
  in the colour noise tiles' plans).
- `crates/develop/src/presets.rs` (`Preset::from_settings`): `background::strip_pictures` on the Portrait group.
- `crates/engine/src/export/prepare.rs`: `RenderWork::backdrop` (`retouch_export_backdrop` when set up, `fill` in
  `run`, whose error fails the export). `crates/engine/src/media/session.rs`: `render_now` and `render_export` load the
  picture on their own thread (`backdrop_now`; a missing one fails `render_export`). `crates/engine/Cargo.toml`: `sha2`
  is a dependency (it was a dev-dependency).
- `crates/ui-egui/locales/*.json`: the section's strings and the two activity labels.
- `crates/heif/src/lib.rs`: `mod depth`, `pub use depth::{DepthMap, decode_depth}`. `crates/codecs/src/heif.rs`:
  `decode_depth` (none without the `heif` feature); `crates/codecs/src/lib.rs`: its `pub use`.
- `crates/engine/src/lib.rs`, `crates/engine/src/cmd.rs`, `crates/engine/src/tests.rs`, `crates/ui-egui/src/tests.rs`:
  `mod` lines and the command specs. `crates/ui-egui/src/menus/ui_commands.rs`: `portrait.backgroundPicture`.
- `crates/catalog`: `fork_format.rs` (`VERSION` 7), `journal.rs` (the fork 7 row), `tests_format_version.rs`,
  `tests_fork_format.rs`; `fork/catalog-format.md`.
- Fork files of the foundation: `crates/retouch/src/feature.rs` (`View::place`, `View::analysis`, `Scene::backdrop`,
  `Scene::depth` and `Scene::input_key`, `Feature::READS_AROUND`), `crates/retouch/src/settings.rs` (`Features::reads`
  skips a feature that reads nothing around a face), `crates/retouch/src/input.rs` (`Input::backdrop`, `Input::depth`,
  `with_backdrop`, `with_depth`, in the fingerprint only when there), `crates/pipeline/src/retouch_step.rs`
  (`analysis`, the render's place and analysis image in its views, a window test), `crates/engine/src/retouch_input.rs`
  (`FaceStore::backdrops`; renders get the picture and the depth map with the faces, and start loading it while the
  faces are searched), `crates/ui-egui/src/panels/portrait.rs` (`portrait.backgroundPicture` in `command`).
- Fork files of Portrait Presets: `crates/engine/src/portrait_presets.rs` (`Portrait::of` and `carry`: a Swap colour,
  never a picture).
- The stub's `retouch.background.amount` is now `retouch.background.blur`, `.swap` and `.clean`. The stub had no effect,
  so no saved edit changes; a saved `amount` is dropped when read.

## Tests

`fork/tests.d/retouch-background.txt`. They cover: every slider at 0 gives back the original bits; the person never
changes; a face without an Entire Person mask leaves the photo alone; no faces, 50 faces, faces past the edge, 1 × 1 and
empty photos; Blur stronger far from the person and following a depth map, and a depth map of another shape left out;
no halo next to a bright person (Blur and Clean); Swap's colour behind the person and at most 30 % of the old
background's blue left in the strands; a Swap picture covering the frame, waiting while it loads, blurred with Blur;
Clean taking 70 % of a crease away and keeping a light stand and a grain; a flat grey photo staying flat; a draft and an
export four times its size making the same change (within 15 %); a window drawing what the whole render draws from an
analysis image half its size, with the work made once for every window (in the crate, and through the pipeline within 1
of 255 with no margin); a small gap in the hair not swapped and a 30 × 30 mask-pixel pocket between arm and body
swapped; the work kept with its own analysis image; the command, a file copied into the library and kept by its hash
with no path in the settings, a library photo loaded off the UI thread and reaching the render, an export waiting for
the picture on its worker, a missing picture failing the export with its name, a picture over 2^26 pixels refused from
its header, pictures kept by version and capped by bytes; an iPhone depth image read from a HEIC (with the `heif`
feature) and reaching the render while Blur is on; presets keeping a Swap colour and never a picture; fork format 7; the
section's sliders writing the photo's setting whichever face is picked, Swap With taking a swatch, a file and a photo of
the view, and the section's line for a missing picture.

Pictures: `cargo run -p lightcraft-retouch --release --example background -- in.jpg out.png --mask person.png
[--hair hair.png] [--blur N] [--swap N] [--clean N] [--colour #RRGGBB | --picture file] [--depth depth.png] [--repeat N]`
(`--repeat` prints the fastest slider time); person masks from `cargo run --release -p lightcraft-engine --features sam
--example hair_edges`, hair masks from `cargo run -p lightcraft-parsing --release --example parts`.

The pull request's pictures come from three CC0 photos on Wikimedia Commons, each the file's 1920-pixel rendition
(its SHA-256 checked against a fresh download from Commons):
- Blur and the Swap pictures: [Confident Eye Contact (Unsplash)](https://commons.wikimedia.org/wiki/File:Confident_Eye_Contact_(Unsplash).jpg),
  CC0 1.0; SHA-256 `532c78caab495c58b4246c9280c11d9d1354fe047b652db31fe15ef8d8f3f27b`.
- The picture swapped in: [Autumn park lane (Unsplash)](https://commons.wikimedia.org/wiki/File:Autumn_park_lane_(Unsplash).jpg),
  CC0 1.0; SHA-256 `fbe5c2777932c631736469b3dfee74f6113db515078cdc966b55ea6bb43d32e2`.
- Clean Backdrop: [Always smile (Unsplash)](https://commons.wikimedia.org/wiki/File:Always_smile_(Unsplash).jpg), CC0
  1.0; SHA-256 `3756e3f8ae40bb1ac03af98c5688ef8630e69bc4de0f1a65665bfa57048ef0dd`.

## What's New entry

### Portrait Background: Blur, Swap and Clean Backdrop
- Blur softens the background more the farther it is behind the person, from an iPhone Portrait photo's depth map or from the distance to the person, with no halo round the hair.
- Swap puts a colour, a picture or another photo behind the person; strands of hair keep their own colour at the edge. A picture is copied into the library, so the edit keeps working when the original file moves.
- Clean Backdrop smooths creases, scuffs and banding on studio paper and keeps what stands in front of it.

## Parity rows

| LR-EDIT-PORTRAIT-BACKGROUND | Portrait Background: Blur, Swap, Clean Backdrop | — | 🟡 | `cmd:retouch.background`, `cmd:portrait.backgroundPicture`, `ctl:retouch.background.blur`, `ctl:retouch.background.swap`, `ctl:retouch.background.clean`, `crates/retouch/src/background/mod.rs`, `crates/engine/src/retouch_background.rs`, `crates/ui-egui/src/panels/portrait/background.rs` | an extra (the reference app has no portrait background tools): on the people parse's Entire Person masks with the chosen hair edge; Blur by an iPhone HEIC's depth map (needs the `heif` feature) or the distance to the person; Swap with a colour, a picture copied into the library (kept by its SHA-256) or a library photo; a zoomed window matches the whole render with no margin (a whole-frame analysis image); CPU, slider drags on kept work; not compared against Evoto |
