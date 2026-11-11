//! Sapiens2 through Core ML on the GPU (issue #216, `fork/coreml.md`), converted once by
//! `tools/convert-coreml-sapiens2.py` into three models: the transformer ("backbone") and the decoder head on two
//! canvases.
//!
//! Core ML plans a model on the GPU again whenever its input shape changes (issue #200: for the backbone about 1.7 s),
//! and Sapiens2 runs on a box around each person, each of another size. So each model takes one fixed shape and a
//! call holds several passes:
//!
//! - the backbone takes a fixed number of rows ([`BACKBONES`]: one package with a function for each, sharing the
//!   weights; a call takes the smallest that holds it), the passes' tokens one after another (each pass's class and
//!   storage tokens, then its patches with their rotary sin and cos), each row carrying its pass's number: a token
//!   attends only to its own pass's tokens, so each pass gives what it gives alone;
//! - the head takes the passes' features laid out on a canvas of patches ([`HEAD_CANVASES`]: the smallest that holds
//!   them), each pass in its own rectangle with an empty patch between two of them and a channel of its own marking
//!   it; every instance norm takes each pass's own statistics and every layer sees zeros past a pass's edge, so each
//!   pass's part of the canvas equals the head run on that pass alone.
//!
//! A pass too large for the canvas is made smaller to fit ([`fit`]); the boxes `crops.rs` makes and the whole photo's
//! pass of photos up to about 2:1 fit as they are.

use std::ops::Range;
use std::path::Path;

use candle_core::{Device, Tensor};
use lightcraft_coreml::{ComputeUnits, Error, Model};

use crate::crops::Pass;
use crate::models::BODY_SAPIENS2;
use crate::sapiens::{self, CLASSES, HEAD_DIM, PATCH, PREFIX};

/// Rows a backbone call holds: the largest pass `crops.rs` makes (1.05 × 1024 × 768 pixels: 3225 patches) with its
/// class and storage tokens, rounded up to a multiple of 64.
pub const TOKENS: usize = 3264;
/// The backbone's functions (rows, name), smallest first: one that holds a whole photo's pass alone (about half the
/// trained size, at most 1600 patches: `fuse.rs` `input_size`), at about half the time of the largest, and [`TOKENS`].
pub const BACKBONES: [(usize, &str); 2] = [(1664, "tokens1664"), (TOKENS, "tokens3264")];
/// Most passes a call holds (the whole photo's and [`crate::crops::MAX_CROPS`] people's).
pub const MAX_PASSES: usize = 9;
/// The head's canvases in patches (height, width), smallest first: one that holds a whole photo's pass of 3:2 either
/// way round (48 × 32), and a tall one for the boxes around people.
pub const HEAD_CANVASES: [(usize, usize); 2] = [(48, 48), (96, 64)];
/// The 0.4B model's width (the backbone's output channels).
const WIDTH: usize = 1024;
/// A row's numbers: the patch's pixels (row, column, channel), which prefix token it is instead (one-hot), the
/// rotary sin and cos, the pass's number.
const PX: usize = PATCH * PATCH * 3;
const ROW: usize = PX + PREFIX + 2 * HEAD_DIM + 1;
/// The pass number of padding rows: one no pass has.
const PADDING: f32 = 1000.0;
const INPUT: &str = "pixels";
const BACKBONE_OUTPUT: &str = "tokens";
const HEAD_OUTPUT: &str = "logits";

/// The names of Sapiens2's Core ML packages: the backbone, then the head for each of [`HEAD_CANVASES`].
pub(crate) fn names() -> Vec<String> {
    let f = BODY_SAPIENS2.folder;
    let mut v = vec![format!("{f}-backbone")];
    v.extend(HEAD_CANVASES.iter().map(|(h, w)| format!("{f}-head-{h}x{w}")));
    v
}

/// Sapiens2's Core ML models (each function of the backbone, each head) and the rotary periods of its weights.
pub(crate) struct Models {
    backbones: Vec<(usize, Model)>,
    heads: Vec<((usize, usize), Model)>,
    periods: Vec<f32>,
}

impl Models {
    pub(crate) fn with_periods(self, periods: Vec<f32>) -> Self {
        Self { periods, ..self }
    }
}

