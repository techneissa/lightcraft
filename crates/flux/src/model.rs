//! The FLUX.1 transformer with GGUF-quantised weights, in the Fill layout.
//!
//! Modified work (MIT OR Apache-2.0): ported by the LightCraft contributors in 2026 from candle-transformers 0.9.2
//! (`src/models/flux/quantized_model.rs` and `model.rs`, Copyright The HuggingFace Team and the candle contributors),
//! which follows Black Forest Labs' reference code (https://github.com/black-forest-labs/flux, Apache-2.0). Changes:
//! the input layer takes `in_channels` (384 for Fill: the noisy latents, the masked image's latents and the mask)
//! while the last layer gives `out_channels` (64), every linear layer can carry a LoRA added at run time
//! ([`crate::lora`]), and indexing returns errors instead of panicking. See NOTICE.

use candle_core::{D, DType, IndexOp, Module, Result, Tensor};
use candle_nn::{LayerNorm, RmsNorm};
use candle_transformers::models::flux::model::EmbedNd;
use candle_transformers::quantized_nn;
use candle_transformers::quantized_var_builder::VarBuilder;

use crate::lora::{Lora, LoraSet};

/// FLUX.1's shape. [`Config::fill`] is FLUX.1 Fill dev.
#[derive(Debug, Clone)]
pub struct Config {
    pub in_channels: usize,
    pub out_channels: usize,
    pub vec_in_dim: usize,
    pub context_in_dim: usize,
    pub hidden_size: usize,
    pub mlp_ratio: f64,
    pub num_heads: usize,
    pub depth: usize,
    pub depth_single_blocks: usize,
    pub axes_dim: Vec<usize>,
    pub theta: usize,
    pub guidance_embed: bool,
}

impl Config {
    /// FLUX.1 Fill dev: FLUX.1 dev with a 384-channel input (64 latents + 64 masked-image latents + 256 mask).
    pub fn fill() -> Self {
        Self {
            in_channels: 384,
            out_channels: 64,
            vec_in_dim: 768,
            context_in_dim: 4096,
            hidden_size: 3072,
            mlp_ratio: 4.0,
            num_heads: 24,
            depth: 19,
            depth_single_blocks: 38,
            axes_dim: vec![16, 56, 56],
            theta: 10_000,
            guidance_embed: true,
        }
    }
}

/// Where a layer's weights are (the GGUF's names) and the LoRA set to take its low-rank update from.
#[derive(Clone)]
struct Ctx<'a> {
    vb: VarBuilder,
    name: String,
    lora: &'a LoraSet,
}

impl<'a> Ctx<'a> {
    fn pp<S: ToString>(&self, s: S) -> Ctx<'a> {
        let s = s.to_string();
        let name = if self.name.is_empty() { s.clone() } else { format!("{}.{s}", self.name) };
        Ctx { vb: self.vb.pp(s), name, lora: self.lora }
    }
}

/// A quantised linear layer and, when the LoRA has one for it, its low-rank update: `x·W + b + (x·down)·up`.
#[derive(Debug, Clone)]
struct Linear {
    base: quantized_nn::Linear,
    lora: Option<Lora>,
}

impl Linear {
    fn new(in_dim: usize, out_dim: usize, cx: &Ctx<'_>) -> Result<Self> {
        let base = quantized_nn::linear(in_dim, out_dim, cx.vb.clone())?;
        let lora = cx.lora.take(&cx.name, in_dim, out_dim, cx.vb.device())?;
        Ok(Self { base, lora })
    }
}

impl Module for Linear {
    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        // candle 0.9.2's Metal quantised matmul drops the input's start offset: a view that starts past the
        // beginning of its buffer (a `narrow` along the tokens) would read the wrong rows. Copy such views first.
        let copied;
        let xs = if xs.layout().start_offset() != 0 {
            copied = xs.force_contiguous()?;
            &copied
        } else {
            xs
        };
        let ys = xs.apply(&self.base)?;
        match &self.lora {
            None => Ok(ys),
            Some(l) => ys + xs.broadcast_matmul(&l.down)?.broadcast_matmul(&l.up)?,
        }
    }
}

