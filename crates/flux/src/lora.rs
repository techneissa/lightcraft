//! A LoRA in the diffusers layout (`transformer.<layer>.lora_A.weight` / `lora_B.weight`), added to the GGUF
//! transformer at run time.
//!
//! The quantised weights are not touched (merging would mean dequantising 12 billion weights and quantising them
//! again); each linear layer adds `(x·down)·up` instead, which is the same sum. Diffusers splits layers that Black
//! Forest Labs' layout fuses (the attention's q, k and v; a single block's q, k, v and MLP input): their low-rank
//! pairs are stacked into one, `down` side by side and `up` block-diagonal, so the fused layer gets exactly the sum
//! of the split ones. The final layer's modulation is stored as (scale, shift) by diffusers and (shift, scale) by
//! Black Forest Labs: its `up` halves are swapped. A LoRA made for FLUX.1 dev (64 input channels) on Fill (384) adds
//! nothing for the extra channels, as diffusers does.

use std::collections::HashMap;
use std::path::Path;

use candle_core::{DType, Device, Result, Tensor};

/// The low-rank update of one linear layer: `down` is `in × r`, `up` is `r × out`, scale included.
#[derive(Debug, Clone)]
pub struct Lora {
    pub down: Tensor,
    pub up: Tensor,
}

/// The updates of every layer a LoRA file covers, by the layer's name in the GGUF (`double_blocks.3.img_attn.qkv`).
#[derive(Debug, Default)]
pub struct LoraSet {
    layers: HashMap<String, (Tensor, Tensor)>,
}

/// One target layer: its GGUF name, the diffusers layers fused into it (in order), and whether its output halves
/// are swapped.
struct Target {
    name: String,
    parts: Vec<String>,
    swap_halves: bool,
}

fn target(name: impl Into<String>, parts: &[String]) -> Target {
    Target { name: name.into(), parts: parts.to_vec(), swap_halves: false }
}

/// Every layer of FLUX.1 with `depth` double and `single` single blocks, Black Forest Labs' name first.
fn targets(depth: usize, single: usize) -> Vec<Target> {
    let one = |s: &str| vec![s.to_string()];
    let mut t = vec![
        target("img_in", &one("x_embedder")),
        target("txt_in", &one("context_embedder")),
        target("time_in.in_layer", &one("time_text_embed.timestep_embedder.linear_1")),
        target("time_in.out_layer", &one("time_text_embed.timestep_embedder.linear_2")),
        target("vector_in.in_layer", &one("time_text_embed.text_embedder.linear_1")),
        target("vector_in.out_layer", &one("time_text_embed.text_embedder.linear_2")),
        target("guidance_in.in_layer", &one("time_text_embed.guidance_embedder.linear_1")),
        target("guidance_in.out_layer", &one("time_text_embed.guidance_embedder.linear_2")),
        target("final_layer.linear", &one("proj_out")),
        Target { name: "final_layer.adaLN_modulation.1".into(), parts: one("norm_out.linear"), swap_halves: true },
    ];
    for i in 0..depth {
        let d = format!("transformer_blocks.{i}");
        let b = format!("double_blocks.{i}");
        let qkv = |a: &str, b2: &str, c: &str| vec![format!("{d}.attn.{a}"), format!("{d}.attn.{b2}"), format!("{d}.attn.{c}")];
        t.push(target(format!("{b}.img_attn.qkv"), &qkv("to_q", "to_k", "to_v")));
        t.push(target(format!("{b}.txt_attn.qkv"), &qkv("add_q_proj", "add_k_proj", "add_v_proj")));
        t.push(target(format!("{b}.img_attn.proj"), &[format!("{d}.attn.to_out.0")]));
        t.push(target(format!("{b}.txt_attn.proj"), &[format!("{d}.attn.to_add_out")]));
        t.push(target(format!("{b}.img_mlp.0"), &[format!("{d}.ff.net.0.proj")]));
        t.push(target(format!("{b}.img_mlp.2"), &[format!("{d}.ff.net.2")]));
        t.push(target(format!("{b}.txt_mlp.0"), &[format!("{d}.ff_context.net.0.proj")]));
        t.push(target(format!("{b}.txt_mlp.2"), &[format!("{d}.ff_context.net.2")]));
        t.push(target(format!("{b}.img_mod.lin"), &[format!("{d}.norm1.linear")]));
        t.push(target(format!("{b}.txt_mod.lin"), &[format!("{d}.norm1_context.linear")]));
    }
    for i in 0..single {
        let s = format!("single_transformer_blocks.{i}");
        let b = format!("single_blocks.{i}");
        let l1 = [format!("{s}.attn.to_q"), format!("{s}.attn.to_k"), format!("{s}.attn.to_v"), format!("{s}.proj_mlp")];
        t.push(target(format!("{b}.linear1"), &l1));
        t.push(target(format!("{b}.linear2"), &[format!("{s}.proj_out")]));
        t.push(target(format!("{b}.modulation.lin"), &[format!("{s}.norm.linear")]));
    }
    t
}

