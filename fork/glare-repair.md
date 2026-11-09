# Glasses Glare: Repair Glare

List: the Portrait tab's Glasses Glare section repairs glasses lenses per face: FLUX.1 Fill (GGUF, Turbo LoRA, pure Rust on candle) repaints a lens where glare hides the eye, LaMa fills small glints in seconds; Amount blends the repair in. Branch `feat/glare-repair-flux`.

## What it does

Portrait ▸ Glasses Glare (issue #74):

- **Method**: **Repaint (FLUX)**, FLUX.1 Fill dev at 768 px over a square crop around the glasses (two to three
  minutes a face on a Mac with a fast GPU), or **Fill Glints (LaMa)**, the Remove panel's LaMa over the glints only (seconds).
- **Lenses** (FLUX): **Auto** repaints a whole lens where glare covers at least 15 % of it (it hides the eye) and only
  the glints of the other lens; **Whole Lenses** repaints both lenses whole (a pale reflection whose colour is close to
  the skin's, which Auto can miss); **Glints Only** the glints.
- **Repair Glare** repairs the picked face, or every face with glasses (All Faces). **Remove Repair** drops it.
- **Amount** (0…100) blends the repair in. Once a repair is kept, the session turns Amount up to 100 where it was 0,
  as one undo step ("Glasses Glare"; `Session::poll_glare`, called every frame by the window and by every
  `retouch.glare*` command). For All Faces that is the global Amount, and also the own Amount of a face whose override
  is at 0. A run that stops before keeping anything, finds no glare or fails to start changes neither Amount nor the
  undo history.
- **Memory**: Repaint peaks at 21–25 GB, so a Mac with less than 32 GB of memory refuses it ("Repaint needs a Mac with
  32 GB of memory or more; use Fill Glints"); the panel greys the FLUX choice and the default method becomes Fill
  Glints. `memory::release()` runs after every run, whether it worked or not.

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

The FLUX files are the user's, nothing is bundled and nothing is downloaded. FLUX.1 Fill [dev] and the Turbo LoRA are
under Black Forest Labs' FLUX.1 [dev] Non-Commercial License; T5-XXL, CLIP-L and the VAE are under the licences their
model cards give (`NOTICE`). `crates/flux` is Apache-2.0 (its CLIP tokenizer is adapted from `crates/segment`, itself
from Hugging Face Transformers' Apache-2.0 code). The folder is `~/Library/Application Support/ArtCraft/models/flux-fill` (or `LIGHTCRAFT_FLUX_DIR`):

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

Measured on a test Mac, FLUX at 768 px, 8 steps: 185–215 s a face (load 3–5 s, 22 s a step, VAE a few
seconds); 14 GB while it denoises, 21–25 GB at the peak (the VAE decode). The diffusers trial took 120–150 s at 14 GB.

Hostile files: before candle reads the GGUF, `gguf.rs` walks its header with every count, string length and tensor
size held against the file's length and caps (tensor count, name length, element count, nested arrays) and the FLUX
names it needs; a truncated, garbage or huge-size file is an error, never an allocation.

Cancel: ✕ is looked at between the T5 shards and before T5 runs, between FLUX steps, between the VAE's convolution
bands and between faces. Each face's repair is kept the moment it finishes, so a stop at face 2 keeps face 1.

The activity row says "Repainting Glare (Face 1 of 2)" while FLUX runs; another job waiting for the GPU (hair edges)
says "Waiting for Glare Repair" (`gpu_run::turn_for`, `holder`). Remove Repair is refused with "Repair Running" while a
repair runs. The store reads, changes and writes under one lock. The step's write boxes include a repair's area
(`retouch_step::read_box`), so a repair reaching past its face's box is drawn.

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

- `crates/flux/` (new crate; `gguf.rs` the header check)
- `crates/engine/src/glare_repair.rs`, `glare_repair/lens.rs`, `glare_repair/store.rs`, `cmd/glare.rs`,
  `examples/glare_repair.rs`
- `crates/retouch/src/glare.rs`, `tests/glare.rs`; `crates/ui-egui/src/panels/portrait/glare.rs`

## Hooks in official files

- `Cargo.toml`: `candle-transformers` (=0.9.2) and `lightcraft-flux`; `xtask/src/layers.rs`: `flux` at L3; `NOTICE`.
- `crates/retouch/src/input.rs`: `Patch`, `Face::patch`, the fingerprint; `feature.rs`: `Scene::area_box`.
- `crates/engine/src/lib.rs`: `Session::glare`; `retouch_input.rs`: `FaceStore` attaches repairs; `lama_fill.rs`:
  `with_default_lama_model` sets the FLUX and cache folders; `cmd.rs`, `cmd/retouch.rs` (helpers made `pub(super)`); `gpu_run.rs` (`turn_for`, `holder`), `hair_edges.rs` (the
  waiting text); `crates/pipeline/src/retouch_step.rs` (`read_box` holds a repair's area); `ui-egui/src/app/frame.rs`
  (`poll_glare`).

## Tests

- `crates/flux`: LoRA stacking and shapes, the VAE's banded convolution, layout and stop between bands, tokenizers,
  packing, prompt bytes, the GGUF header check (truncated, garbage, huge counts and sizes), the start-offset
  workaround of the linear layer (fails on Metal without it); `tests/fill.rs` runs the real model only with `LIGHTCRAFT_FLUX_DIR` set.
- `crates/retouch/tests/glare.rs`: off is exact, the patch shows by Amount inside its mask only, preview equals
  export, malformed patches change nothing, the patch is in the fingerprint.
- `crates/pipeline` (`tests_retouch_glare.rs`): a kept repair under a zoomed window and under Face Reshape, inside
  the step's write boxes.
- `crates/engine`: lens geometry (crop, morphology, lenses and what to repaint), the gain, the crop sampler, the
  store (restart, matching faces, damaged files, adds and clears at once), the commands' refusals, the memory check,
  a stop at the second face keeping the first, a run that keeps nothing leaving Amount and history alone, All Faces
  turning up a face whose own Amount is 0, the stamp over the whole face box, the GPU holder's name.
- `crates/ui-egui`: the section shows its controls; Repair Glare without FLUX says why; the files are looked up
  again only when the section opens, the photo or the run changes, or every few seconds while they are missing.
  `fork/tests.d/glare-repair.txt` lists the commands.

## What's New entry

### Repair Glasses Glare in the Portrait Tab
Portrait ▸ Glasses Glare can repair the lenses of a face, or of every face with glasses. Repaint (FLUX) redraws a lens
where glare hides the eye with FLUX.1 Fill, if you have its files, in two to three minutes a face; Fill Glints (LaMa)
fills small glints in seconds. The frames stay as they are, and Amount blends the repair in. Repaint needs a Mac
with 32 GB of memory or more.
