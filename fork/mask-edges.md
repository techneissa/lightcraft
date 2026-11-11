# Mask edges at 1:1: finger gaps and hair

List: Subject, Background and Person masks right at 1:1 by default: BiRefNet Matting is the default edge where installed, its matte opens the gaps it is sure of inside a person, and every edge is snapped to the photo at a finer size and stored as fine grids (issue #240). Branch `feat/240-mask-edges`.

## What it does

The models see a photo at about 1000 px (BiRefNet) or 2048 px (people parsing) and give a 1024² grid at most: on a 24 MP photo each cell is 6 px wide. So at 1:1 an edge was a soft blur, thin strands and the narrow end of a gap between fingers fell inside one cell, and a people parse's soft edge kept blobs of background (bokeh between the fingers, foliage behind curly hair, the wall in a gap between hand and hood) and halos round the hair. Three steps, all around the models already in the shared folder (no new model):

- **The edge model is on by default.** Settings ▸ Faces ▸ Hair Edges now defaults to BiRefNet Matting where its model is installed, for both kinds (`HairEdges::default_edge`); a choice the user saved, Current included, stands. Without the model, Current as before.
- **The matte corrects, it does not replace** (`lightcraft_edgerefine::decide`). For a Subject BiRefNet already cut out, within the band round its edge: where both agree, the sharper of the two stands (the matte's softer edge never blurs a crisp one); where they disagree, the matte adds or takes away only as far as it is sure (a strand comes in and a blob of bokeh goes; a glow round a finger and a defocused hand it half doubts do not). For SAM 3's coarse grid the matte draws the edge, as before. For people, the matte also opens the **gaps** it is sure of inside a person, however far from the parse's edge (`gaps`: sure background, at most 5 % of the person, compact enough: a long thin strip such as a zipper's dark line is no gap); thin seams inside the parse are closed first, so they are never taken for edges.
- **Snapped to the photo** (`lightcraft_edgerefine::snap`). The selection is drawn at a finer size (Subject and Background: the photo at 3072 px; people: the parse's own 2048 px picture) and, round its edge, a colour guided filter (He, Sun and Tang) lands it on the photo's own colour edges, only where the coverage is uncertain (between about 15 % and 85 %: a crisp edge stays crisp on a busy background). Where the background next to the edge is one plain colour (a wall, a sky), each edge pixel is also placed between the two sides' colours, which removes the halo a matte leaves there. People are snapped as one body: their joined coverage is snapped and each person keeps their share, so a body a parse splits into pieces (hands found as people of their own) never cracks along the split.
- **Stored fine** (`lightcraft_edgerefine::store`). Subject and Background keep one 1024² grid over the photo plus up to four finer grids (each ≤ 1024²) tiling the edge, so on a 24 MP photo the edge is held in cells of about 2 px instead of 6; people keep each person's Entire Person plane at the parse picture's size (≤ 1536 cells), and Select People cuts a person whose planes are finer than one grid into up to four finer grids. Such grids are marked `+refined` (`SegMask::model`) and sampled as stored: a render does not filter them again and reads their outermost cells too, so a preview, a zoomed view and an export draw the same edge. A refined Subject that lost its finer grids (an older build) is filtered as before.

**When it runs**: once per selection, on the worker that made it (the Subject worker after BiRefNet and its matte; people parsing after the parse and its matte), with a ring step "Refining mask edges" in the top bar (`start_background`); cached with what it refines (people: the parse file, version `+refined1`; Subject: the stored mask). Dragging a mask's sliders never runs it. The fine grids travel beside the selection from the worker to `mask.add` (`Segmenter::attach_refined`), so the commands' parameters and `segment_concept`'s signature are unchanged.

## How it does

Audit (2026-11-11): 15 CC0 photos from Wikimedia Commons (Unsplash uploads: fingers spread, raised and outstretched arms, hands on a face, curly and wind-blown hair, bokeh, foliage, a street, a gate, a plain wall, sky), 32 spots at 1:1 round fingers, arm gaps and hair, Subject, Background and Person each, today's build against this one (`crates/engine/examples/mask_edges_audit.rs`; photos in the gitignored `corpus/masks/`, sources in its `SOURCES.md`).

Misses before, by cause (Subject and Background share a grid, counted once):

| Cause | Subject / Background | Person | Fixed |
| --- | --- | --- | --- |
| The model's coarse answer was wrong (background kept: bokeh, foliage, a wall, gate bars; a body split in pieces) | 1 | 8 | 8 of 9 (the Subject's bokeh blob is smaller, not gone) |
| Our processing: input size and upsampling (strands and the narrow end of a finger gap below one cell; a soft parse edge enlarged) | 2 | 4 | 3 of 6 (the finger gap's narrow end and the bright-on-bright hair of photo 1 remain) |
| Our processing: no snapping to the photo's edges (halos and rims following a soft edge; a luminance-only filter landing on a gate bar's edge) | 2 | 6 | 8 of 8 |
| Our processing: edge refinement off by default (strands and fur cut; the matte's gaps never asked for) | 3 | 1 | 3 of 4 (the strands over foliage of photo 3 stay half-transparent) |
| Our processing: a fixed 0.5 threshold | 0 | 0 | — |
| Our processing: one pass, no mirrored or multi-scale look | not measured | not measured | not tried |

What is left: the narrow end of a gap between defocused fingers (photo 4, below the matte's cells), backlit blonde hair against a bright sky (photo 1: the colours are too close for the snap, so it stays soft), strands over busy foliage (photo 3: kept, half-transparent), a smaller bokeh blob at a fingertip (photo 2), and a faint 1 px line where two of a parse's people meet (photo 10).

Time (a 24 MP photo, Subject, warm, the Mac under load): 8.7 s before, 19.0 s now (BiRefNet, then BiRefNet Matting, then the refinement). People: the matte adds about 6 to 7 s per parse, the refinement about 1 to 3 s.

Tried and dropped: BiRefNet (general) run on zoomed tiles (without the whole photo it picks the wrong object in a tile); the matting model at 2048 px input (25 s a photo, not better than 1024 plus the snap); a colour projection against busy backgrounds (speckles on bokeh: the step now only acts on plain backgrounds).

## Branch

`feat/240-mask-edges`

## Own files

`crates/edgerefine/` (`src/{lib,decide,gaps,guided,seams,store,tests}.rs`), `crates/engine/src/hair_edges/refine.rs`, `crates/engine/examples/mask_edges_audit.rs`, `fork/mask-edges.md`, `fork/tests.d/mask-edges.txt`

## Hooks in official files

- `Cargo.toml`: workspace dependency `lightcraft-edgerefine`. `xtask/src/layers.rs`: `edgerefine` is layer 1. `crates/engine/Cargo.toml`: the dependency and the `mask_edges_audit` example.
- `crates/pipeline/src/masks.rs`: `REFINED`, `edge_drawn` (a `+refined` grid with its finer grids is not filtered again), `SegGrid::exact` (its outermost cells are read). `crates/pipeline/src/masks/people.rs`: `filter` asks `edge_drawn`.
- `crates/engine/src/segment.rs`: `Segmenter::refined`, `keep_refined`, `attach_refined`; `segment_job` builds the finer render for a BiRefNet job; `apply_outcome` keeps a result's finer grids. `crates/engine/src/segment/worker.rs`: `Job::refine`, `Job::detail`, `Outcome::detail`, `Kind::BiRefNet::activity`. `crates/engine/src/cmd/masks.rs`: `mask.add`, `mask.addComponent`, `mask.update` call `attach_refined`.
- Fork files of other features: `crates/engine/src/segment/worker/birefnet.rs` (`run` = `decide` then `refined`), `crates/engine/src/segment/birefnet.rs` (the activity in the BiRefNet job), `crates/engine/src/hair_edges.rs` (`pub mod refine`, the default edge, `subject_edge` through `decide`), `crates/engine/src/hair_edges/people.rs` (gaps, seams, split planes), `crates/engine/src/people_parsing.rs` (the refinement after a parse; `version_of` adds `+refined1`), `crates/engine/src/select_people/grid.rs` (finer grids in tiles, a Background's from its people, the `+refined` mark). `crates/engine/src/adaptive.rs` and `crates/engine/src/segment/adaptive_jobs.rs` (#239's re-detected masks keep their finer grids: `answered_with`, `drain` returns them, `Segmenter::take_refined`).

## Tests

`cargo test -p lightcraft-edgerefine` (the synthetic hand: an enclosed finger gap and a lock of hair, failing on the coarse path and on band-only matting) · `cargo test -p lightcraft-engine --features sam --lib -- hair_edges segment select_people birefnet people_parsing parse_ahead masks` · `cargo test -p lightcraft-pipeline --lib -- masks tests_window` (also in `fork/tests.d/mask-edges.txt`). Pictures: `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features sam --example mask_edges_audit -- OUT_DIR PHOTO...`.

## What's New entry

### Subject, Background and Person Masks Right at 1:1
- Masks follow fingers, arm gaps and hair at full size: the background between fingers and in gaps is left out, strands stay in, and edges sit on the photo's own edges without a halo.
- BiRefNet Matting is now the default hair edge when it is installed; Settings ▸ Faces ▸ Hair Edges still offers Current.
- New masks take a few more seconds to make, once per photo; sliders stay as fast.

## Parity rows

| LR-MASK-SUBJECT | Select subject | P2 | 🟡 | `cmd:mask.add` (`subject`), `crates/edgerefine` | fork: the edge is refined on the photo at 3072 px and stored as finer grids (about 2 px cells on 24 MP); BiRefNet Matting corrects BiRefNet's cut-out by default where installed; not compared against Lightroom |