/// Load the three models, on the GPU: the Neural Engine ran the backbone in 1.43 s against 0.75 s on the GPU and was
/// further from float32 (97 % of pixels the same class against 99.9 %).
pub(crate) fn load(models: &Path) -> Result<Models, Error> {
    let names = names();
    let (first, rest) = names.split_first().ok_or_else(|| Error::Load("no Sapiens2 models".into()))?;
    let backbones = BACKBONES
        .iter()
        .map(|&(n, f)| Ok((n, super::load_named(models, &BODY_SAPIENS2, first, Some(f), BACKBONE_OUTPUT, ComputeUnits::CpuAndGpu)?)))
        .collect::<Result<Vec<_>, Error>>()?;
    let heads = HEAD_CANVASES
        .iter()
        .zip(rest)
        .map(|(&c, n)| Ok((c, super::load_named(models, &BODY_SAPIENS2, n, None, HEAD_OUTPUT, ComputeUnits::CpuAndGpu)?)))
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(Models { backbones, heads, periods: Vec::new() })
}

/// The input a pass wanted at `w × h` pixels gets: the same when the largest canvas and a call hold it, else smaller,
/// in the same shape, until they do.
pub(crate) fn fit(w: usize, h: usize) -> (usize, usize) {
    let (gw, gh) = ((w / PATCH).max(1), (h / PATCH).max(1));
    let (ch, cw) = biggest();
    let s = (ch as f32 / gh as f32).min(cw as f32 / gw as f32).min(((TOKENS - PREFIX) as f32 / (gw * gh) as f32).sqrt());
    if s >= 1.0 {
        return (w, h);
    }
    let side = |g: usize| ((g as f32 * s).floor() as usize).max(2) * PATCH;
    (side(gw), side(gh))
}

fn biggest() -> (usize, usize) {
    HEAD_CANVASES.last().copied().unwrap_or((0, 0))
}

/// Where passes of `sizes` (patches, height × width) go on a canvas of `(height, width)` patches, in rows of passes
/// side by side with an empty patch between two of them (row, column of each); `None` when they don't fit.
fn shelf(sizes: &[(usize, usize)], (ch, cw): (usize, usize)) -> Option<Vec<(usize, usize)>> {
    let (mut x, mut y, mut tall) = (0, 0, 0);
    let mut out = Vec::with_capacity(sizes.len());
    for &(gh, gw) in sizes {
        if gw > cw {
            return None;
        }
        if x > 0 && x + gw > cw {
            (x, y, tall) = (0, y + tall + 1, 0);
        }
        if y + gh > ch {
            return None;
        }
        out.push((y, x));
        x += gw + 1;
        tall = tall.max(gh);
    }
    Some(out)
}

/// The passes of `sizes`, in order, cut into calls: each within [`TOKENS`], [`MAX_PASSES`] and the largest canvas.
fn calls(sizes: &[(usize, usize)]) -> Result<Vec<Range<usize>>, Error> {
    let mut out = Vec::new();
    let (mut start, mut tokens) = (0, 0);
    for (i, &(gh, gw)) in sizes.iter().enumerate() {
        let need = gh * gw + PREFIX;
        if need > TOKENS || shelf(&[(gh, gw)], biggest()).is_none() {
            return Err(Error::Input(format!("a Sapiens2 pass of {gw} × {gh} patches does not fit a call")));
        }
        let fits = i - start < MAX_PASSES && tokens + need <= TOKENS && sizes.get(start..=i).and_then(|s| shelf(s, biggest())).is_some();
        if !fits {
            out.push(start..i);
            (start, tokens) = (i, 0);
        }
        tokens += need;
    }
    if start < sizes.len() {
        out.push(start..sizes.len());
    }
    Ok(out)
}

/// Each pass's class probabilities on its grid (class-major), as candle's [`crate::crops::OnCandle`] gives them;
/// fewer when `cancelled` (asked between calls).
pub(crate) fn probs(m: &Models, passes: &[Pass], cancelled: &dyn Fn() -> bool) -> Result<Vec<Vec<f32>>, Error> {
    if m.periods.len() != HEAD_DIM / 4 {
        return Err(Error::Load(format!("Sapiens2 has {} rotary periods, not {}", m.periods.len(), HEAD_DIM / 4)));
    }
    for p in passes {
        if p.iw == 0 || p.ih == 0 || p.iw % PATCH != 0 || p.ih % PATCH != 0 || p.px.len() != 3 * p.iw * p.ih {
            return Err(Error::Input(format!("a Sapiens2 pass of {} numbers for {} × {}", p.px.len(), p.iw, p.ih)));
        }
    }
    let sizes: Vec<(usize, usize)> = passes.iter().map(|p| (p.ih / PATCH, p.iw / PATCH)).collect();
    let mut out = Vec::with_capacity(passes.len());
    for range in calls(&sizes)? {
        if cancelled() {
            break;
        }
        let (Some(group), Some(at)) = (passes.get(range.clone()), sizes.get(range)) else { break };
        out.extend(call(m, group, at)?);
    }
    Ok(out)
}

