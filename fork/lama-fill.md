# LaMa fill (first part: the model and the comparison)

List: LaMa (big-lama) ported to pure Rust on Candle as the new crate `lightcraft-lama`, with a comparison against today's PatchMatch fill; the Remove panel's Fill choice is not wired yet. Branch `feat/47-lama-fill`.

## What it does

`crates/lama` loads `big-lama.pt` (TorchScript, 206 MB, Apache-2.0) straight from the PyTorch file (Candle's pickle reader plus a walk over the module tree; no Python, nothing in the file is run) and fills a hole with LaMa's Fourier-convolution network. Output matches PyTorch on a 640 px test (largest difference 1/255, mean 0.00016). `rfft2` / `irfft2` are exact DFT matrix products on the model's device (`fft.rs`) instead of `rustfft`: a CPU FFT would copy the feature maps off the GPU and back around each of the 36 Fourier units.

`region.rs` fills a hole in a picture of any size: it crops the hole plus half its size around it, scales the crop to at most 512 px (exposure normalised, gamma encoded), grows the hole 2 px, runs LaMa, scales the result back and writes only the hole's pixels; everything else stays byte for byte.

Speed on this Mac (M4 Pro, Metal, machine under load ~40): 640×640 in 0.71 s, 512 px crops 0.3 to 0.5 s; CPU 2.8 s for 640×640 (PyTorch CPU: 1.1 s). Model load 0.3 to 1.2 s.

Not done yet (see the PR text): the `fill` field on a spot, the render hook, the engine installer and download, the Fill dropdown, activity rows.

## Branch

`feat/47-lama-fill`

## Own files

`crates/lama/` (`checkpoint.rs`, `fft.rs`, `layers.rs`, `cl.rs`, `net.rs`, `region.rs`, `tests.rs`, `examples/compare.rs`)

## Hooks in official files

`Cargo.lock` only (the new crate and `zip`).

## Tests

`cargo test -p lightcraft-lama --lib`
