//! Mask evaluation on the device (`lightcraft_pipeline::masks`): each visible mask's alpha plane
//! and its effect terms.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lightcraft_develop::BrushStroke;
use lightcraft_pipeline::Plan;
use lightcraft_pipeline::finish::{MASK_TERMS, effect_terms_with};
use lightcraft_pipeline::geometry::Frame;
use lightcraft_pipeline::masks::grid::{PosGrid, Ramp};
use lightcraft_pipeline::masks::{
    AlphaKeys, BRUSH_ENTRIES, BrushCache, BrushCtx, Mattes, alpha_key, brush_base, brush_chain, brush_dabs, held_brush, note_evaluation, note_stroke,
    rasterize_brush,
};
use lightcraft_raster::Rgb32f;

use crate::ctx::{Buf, groups1, groups2};
use crate::render::{Cx, Host, Prep, gaussian_at, map};

pub(crate) mod bands;

/// The alphas of the last render's masks and the brush parts they were built from, on the device: the
/// GPU twin of `lightcraft_pipeline::masks::AlphaCache`, kept next to the view's other stages. An
/// effect slider, the Amount or Exposure for a gradient or a brush changes nothing a mask's alpha is built
/// from, so no mask is evaluated again (each alpha is copied into the render's stack of planes), and a
/// brush part extended by a stroke stamps only that stroke.
#[derive(Default)]
pub(crate) struct AlphaStore {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The source the alphas were evaluated on: a mask that reads the image (a range, Sky, Refine Edges, Auto Mask) keeps no
    /// alpha for another picture, whatever its settings say (`alpha_key`'s image half has no picture's identity). Weak: the
    /// store never keeps a photo alive, and a dropped source's address can't be taken again while this is held.
    source: Option<std::sync::Weak<Rgb32f>>,
    mattes: Option<Arc<Mattes>>,
    /// The alphas by key, with the frame (`plan.geo`) each was evaluated for: each a range of the stack of planes its
    /// render evaluated, not a copy of it.
    slots: Vec<Kept>,
    brushes: Vec<BrushPart>,
    /// The planes of brush parts with Auto Mask strokes, which the CPU stamps (they read the photo around each dab).
    cpu_brushes: BrushCache,
    /// The edges a stored selection follows (`masks::guide_plane` of the resampled photo) for the frame `plan.geo` keys.
    guide: Option<(u64, Arc<lightcraft_raster::Plane>)>,
}

/// A mask's alpha kept from a render: `len` values at `at` in that render's stack of alpha planes (the buffer the
/// finish stage read). The alphas of one render share its stack: they are held once, and a render with no store copies
/// none of them out.
#[derive(Clone)]
struct Kept {
    key: u64,
    /// The frame (`plan.geo`) it was evaluated for.
    geo: u64,
    stack: Arc<Buf>,
    at: usize,
    len: usize,
}

/// A brush part's plane and the strokes it holds (`masks::brush_chain`).
#[derive(Clone)]
struct BrushPart {
    base: u64,
    chain: Vec<u64>,
    plane: Arc<Buf>,
}

impl AlphaStore {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Drop everything.
    pub(crate) fn clear(&self) {
        *self.lock() = Inner::default();
    }

    /// Host memory held (the planes of Auto Mask brush parts).
    pub(crate) fn host_bytes(&self) -> usize {
        self.lock().cpu_brushes.bytes()
    }

    /// Hand every device buffer held to `add` with its part (the caller counts each once: the alphas of a render share
    /// its stack).
    pub(crate) fn each(&self, mut add: impl FnMut(AlphaPart, &Arc<Buf>)) {
        let g = self.lock();
        // (the last render's alphas come first, then the ones of earlier frames)
        let current = g.slots.first().map(|k| k.geo);
        g.slots.iter().for_each(|k| add(if Some(k.geo) == current { AlphaPart::Current } else { AlphaPart::Other }, &k.stack));
        g.brushes.iter().for_each(|p| add(AlphaPart::Brush, &p.plane));
    }
}

/// Which of a view's kept alphas a device buffer is ([`AlphaStore::each`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlphaPart {
    /// The last render's.
    Current,
    /// An earlier window's or scale's.
    Other,
    /// A brush part's plane.
    Brush,
}

/// Rows `y0..y0 + rows` of a render's `w × h` frame: what a mask is evaluated over (issue #406). A whole render's is
/// the whole frame; a band's ([`bands`]) is the band and its mask's reach around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cut {
    pub y0: usize,
    pub rows: usize,
}