fn layer_norm(dim: usize, dev: &candle_core::Device) -> Result<LayerNorm> {
    let ws = Tensor::ones(dim, DType::F32, dev)?;
    Ok(LayerNorm::new_no_bias(ws, 1e-6))
}

fn scaled_dot_product_attention(q: &Tensor, k: &Tensor, v: &Tensor) -> Result<Tensor> {
    let dim = q.dim(D::Minus1)?;
    let scale_factor = 1.0 / (dim as f64).sqrt();
    let mut batch_dims = q.dims().to_vec();
    batch_dims.truncate(batch_dims.len().saturating_sub(2));
    let n = batch_dims.len().saturating_sub(1);
    let q = q.flatten_to(n)?;
    let k = k.flatten_to(n)?;
    let v = v.flatten_to(n)?;
    let attn_weights = (q.matmul(&k.t()?)? * scale_factor)?;
    let attn_scores = candle_nn::ops::softmax_last_dim(&attn_weights)?.matmul(&v)?;
    batch_dims.push(attn_scores.dim(D::Minus2)?);
    batch_dims.push(attn_scores.dim(D::Minus1)?);
    attn_scores.reshape(batch_dims)
}

fn apply_rope(x: &Tensor, freq_cis: &Tensor) -> Result<Tensor> {
    let dims = x.dims();
    let (b_sz, n_head, seq_len, n_embd) = x.dims4()?;
    let x = x.reshape((b_sz, n_head, seq_len, n_embd / 2, 2))?;
    let x0 = x.narrow(D::Minus1, 0, 1)?;
    let x1 = x.narrow(D::Minus1, 1, 1)?;
    let fr0 = freq_cis.get_on_dim(D::Minus1, 0)?;
    let fr1 = freq_cis.get_on_dim(D::Minus1, 1)?;
    (fr0.broadcast_mul(&x0)? + fr1.broadcast_mul(&x1)?)?.reshape(dims.to_vec())
}

fn attention(q: &Tensor, k: &Tensor, v: &Tensor, pe: &Tensor) -> Result<Tensor> {
    let q = apply_rope(q, pe)?.contiguous()?;
    let k = apply_rope(k, pe)?.contiguous()?;
    let x = scaled_dot_product_attention(&q, &k, v)?;
    x.transpose(1, 2)?.flatten_from(2)
}

/// The sinusoidal embedding of `t` (0…1, scaled by 1000 as FLUX does), `dim` wide.
pub(crate) fn timestep_embedding(t: &Tensor, dim: usize, dtype: DType) -> Result<Tensor> {
    const TIME_FACTOR: f64 = 1000.;
    const MAX_PERIOD: f64 = 10000.;
    if dim % 2 == 1 {
        candle_core::bail!("{dim} is odd")
    }
    let dev = t.device();
    let half = dim / 2;
    let t = (t * TIME_FACTOR)?;
    let arange = Tensor::arange(0, half as u32, dev)?.to_dtype(DType::F32)?;
    let freqs = (arange * (-MAX_PERIOD.ln() / half as f64))?.exp()?;
    let args = t.unsqueeze(1)?.to_dtype(DType::F32)?.broadcast_mul(&freqs.unsqueeze(0)?)?;
    Tensor::cat(&[args.cos()?, args.sin()?], D::Minus1)?.to_dtype(dtype)
}

#[derive(Debug, Clone)]
struct MlpEmbedder {
    in_layer: Linear,
    out_layer: Linear,
}

impl MlpEmbedder {
    fn new(in_sz: usize, h_sz: usize, cx: &Ctx<'_>) -> Result<Self> {
        Ok(Self { in_layer: Linear::new(in_sz, h_sz, &cx.pp("in_layer"))?, out_layer: Linear::new(h_sz, h_sz, &cx.pp("out_layer"))? })
    }
}

