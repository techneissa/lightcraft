//! FLUX.1 Fill dev (Black Forest Labs) in pure Rust on candle: masked repainting, used to repaint glasses lenses
//! where glare hides the eyes (`fork/glare-repair.md`).
//!
//! - [`Weights::in_dir`] finds the files in one folder: the transformer as a GGUF (Q4_K_S, 6.8 GB), an optional
//!   LoRA in the diffusers layout (the Turbo LoRA: 8 steps instead of about 28), and FLUX.1 Fill dev's own
//!   `vae/`, `text_encoder/` (CLIP-L), `text_encoder_2/` (T5-XXL), `tokenizer/` and `tokenizer_2/`.
//! - [`text::encode`] turns a prompt into embeddings ([`Prompt`]) with both text encoders, then frees them; keep the
//!   result ([`Prompt::to_bytes`]) and the encoders never load again for that prompt.
//! - [`FillModel::fill`] repaints the masked part of a square picture, with progress and cancel between steps.
//!
//! The weights are not part of LightCraft: FLUX.1 [dev] models are under Black Forest Labs' non-commercial licence.
//! Metal on macOS ([`best_device`]), else the CPU (far too slow to be useful). Native only: on wasm32 this crate is
//! empty.

#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod clip_tokenizer;
mod fill;
mod gguf;
mod lora;
mod model;
mod t5_tokenizer;
pub mod text;
mod vae;

use std::path::{Path, PathBuf};

use candle_core::Device;

pub use fill::{FillModel, MAX_SIDE, MULTIPLE, Request};
/// Checks a GGUF's header against its file's size before anything is loaded from it (a hostile file is an error).
pub use gguf::check as check_gguf;
pub use text::Prompt;

/// Why a FLUX call failed. Every failure is an error the caller can show; nothing panics.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file is missing, unreadable or not what it should be.
    #[error("FLUX model: {0}")]
    Model(String),
    /// The picture, mask or settings handed in can't be used.
    #[error("FLUX input: {0}")]
    Input(String),
    /// The caller asked to stop.
    #[error("FLUX fill cancelled")]
    Cancelled,
    /// The tensor library failed (out of memory, a missing GPU op).
    #[error("FLUX compute: {0}")]
    Compute(String),
}

impl From<candle_core::Error> for Error {
    fn from(e: candle_core::Error) -> Self {
        Self::Compute(e.to_string())
    }
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// The best device here: Metal on macOS when a GPU is available, else the CPU.
pub fn best_device() -> Device {
    #[cfg(target_os = "macos")]
    if let Ok(d) = Device::new_metal(0) {
        return d;
    }
    Device::Cpu
}

/// The files of one FLUX.1 Fill setup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Weights {
    /// The transformer, a GGUF with Black Forest Labs' tensor names.
    pub transformer: PathBuf,
    /// A LoRA (diffusers layout) added to the transformer.
    pub lora: Option<PathBuf>,
    pub vae: PathBuf,
    /// CLIP-L's weights (`text_encoder/model.safetensors`).
    pub clip: PathBuf,
    /// The folder with CLIP's `vocab.json` and `merges.txt`.
    pub clip_tokenizer: PathBuf,
    /// T5-XXL's weight shards.
    pub t5: Vec<PathBuf>,
    pub t5_config: PathBuf,
    pub t5_tokenizer: PathBuf,
}