impl Cut {
    pub(crate) fn whole(h: usize) -> Cut {
        Cut { y0: 0, rows: h }
    }

    pub(crate) fn end(self) -> usize {
        self.y0 + self.rows
    }

    /// Its rows of a plane of the whole frame, `w` wide.
    fn of(self, data: &[f32], w: usize) -> &[f32] {
        data.get(self.y0 * w..self.end() * w).unwrap_or(&[])
    }
}

/// The images a render's masks are evaluated on: its linear image, its resampled photo and the planes of `prep`.
pub(crate) struct Images<'a> {
    pub lin: &'a Buf,
    pub sampled: &'a Buf,
    pub prep: &'a Prep,
}

/// What a brush part is stamped against: rows `cut` of the frame, `w × h`.
struct BrushEnv<'a> {
    lin: &'a Buf,
    log_l: &'a Buf,
    frame: &'a Frame,
    w: usize,
    h: usize,
    cut: Cut,
    keys: AlphaKeys,
}

/// The plane of brush strokes `strokes` on the device (`masks::brush`) over the rows `env.cut`, or `None` when the device
/// can't hold their dabs (the CPU rasterizes the part). Strokes that extend a part held in `parts` are stamped over a copy of its plane; the
/// result is kept there when `keep` (a render with a store: one that keeps nothing copies nothing). `inverted`: the part is
/// turned around. Parts with Auto Mask strokes are not stamped here (`None`).
fn brush_part(cx: &mut Cx<'_>, env: &BrushEnv<'_>, strokes: &[BrushStroke], inverted: bool, parts: &mut Vec<BrushPart>, keep: bool) -> Option<Buf> {
    let n = env.w * env.cut.rows;
    if strokes.iter().any(|s| s.auto_mask) {
        return None;
    }
    // (a part held is the whole frame's: a band's is never held nor stamped over one)
    let whole = env.cut == Cut::whole(env.h);
    let base = brush_base(env.keys.geo, env.keys.image, false, env.w, env.h);
    let chain = brush_chain(strokes);
    // the part this one extends (the longest held prefix of its strokes)
    let hit = parts
        .iter()
        .enumerate()
        .filter(|(_, e)| whole && e.base == base && !e.chain.is_empty() && e.chain.len() <= chain.len() && chain.starts_with(&e.chain))
        .max_by_key(|(_, e)| e.chain.len())
        .map(|(i, _)| i);
    let (plane, from) = match hit {
        Some(i) => {
            // (the most recently used goes last)
            let e = parts.remove(i);
            parts.push(e.clone());
            (cx.copy(&e.plane), e.chain.len())
        }
        None => (cx.zeroed(n), 0),
    };
    for s in strokes.iter().skip(from) {
        if !stamp_stroke(cx, env, s, &plane) {
            return None;
        }
    }
    if keep && whole && !strokes.is_empty() && from < strokes.len() {
        parts.push(BrushPart { base, chain, plane: Arc::new(cx.copy(&plane)) });
        while parts.len() > BRUSH_ENTRIES {
            parts.remove(0);
        }
    }
    if inverted {
        // (the kernel's unused `c` binding is writable, so it can't be the unused read-only dummy as well)
        let unused = cx.gpu.buffer(1);
        cx.run("finalize", &[n as u32, 0, 0, 1, 1f32.to_bits()], &[None, None, None, Some(&unused), Some(&plane)], groups1(n));
    }
    Some(plane)
}

/// A brush part with Auto Mask strokes, stamped on the CPU (they read the photo around each dab, `masks::auto_edge`)
/// with its planes kept in `cache`, so a new stroke stamps only that stroke; the rows `env.cut` of it uploaded.
/// `inverted`: the part is turned around.
fn auto_brush_part(cx: &mut Cx<'_>, host: &mut Host, env: &BrushEnv<'_>, strokes: &[BrushStroke], inverted: bool, cache: &mut BrushCache) -> Buf {
    let (w, h) = (env.w, env.h);
    let mut ctx = BrushCtx { cache, geo: env.keys.geo, image: env.keys.image };
    // (a part held whole needs no picture: the read-back is only for strokes to stamp)
    let mut v = match held_brush(strokes, w, h, &mut ctx) {
        Some(v) => v,
        None => {
            let img = host.lin.get_or_insert_with(|| Arc::new(cx.read_rgb(env.lin, w, h))).clone();
            let l = host.log_l.get_or_insert_with(|| Arc::new(cx.read_plane(env.log_l, w, h))).clone();
            rasterize_brush(strokes, env.frame, w, h, &img, &l, Some(&mut ctx))
        }
    };
    if inverted {
        v.data.iter_mut().for_each(|x| *x = 1.0 - *x);
    }
    cx.gpu.upload(env.cut.of(&v.data, w))
}