impl LoraSet {
    /// No LoRA.
    pub fn none() -> Self {
        Self::default()
    }

    /// Reads a diffusers-layout LoRA for a FLUX.1 transformer with `depth` double and `single` single blocks.
    /// Layers it lacks get no update; a layer it covers only in part, or a tensor of the wrong rank, is an error.
    pub fn load(path: &Path, depth: usize, single: usize) -> Result<Self> {
        let raw = candle_core::safetensors::load(path, &Device::Cpu)?;
        let tensors: HashMap<String, Tensor> =
            raw.into_iter().map(|(k, v)| (k.strip_prefix("transformer.").map(str::to_string).unwrap_or(k), v)).collect();
        Self::from_tensors(&tensors, depth, single)
    }

    /// As [`LoraSet::load`], from tensors named without the `transformer.` prefix.
    pub fn from_tensors(tensors: &HashMap<String, Tensor>, depth: usize, single: usize) -> Result<Self> {
        let mut layers = HashMap::new();
        for t in targets(depth, single) {
            let found: Vec<Option<(Tensor, Tensor)>> = t.parts.iter().map(|p| pair(tensors, p)).collect::<Result<_>>()?;
            if found.iter().all(Option::is_none) {
                continue;
            }
            let Some(pairs) = found.into_iter().collect::<Option<Vec<_>>>() else { candle_core::bail!("LoRA covers only part of {}", t.name) };
            let (down, mut up) = stack(&pairs)?;
            if t.swap_halves {
                let out = up.dim(1)?;
                let half = out / 2;
                up = Tensor::cat(&[up.narrow(1, half, out - half)?, up.narrow(1, 0, half)?], 1)?;
            }
            layers.insert(t.name, (down, up));
        }
        Ok(Self { layers })
    }

    /// How many layers get an update.
    pub fn len(&self) -> usize {
        self.layers.len()
    }

    /// The update of layer `name` (`in_dim` → `out_dim`) on `dev` as f32, if the LoRA has one. A LoRA with fewer
    /// input channels than the layer (FLUX.1 dev's 64 on Fill's 384) adds nothing for the rest.
    pub fn take(&self, name: &str, in_dim: usize, out_dim: usize, dev: &Device) -> Result<Option<Lora>> {
        let Some((down, up)) = self.layers.get(name) else { return Ok(None) };
        let (d_in, r) = down.dims2()?;
        let (r2, d_out) = up.dims2()?;
        if r != r2 || d_out != out_dim || d_in > in_dim {
            candle_core::bail!("LoRA for {name}: {d_in}→{d_out} (rank {r}/{r2}) does not fit the layer {in_dim}→{out_dim}")
        }
        let down = if d_in < in_dim {
            Tensor::cat(&[down.clone(), Tensor::zeros((in_dim - d_in, r), down.dtype(), down.device())?], 0)?
        } else {
            down.clone()
        };
        Ok(Some(Lora { down: down.to_dtype(DType::F32)?.to_device(dev)?, up: up.to_dtype(DType::F32)?.to_device(dev)? }))
    }
}

/// `prefix`'s `(down, up)` = (`lora_A`ᵀ, `lora_Bᵀ · alpha / r`): `in × r` and `r × out`, f32 on the CPU.
fn pair(tensors: &HashMap<String, Tensor>, prefix: &str) -> Result<Option<(Tensor, Tensor)>> {
    let (Some(a), Some(b)) = (tensors.get(&format!("{prefix}.lora_A.weight")), tensors.get(&format!("{prefix}.lora_B.weight"))) else {
        return Ok(None);
    };
    let (r, _in) = a.dims2()?;
    let (_out, r2) = b.dims2()?;
    if r != r2 || r == 0 {
        candle_core::bail!("LoRA {prefix}: ranks {r} and {r2} differ")
    }
    let scale = match tensors.get(&format!("{prefix}.alpha")) {
        Some(alpha) => f64::from(alpha.to_dtype(DType::F32)?.flatten_all()?.to_vec1::<f32>()?.first().copied().unwrap_or(r as f32)) / r as f64,
        None => 1.0,
    };
    let down = a.to_dtype(DType::F32)?.t()?.contiguous()?;
    let up = (b.to_dtype(DType::F32)?.t()?.contiguous()? * scale)?;
    Ok(Some((down, up)))
}