impl Weights {
    /// Finds the files in `dir`: any `*.gguf` (one whose name has `Q4_K_S` first), `lora/*.safetensors` (optional),
    /// and FLUX.1 Fill dev's folders. The error names what is missing.
    pub fn in_dir(dir: &Path) -> Result<Weights> {
        let listed = |d: &Path, ext: &str| -> Vec<PathBuf> {
            let mut v: Vec<PathBuf> = std::fs::read_dir(d)
                .map(|it| it.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == ext) && p.is_file()).collect())
                .unwrap_or_default();
            v.sort();
            v
        };
        let need = |p: PathBuf| if p.is_file() { Ok(p) } else { Err(Error::Model(format!("missing {}", p.display()))) };
        let ggufs = listed(dir, "gguf");
        let transformer = ggufs
            .iter()
            .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().contains("Q4_K_S")))
            .or_else(|| ggufs.first())
            .cloned()
            .ok_or_else(|| Error::Model(format!("no .gguf transformer in {}", dir.display())))?;
        let lora = listed(&dir.join("lora"), "safetensors").into_iter().next();
        let t5 = listed(&dir.join("text_encoder_2"), "safetensors");
        if t5.is_empty() {
            return Err(Error::Model(format!("no T5 weights in {}", dir.join("text_encoder_2").display())));
        }
        let clip_tokenizer = dir.join("tokenizer");
        need(clip_tokenizer.join("vocab.json"))?;
        need(clip_tokenizer.join("merges.txt"))?;
        Ok(Weights {
            transformer,
            lora,
            vae: need(dir.join("vae").join("diffusion_pytorch_model.safetensors"))?,
            clip: need(dir.join("text_encoder").join("model.safetensors"))?,
            clip_tokenizer,
            t5,
            t5_config: need(dir.join("text_encoder_2").join("config.json"))?,
            t5_tokenizer: need(dir.join("tokenizer_2").join("tokenizer.json"))?,
        })
    }

    /// The steps that suit these weights: 8 with a (Turbo) LoRA, 28 without.
    pub fn default_steps(&self) -> usize {
        if self.lora.is_some() { 8 } else { 28 }
    }

    /// A stamp of the files (names, sizes and times), so results made with other weights are not reused.
    pub fn stamp(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |b: &[u8]| {
            for x in b {
                h ^= u64::from(*x);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        let files = [&self.transformer, &self.vae, &self.clip, &self.t5_config, &self.t5_tokenizer].into_iter().chain(&self.lora).chain(&self.t5);
        for p in files {
            eat(p.to_string_lossy().as_bytes());
            if let Ok(m) = std::fs::metadata(p) {
                eat(&m.len().to_le_bytes());
                if let Ok(t) = m.modified().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).map_err(std::io::Error::other)) {
                    eat(&t.as_secs().to_le_bytes());
                }
            }
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_are_named() {
        let dir = std::env::temp_dir().join(format!("lc-flux-weights-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let e = Weights::in_dir(&dir).unwrap_err().to_string();
        assert!(e.contains(".gguf"), "{e}");
        std::fs::write(dir.join("flux1-fill-dev-Q8_0.gguf"), b"x").unwrap();
        std::fs::write(dir.join("flux1-fill-dev-Q4_K_S.gguf"), b"x").unwrap();
        for (d, f) in [
            ("text_encoder_2", "model-00001-of-00001.safetensors"),
            ("text_encoder_2", "config.json"),
            ("tokenizer", "vocab.json"),
            ("tokenizer", "merges.txt"),
            ("tokenizer_2", "tokenizer.json"),
            ("text_encoder", "model.safetensors"),
        ] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
            std::fs::write(dir.join(d).join(f), b"x").unwrap();
        }
        let e = Weights::in_dir(&dir).unwrap_err().to_string();
        assert!(e.contains("diffusion_pytorch_model.safetensors"), "{e}");
        std::fs::create_dir_all(dir.join("vae")).unwrap();
        std::fs::write(dir.join("vae").join("diffusion_pytorch_model.safetensors"), b"x").unwrap();
        let w = Weights::in_dir(&dir).unwrap();
        assert!(w.transformer.ends_with("flux1-fill-dev-Q4_K_S.gguf"));
        assert_eq!(w.lora, None);
        assert_eq!(w.default_steps(), 28);
        std::fs::create_dir_all(dir.join("lora")).unwrap();
        std::fs::write(dir.join("lora").join("turbo.safetensors"), b"x").unwrap();
        let w2 = Weights::in_dir(&dir).unwrap();
        assert_eq!(w2.default_steps(), 8);
        assert_ne!(w.stamp(), w2.stamp());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