/// Stamp one stroke (not Auto Mask) into `plane` (the rows `env.cut`) inside its bounding box (the `stroke` kernel).
fn stamp_stroke(cx: &mut Cx<'_>, env: &BrushEnv<'_>, s: &BrushStroke, plane: &Buf) -> bool {
    note_stroke();
    let (w, h) = (env.w, env.h);
    let bd = brush_dabs(s, env.frame, w, h);
    if bd.dabs.is_empty() {
        return true;
    }
    // five words a dab, pen pressure applied
    if !bd.dabs.len().checked_mul(5).is_some_and(|words| cx.gpu.fits(words)) {
        return false;
    }
    let rmax = bd.r.max(0.5);
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for d in &bd.dabs {
        lo = [lo[0].min(d.x), lo[1].min(d.y)];
        hi = [hi[0].max(d.x), hi[1].max(d.y)];
    }
    let clamp = |v: f64, max: usize| v.clamp(0.0, max as f64) as usize;
    let (x0, x1) = (clamp((lo[0] - rmax).floor(), w), clamp((hi[0] + rmax).ceil(), w));
    let (y0, y1) = (clamp((lo[1] - rmax).floor(), h), clamp((hi[1] + rmax).ceil(), h));
    // the box's rows in the cut, from the first of the whole box's 16-row tiles there: each pixel is in the tile it is
    // in when the whole frame is stamped, which picks the same dabs in the same order
    let cut = env.cut;
    let (t0, y1) = (y0 + cut.y0.saturating_sub(y0) / 16 * 16, y1.min(cut.end()));
    if x0 >= x1 || y0 >= y1 || t0 >= y1 {
        return true;
    }
    let aux: Vec<f32> = bd
        .dabs
        .iter()
        .enumerate()
        .flat_map(|(k, d)| {
            let (r, hard, flow) = bd.dab(k);
            [d.x as f32, d.y as f32, r as f32, hard as f32, flow]
        })
        .collect();
    let aux = cx.gpu.upload(&aux);
    let mut p = vec![w as u32, cut.end() as u32, cut.y0 as u32, 0, 0, 0, 0, 0, 0, 0];
    p.extend([x0, t0, x1, y1, bd.dabs.len()].map(|v| v as u32));
    p.extend([bd.density.to_bits(), bd.erase as u32, bd.feather_by_stroke as u32, 0]);
    let grid = groups2(x1 - x0, y1 - t0, [16, 16]);
    cx.run("stroke", &p, &[Some(env.lin), Some(env.log_l), Some(&aux), Some(plane), None], grid);
    true
}

/// The blurred coefficients of the cross guided filter (`masks::guided_cross`) of `p` steered by `guide`, both `w × h` cuts of a
/// frame (`whole`: its size and the cut's first row, as `gaussian_at`): interleaved (a, b), the model `q = a · guide + b`.
#[allow(clippy::too_many_arguments)]
fn guided_ab(cx: &mut Cx<'_>, guide: &Buf, p: &Buf, (w, h): (usize, usize), whole: ((usize, usize), usize), sigma: f32, eps: f32) -> Buf {
    let n = w * h;
    let (m0, m1) = (cx.gpu.buffer(2 * n), cx.gpu.buffer(2 * n));
    map(cx, "xguided_pre", n, &[0], [Some(guide), Some(p), None], &m0);
    map(cx, "xguided_pre", n, &[1], [Some(guide), Some(p), None], &m1);
    let (b0, b1) = (gaussian_at(cx, &m0, (w, h), 2, sigma, whole), gaussian_at(cx, &m1, (w, h), 2, sigma, whole));
    let ab = cx.gpu.buffer(2 * n);
    map(cx, "xguided_ab", n, &[eps.to_bits()], [Some(&b0), Some(&b1), None], &ab);
    gaussian_at(cx, &ab, (w, h), 2, sigma, whole)
}

