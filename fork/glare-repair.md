# Glasses Glare: Repair Glare

List: the Portrait tab's Glasses Glare section repairs glasses lenses per face: FLUX.1 Fill (GGUF, Turbo LoRA, pure Rust on candle) repaints a lens where glare hides the eye, LaMa fills small glints in seconds; Amount blends the repair in. Branch `feat/glare-repair-flux`.

## What it does

Portrait ▸ Glasses Glare (issue #74; the trials that picked the model: `~/scripts/lightcraft/glare-trials/`):

- **Method**: **Repaint (FLUX)**, FLUX.1 Fill dev at 768 px over a square crop around the glasses (two to three
  minutes a face on an M4 Pro), or **Fill Glints (LaMa)**, the Remove panel's LaMa over the glints only (seconds).
- **Lenses** (FLUX): **Auto** repaints a whole lens where glare covers at least 15 % of it (it hides the eye) and only
  the glints of the other lens; **Whole Lenses** repaints both lenses whole (a pale reflection whose colour is close to
  the skin's, which Auto can miss); **Glints Only** the glints.
- **Repair Glare** repairs the picked face, or every face with glasses (All Faces). **Remove Repair** drops it.
- **Amount** (0…100) blends the repair in; a repair sets it to 100 when it was 0.

How a run works (`crates/engine/src/glare_repair.rs`, on a worker with a "Repairing Glare" row in the activity stack:
progress per FLUX step, ✕ stops it):

1. The photo is rendered as the people parse sees it (default settings, before the crop, turned by the orientation),
   large enough that the crop is 768 px.
2. The crop is a square 2.6 eye distances wide around the eyes. The glasses are the face's Others mask near the eyes;
   closed and filled they are the glasses' outline, and shrunk past the rim (12 px of 768 for a thin rim, 22 for a
   thick dark one) the lens interiors, so the frames stay (`glare_repair/lens.rs`).
3. Glare in each lens: the Highlights finder's spots (`reflections/glare.rs`) and a veil (the trials' rule: brighter
   and paler than the skin around the glasses, far paler and not darker, brighter than its neighbourhood, or of a hue
   far from the skin's).
4. FLUX.1 Fill repaints the mask with a fixed prompt (`glare_repair::PROMPT`; its T5/CLIP embeddings are made once and
   kept in `<cache>/glare-repair/prompt-*.bin`), 8 steps with the Turbo LoRA, guidance 30, seed 0. LaMa fills in linear
   light.
5. The result is kept as the face's patch: a gain per pixel (repair ÷ original in linear Rec. 2020, the retouching
   step's space; each channel held within 2.5× of the luminance's gain so a near-empty channel can't run away) and the
   mask grown 4 px and feathered. The Glasses Glare feature multiplies the step's pixels by `1 + Amount · alpha ·
   (gain − 1)`, sampled in photo coordinates: preview, loupe and export match, a slider drag runs no model, and later
   exposure or white balance changes act on the repair too.
6. Patches are kept on disk (`<cache>/glare-repair/<hash>.lcgr`, by the photo's faces key) and attached again when the
   faces are found again (after a restart, or another photo and back).

The FLUX files are the user's: FLUX.1 [dev] is under Black Forest Labs' non-commercial licence, and nothing is
downloaded. The folder is `~/Library/Application Support/ArtCraft/models/flux-fill` (or `LIGHTCRAFT_FLUX_DIR`):

```text
flux-fill/
  flux1-fill-dev-Q4_K_S.gguf      any *.gguf (Q4_K_S preferred), Black Forest Labs' tensor names
  lora/<turbo>.safetensors        optional: FLUX.1-Turbo-Alpha (diffusers layout); 8 steps instead of 28
  vae/diffusion_pytorch_model.safetensors
  text_encoder/model.safetensors  CLIP-L
  text_encoder_2/                 T5-XXL shards and config.json
  tokenizer/                      vocab.json, merges.txt
  tokenizer_2/tokenizer.json
```

The model crate (`crates/flux`, `lightcraft-flux`): candle-transformers 0.9.2's quantised FLUX model, vendored with
the Fill layout (384 channels in, 64 out) and LoRA layers added at run time (`x·W + (x·down)·up`; diffusers' split
q/k/v stacked block-diagonally onto Black Forest Labs' fused layers); candle's T5 and CLIP text models; the VAE read
from the diffusers file, its convolutions run in row bands (candle's GPU convolution unrolls the whole input: one
768 px decoder layer is 5.4 GB). Its first step matches diffusers' `FluxFillPipeline` (correlation 0.9999998), after a
workaround: candle 0.9.2's Metal quantised matmul ignores a view's start offset, so a linear layer copies such views.

Measured on this Mac (M4 Pro, 48 GB), FLUX at 768 px, 8 steps: 185–215 s a face (load 3–5 s, 22 s a step, VAE a few
seconds); 14 GB while it denoises, 21–25 GB at the peak (the VAE decode). The diffusers trial took 120–150 s at 14 GB.

Developer tools: `cargo run --release -p lightcraft-flux --example fill -- DIR IN MASK OUT` (the model alone) and
`cargo run --release -p lightcraft-engine --example glare_repair -- PHOTO OUT.jpg [--method flux|lama]
[--lenses auto|whole|glints]` (the whole run, before and after side by side).

Commands: `retouch.glareRepair {face?, method?: flux|lama, lenses?: auto|whole|glints}`, `retouch.glareStatus`,
`retouch.glareClear {face?}`.

Not done: downloading the files for another Mac (the GGUF from its ungated mirror, size and SHA-256 checked, asked
first; the text encoders and VAE come from the gated FLUX.1 Fill dev repo, so a mirror has to be chosen); a face's own
description as the prompt; speed (22 s a step against diffusers' 15–18).

## Branch

`feat/glare-repair-flux`, from `custom`.

## Own files

- `crates/flux/` (new crate)
- `crates/engine/src/glare_repair.rs`, `glare_repair/lens.rs`, `glare_repair/store.rs`, `cmd/glare.rs`,
  `examples/glare_repair.rs`
- `crates/retouch/src/glare.rs`, `tests/glare.rs`; `crates/ui-egui/src/panels/portrait/glare.rs`

## Hooks in official files

- `Cargo.toml`: `candle-transformers` (=0.9.2) and `lightcraft-flux`; `xtask/src/layers.rs`: `flux` at L3; `NOTICE`.
- `crates/retouch/src/input.rs`: `Patch`, `Face::patch`, the fingerprint; `feature.rs`: `Scene::area_box`.
- `crates/engine/src/lib.rs`: `Session::glare`; `retouch_input.rs`: `FaceStore` attaches repairs; `lama_fill.rs`:
  `with_default_lama_model` sets the FLUX and cache folders; `cmd.rs`, `cmd/retouch.rs` (helpers made `pub(super)`).

## Tests

- `crates/flux`: LoRA stacking and shapes, the VAE's banded convolution and layout, tokenizers, packing, prompt
  bytes; `tests/fill.rs` runs the real model only with `LIGHTCRAFT_FLUX_DIR` set.
- `crates/retouch/tests/glare.rs`: off is exact, the patch shows by Amount inside its mask only, preview equals
  export, malformed patches change nothing, the patch is in the fingerprint.
- `crates/engine`: lens geometry (crop, morphology, lenses and what to repaint), the gain, the crop sampler, the
  store (restart, matching faces, damaged files), the commands' refusals.
- `crates/ui-egui`: the section shows its controls; Repair Glare without FLUX says why.

## What's New entry

### Repair Glasses Glare in the Portrait Tab
Portrait ▸ Glasses Glare can repair the lenses of a face, or of every face with glasses. Repaint (FLUX) redraws a lens
where glare hides the eye with FLUX.1 Fill, if you have its files, in two to three minutes a face; Fill Glints (LaMa)
fills small glints in seconds. The frames stay as they are, and Amount blends the repair in.