impl Module for MlpEmbedder {
    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        xs.apply(&self.in_layer)?.silu()?.apply(&self.out_layer)
    }
}

#[derive(Debug, Clone)]
struct QkNorm {
    query_norm: RmsNorm,
    key_norm: RmsNorm,
}

impl QkNorm {
    fn new(dim: usize, vb: VarBuilder) -> Result<Self> {
        let query_norm = vb.get(dim, "query_norm.scale")?.dequantize(vb.device())?;
        let key_norm = vb.get(dim, "key_norm.scale")?.dequantize(vb.device())?;
        Ok(Self { query_norm: RmsNorm::new(query_norm, 1e-6), key_norm: RmsNorm::new(key_norm, 1e-6) })
    }
}

struct ModulationOut {
    shift: Tensor,
    scale: Tensor,
    gate: Tensor,
}

impl ModulationOut {
    fn scale_shift(&self, xs: &Tensor) -> Result<Tensor> {
        xs.broadcast_mul(&(&self.scale + 1.)?)?.broadcast_add(&self.shift)
    }

    fn gate(&self, xs: &Tensor) -> Result<Tensor> {
        self.gate.broadcast_mul(xs)
    }
}

/// `n` modulation triples (shift, scale, gate) from the conditioning vector.
fn modulation(lin: &Linear, vec_: &Tensor, n: usize) -> Result<Vec<ModulationOut>> {
    let ys = vec_.silu()?.apply(lin)?.unsqueeze(1)?.chunk(3 * n, D::Minus1)?;
    if ys.len() != 3 * n {
        candle_core::bail!("modulation: {} chunks, expected {}", ys.len(), 3 * n)
    }
    Ok(ys
        .as_chunks::<3>()
        .0
        .iter()
        .map(|[shift, scale, gate]| ModulationOut { shift: shift.clone(), scale: scale.clone(), gate: gate.clone() })
        .collect())
}

fn two<T>(mut v: Vec<T>) -> Result<(T, T)> {
    match (v.pop(), v.pop()) {
        (Some(b), Some(a)) => Ok((a, b)),
        _ => candle_core::bail!("expected two modulations"),
    }
}

#[derive(Debug, Clone)]
struct SelfAttention {
    qkv: Linear,
    norm: QkNorm,
    proj: Linear,
    num_heads: usize,
}

impl SelfAttention {
    fn new(dim: usize, num_heads: usize, cx: &Ctx<'_>) -> Result<Self> {
        let head_dim = dim / num_heads;
        Ok(Self {
            qkv: Linear::new(dim, dim * 3, &cx.pp("qkv"))?,
            norm: QkNorm::new(head_dim, cx.vb.pp("norm"))?,
            proj: Linear::new(dim, dim, &cx.pp("proj"))?,
            num_heads,
        })
    }

    fn qkv(&self, xs: &Tensor) -> Result<(Tensor, Tensor, Tensor)> {
        let qkv = xs.apply(&self.qkv)?;
        let (b, l, _khd) = qkv.dims3()?;
        let qkv = qkv.reshape((b, l, 3, self.num_heads, ()))?;
        let q = qkv.i((.., .., 0))?.transpose(1, 2)?;
        let k = qkv.i((.., .., 1))?.transpose(1, 2)?;
        let v = qkv.i((.., .., 2))?.transpose(1, 2)?;
        let q = q.apply(&self.norm.query_norm)?;
        let k = k.apply(&self.norm.key_norm)?;
        Ok((q, k, v))
    }
}

#[derive(Debug, Clone)]
struct Mlp {
    lin1: Linear,
    lin2: Linear,
}

impl Mlp {
    fn new(in_sz: usize, mlp_sz: usize, cx: &Ctx<'_>) -> Result<Self> {
        Ok(Self { lin1: Linear::new(in_sz, mlp_sz, &cx.pp("0"))?, lin2: Linear::new(mlp_sz, in_sz, &cx.pp("2"))? })
    }
}