/// Cross guided filter (`masks::guided_cross`) of `p` steered by `guide`, with `p` moved towards it by `mix` (Refine Edges):
/// both the rows `cut` of a `w × h` frame. Its rows farther than [`bands::guided_reach`] from a cut's edge inside the
/// frame are the whole frame's, bit for bit.
#[allow(clippy::too_many_arguments)]
fn guided_cross(cx: &mut Cx<'_>, guide: &Buf, p: &Buf, (w, h): (usize, usize), cut: Cut, sigma: f32, eps: f32, mix: f32) -> Buf {
    let n = w * cut.rows;
    let ab = guided_ab(cx, guide, p, (w, cut.rows), ((w, h), cut.y0), sigma, eps);
    let q = cx.gpu.buffer(n);
    map(cx, "xguided_apply", n, &[1, mix.to_bits()], [Some(guide), Some(&ab), Some(p)], &q);
    q
}

/// How far the `shape` kernel's grid positions may be from the CPU's (long-edge units): a few `f32` roundings of
/// values up to about 2.
const GRID_F32: f64 = 5e-7;

/// Finish a gradient's `shape` parameters `p` (kinds 0 and 1, filled up to their own): without a warp (`grid` `None`)
/// its positions are the affine's; through one they come from the position grid, its points in `aux`. `false` when
/// the grid's positions in `f32` are not close enough for `ramp` (a very sharp gradient): the CPU evaluates it.
fn gradient(p: &mut Vec<u32>, aux: &mut Vec<f32>, grid: Option<&PosGrid>, ramp: &Ramp) -> bool {
    // (P[18] says whether there is a grid; P[19..25] find a pixel's cell in it)
    p.resize(18, 0);
    let Some(g) = grid else {
        p.extend([0; 7]);
        return true;
    };
    if !g.fits(ramp, GRID_F32) {
        return false;
    }
    let (pts, table) = g.gpu_table();
    p.push(1);
    p.extend(table);
    *aux = pts;
    true
}

/// Fork (issue #288): Relight's planes (`relit`, each `n` texels) into the stack `alpha` after its first `from`.
fn upload_relit(cx: &mut Cx<'_>, relit: &[lightcraft_pipeline::masks::Evaluated], alpha: &Buf, from: usize, n: usize) {
    for (k, e) in relit.iter().enumerate() {
        let plane = e.alpha.to_plane();
        if plane.data.len() == n {
            let buf = cx.gpu.upload(&plane.data);
            cx.copy_into(&buf, alpha, (from + k) * n);
        }
    }
}

/// The alphas a render of frame `geo` used, and after them the ones of earlier windows and scales in `old` (most recent
/// first) up to [`lightcraft_pipeline::masks::KEPT_ALPHAS`] and [`lightcraft_pipeline::masks::KEPT_ALPHA_BYTES`]: a pan
/// back or a zoom to a scale seen before finds its masks (issue #175). Of this frame only the alphas the render used are
/// looked up again (an edited mask's earlier alpha goes); this render's stack is kept whatever its size, and counts
/// against the bytes. A stack is held whole, so it also holds planes no kept alpha points at: the chromaticity planes of
/// local Moiré / Noise, Relight's planes, and a second mask with the same alpha. It costs its whole size the first time
/// one of its alphas is kept.
fn with_earlier(used: Vec<Kept>, old: Vec<Kept>, geo: u64) -> Vec<Kept> {
    let mut stacks: Vec<*const Buf> = Vec::new();
    let mut bytes = 0usize;
    for k in &used {
        if !stacks.contains(&Arc::as_ptr(&k.stack)) {
            stacks.push(Arc::as_ptr(&k.stack));
            bytes = bytes.saturating_add(k.stack.len * 4);
        }
    }
    let mut kept = used;
    for k in old {
        if kept.len() >= lightcraft_pipeline::masks::KEPT_ALPHAS {
            break;
        }
        if k.geo == geo || kept.iter().any(|q| q.key == k.key) {
            continue;
        }
        let cost = if stacks.contains(&Arc::as_ptr(&k.stack)) { 0 } else { k.stack.len * 4 };
        if bytes.saturating_add(cost) > lightcraft_pipeline::masks::KEPT_ALPHA_BYTES {
            continue;
        }
        if cost > 0 {
            stacks.push(Arc::as_ptr(&k.stack));
            bytes += cost;
        }
        kept.push(k);
    }
    kept
}