/// One backbone call and one head call for `passes` (of `sizes` patches).
fn call(m: &Models, passes: &[Pass], sizes: &[(usize, usize)]) -> Result<Vec<Vec<f32>>, Error> {
    let need: usize = sizes.iter().map(|(h, w)| h * w + PREFIX).sum();
    let (tokens, backbone) =
        m.backbones.iter().find(|(n, _)| *n >= need).ok_or_else(|| Error::Input(format!("{need} Sapiens2 tokens do not fit a call")))?;
    let (x, starts) = rows(passes, sizes, &m.periods, *tokens);
    crate::profile::mark("sapiens2 rows");
    let t = backbone.predict(INPUT, &[1, *tokens, ROW], &x, BACKBONE_OUTPUT)?;
    crate::profile::mark("sapiens2 backbone");
    if t.shape != [1, *tokens, WIDTH] || t.data.len() != tokens * WIDTH {
        return Err(Error::Output(format!("the Sapiens2 backbone gave {:?}, not [1, {tokens}, {WIDTH}]", t.shape)));
    }
    let ((ch, cw), head, places) = m
        .heads
        .iter()
        .find_map(|(c, h)| shelf(sizes, *c).map(|p| (*c, h, p)))
        .ok_or_else(|| Error::Input("the Sapiens2 passes do not fit the head's canvas".into()))?;
    let x = canvas(&t.data, &starts, sizes, &places, (ch, cw));
    crate::profile::mark("sapiens2 canvas");
    let out = head.predict(INPUT, &[1, WIDTH + MAX_PASSES, ch, cw], &x, HEAD_OUTPUT)?;
    crate::profile::mark("sapiens2 head");
    let (oh, ow) = (ch * PATCH, cw * PATCH);
    if out.shape != [1, CLASSES, oh, ow] || out.data.len() != CLASSES * oh * ow {
        return Err(Error::Output(format!("the Sapiens2 head gave {:?}, not [1, {CLASSES}, {oh}, {ow}]", out.shape)));
    }
    passes
        .iter()
        .zip(sizes)
        .zip(&places)
        .map(|((p, &(gh, gw)), &(r, c))| {
            let (ih, iw) = (gh * PATCH, gw * PATCH);
            let mut v = Vec::with_capacity(CLASSES * ih * iw);
            for k in 0..CLASSES {
                for y in 0..ih {
                    let at = (k * oh + r * PATCH + y) * ow + c * PATCH;
                    v.extend_from_slice(out.data.get(at..at + iw).ok_or_else(|| Error::Output("a pass outside the head's canvas".into()))?);
                }
            }
            let logits = Tensor::from_vec(v, (1, CLASSES, ih, iw), &Device::Cpu).map_err(|e| Error::Output(e.to_string()))?;
            crate::crops::probs_of(&logits, p.w, p.h).map_err(|e| Error::Output(e.to_string()))
        })
        .collect()
}

/// The backbone's input for `passes` (`tokens × ROW`, enough for them), and the row of each pass's first patch.
fn rows(passes: &[Pass], sizes: &[(usize, usize)], periods: &[f32], tokens: usize) -> (Vec<f32>, Vec<usize>) {
    let rope = PX + PREFIX;
    let mut x = vec![0.0f32; tokens * ROW];
    for row in x.as_chunks_mut::<ROW>().0 {
        // padding, and the prefix tokens until written: not turned (cos 1, sin 0), a pass of their own
        if let Some(cos) = row.get_mut(rope + HEAD_DIM..rope + 2 * HEAD_DIM) {
            cos.fill(1.0);
        }
        if let Some(n) = row.last_mut() {
            *n = PADDING;
        }
    }
    let mut starts = Vec::with_capacity(passes.len());
    let mut at = 0;
    for (k, (p, &(gh, gw))) in passes.iter().zip(sizes).enumerate() {
        let mut planes = p.px.clone();
        let n = p.iw * p.ih;
        crate::image::imagenet(&mut planes, n);
        let (sin, cos) = sapiens::rope_table(periods, gh, gw);
        for j in 0..PREFIX {
            if let Some(row) = x.get_mut((at + j) * ROW..(at + j + 1) * ROW) {
                if let Some(v) = row.get_mut(PX + j) {
                    *v = 1.0;
                }
                if let Some(v) = row.last_mut() {
                    *v = k as f32;
                }
            }
        }
        let first = at + PREFIX;
        for t in 0..gh * gw {
            let Some(row) = x.get_mut((first + t) * ROW..(first + t + 1) * ROW) else { break };
            let (py, pxx) = (t / gw, t % gw);
            for ky in 0..PATCH {
                for kx in 0..PATCH {
                    let src = (py * PATCH + ky) * p.iw + pxx * PATCH + kx;
                    for c in 0..3 {
                        if let (Some(d), Some(&v)) = (row.get_mut((ky * PATCH + kx) * 3 + c), planes.get(c * n + src)) {
                            *d = v;
                        }
                    }
                }
            }
            if let (Some(d), Some(s)) = (row.get_mut(rope..rope + HEAD_DIM), sin.get(t * HEAD_DIM..(t + 1) * HEAD_DIM)) {
                d.copy_from_slice(s);
            }
            if let (Some(d), Some(s)) = (row.get_mut(rope + HEAD_DIM..rope + 2 * HEAD_DIM), cos.get(t * HEAD_DIM..(t + 1) * HEAD_DIM)) {
                d.copy_from_slice(s);
            }
            if let Some(v) = row.last_mut() {
                *v = k as f32;
            }
        }
        starts.push(first);
        at = first + gh * gw;
    }
    (x, starts)
}