impl Module for Mlp {
    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        xs.apply(&self.lin1)?.gelu()?.apply(&self.lin2)
    }
}

#[derive(Debug, Clone)]
struct DoubleStreamBlock {
    img_mod: Linear,
    img_norm1: LayerNorm,
    img_attn: SelfAttention,
    img_norm2: LayerNorm,
    img_mlp: Mlp,
    txt_mod: Linear,
    txt_norm1: LayerNorm,
    txt_attn: SelfAttention,
    txt_norm2: LayerNorm,
    txt_mlp: Mlp,
}

impl DoubleStreamBlock {
    fn new(cfg: &Config, cx: &Ctx<'_>) -> Result<Self> {
        let h_sz = cfg.hidden_size;
        let mlp_sz = (h_sz as f64 * cfg.mlp_ratio) as usize;
        let dev = cx.vb.device();
        Ok(Self {
            img_mod: Linear::new(h_sz, 6 * h_sz, &cx.pp("img_mod").pp("lin"))?,
            img_norm1: layer_norm(h_sz, dev)?,
            img_attn: SelfAttention::new(h_sz, cfg.num_heads, &cx.pp("img_attn"))?,
            img_norm2: layer_norm(h_sz, dev)?,
            img_mlp: Mlp::new(h_sz, mlp_sz, &cx.pp("img_mlp"))?,
            txt_mod: Linear::new(h_sz, 6 * h_sz, &cx.pp("txt_mod").pp("lin"))?,
            txt_norm1: layer_norm(h_sz, dev)?,
            txt_attn: SelfAttention::new(h_sz, cfg.num_heads, &cx.pp("txt_attn"))?,
            txt_norm2: layer_norm(h_sz, dev)?,
            txt_mlp: Mlp::new(h_sz, mlp_sz, &cx.pp("txt_mlp"))?,
        })
    }

    fn forward(&self, img: &Tensor, txt: &Tensor, vec_: &Tensor, pe: &Tensor) -> Result<(Tensor, Tensor)> {
        let (img_mod1, img_mod2) = two(modulation(&self.img_mod, vec_, 2)?)?;
        let (txt_mod1, txt_mod2) = two(modulation(&self.txt_mod, vec_, 2)?)?;
        let img_modulated = img_mod1.scale_shift(&img.apply(&self.img_norm1)?)?;
        let (img_q, img_k, img_v) = self.img_attn.qkv(&img_modulated)?;

        let txt_modulated = txt_mod1.scale_shift(&txt.apply(&self.txt_norm1)?)?;
        let (txt_q, txt_k, txt_v) = self.txt_attn.qkv(&txt_modulated)?;

        let q = Tensor::cat(&[txt_q, img_q], 2)?;
        let k = Tensor::cat(&[txt_k, img_k], 2)?;
        let v = Tensor::cat(&[txt_v, img_v], 2)?;

        let attn = attention(&q, &k, &v, pe)?;
        let n_txt = txt.dim(1)?;
        let txt_attn = attn.narrow(1, 0, n_txt)?;
        let img_attn = attn.narrow(1, n_txt, attn.dim(1)?.saturating_sub(n_txt))?;

        let img = (img + img_mod1.gate(&img_attn.apply(&self.img_attn.proj)?))?;
        let img = (&img + img_mod2.gate(&img_mod2.scale_shift(&img.apply(&self.img_norm2)?)?.apply(&self.img_mlp)?)?)?;

        let txt = (txt + txt_mod1.gate(&txt_attn.apply(&self.txt_attn.proj)?))?;
        let txt = (&txt + txt_mod2.gate(&txt_mod2.scale_shift(&txt.apply(&self.txt_norm2)?)?.apply(&self.txt_mlp)?)?)?;
        Ok((img, txt))
    }
}

#[derive(Debug, Clone)]
struct SingleStreamBlock {
    linear1: Linear,
    linear2: Linear,
    norm: QkNorm,
    pre_norm: LayerNorm,
    modulation: Linear,
    h_sz: usize,
    mlp_sz: usize,
    num_heads: usize,
}