/// Refine Edges' sigma (px) at setting `refine` (0..100) on a `w × h` frame.
fn refine_sigma(frame: &Frame, (w, h): (usize, usize), refine: f64) -> f32 {
    let k = (refine / 100.0).clamp(0.0, 1.0) as f32;
    (0.04 * frame.output_long(w, h) as f32 * k).max(1.0)
}

/// `log_l`'s rows `cut` of a `w × h` frame: the plane itself for the whole frame, else cut into `slot` once.
fn guide_rows<'b>(cx: &mut Cx<'_>, log_l: &'b Buf, (w, h): (usize, usize), cut: Cut, slot: &'b mut Option<Buf>) -> &'b Buf {
    if cut == Cut::whole(h) {
        return log_l;
    }
    slot.get_or_insert_with(|| cx.slice(log_l, cut.y0 * w, w * cut.rows))
}

/// The alpha of a shape the CPU evaluated over the whole frame (`v`), its rows `cut` uploaded; turned around when
/// `inverted`.
fn upload_cut(cx: &mut Cx<'_>, mut v: lightcraft_raster::Plane, inverted: bool, cut: Cut) -> Buf {
    if inverted {
        v.data.iter_mut().for_each(|x| *x = 1.0 - *x);
    }
    cx.gpu.upload(cut.of(&v.data, v.width))
}

