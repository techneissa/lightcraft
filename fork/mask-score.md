# Mask scorer

List: `cargo xtask mask-score` scores the app's AI masks (Subject, hair edge, Click, Sky) against human-drawn masks from public test sets, and compares two runs photo by photo (issue #256, step 1 of the training plan #422). Branch `feat/256-mask-scorer`.

## Run it

1. Once, fetch the test sets (about 2.3 GB downloaded, outside the repository; needs `uv`):
   `uv run tools/mask-testsets/fetch.py`
2. Score today's models (about 45 minutes on a test Mac): `cargo xtask mask-score --name <run>`

Parts of it: `--job subject,hair,click,sky`, `--method birefnet,matting,vision,sam3`, `--limit N` (every k-th photo),
`--data DIR` (another folder in the format below), `--out DIR`.

Results go to `~/Library/Application Support/ArtCraft/datasets/mask-score-runs/<run>/`:

| File | Holds |
| --- | --- |
| `summary.md`, `summary.json` | means per job, method (and click count, and set); size on disk, peak memory, engine; the worst photos |
| `scores.csv` | one row a mask |
| `chart.png` | the summary |
| `sheets/` | each method's three worst masks and its median one, at 1:1 beside the truth |
| `<job>/<method>/` | the masks themselves, `predict.json`, `log.txt` |

## Compare two runs

`cargo xtask mask-score --compare RUN_A RUN_B` prints the change of each mean (B against A), names the photos that got
0.10 IoU worse or more (the ship rule of #422) and writes `compare.csv` into B.

## Score a candidate model

- A model the app can load: put it in a models folder of its own (the same folder names as
  `ArtCraft/models`) and run with `LIGHTCRAFT_BIREFNET_DIR=<that folder>` (BiRefNet and BiRefNet Matting), or
  `LIGHTCRAFT_SAM3_DIR=<folder>` (SAM 3), under its own `--name`.
- Masks made elsewhere: write them as `<run>/<job>/<method>/<id>.png` (grey, the photo's size; Click: `<id>@1.png`,
  `<id>@3.png`, `<id>@5.png`), then `cargo xtask mask-score --score-only --name <run>`.

## What is measured

Each mask is made by the app's own commands (`mask.add`, `mask.objectPoint`, then `mask.refineDetail`, the zoomed pass
the app runs after a selection) and rendered as the app draws it (`crates/engine/examples/mask_score.rs`). The app's
settings are not read or written.

| Job | Methods | Truth |
| --- | --- | --- |
| `subject` | BiRefNet with the hair edge Current (`birefnet`), BiRefNet Matting (`matting`, the default) or Apple Vision (`vision`) | DIS-VD, 100 photos |
| `hair` | the same three Subject masks | P3M-500-NP (100) and AIM-500 (50) alpha mattes |
| `click` | SAM 3 Object | COCO val2017, 100 objects (polygon outlines: coarse at the edge) |
| `sky` | SAM 3 Sky | COCO-Stuff val2017 `sky-other` + `clouds`, 100 photos (coarse at the edge; unlabelled pixels not counted) |

The metrics and the click rule are in `crates/maskscore/src/lib.rs`: IoU (and for the mask turned around, the app's
Background), boundary F within 0.2 % of the long side, IoU in a band of 1 % round the truth's edge, the share of
enclosed gaps left out, SAD and MSE in the edge band. Click rule: each click goes to the deepest point of the largest
wrong area (include where something is missed, exclude where something is extra), so the first click is the same for
every model and later ones follow that model's mistakes. IoU is taken after 1, 3 and 5 clicks.

Seconds are the mask requests (model and zoomed pass, not the final render), one run each. Peak memory is the whole
scoring process's, from `/usr/bin/time -l`.

## Test sets

The photos are fixed in `tools/mask-testsets/lists.json` (names and ids only). `fetch.py` converts them into
`<data>/mask-score/<job>/<id>/{photo.png, mask.png, ignore.png?, meta.json}`, long side at most 2048, the format of
#422 (`--data <dir>/holdout` scores a held-out set in it). `--select` rebuilds the list; every baseline then has to be
scored again.

| Set | From | Licence |
| --- | --- | --- |
| DIS-VD | Hugging Face `nobg/DIS5K` @ `e007e9a` | DIS5K Terms of Use (non-commercial research and education) |
| P3M-500-NP | Hugging Face `nobg/P3M-10K` @ `a239de9` | MIT (release agreement); photo copyright stays with the owners |
| AIM-500 | Hugging Face `nobg/AIM-500` @ `b7d6542` | MIT (release agreement) |
| COCO val2017, COCO-Stuff | cocodataset.org, COCO-Stuff pixel maps | labels CC BY 4.0; photos under their owners' Flickr terms |

For evaluation only; nothing of them is in the repository.

## Branch

`feat/256-mask-scorer`

## Own files

`crates/maskscore/`, `crates/engine/examples/mask_score.rs`, `xtask/src/mask_score.rs`, `xtask/src/mask_score/`,
`tools/mask-testsets/`, `fork/mask-score.md`, `fork/tests.d/mask-score.txt`

## Hooks in official files

- `Cargo.toml`: workspace dependency `lightcraft-maskscore`. `xtask/src/layers.rs`: `maskscore` is layer 1.
- `crates/engine/Cargo.toml`: the dev-dependency and the `mask_score` example.
- `xtask/Cargo.toml`: `lightcraft-maskscore`, `image`, `ab_glyph`, `rayon` (all already in the workspace).
- `xtask/src/main.rs`: the module, the `mask-score` command and its usage lines.

## Tests

`cargo test -p lightcraft-maskscore` (also in `fork/tests.d/mask-score.txt`): each metric moves the right way on
made-up masks, and the click rule goes to the largest error.