impl SingleStreamBlock {
    fn new(cfg: &Config, cx: &Ctx<'_>) -> Result<Self> {
        let h_sz = cfg.hidden_size;
        let mlp_sz = (h_sz as f64 * cfg.mlp_ratio) as usize;
        let head_dim = h_sz / cfg.num_heads;
        Ok(Self {
            linear1: Linear::new(h_sz, h_sz * 3 + mlp_sz, &cx.pp("linear1"))?,
            linear2: Linear::new(h_sz + mlp_sz, h_sz, &cx.pp("linear2"))?,
            norm: QkNorm::new(head_dim, cx.vb.pp("norm"))?,
            pre_norm: layer_norm(h_sz, cx.vb.device())?,
            modulation: Linear::new(h_sz, 3 * h_sz, &cx.pp("modulation").pp("lin"))?,
            h_sz,
            mlp_sz,
            num_heads: cfg.num_heads,
        })
    }

    fn forward(&self, xs: &Tensor, vec_: &Tensor, pe: &Tensor) -> Result<Tensor> {
        let Some(mod_) = modulation(&self.modulation, vec_, 1)?.pop() else { candle_core::bail!("expected one modulation") };
        let x_mod = mod_.scale_shift(&xs.apply(&self.pre_norm)?)?;
        let x_mod = x_mod.apply(&self.linear1)?;
        let qkv = x_mod.narrow(D::Minus1, 0, 3 * self.h_sz)?;
        let (b, l, _khd) = qkv.dims3()?;
        let qkv = qkv.reshape((b, l, 3, self.num_heads, ()))?;
        let q = qkv.i((.., .., 0))?.transpose(1, 2)?;
        let k = qkv.i((.., .., 1))?.transpose(1, 2)?;
        let v = qkv.i((.., .., 2))?.transpose(1, 2)?;
        let mlp = x_mod.narrow(D::Minus1, 3 * self.h_sz, self.mlp_sz)?;
        let q = q.apply(&self.norm.query_norm)?;
        let k = k.apply(&self.norm.key_norm)?;
        let attn = attention(&q, &k, &v, pe)?;
        let output = Tensor::cat(&[attn, mlp.gelu()?], 2)?.apply(&self.linear2)?;
        xs + mod_.gate(&output)
    }
}

#[derive(Debug, Clone)]
struct LastLayer {
    norm_final: LayerNorm,
    linear: Linear,
    ada_ln_modulation: Linear,
}

impl LastLayer {
    fn new(h_sz: usize, out_c: usize, cx: &Ctx<'_>) -> Result<Self> {
        Ok(Self {
            norm_final: layer_norm(h_sz, cx.vb.device())?,
            linear: Linear::new(h_sz, out_c, &cx.pp("linear"))?,
            ada_ln_modulation: Linear::new(h_sz, 2 * h_sz, &cx.pp("adaLN_modulation.1"))?,
        })
    }

    fn forward(&self, xs: &Tensor, vec: &Tensor) -> Result<Tensor> {
        let chunks = vec.silu()?.apply(&self.ada_ln_modulation)?.chunk(2, 1)?;
        let (Some(shift), Some(scale)) = (chunks.first(), chunks.get(1)) else { candle_core::bail!("final modulation: expected two chunks") };
        let xs = xs.apply(&self.norm_final)?.broadcast_mul(&(scale.unsqueeze(1)? + 1.0)?)?.broadcast_add(&shift.unsqueeze(1)?)?;
        xs.apply(&self.linear)
    }
}

/// The FLUX.1 transformer.
#[derive(Debug, Clone)]
pub struct Flux {
    img_in: Linear,
    txt_in: Linear,
    time_in: MlpEmbedder,
    vector_in: MlpEmbedder,
    guidance_in: Option<MlpEmbedder>,
    pe_embedder: EmbedNd,
    double_blocks: Vec<DoubleStreamBlock>,
    single_blocks: Vec<SingleStreamBlock>,
    final_layer: LastLayer,
}