/// Evaluate mask `m` (`lightcraft_pipeline::masks::evaluate_one`) over the rows `cut` of the frame into plane `at` of
/// `dst` (planes of the cut's size): its parts combined, then Refine Edges (before its invert and Amount, which the
/// finish stage applies). Parts with no kernel are evaluated on the CPU over the whole frame, and their rows `cut`
/// uploaded. `comp` holds one part at a time (a plane of the cut's size). `keep`: a brush part stamped here may be kept
/// in `held` for the next render (one with a store).
#[allow(clippy::too_many_arguments)]
fn evaluate(
    cx: &mut Cx<'_>,
    img: &Images<'_>,
    plan: &Plan<'_>,
    host: &mut Host,
    held: &mut Inner,
    keep: bool,
    m: &lightcraft_develop::Mask,
    cut: Cut,
    (dst, at): (&Buf, usize),
    comp: &Buf,
) {
    use lightcraft_develop::{MaskOp, MaskShape};
    let (lin, prep) = (img.lin, img.prep);
    let (w, h) = (plan.w, plan.h);
    let n = w * cut.rows;
    let ev = plan.settings.light.exposure as f32;
    let keys = AlphaKeys { geo: plan.geo, image: plan.lin_key };
    let long = plan.frame.ow.max(plan.frame.oh);
    let [a, b, c, d, e, f] = plan.frame.out_to_norm(w, h).0;
    let (kx, ky) = (plan.frame.ow / long, plan.frame.oh / long);
    let aff = [a * kx, b * ky, c * kx, d * ky, e * kx, f * ky].map(|v| (v as f32).to_bits());
    // the edge-aware filters' guide, cut to the rows when first needed
    let mut guide_cut: Option<Buf> = None;
    // its coordinates on the photo before the warp (`masks::evaluate_one`): through a lens or perspective warp
    // the gradients read their positions from the frame's position grid (`masks::grid`)
    let mframe = plan.frame.for_mask(m.on_photo);
    let frame = &*mframe;
    let gradients = m.components.iter().any(|c| matches!(c.shape, MaskShape::Linear { .. } | MaskShape::Radial { .. }));
    let grid = gradients.then(|| lightcraft_pipeline::masks::grid::pos_grid(frame, w, h)).flatten();
    let mut first = true;
    for part in &m.components {
        // Invert Mask turns the base part around (see `lightcraft_pipeline::masks::evaluate_one`)
        let inverted = part.invert != (first && m.invert);
        let op = match part.op {
            _ if first => 0u32,
            MaskOp::Add => 1,
            MaskOp::Subtract => 2,
            MaskOp::Intersect => 3,
        };
        first = false;
        let mut p = vec![w as u32, cut.rows as u32, 0, inverted as u32];
        p.extend_from_slice(&aff);
        let f32s = |v: &[f64]| v.iter().map(|x| (*x as f32).to_bits()).collect::<Vec<u32>>();
        let mut aux: Vec<f32> = Vec::new();
        let mut own: Option<Buf> = None;
        let mut smooth: Option<f32> = None;
        let kind = match &part.shape {
            MaskShape::Linear { start, end } => {
                let (a, b) = (frame.norm_to_long(*start), frame.norm_to_long(*end));
                let d = b - a;
                p.extend(f32s(&[a.x, a.y, d.x, d.y, d.dot(d).max(1e-12)]));
                gradient(&mut p, &mut aux, grid.as_deref(), &Ramp::linear(a, d)).then_some(0)
            }
            MaskShape::Radial { center, rx, ry, angle, feather, invert } => {
                let c = frame.norm_to_long(*center);
                let (sn, co) = (-angle.to_radians()).sin_cos();
                p.extend(f32s(&[c.x, c.y, co, sn, rx.max(1e-6), ry.max(1e-6), (*feather / 100.0).clamp(0.0, 1.0)]));
                p.push(*invert as u32);
                gradient(&mut p, &mut aux, grid.as_deref(), &Ramp::radial(c, *rx, *ry, *angle, *feather / 100.0)).then_some(1)
            }
            MaskShape::LuminanceRange { lo, hi, lo_feather, hi_feather } => {
                p.extend(f32s(&[*lo, *hi, lo_feather.max(1e-3), hi_feather.max(1e-3)]));
                p.push(ev.to_bits());
                Some(2)
            }
            MaskShape::ColorRange { samples, refine } => {
                use lightcraft_pipeline::masks::color_range;
                let tol = color_range::color_range_tolerance(*refine);
                // (as the CPU: at most `MAX_COLOR_RANGE_SAMPLES`, cost is pixels x samples; no sample, nothing selected)
                let keys = color_range::keys(samples);
                p.extend([tol.to_bits(), ev.exp2().to_bits(), keys.len() as u32]);
                aux.extend(keys.iter().flatten());
                // then smoothed along the photo's edges, as on the CPU (`color_range::fill`)
                smooth = (!keys.is_empty()).then(|| color_range::smooth_sigma(frame, w, h));
                Some(3)
            }
            MaskShape::Brush { strokes } => {
                // (None: the device can't hold the dabs, and the CPU rasterizes the part)
                let env = BrushEnv { lin, log_l: &prep.log_l, frame, w, h, cut, keys };
                own = if strokes.iter().any(|s| s.auto_mask) {
                    Some(auto_brush_part(cx, host, &env, strokes, inverted, &mut held.cpu_brushes))
                } else {
                    brush_part(cx, &env, strokes, inverted, &mut held.brushes, keep)
                };
                None
            }
            _ => None,
        };
        let plane = match kind {
            _ if own.is_some() => own,
            Some(k) => {
                p[2] = k;
                // (the kernel reads the frame's pixels and writes the cut's: P[25] is the cut's first row)
                p.resize(25, 0);
                p.push(cut.y0 as u32);
                let aux = (!aux.is_empty()).then(|| cx.gpu.upload(&aux));
                cx.run("shape", &p, &[Some(lin), Some(&prep.log_l), aux.as_ref(), Some(comp), Some(dst)], groups2(w, cut.rows, [16, 16]));
                // (the filter turns with an inverted part: smoothing `1 - c` gives `1 -` the smoothed `c`)
                smooth.map(|sigma| {
                    let g = guide_rows(cx, &prep.log_l, (w, h), cut, &mut guide_cut);
                    guided_cross(cx, g, comp, (w, h), cut, sigma, lightcraft_pipeline::masks::color_range::SMOOTH_EPS, 1.0)
                })
            }
            None if lightcraft_pipeline::masks::uses_guide(&part.shape) => {
                // a stored selection, on the CPU: it follows the resampled photo's edges (made once per frame and kept), and
                // reads nothing of the edited photo (issue #175)
                if held.guide.as_ref().is_none_or(|(geo, g)| *geo != plan.geo || (g.width, g.height) != (w, h)) {
                    let img = cx.read_rgb(img.sampled, w, h);
                    held.guide = Some((plan.geo, Arc::new(lightcraft_pipeline::masks::guide_plane(&img))));
                }
                let guide = held.guide.as_ref().map(|(_, g)| g.clone());
                let (img, l) = (Rgb32f::new(0, 0), lightcraft_raster::Plane::new(0, 0));
                let v = lightcraft_pipeline::masks::shape_alpha_guided(
                    &part.shape,
                    frame,
                    w,
                    h,
                    (&img, &l, guide.as_deref()),
                    ev,
                    plan.mattes.as_deref(),
                );
                Some(upload_cut(cx, v, inverted, cut))
            }
            None if matches!(part.shape, MaskShape::Linear { .. } | MaskShape::Radial { .. }) => {
                // a gradient too sharp for the grid's positions in `f32` (`gradient`): on the CPU, which reads no
                // pixel of the image for it
                let (img, l) = (Rgb32f::new(0, 0), lightcraft_raster::Plane::new(0, 0));
                let v = lightcraft_pipeline::masks::shape_alpha(&part.shape, frame, w, h, &img, &l, ev, plan.mattes.as_deref());
                Some(upload_cut(cx, v, inverted, cut))
            }
            None => {
                // no kernel: evaluate on the CPU
                let img = host.lin.get_or_insert_with(|| Arc::new(cx.read_rgb(lin, w, h))).clone();
                let l = host.log_l.get_or_insert_with(|| Arc::new(cx.read_plane(&prep.log_l, w, h))).clone();
                let v = lightcraft_pipeline::masks::shape_alpha(&part.shape, frame, w, h, &img, &l, ev, plan.mattes.as_deref());
                Some(upload_cut(cx, v, inverted, cut))
            }
        };
        let cbuf = plane.as_ref().unwrap_or(comp);
        let k = [n as u32, 0, at as u32, op];
        cx.run("combine", &k, &[None, None, None, Some(cbuf), Some(dst)], groups1(n));
    }
    if m.refine > 0.0 {
        // Refine Edges (`masks::evaluate_one`): the combined alpha follows the photo's edges,
        // before the mask's invert and amount
        let k = (m.refine / 100.0).clamp(0.0, 1.0) as f32;
        let plane = cx.slice(dst, at * n, n);
        let g = guide_rows(cx, &prep.log_l, (w, h), cut, &mut guide_cut);
        let refined = guided_cross(cx, g, &plane, (w, h), cut, refine_sigma(frame, (w, h), m.refine), 0.02, k.sqrt());
        cx.copy_into(&refined, dst, at * n);
    }
}