/// The head's input: each pass's features (`tokens`, a row of `WIDTH` each, its patches from row `starts[k]`) at its place
/// on a canvas of `(ch, cw)` patches, channel-major, then a channel per pass marking its patches.
fn canvas(tokens: &[f32], starts: &[usize], sizes: &[(usize, usize)], places: &[(usize, usize)], (ch, cw): (usize, usize)) -> Vec<f32> {
    let plane = ch * cw;
    let mut x = vec![0.0f32; (WIDTH + MAX_PASSES) * plane];
    for (k, ((&first, &(gh, gw)), &(r, c))) in starts.iter().zip(sizes).zip(places).enumerate() {
        for t in 0..gh * gw {
            let cell = (r + t / gw) * cw + c + t % gw;
            if let Some(tok) = tokens.get((first + t) * WIDTH..(first + t + 1) * WIDTH) {
                for (ci, &v) in tok.iter().enumerate() {
                    if let Some(d) = x.get_mut(ci * plane + cell) {
                        *d = v;
                    }
                }
            }
            if let Some(d) = x.get_mut((WIDTH + k) * plane + cell) {
                *d = 1.0;
            }
        }
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_in_a_call_never_touch_and_every_pass_fits_one() {
        // the shapes `crops.rs` makes: a whole photo of 3:2 either way, the largest tall box, small boxes
        let sizes = [(32, 48), (48, 32), (82, 38), (24, 12), (24, 12), (30, 16), (40, 20), (24, 12), (24, 12), (24, 12), (24, 12)];
        let cut = calls(&sizes).unwrap();
        assert_eq!(cut.iter().map(|r| r.len()).sum::<usize>(), sizes.len());
        assert!(cut.windows(2).all(|w| w[0].end == w[1].start), "in order: {cut:?}");
        for r in cut {
            let group = &sizes[r];
            assert!(group.len() <= MAX_PASSES && group.iter().map(|(h, w)| h * w + PREFIX).sum::<usize>() <= TOKENS);
            let canvas = HEAD_CANVASES.iter().copied().find(|&c| shelf(group, c).is_some()).unwrap();
            let at = shelf(group, canvas).unwrap();
            for (i, (&(h, w), &(y, x))) in group.iter().zip(&at).enumerate() {
                assert!(y + h <= canvas.0 && x + w <= canvas.1, "{group:?} on {canvas:?}");
                for (&(h2, w2), &(y2, x2)) in group.iter().zip(&at).skip(i + 1) {
                    // grown by the empty patch between them, two passes do not overlap
                    let apart = y + h < y2 || y2 + h2 < y || x + w < x2 || x2 + w2 < x;
                    assert!(apart, "{:?} and {:?} touch", (y, x, h, w), (y2, x2, h2, w2));
                }
            }
        }
        // a whole photo's pass of 3:2 takes the small canvas alone
        assert_eq!(shelf(&[(32, 48)], HEAD_CANVASES[0]), Some(vec![(0, 0)]));
        // too large for any canvas: made smaller in its own shape, and the same when it fits
        assert_eq!(fit(512, 768), (512, 768));
        let (w, h) = fit(1600, 480);
        assert!(w / PATCH <= 64 && h / PATCH <= 96 && w % PATCH == 0 && (w as f32 / h as f32 - 1600.0 / 480.0).abs() < 0.2, "{w} × {h}");
        assert!(calls(&[(100, 10)]).is_err());
    }
}
