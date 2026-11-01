//! The Swin Transformer prior DIRS feeds on (Swin-L, Objects365 detection weights): a Rust port
//! of the backbone in mingcv/DIRS `xreflection/archs/swin_det.py` (see the crate docs for licences).
//! Only the inference path: no dropout, no stochastic depth, features of all four stages.
use crate::ops::{R, conv, layer_norm, linear};
use candle_core::{D, Device, Tensor};
use candle_nn::VarBuilder;

#[derive(Clone, Debug)]
pub(crate) struct SwinConfig {
    pub embed: usize,
    pub depths: [usize; 4],
    pub heads: [usize; 4],
    pub window: usize,
}

impl SwinConfig {
    /// Swin-L at 384 px with a 12×12 window: the weights `swin_large_o365.pth`.
    pub(crate) fn large() -> Self {
        Self { embed: 192, depths: [2, 2, 18, 2], heads: [6, 12, 24, 48], window: 12 }
    }
}

pub(crate) struct Swin {
    vb: VarBuilder<'static>,
    cfg: SwinConfig,
    device: Device,
}

impl Swin {
    pub(crate) fn new(vb: VarBuilder<'static>, cfg: SwinConfig, device: Device) -> Self {
        Self { vb, cfg, device }
    }

    /// `img` is `[1, 3, h, w]` with h and w multiples of 4. Returns the four stage outputs
    /// (strides 4, 8, 16, 32; 1×, 2×, 4×, 8× the embedding width) as `[1, c, h, w]`.
    pub(crate) fn forward(&self, img: &Tensor) -> R<Vec<Tensor>> {
        let (_, three, h, w) = img.dims4()?;
        if three != 3 || h == 0 || w == 0 || h % 4 != 0 || w % 4 != 0 {
            candle_core::bail!("the Swin prior needs a 3-channel image with sides divisible by 4");
        }
        let e = self.cfg.embed;
        let x = conv(&self.vb.pp("patch_embed.proj"), img, (3, e, 4), 4, 0)?;
        let (mut gh, mut gw) = (h / 4, w / 4);
        let mut tokens = x.reshape((e, gh * gw))?.t()?.contiguous()?;
        tokens = layer_norm(&self.vb.pp("patch_embed.norm"), &tokens, e)?;
        let mut outs = Vec::with_capacity(4);
        for stage in 0..4 {
            let dim = e << stage;
            for block in 0..self.cfg.depths[stage] {
                let shift = if block % 2 == 1 { self.cfg.window / 2 } else { 0 };
                tokens = self.block(&tokens, (gh, gw), &format!("layers.{stage}.blocks.{block}"), (dim, self.cfg.heads[stage], shift))?;
            }
            outs.push(tokens.t()?.contiguous()?.reshape((1, dim, gh, gw))?);
            if stage < 3 {
                tokens = self.merge(&tokens, (gh, gw), &format!("layers.{stage}.downsample"), dim)?;
                gh = gh.div_ceil(2);
                gw = gw.div_ceil(2);
            }
        }
        Ok(outs)
    }

    fn block(&self, x: &Tensor, grid: (usize, usize), p: &str, spec: (usize, usize, usize)) -> R<Tensor> {
        let (gh, gw) = grid;
        let (dim, heads, shift) = spec;
        let ws = self.cfg.window;
        let vb = self.vb.pp(p);
        let normed = layer_norm(&vb.pp("norm1"), x, dim)?.reshape((gh, gw, dim))?;
        let (pad_r, pad_b) = ((ws - gw % ws) % ws, (ws - gh % ws) % ws);
        let (hp, wp) = (gh + pad_b, gw + pad_r);
        let mut g = normed.pad_with_zeros(0, 0, pad_b)?.pad_with_zeros(1, 0, pad_r)?;
        if shift > 0 {
            g = g.roll(-(shift as i32), 0)?.roll(-(shift as i32), 1)?;
        }
        let windows = window_partition(&g, ws)?; // [nW, ws*ws, c]
        let mask = if shift > 0 { Some(self.mask(hp, wp, shift)?) } else { None };
        let attended = self.attention(&vb.pp("attn"), &windows, dim, heads, mask.as_ref())?;
        let mut g = window_reverse(&attended, ws, hp, wp, dim)?;
        if shift > 0 {
            g = g.roll(shift as i32, 0)?.roll(shift as i32, 1)?;
        }
        let g = g.narrow(0, 0, gh)?.narrow(1, 0, gw)?.reshape((gh * gw, dim))?;
        let x = (x + g)?;
        let mlp = vb.pp("mlp");
        let hidden = linear(&mlp.pp("fc1"), &layer_norm(&vb.pp("norm2"), &x, dim)?, dim, dim * 4, true)?.gelu_erf()?;
        x + linear(&mlp.pp("fc2"), &hidden, dim * 4, dim, true)?
    }