/// Evaluate the masks (`lightcraft_pipeline::masks::evaluate`) over the whole frame: their alpha planes (concatenated,
/// followed by the blurred chromaticity when local Moiré / Noise need it) and their adjustment terms. Shapes without a
/// kernel (Sky, Subject, …) run on the CPU. (A render that keeps nothing evaluates them band by band instead when they
/// are large: [`bands`].)
///
/// `store`: the view's alphas of the last render (see [`AlphaStore`]); without one every mask is evaluated.
pub(crate) fn masks(
    cx: &mut Cx<'_>,
    img: &Images<'_>,
    plan: &Plan<'_>,
    host: &mut Host,
    store: Option<(&AlphaStore, &Arc<Rgb32f>)>,
) -> (Option<Arc<Buf>>, Vec<[f32; MASK_TERMS]>) {
    let s = &*plan.settings;
    let list: Vec<_> = s.masks.iter().filter(|m| m.renders()).collect();
    // (fork, issue #288: Relight's light, as local adjustments after the masks, `lightcraft_pipeline::relight`)
    let relit = lightcraft_pipeline::relight::masks(plan);
    if list.is_empty() && relit.is_empty() {
        return (None, Vec::new());
    }
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let stack = list.len() + relit.len();
    let ev = s.light.exposure as f32;
    let planes = Arc::new(cx.zeroed(n * stack + if img.prep.chroma.is_some() { 3 * n } else { 0 }));
    let alpha: &Buf = &planes;
    if let Some(ch) = &img.prep.chroma {
        cx.copy_into(ch, alpha, n * stack);
    }
    upload_relit(cx, &relit, alpha, list.len(), n);
    bands::note_alphas(planes.len * 4);
    let comp = cx.gpu.buffer(n);
    let mut guard = store.map(|(s, _)| s.lock());
    let mut unkept = Inner::default();
    let held: &mut Inner = guard.as_deref_mut().unwrap_or(&mut unkept);
    // another picture (the next photo, a sharper level, the Denoise mix): nothing held was made from this one
    if let Some((_, src)) = store
        && !held.source.as_ref().is_some_and(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(src)))
    {
        *held = Inner { source: Some(Arc::downgrade(src)), ..Inner::default() };
    }
    // the alphas of the last render, when it had the same segmentation mattes (taken out: hand back the ones used)
    let same = match (&held.mattes, &plan.mattes) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    };
    let old = if same { std::mem::take(&mut held.slots) } else { Vec::new() };
    // (each a range of this render's stack: the earlier stacks go once nothing of theirs is kept)
    let mut used: Vec<Kept> = Vec::new();
    let keys = AlphaKeys { geo: plan.geo, image: plan.lin_key };
    let here = |key: u64, mi: usize| Kept { key, geo: plan.geo, stack: planes.clone(), at: mi * n, len: n };
    for (mi, m) in list.iter().enumerate() {
        let key = alpha_key(m, keys, ev);
        if let Some(k) = old.iter().find(|k| k.key == key && k.len == n) {
            cx.copy_range(&k.stack, k.at, alpha, mi * n, n);
        } else {
            note_evaluation();
            evaluate(cx, img, plan, host, held, store.is_some(), m, Cut::whole(h), (alpha, mi), &comp);
        }
        // (the mask's Amount scales its effect terms, not its alpha: see `effect_terms`)
        if !used.iter().any(|u| u.key == key) {
            used.push(here(key, mi));
        }
    }
    // (a render that failed is redone on the CPU: what it left on the device is not kept)
    if !crate::ctx::failed() {
        held.mattes = plan.mattes.clone();
        held.slots = with_earlier(used, old, plan.geo);
    }
    (Some(planes), terms(plan, &list, &relit))
}