/// Stacks low-rank pairs of layers whose outputs are concatenated: `down` side by side, `up` block-diagonal.
fn stack(pairs: &[(Tensor, Tensor)]) -> Result<(Tensor, Tensor)> {
    if let [(d, u)] = pairs {
        return Ok((d.clone(), u.clone()));
    }
    let downs: Vec<Tensor> = pairs.iter().map(|(d, _)| d.clone()).collect();
    let down = Tensor::cat(&downs, 1)?;
    let total: usize = pairs.iter().map(|(_, u)| u.dim(1)).sum::<Result<usize>>()?;
    let mut rows = Vec::with_capacity(pairs.len());
    let mut at = 0;
    for (_, u) in pairs {
        let (r, out) = u.dims2()?;
        let mut parts = Vec::with_capacity(3);
        if at > 0 {
            parts.push(Tensor::zeros((r, at), DType::F32, u.device())?);
        }
        parts.push(u.clone());
        let rest = total.saturating_sub(at + out);
        if rest > 0 {
            parts.push(Tensor::zeros((r, rest), DType::F32, u.device())?);
        }
        rows.push(Tensor::cat(&parts, 1)?);
        at += out;
    }
    Ok((down, Tensor::cat(&rows, 0)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(v: &[f32], shape: (usize, usize)) -> Tensor {
        Tensor::from_vec(v.to_vec(), shape, &Device::Cpu).unwrap()
    }

    /// The fused layer's update equals the split layers' updates side by side.
    #[test]
    fn fused_update_is_the_split_updates() {
        let mut m = HashMap::new();
        // in = 2, rank 1, out 2 per part
        for (i, p) in ["to_q", "to_k", "to_v"].iter().enumerate() {
            let f = i as f32 + 1.0;
            m.insert(format!("transformer_blocks.0.attn.{p}.lora_A.weight"), t(&[f, -f], (1, 2)));
            m.insert(format!("transformer_blocks.0.attn.{p}.lora_B.weight"), t(&[1.0, 2.0 * f], (2, 1)));
        }
        let set = LoraSet::from_tensors(&m, 1, 0).unwrap();
        assert_eq!(set.len(), 1);
        let l = set.take("double_blocks.0.img_attn.qkv", 2, 6, &Device::Cpu).unwrap().unwrap();
        let x = t(&[3.0, 1.0], (1, 2));
        let y = x.matmul(&l.down).unwrap().matmul(&l.up).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
        // part i: (3f - f) = 2f, times (1, 2f)
        assert_eq!(y, vec![2.0, 4.0, 4.0, 16.0, 6.0, 36.0]);
    }

    #[test]
    fn final_modulation_halves_are_swapped() {
        let mut m = HashMap::new();
        m.insert("norm_out.linear.lora_A.weight".to_string(), t(&[1.0], (1, 1)));
        m.insert("norm_out.linear.lora_B.weight".to_string(), t(&[1.0, 2.0, 3.0, 4.0], (4, 1)));
        let set = LoraSet::from_tensors(&m, 0, 0).unwrap();
        let l = set.take("final_layer.adaLN_modulation.1", 1, 4, &Device::Cpu).unwrap().unwrap();
        assert_eq!(l.up.flatten_all().unwrap().to_vec1::<f32>().unwrap(), vec![3.0, 4.0, 1.0, 2.0]);
    }

    #[test]
    fn narrower_input_is_padded_and_wrong_shapes_are_errors() {
        let mut m = HashMap::new();
        m.insert("x_embedder.lora_A.weight".to_string(), t(&[1.0, 1.0], (1, 2)));
        m.insert("x_embedder.lora_B.weight".to_string(), t(&[1.0], (1, 1)));
        let set = LoraSet::from_tensors(&m, 0, 0).unwrap();
        let l = set.take("img_in", 4, 1, &Device::Cpu).unwrap().unwrap();
        assert_eq!(l.down.dims(), &[4, 1]);
        assert_eq!(l.down.flatten_all().unwrap().to_vec1::<f32>().unwrap(), vec![1.0, 1.0, 0.0, 0.0]);
        assert!(set.take("img_in", 1, 1, &Device::Cpu).is_err());
        assert!(set.take("img_in", 4, 2, &Device::Cpu).is_err());
        assert!(set.take("txt_in", 4, 1, &Device::Cpu).unwrap().is_none());
    }

    #[test]
    fn a_layer_covered_in_part_is_an_error() {
        let mut m = HashMap::new();
        m.insert("transformer_blocks.0.attn.to_q.lora_A.weight".to_string(), t(&[1.0], (1, 1)));
        m.insert("transformer_blocks.0.attn.to_q.lora_B.weight".to_string(), t(&[1.0], (1, 1)));
        assert!(LoraSet::from_tensors(&m, 1, 0).is_err());
    }
}