    fn attention(&self, vb: &VarBuilder<'static>, windows: &Tensor, dim: usize, heads: usize, mask: Option<&Tensor>) -> R<Tensor> {
        let ws = self.cfg.window;
        let n = ws * ws;
        let nw = windows.dim(0)?;
        let hd = dim / heads;
        let qkv = linear(&vb.pp("qkv"), &windows.reshape((nw * n, dim))?, dim, dim * 3, true)?
            .reshape((nw, n, 3, heads, hd))?
            .permute((2, 0, 3, 1, 4))?
            .contiguous()?;
        let q = (qkv.get(0)? * (hd as f64).powf(-0.5))?;
        let k = qkv.get(1)?;
        let v = qkv.get(2)?;
        let mut attn = q.matmul(&k.transpose(2, 3)?.contiguous()?)?;
        attn = attn.broadcast_add(&self.relative_bias(vb, heads)?)?;
        if let Some(mask) = mask {
            attn = attn.broadcast_add(mask)?;
        }
        let attn = candle_nn::ops::softmax_last_dim(&attn)?;
        let out = attn.matmul(&v)?.transpose(1, 2)?.contiguous()?.reshape((nw * n, dim))?;
        linear(&vb.pp("proj"), &out, dim, dim, true)?.reshape((nw, n, dim))
    }

    /// The learned relative-position bias as `[1, heads, n, n]` (the checkpoint's own index buffer is not read).
    fn relative_bias(&self, vb: &VarBuilder<'static>, heads: usize) -> R<Tensor> {
        let ws = self.cfg.window;
        let side = 2 * ws - 1;
        let table = vb.get((side * side, heads), "relative_position_bias_table")?;
        let mut index = Vec::with_capacity(ws.pow(4));
        for (ay, ax) in (0..ws).flat_map(|y| (0..ws).map(move |x| (y, x))) {
            for (by, bx) in (0..ws).flat_map(|y| (0..ws).map(move |x| (y, x))) {
                index.push(((ay + ws - 1 - by) * side + (ax + ws - 1 - bx)) as u32);
            }
        }
        let index = Tensor::from_vec(index, ws.pow(4), &self.device)?;
        table.index_select(&index, 0)?.reshape((ws * ws, ws * ws, heads))?.permute((2, 0, 1))?.contiguous()?.unsqueeze(0)
    }

    /// The additive mask of shifted windows: -100 between tokens that came from different regions.
    fn mask(&self, hp: usize, wp: usize, shift: usize) -> R<Tensor> {
        let ws = self.cfg.window;
        let region = |v: usize, size: usize| usize::from(v >= size - ws) + usize::from(v >= size - shift);
        let (wh, ww) = (hp / ws, wp / ws);
        let n = ws * ws;
        let mut ids = vec![0u8; hp * wp];
        for y in 0..hp {
            for x in 0..wp {
                ids[y * wp + x] = (region(y, hp) * 3 + region(x, wp)) as u8;
            }
        }
        let mut data = Vec::with_capacity(wh * ww * n * n);
        for win_y in 0..wh {
            for win_x in 0..ww {
                let id = |i: usize| ids[(win_y * ws + i / ws) * wp + win_x * ws + i % ws];
                for i in 0..n {
                    for j in 0..n {
                        data.push(if id(i) == id(j) { 0.0f32 } else { -100.0 });
                    }
                }
            }
        }
        Tensor::from_vec(data, (wh * ww, 1, n, n), &self.device)
    }

    fn merge(&self, x: &Tensor, grid: (usize, usize), p: &str, dim: usize) -> R<Tensor> {
        let (gh, gw) = grid;
        let vb = self.vb.pp(p);
        let g = x.reshape((gh, gw, dim))?.pad_with_zeros(0, 0, gh % 2)?.pad_with_zeros(1, 0, gw % 2)?;
        let (h2, w2) = (g.dim(0)? / 2, g.dim(1)? / 2);
        let g = g.reshape((h2, 2, w2, 2, dim))?;
        let part = |a: usize, b: usize| g.narrow(1, a, 1)?.narrow(3, b, 1)?.reshape((h2 * w2, dim));
        let merged = Tensor::cat(&[&part(0, 0)?, &part(1, 0)?, &part(0, 1)?, &part(1, 1)?], D::Minus1)?;
        linear(&vb.pp("reduction"), &layer_norm(&vb.pp("norm"), &merged, dim * 4)?, dim * 4, dim * 2, false)
    }
}