impl Flux {
    /// Builds the model from `vb` (a GGUF with Black Forest Labs' tensor names) and adds `lora`'s updates.
    pub fn new(cfg: &Config, vb: VarBuilder, lora: &LoraSet) -> Result<Self> {
        let cx = Ctx { vb, name: String::new(), lora };
        let img_in = Linear::new(cfg.in_channels, cfg.hidden_size, &cx.pp("img_in"))?;
        let txt_in = Linear::new(cfg.context_in_dim, cfg.hidden_size, &cx.pp("txt_in"))?;
        let double_blocks = (0..cfg.depth).map(|i| DoubleStreamBlock::new(cfg, &cx.pp("double_blocks").pp(i))).collect::<Result<Vec<_>>>()?;
        let single_blocks =
            (0..cfg.depth_single_blocks).map(|i| SingleStreamBlock::new(cfg, &cx.pp("single_blocks").pp(i))).collect::<Result<Vec<_>>>()?;
        let time_in = MlpEmbedder::new(256, cfg.hidden_size, &cx.pp("time_in"))?;
        let vector_in = MlpEmbedder::new(cfg.vec_in_dim, cfg.hidden_size, &cx.pp("vector_in"))?;
        let guidance_in = if cfg.guidance_embed { Some(MlpEmbedder::new(256, cfg.hidden_size, &cx.pp("guidance_in"))?) } else { None };
        let final_layer = LastLayer::new(cfg.hidden_size, cfg.out_channels, &cx.pp("final_layer"))?;
        let pe_dim = cfg.hidden_size / cfg.num_heads;
        let pe_embedder = EmbedNd::new(pe_dim, cfg.theta, cfg.axes_dim.clone());
        Ok(Self { img_in, txt_in, time_in, vector_in, guidance_in, pe_embedder, double_blocks, single_blocks, final_layer })
    }

    /// The velocity for `img` (packed tokens, `in_channels` wide) at time `timesteps` (0…1).
    #[allow(clippy::too_many_arguments)]
    pub fn forward(
        &self,
        img: &Tensor,
        img_ids: &Tensor,
        txt: &Tensor,
        txt_ids: &Tensor,
        timesteps: &Tensor,
        y: &Tensor,
        guidance: Option<&Tensor>,
    ) -> Result<Tensor> {
        if txt.rank() != 3 {
            candle_core::bail!("unexpected shape for txt {:?}", txt.shape())
        }
        if img.rank() != 3 {
            candle_core::bail!("unexpected shape for img {:?}", img.shape())
        }
        let dtype = img.dtype();
        let pe = Tensor::cat(&[txt_ids, img_ids], 1)?.apply(&self.pe_embedder)?;
        let mut txt = txt.apply(&self.txt_in)?;
        let mut img = img.apply(&self.img_in)?;
        let vec_ = timestep_embedding(timesteps, 256, dtype)?.apply(&self.time_in)?;
        let vec_ = match (self.guidance_in.as_ref(), guidance) {
            (Some(g_in), Some(guidance)) => (vec_ + timestep_embedding(guidance, 256, dtype)?.apply(g_in))?,
            _ => vec_,
        };
        let vec_ = (vec_ + y.apply(&self.vector_in))?;
        // Wait for the GPU after each block: candle queues work ahead of the GPU and keeps every queued block's
        // temporaries alive, which for a whole pass is tens of GB. Waiting lets them be reused.
        let dev = img.device().clone();
        for block in &self.double_blocks {
            (img, txt) = block.forward(&img, &txt, &vec_, &pe)?;
            dev.synchronize()?;
        }
        let mut img = Tensor::cat(&[&txt, &img], 1)?;
        for block in &self.single_blocks {
            img = block.forward(&img, &vec_, &pe)?;
            dev.synchronize()?;
        }
        let img = img.i((.., txt.dim(1)?..))?;
        self.final_layer.forward(&img, &vec_)
    }
}
