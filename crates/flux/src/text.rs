//! The prompt's embeddings: T5-XXL's per-token states (the transformer's text tokens) and CLIP-L's pooled output
//! (its conditioning vector). Both encoders are loaded, run once and dropped: T5 alone is 9.5 GB, more than the
//! quantised transformer. [`Prompt`] is what they give, small enough to keep on disk.

use std::collections::HashMap;

use candle_core::{DType, Device, IndexOp, Result as CResult, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::clip::text_model::{Activation, ClipTextConfig, ClipTextTransformer};
use candle_transformers::models::t5;

use crate::clip_tokenizer::Tokenizer as ClipTokenizer;
use crate::t5_tokenizer::T5Tokenizer;
use crate::{Error, Result, Weights};

/// The T5 tokens a prompt is encoded to (padded), as in the trials: enough for a description of a face.
pub const T5_TOKENS: usize = 256;
const T5_WIDTH: usize = 4096;
const CLIP_WIDTH: usize = 768;
const MAGIC: &[u8; 8] = b"LCFLXP01";

/// A prompt's embeddings: `t5` is `tokens × 4096`, `clip` is 768, both f32.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub tokens: usize,
    pub t5: Vec<f32>,
    pub clip: Vec<f32>,
}

impl Prompt {
    /// The embeddings as bytes (a magic, the token count, then little-endian f32s), for a disk cache.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + 4 * (self.t5.len() + self.clip.len()));
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.tokens as u64).to_le_bytes());
        for v in self.t5.iter().chain(&self.clip) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Reads [`Prompt::to_bytes`]; `None` for anything else (a damaged or old cache file).
    pub fn from_bytes(b: &[u8]) -> Option<Prompt> {
        let (magic, rest) = b.split_at_checked(8)?;
        if magic != MAGIC {
            return None;
        }
        let (n, rest) = rest.split_at_checked(8)?;
        let tokens = usize::try_from(u64::from_le_bytes(n.try_into().ok()?)).ok()?;
        let t5_len = tokens.checked_mul(T5_WIDTH)?;
        if tokens == 0 || tokens > 4096 || rest.len() != t5_len.checked_add(CLIP_WIDTH)?.checked_mul(4)? {
            return None;
        }
        let floats: Vec<f32> = rest.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
        if floats.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let (t5, clip) = floats.split_at_checked(t5_len)?;
        Some(Prompt { tokens, t5: t5.to_vec(), clip: clip.to_vec() })
    }
}

fn clip_config() -> ClipTextConfig {
    ClipTextConfig {
        vocab_size: 49408,
        projection_dim: 768,
        activation: Activation::QuickGelu,
        intermediate_size: 3072,
        embed_dim: CLIP_WIDTH,
        max_position_embeddings: 77,
        pad_with: None,
        num_hidden_layers: 12,
        num_attention_heads: 12,
    }
}

/// Encodes `prompt` with both text encoders on `dev`; `cancelled` is asked between them.
pub fn encode(w: &Weights, prompt: &str, dev: &Device, cancelled: &dyn Fn() -> bool) -> Result<Prompt> {
    let clip = clip_pooled(w, prompt, dev)?;
    if cancelled() {
        return Err(Error::Cancelled);
    }
    let t5 = t5_states(w, prompt, dev, cancelled)?;
    Ok(Prompt { tokens: T5_TOKENS, t5, clip })
}

fn clip_pooled(w: &Weights, prompt: &str, dev: &Device) -> Result<Vec<f32>> {
    let ids = ClipTokenizer::load(&w.clip_tokenizer)?.encode(prompt);
    let tensors = candle_core::safetensors::load(&w.clip, dev)?;
    let vb = VarBuilder::from_tensors(tensors, DType::F32, dev);
    let model = ClipTextTransformer::new(vb.pp("text_model"), &clip_config())?;
    let n = ids.len();
    let ids = Tensor::new(ids.as_slice(), dev)?.unsqueeze(0)?;
    let out = model.forward_with_mask(&ids, usize::MAX)?;
    // the pooled output: the state at the end token, the last one
    let pooled = out.i((0, n.saturating_sub(1)))?.to_dtype(DType::F32)?.to_vec1::<f32>()?;
    Ok(pooled)
}

fn t5_states(w: &Weights, prompt: &str, dev: &Device, cancelled: &dyn Fn() -> bool) -> Result<Vec<f32>> {
    let ids = T5Tokenizer::load(&w.t5_tokenizer)?.encode(prompt, T5_TOKENS);
    let cfg_text = std::fs::read_to_string(&w.t5_config).map_err(|e| Error::Model(format!("T5 config: {e}")))?;
    let mut cfg: t5::Config = serde_json::from_str(&cfg_text).map_err(|e| Error::Model(format!("T5 config: {e}")))?;
    cfg.use_cache = false;
    // bf16 on the GPU (its own type; f16 overflows in T5), f32 on the CPU
    let dtype = if dev.is_cpu() { DType::F32 } else { DType::BF16 };
    let mut tensors = HashMap::new();
    for shard in &w.t5 {
        // loading a shard (several GB) is what takes the time: ask between them, and before the model runs
        if cancelled() {
            return Err(Error::Cancelled);
        }
        tensors.extend(candle_core::safetensors::load(shard, dev)?);
    }
    if cancelled() {
        return Err(Error::Cancelled);
    }
    let vb = VarBuilder::from_tensors(tensors, dtype, dev);
    let mut model = t5::T5EncoderModel::load(vb, &cfg)?;
    if cancelled() {
        return Err(Error::Cancelled);
    }
    let ids = Tensor::new(ids.as_slice(), dev)?.unsqueeze(0)?;
    let states: CResult<Vec<f32>> = model.forward(&ids)?.to_dtype(DType::F32)?.flatten_all()?.to_vec1::<f32>();
    let states = states?;
    if states.len() != T5_TOKENS * T5_WIDTH || states.iter().any(|v| !v.is_finite()) {
        return Err(Error::Compute(format!("T5 gave {} values, or values that are not numbers", states.len())));
    }
    Ok(states)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_bytes_round_trip_and_reject_damage() {
        let p = Prompt { tokens: 2, t5: (0..2 * T5_WIDTH).map(|i| i as f32 * 0.5).collect(), clip: vec![1.5; CLIP_WIDTH] };
        let b = p.to_bytes();
        assert_eq!(Prompt::from_bytes(&b), Some(p));
        assert_eq!(Prompt::from_bytes(b.get(..b.len() - 4).unwrap_or_default()), None);
        assert_eq!(Prompt::from_bytes(b"LCFLXP01"), None);
        assert_eq!(Prompt::from_bytes(&[]), None);
        let mut bad = b.clone();
        bad[20..24].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(Prompt::from_bytes(&bad), None);
    }
}
