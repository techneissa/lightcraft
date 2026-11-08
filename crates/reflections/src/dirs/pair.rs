//! DIRS-PAIR's wide block (`DualAttentionInteractiveBlock` in mingcv/DIRS `dirs_pair_arch.py`): window
//! self-attention on each stream plus a "layered" cross attention over the two streams' windows side by side,
//! no window shift, then the local feed-forward exchange. The windows are 12×12 and the feature map is padded
//! with zeros to a multiple of 12, as the released inference path does.
use super::DirsNet;
use crate::ops::{R, layer_norm, linear};
use crate::swin::{window_partition, window_reverse};
use candle_core::Tensor;
use candle_nn::VarBuilder;

const WINDOW: usize = 12;
/// Attention scores held at once, in elements (the full-resolution stage has thousands of windows).
const SCORE_BUDGET: usize = 48 << 20;

impl DirsNet {
    pub(super) fn attention_block(&self, x: &Tensor, y: &Tensor, p: &str, c: usize) -> R<(Tensor, Tensor)> {
        let (_, _, h, w) = x.dims4()?;
        let vb = self.vb.pp(p);
        let heads = (c / self.cfg.dim).clamp(1, 8);
        let tokens = |t: &Tensor| t.reshape((c, h * w))?.t()?.contiguous();
        let (xs, ys) = (tokens(x)?, tokens(y)?);
        let (pad_r, pad_b) = ((WINDOW - w % WINDOW) % WINDOW, (WINDOW - h % WINDOW) % WINDOW);
        let (hp, wp) = (h + pad_b, w + pad_r);
        let norm = vb.pp("norm1.seq.0");
        let windows = |t: &Tensor| -> R<Tensor> {
            let grid = layer_norm(&norm, t, c)?.reshape((h, w, c))?;
            window_partition(&grid.pad_with_zeros(0, 0, pad_b)?.pad_with_zeros(1, 0, pad_r)?, WINDOW)
        };
        let (xw, yw) = (windows(&xs)?, windows(&ys)?);
        let nw = xw.dim(0)?;
        let n = WINDOW * WINDOW;
        let own = self.window_attention(&vb.pp("dsia_sa"), &Tensor::cat(&[&xw, &yw], 0)?, c, (heads, 1), SCORE_BUDGET)?;
        let cross = self.window_attention(&vb.pp("dsia_ca"), &Tensor::cat(&[&xw, &yw], 1)?, c, (heads, 2), SCORE_BUDGET)?;
        let x_mix = (own.narrow(0, 0, nw)? + cross.narrow(1, 0, n)?)?;
        let y_mix = (own.narrow(0, nw, nw)? + cross.narrow(1, n, n)?)?;
        let back = |t: &Tensor| -> R<Tensor> { window_reverse(t, WINDOW, hp, wp, c)?.narrow(0, 0, h)?.narrow(1, 0, w)?.reshape((h * w, c)) };
        let a = vb.get((1, 1, c), "a")?.reshape((1, c))?;
        let image = |t: &Tensor| t.t()?.contiguous()?.reshape((1, c, h, w));
        let x_skip = image(&(xs + back(&x_mix)?.broadcast_mul(&a)?)?)?;
        let y_skip = image(&(ys + back(&y_mix)?.broadcast_mul(&a)?)?)?;
        let (fx, fy) = self.block1(&vb.pp("feedforward"), &x_skip, &y_skip, c)?;
        let b = vb.get((1, c, 1, 1), "b")?;
        Ok(((x_skip + fx.broadcast_mul(&b)?)?, (y_skip + fy.broadcast_mul(&b)?)?))
    }

    /// `WindowAttention` (`layers` 1) or `LayeredWindowAttention` (`layers` 2) on `[batch, tokens, c]`, in chunks of
    /// windows so that at most `budget` attention scores exist at once.
    pub(super) fn window_attention(&self, vb: &VarBuilder<'static>, x: &Tensor, c: usize, spec: (usize, usize), budget: usize) -> R<Tensor> {
        let (heads, layers) = spec;
        let (batch, n, _) = x.dims3()?;
        let hd = c / heads;
        let bias = self.relative_bias(vb, heads, layers)?;
        let per_window = (heads * n * n).max(1);
        let step = (budget / per_window).clamp(1, batch.max(1));
        let mut parts = Vec::with_capacity(batch.div_ceil(step));
        let mut start = 0;
        while start < batch {
            let len = step.min(batch - start);
            let part = x.narrow(0, start, len)?;
            let qkv = linear(&vb.pp("qkv"), &part.reshape((len * n, c))?, c, c * 3, true)?
                .reshape((len, n, 3, heads, hd))?
                .permute((2, 0, 3, 1, 4))?
                .contiguous()?;
            let q = (qkv.get(0)? * (hd as f64).powf(-0.5))?;
            let attn = q.matmul(&qkv.get(1)?.transpose(2, 3)?.contiguous()?)?.broadcast_add(&bias)?;
            let attn = candle_nn::ops::softmax_last_dim(&attn)?;
            let out = attn.matmul(&qkv.get(2)?)?.transpose(1, 2)?.contiguous()?.reshape((len * n, c))?;
            parts.push(linear(&vb.pp("proj"), &out, c, c, true)?.reshape((len, n, c))?);
            start += len;
        }
        if let [only] = parts.as_slice() { Ok(only.clone()) } else { Tensor::cat(&parts.iter().collect::<Vec<_>>(), 0) }
    }

    /// The learned relative-position bias as `[1, heads, n, n]`, `n = layers * 144`. For two layers the table has
    /// three blocks (layer offset -1, 0, +1), each laid out as in Swin. The checkpoint's own index buffer is not read.
    pub(super) fn relative_bias(&self, vb: &VarBuilder<'static>, heads: usize, layers: usize) -> R<Tensor> {
        let side = 2 * WINDOW - 1;
        let rows = side * side * if layers == 2 { 3 } else { 1 };
        let table = vb.get((rows, heads), "relative_position_bias_table")?;
        let tokens: Vec<(usize, usize, usize)> =
            (0..layers).flat_map(|l| (0..WINDOW).flat_map(move |y| (0..WINDOW).map(move |x| (l, y, x)))).collect();
        let mut index = Vec::with_capacity(tokens.len() * tokens.len());
        for a in &tokens {
            for b in &tokens {
                let layer = if layers == 2 { (a.0 + 1 - b.0) * side * side } else { 0 };
                index.push((layer + (a.1 + WINDOW - 1 - b.1) * side + (a.2 + WINDOW - 1 - b.2)) as u32);
            }
        }
        let count = tokens.len();
        let index = Tensor::from_vec(index, count * count, &self.device)?;
        table.index_select(&index, 0)?.reshape((count, count, heads))?.permute((2, 0, 1))?.contiguous()?.unsqueeze(0)
    }
}