/// `[hp, wp, c]` to `[nW, ws*ws, c]`.
fn window_partition(g: &Tensor, ws: usize) -> R<Tensor> {
    let (hp, wp, c) = g.dims3()?;
    g.reshape((hp / ws, ws, wp / ws, ws, c))?.permute((0, 2, 1, 3, 4))?.contiguous()?.reshape(((hp / ws) * (wp / ws), ws * ws, c))
}

/// `[nW, ws*ws, c]` back to `[hp, wp, c]`.
fn window_reverse(w: &Tensor, ws: usize, hp: usize, wp: usize, c: usize) -> R<Tensor> {
    w.reshape((hp / ws, wp / ws, ws, ws, c))?.permute((0, 2, 1, 3, 4))?.contiguous()?.reshape((hp, wp, c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swin() -> Swin {
        let vb = VarBuilder::zeros(candle_core::DType::F32, &Device::Cpu);
        Swin::new(vb, SwinConfig { embed: 8, depths: [1, 1, 1, 1], heads: [1, 1, 1, 1], window: 4 }, Device::Cpu)
    }

    #[test]
    fn shifted_window_mask_separates_the_wrapped_regions() {
        let s = swin();
        // 8×8 tokens, window 4, shift 2: four windows; the top-left one is a single region.
        let mask = s.mask(8, 8, 2).unwrap().reshape((4, 16, 16)).unwrap().to_vec3::<f32>().unwrap();
        assert!(mask[0].iter().flatten().all(|v| *v == 0.0));
        // Bottom-right window: rows 0-1 / 2-3 and columns 0-1 / 2-3 are four regions of four tokens.
        let br = &mask[3];
        for i in 0..16 {
            for j in 0..16 {
                let (ri, rj) = ((i / 4 / 2, i % 4 / 2), (j / 4 / 2, j % 4 / 2));
                assert_eq!(br[i][j], if ri == rj { 0.0 } else { -100.0 }, "token pair {i},{j}");
            }
        }
        // The top-right window splits by columns only, the bottom-left one by rows only.
        assert_eq!(mask[1][0][3], -100.0);
        assert_eq!(mask[1][0][1], 0.0);
        assert_eq!(mask[2][0][12], -100.0);
        assert_eq!(mask[2][0][3], 0.0);
    }

    #[test]
    fn windows_partition_and_reverse_are_inverse() {
        let g = Tensor::arange(0f32, (8 * 12 * 3) as f32, &Device::Cpu).unwrap().reshape((8, 12, 3)).unwrap();
        let windows = window_partition(&g, 4).unwrap();
        assert_eq!(windows.dims(), &[6, 16, 3]);
        let back = window_reverse(&windows, 4, 8, 12, 3).unwrap();
        assert_eq!(back.flatten_all().unwrap().to_vec1::<f32>().unwrap(), g.flatten_all().unwrap().to_vec1::<f32>().unwrap());
        // Window 1 starts at column 4 of the first row.
        assert_eq!(windows.get(1).unwrap().get(0).unwrap().to_vec1::<f32>().unwrap(), vec![12.0, 13.0, 14.0]);
    }

    #[test]
    fn relative_bias_indexes_the_table_by_offset() {
        let s = swin();
        let side = 2 * 4 - 1;
        let table = Tensor::arange(0f32, (side * side) as f32, &Device::Cpu).unwrap().reshape((side * side, 1)).unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert("relative_position_bias_table".to_string(), table);
        let vb = VarBuilder::from_tensors(map, candle_core::DType::F32, &Device::Cpu);
        let bias = s.relative_bias(&vb, 1).unwrap().reshape((16, 16)).unwrap().to_vec2::<f32>().unwrap();
        // Same token: offset (0,0) is the table's centre; token (0,1) against (0,0) is one column to the right.
        assert_eq!(bias[0][0], (3 * side + 3) as f32);
        assert_eq!(bias[1][0], (3 * side + 4) as f32);
        assert_eq!(bias[4][0], (4 * side + 3) as f32);
        assert_eq!(bias[0][5], (2 * side + 2) as f32);
    }
}