/// The adjustment terms of the masks `list` and Relight's `relit`, in the order of their planes.
fn terms(plan: &Plan<'_>, list: &[&lightcraft_develop::Mask], relit: &[lightcraft_pipeline::masks::Evaluated]) -> Vec<[f32; MASK_TERMS]> {
    list.iter()
        .map(|m| {
            let r = plan.recolor.iter().find(|(id, _)| *id == m.id).map(|(_, r)| r);
            effect_terms_with(&m.adjust, (m.adjust.amount / 100.0) as f32, r)
        })
        .chain(relit.iter().map(|e| effect_terms_with(&e.adjust, e.amount, None)))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lightcraft_develop::{BrushStroke, DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};
    use lightcraft_geom::Point;
    use lightcraft_pipeline::{RenderRequest, SourceInfo, StageCache};

    /// A render's mask alphas are kept once (issue #402): each is a range of the one stack of planes the render made,
    /// not a copy of its own, so a view of 12 masks never holds them twice; a slider tick afterwards keeps them so.
    #[test]
    fn the_kept_alphas_share_their_renders_stack() {
        if !crate::available() {
            lightcraft_testkit::skip!(Gpu, "no GPU adapter");
            return;
        }
        let src = Arc::new(lightcraft_scenes::demo_library()[0].render(480, 320));
        let stroke = BrushStroke { points: vec![Point::new(0.2, 0.3), Point::new(0.6, 0.5)].into(), ..Default::default() };
        let shapes = [
            MaskShape::Radial { center: Point::new(0.5, 0.45), rx: 0.3, ry: 0.2, angle: 0.0, feather: 50.0, invert: false },
            MaskShape::Linear { start: Point::new(0.0, 0.0), end: Point::new(0.0, 0.5) },
            MaskShape::Brush { strokes: vec![stroke] },
        ];
        let masks = (1..=3u32)
            .zip(shapes)
            .map(|(id, shape)| Mask {
                id,
                components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }],
                adjust: LocalAdjustments { exposure: 0.5, ..Default::default() },
                ..Default::default()
            })
            .collect();
        let mut s = DevelopSettings { masks, ..Default::default() };
        let (info, req, cache) = (SourceInfo { raw: true, ..Default::default() }, RenderRequest::fit(480, 480), StageCache::default());
        for tick in 0..2 {
            crate::render(&src, &info, &s, &req, Some(&cache)).expect("gpu");
            let stages = cache.peek_extension::<crate::GpuStages>().expect("stages");
            let held = stages.alphas().lock();
            assert_eq!(held.slots.len(), 3, "tick {tick}: one alpha a mask");
            assert!(held.slots.iter().all(|k| Arc::ptr_eq(&k.stack, &held.slots[0].stack)), "tick {tick}: every alpha in the one stack");
            drop(held);
            s.masks[0].adjust.exposure += 0.25;
        }
    }
}
