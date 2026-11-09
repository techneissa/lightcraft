# Apple Vision masks as a hair edge

List: Apple Vision's own person and subject masks as a third hair edge in Settings ▸ Faces ▸ Hair Edges, next to Current and BiRefNet Matting: no download, about a tenth of a second per photo on the Neural Engine (issue #115). Branch `feat/115-apple-vision-masks`.

## What it does

- **Apple Vision** is a third choice in each Hair Edges row (`maskEdges.edge {kind, edge: "apple-vision"}`), shown on a
  Mac that runs it (macOS 14 or later, `maskEdges.status` → `visionAvailable`). Current stays the default. A chosen
  Apple Vision counts as Current on a Mac without it.
- **Entire Person / Background**: Vision's person segmentation (Accurate quality, fine hair) draws the edge, within
  the same 3 % band around each parsed person's edge as BiRefNet Matting. Vision's per-person masks say whose each
  pixel is: each Vision person is matched to the parsed person they overlap most, and where two people meet, each
  keeps their own, also beside someone only Vision sees (every other Vision person's share is taken off). A parsed
  person no Vision person matches (often a hand or a patch the parse split off one person, or someone Vision missed)
  keeps the parse's edge, so no ring of background opens inside the person and no edge is cut away. The matte is at
  most 1536 px a side, the widest plane the parse cache reads back.
- **Subject**: Vision's subject masks (every subject together) redraw the Subject part's edge, the same band rule as
  BiRefNet Matting. Their edge is hard and drops strands, so the person segmentation adds the hair of the people who
  are part of the subject: the Vision people with at least half their mask inside it, each pixel their share of the
  segmentation where it is larger. A bystander beside the subject adds nothing. The part's model ends `+vision` (hover: "…, Apple Vision Edges"); such a grid is sampled without the guided
  filter, like `+matting`, so preview = export. When Vision finds no subject, the part keeps its own edge.
- **Runs** once per photo on the worker thread that asked (people parsing, the Subject worker) with the "Refining
  hair edges" row (detail "Apple Vision"). A people parse with Vision edges is cached as its own version
  (`+vision1`); the newest Subject mattes are kept by a hash of the pixels. No model runs during a slider drag. Vision
  runs on the Neural Engine, not on a Metal device of ours, so it does not take the GPU run lock (`gpu_run.rs`).
- **Never crashes**: an older macOS, a damaged picture, a Vision error or a panic is a message ("hair edges:
  Apple Vision: …").
- **`crates/coreml`** gains `vision` (safe API) and `vision_macos.rs` (the `unsafe`): its README lists every call.
  It runs on macOS 14 (the output-format list, macOS 15, is used only where it is there); subject masks are folded
  into one matte as they are read, and the person masks one call keeps are capped at 64 M pixels.

## How it does

Four CC0 portraits (blue sky ×2, green field, busy garden), M4 Pro, 2048 px on the long side; pictures in the PR
and in `$WORKSPACE/assets/lightcraft/shots/portrait/hair/apple-vision-*`.

- Entire Person / Background: the tightest edge of the three. The parse's blur of background beside the hair goes,
  as with BiRefNet Matting, and Vision keeps no soft halo. Loose dark strands against dark foliage are lost a little
  more than with BiRefNet Matting (hair-sky-3, left).
- Subject: a hard, clean edge; strands come from the person segmentation.
- Background colour between strands stays in every edge model: that is colour decontamination (PR #119), not the
  matte.
- Time (Vision alone, `vision_speed` example, after a warm-up, load 16–18): person segmentation 56–93 ms, per-person
  masks 27–194 ms, subject masks 23–44 ms. In the app (load 6–15): a Subject part 6.9–8.3 s with Apple Vision against
  7.1–8.0 s Current and 13.9–16.1 s BiRefNet Matting; a people parse 1.9–6.6 s against 1.6–5.4 s Current and
  8.7–12.8 s BiRefNet Matting (the parse and the Subject model dominate; Vision adds about 0.1–0.3 s).

Photos (Wikimedia Commons, CC0; 1800–1920 px thumbnails):

| Photo | Commons file | Background |
|---|---|---|
| hair-sky-2 | [Let Your Hair Out (Unsplash).jpg](https://commons.wikimedia.org/wiki/File:Let_Your_Hair_Out_(Unsplash).jpg) | blue sky, buildings |
| hair-sky-3 | [Windblown (Unsplash).jpg](https://commons.wikimedia.org/wiki/File:Windblown_(Unsplash).jpg) | pale sky, foliage |
| hair-green | [Long-haired woman laughing (Unsplash).jpg](https://commons.wikimedia.org/wiki/File:Long-haired_woman_laughing_(Unsplash).jpg) | green field |
| hair-busy | [Woman in a flower garden (Unsplash).jpg](https://commons.wikimedia.org/wiki/File:Woman_in_a_flower_garden_(Unsplash).jpg) | busy foliage and flowers |

## Branch

`feat/115-apple-vision-masks`

## Own files

`crates/coreml/src/vision.rs`, `crates/coreml/src/vision_macos.rs`, `crates/coreml/src/tests_vision.rs`,
`crates/coreml/examples/vision_speed.rs`, `crates/engine/src/hair_edges/vision.rs`,
`crates/engine/src/tests/vision_edges.rs`, `crates/pipeline/src/masks/tests_vision_edges.rs`,
`crates/ui-egui/src/tests/vision_edges.rs`, `fork/apple-vision-masks.md`, `fork/tests.d/apple-vision-masks.txt`

## Hooks in official files

- `AGENTS.md`: the `unsafe` bullet names Apple Vision for `crates/coreml`.
- `crates/pipeline/src/masks.rs` (`sample_seg`): a `+vision` grid is not guided-filtered, like `+matting`; the test
  module line.
- `crates/engine/src/tests.rs`, `crates/ui-egui/src/tests.rs`: `mod` lines. `Cargo.lock`: `objc2-vision`,
  `objc2-core-video` 0.3.2 (Zlib OR Apache-2.0 OR MIT).
- Fork files of other features: `crates/parsing/src/cache.rs` (`MAX_PLANE_SIDE` public, so the Vision matte keeps to
  it); `crates/coreml/{Cargo.toml,README.md,src/lib.rs,src/stub.rs}` (the `vision` module,
  `Error::Vision`, the Vision and Core Video bindings); `crates/engine/src/hair_edges.rs` (`Edge::Vision`, `mod
  vision`, its arms in `suffix` and `available` (so `edge` and `set_edge` check Vision runs here), `matte_for` and
  `make` refuse it: Vision's mattes are made in `vision`); `crates/engine/src/hair_edges/people.rs` (`entire_rect`,
  `Refined`, `refine_person` visible to `vision`); `crates/engine/src/people_parsing.rs` (the edge chosen goes with
  the parse: its cache version and the Vision step); `crates/engine/src/segment/birefnet.rs` (`subject_matting`
  takes any edge but Current, `label_of` reads `+vision` through `Edge::suffix`); `crates/engine/src/segment/worker/birefnet.rs` (the
  Subject matte by edge); `crates/engine/src/cmd/hair_edges.rs` (`apple-vision` in messages and help,
  `visionAvailable`); `crates/ui-egui/src/dialogs/hair_edges.rs` (the Apple Vision button);
  `crates/engine/examples/hair_edges.rs` (`HAIR_EDGES` picks the edges compared).

## Tests

See `fork/tests.d/apple-vision-masks.txt`. The Core ML crate's tests call Vision on small synthetic pictures (8-bit
and float; a disc on a plain backdrop is found as a subject) and refuse damaged sizes before any call; they skip off
the Mac or on an older macOS; the subject masks fold into their largest coverage, the kept person masks are capped,
and the macOS 15 format list is not required. The engine tests match Vision people to parsed people, keep each
person's own edge where two meet and beside a neighbour only Vision sees, keep a split-off parsed person whole (a ring
opened before the fix) and an unmatched person's edge where the segmentation is empty, read a full-frame parse with
Vision edges back from the cache, add only the subject's people's hair to a Subject matte, make a people parse with
Vision edges once and cache it, keep the choice, and mark and name a Subject part `+vision`; the pipeline test draws a
`+vision` grid's outline the same in a preview and an export; the UI test chooses Apple Vision with one click.
Pictures: `HAIR_EDGES=current,birefnet-matting,apple-vision LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p
lightcraft-engine --features sam --example hair_edges -- OUT_DIR PHOTO...`; Vision alone:
`cargo run --release -p lightcraft-coreml --example vision_speed -- OUT_DIR W H PHOTO.rgb`.

## What's New entry

### Apple Vision Hair Edges
- Settings ▸ Faces ▸ Hair Edges has a third choice, Apple Vision: the Mac's own person and subject masks draw the
  edge of Entire Person / Background and Subject masks. No download, about a tenth of a second per photo.

## Parity rows

| LR-MASK-HAIR-EDGES-VISION | Apple Vision masks as a hair edge for people and subject masks | P2 | 🟡 | `cmd:maskEdges.edge`, `crates/engine/src/hair_edges/vision.rs`, `crates/coreml/src/vision.rs` | fork: Apple Vision's person segmentation, per-person masks and subject masks redraw the edge of Entire Person / Background and Subject masks within a band (macOS 14+, default Current); once per photo, cached, preview = export; not compared against Lightroom |
