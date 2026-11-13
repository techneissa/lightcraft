//! Mask evaluation on the device (`lightcraft_pipeline::masks`): each visible mask's alpha plane
//! and its effect terms.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lightcraft_develop::BrushStroke;
use lightcraft_pipeline::Plan;
use lightcraft_pipeline::finish::{MASK_TERMS, effect_terms_with};
use lightcraft_pipeline::geometry::Frame;
use lightcraft_pipeline::masks::grid::{PosGrid, Ramp};
use lightcraft_pipeline::masks::{
    AlphaKeys, BRUSH_ENTRIES, Mattes, alpha_key, brush_base, brush_chain, brush_dabs, note_evaluation, note_filtered, note_stroke,
};
use lightcraft_raster::Rgb32f;

use crate::ctx::{Buf, groups1, groups2};
use crate::render::{Cx, Host, Prep, gaussian_at, map};

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
    /// The alphas by key, with the frame (`plan.geo`) each was evaluated for.
    slots: Vec<(u64, u64, Arc<Buf>)>,
    brushes: Vec<BrushPart>,
    /// The edges a stored selection follows (`masks::guide_plane` of the resampled photo) for the frame `plan.geo` keys.
    guide: Option<(u64, Arc<lightcraft_raster::Plane>)>,
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

    /// Hand every device buffer held to `add` (the caller counts each once).
    pub(crate) fn each(&self, mut add: impl FnMut(&Arc<Buf>)) {
        let g = self.lock();
        g.slots.iter().for_each(|(_, _, b)| add(b));
        g.brushes.iter().for_each(|p| add(&p.plane));
    }
}

/// What a brush part is stamped against.
struct BrushEnv<'a> {
    lin: &'a Buf,
    log_l: &'a Buf,
    frame: &'a Frame,
    w: usize,
    h: usize,
    keys: AlphaKeys,
}

/// The plane of brush strokes `strokes` on the device (`masks::brush`), or `None` when the device can't hold their dabs
/// (the CPU rasterizes the part). Strokes that extend a part held in `parts` are stamped over a copy of its plane; the
/// result is kept there. `inverted`: the part is turned around.
fn brush_part(cx: &mut Cx<'_>, env: &BrushEnv<'_>, strokes: &[BrushStroke], inverted: bool, parts: &mut Vec<BrushPart>) -> Option<Buf> {
    let n = env.w * env.h;
    let auto = strokes.iter().any(|s| s.auto_mask);
    let base = brush_base(env.keys.geo, env.keys.image, auto, env.w, env.h);
    let chain = brush_chain(strokes);
    // the part this one extends (the longest held prefix of its strokes)
    let hit = parts
        .iter()
        .enumerate()
        .filter(|(_, e)| e.base == base && !e.chain.is_empty() && e.chain.len() <= chain.len() && chain.starts_with(&e.chain))
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
    if !strokes.is_empty() && from < strokes.len() {
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

/// Stamp one stroke into `plane` inside its bounding box (the `stroke` kernel). An Auto Mask stroke is stamped
/// alone, refined with the guided filter and combined, as on the CPU.
fn stamp_stroke(cx: &mut Cx<'_>, env: &BrushEnv<'_>, s: &BrushStroke, plane: &Buf) -> bool {
    note_stroke();
    let (w, h) = (env.w, env.h);
    let n = w * h;
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
    if x0 >= x1 || y0 >= y1 {
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
    let mut p = vec![w as u32, h as u32, 0, 0, 0, 0, 0, 0, 0, 0];
    p.extend([x0, y0, x1, y1, bd.dabs.len()].map(|v| v as u32));
    p.extend([bd.density.to_bits(), bd.erase as u32, bd.auto as u32, 0]);
    let grid = groups2(x1 - x0, y1 - y0, [16, 16]);
    if !bd.auto {
        cx.run("stroke", &p, &[Some(env.lin), Some(env.log_l), Some(&aux), Some(plane), None], grid);
        return true;
    }
    // the stroke alone (mode 1), refined, then combined into the part in order
    let own = cx.zeroed(n);
    p[18] = 1;
    cx.run("stroke", &p, &[Some(env.lin), Some(env.log_l), Some(&aux), Some(&own), None], grid);
    let (sigma, eps) = lightcraft_pipeline::masks::auto_refine(bd.r);
    // The filter only moves alpha within its reach of the stroke (two Gaussians in a row: twice the sum of the box radii,
    // and a little over): it runs on that cut of the frame, not on all of it. Past the cut the refined alpha is 0.
    let reach = lightcraft_raster::blur::radii_for(sigma, w, h).map_or(0, |r| 2 * r.iter().sum::<usize>() + 2);
    let (cx0, cy0) = (x0.saturating_sub(reach), y0.saturating_sub(reach));
    let (cx1, cy1) = (x1.saturating_add(reach).min(w), y1.saturating_add(reach).min(h));
    let (cw, ch) = (cx1 - cx0, cy1 - cy0);
    let cut = |cx: &mut Cx<'_>, plane: &Buf| {
        let out = cx.gpu.buffer(cw * ch);
        map(cx, "crop_k", cw * ch, &[cw as u32, w as u32, cx0 as u32, cy0 as u32], [Some(plane), None, None], &out);
        out
    };
    let (guide, alone) = (cut(cx, env.log_l), cut(cx, &own));
    note_filtered(cw * ch);
    let ab = guided_ab(cx, &guide, &alone, (cw, ch), ((w, h), cy0), sigma, eps);
    let op = if bd.erase { 2 } else { 1 };
    map(cx, "xguided_stamp", cw * ch, &[cw as u32, w as u32, cx0 as u32, cy0 as u32, op], [Some(&guide), Some(&ab), Some(&alone)], plane);
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

/// Cross guided filter (`masks::guided_cross`) of `p` steered by `guide`, with `p` moved towards it by `mix` (Refine Edges).
#[allow(clippy::too_many_arguments)]
fn guided_cross(cx: &mut Cx<'_>, guide: &Buf, p: &Buf, w: usize, h: usize, sigma: f32, eps: f32, mix: f32) -> Buf {
    let n = w * h;
    let ab = guided_ab(cx, guide, p, (w, h), ((w, h), 0), sigma, eps);
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

/// Evaluate the masks (`lightcraft_pipeline::masks::evaluate`): their alpha planes (concatenated,
/// followed by the blurred chromaticity when local Moiré / Noise need it) and their adjustment
/// terms. Shapes without a kernel (Sky, Subject, …) run on the CPU.
///
/// `store`: the view's alphas of the last render (see [`AlphaStore`]); without one every mask is evaluated.
pub(crate) fn masks(
    cx: &mut Cx<'_>,
    (lin, sampled): (&Buf, &Buf),
    prep: &Prep,
    plan: &Plan<'_>,
    host: &mut Host,
    store: Option<(&AlphaStore, &Arc<Rgb32f>)>,
) -> (Option<Buf>, Vec<[f32; MASK_TERMS]>) {
    use lightcraft_develop::{MaskOp, MaskShape};
    let s = &*plan.settings;
    let list: Vec<_> = s.masks.iter().filter(|m| m.renders()).collect();
    if list.is_empty() {
        return (None, Vec::new());
    }
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let frame = &plan.frame;
    let ev = s.light.exposure as f32;
    let long = frame.ow.max(frame.oh);
    let [a, b, c, d, e, f] = frame.out_to_norm(w, h).0;
    let (kx, ky) = (frame.ow / long, frame.oh / long);
    let aff = [a * kx, b * ky, c * kx, d * ky, e * kx, f * ky].map(|v| (v as f32).to_bits());
    let alpha = cx.zeroed(n * list.len() + if prep.chroma.is_some() { 3 * n } else { 0 });
    if let Some(ch) = &prep.chroma {
        cx.copy_into(ch, &alpha, n * list.len());
    }
    let comp_plane = cx.gpu.buffer(n);
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
    let mut used: Vec<(u64, Arc<Buf>)> = Vec::new();
    let keys = AlphaKeys { geo: plan.geo, image: plan.lin_key };
    for (mi, m) in list.iter().enumerate() {
        let key = alpha_key(m, keys, ev);
        if let Some((_, _, buf)) = old.iter().find(|(k, _, _)| *k == key) {
            cx.copy_into(buf, &alpha, mi * n);
            if !used.iter().any(|(k, _)| *k == key) {
                used.push((key, buf.clone()));
            }
            continue;
        }
        note_evaluation();
        // its coordinates on the photo before the warp (`masks::evaluate_one`): through a lens or perspective warp
        // the gradients read their positions from the frame's position grid (`masks::grid`)
        let mframe = plan.frame.for_mask(m.on_photo);
        let frame = &*mframe;
        let gradients = m.components.iter().any(|c| matches!(c.shape, MaskShape::Linear { .. } | MaskShape::Radial { .. }));
        let grid = gradients.then(|| lightcraft_pipeline::masks::grid::pos_grid(frame, w, h)).flatten();
        let mut first = true;
        for comp in &m.components {
            // Invert Mask turns the base part around (see `lightcraft_pipeline::masks::evaluate_one`)
            let inverted = comp.invert != (first && m.invert);
            let op = match comp.op {
                _ if first => 0u32,
                MaskOp::Add => 1,
                MaskOp::Subtract => 2,
                MaskOp::Intersect => 3,
            };
            first = false;
            let mut p = vec![w as u32, h as u32, 0, inverted as u32];
            p.extend_from_slice(&aff);
            let f32s = |v: &[f64]| v.iter().map(|x| (*x as f32).to_bits()).collect::<Vec<u32>>();
            let mut aux: Vec<f32> = Vec::new();
            let mut own: Option<Buf> = None;
            let mut smooth: Option<f32> = None;
            let kind = match &comp.shape {
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
                    let env = BrushEnv { lin, log_l: &prep.log_l, frame, w, h, keys };
                    own = brush_part(cx, &env, strokes, inverted, &mut held.brushes);
                    None
                }
                _ => None,
            };
            let plane = match kind {
                _ if own.is_some() => own,
                Some(k) => {
                    p[2] = k;
                    let aux = (!aux.is_empty()).then(|| cx.gpu.upload(&aux));
                    cx.run("shape", &p, &[Some(lin), Some(&prep.log_l), aux.as_ref(), Some(&comp_plane), Some(&alpha)], groups2(w, h, [16, 16]));
                    // (the filter turns with an inverted part: smoothing `1 - c` gives `1 -` the smoothed `c`)
                    smooth.map(|sigma| {
                        guided_cross(cx, &prep.log_l, &comp_plane, w, h, sigma, lightcraft_pipeline::masks::color_range::SMOOTH_EPS, 1.0)
                    })
                }
                None if lightcraft_pipeline::masks::uses_guide(&comp.shape) => {
                    // a stored selection, on the CPU: it follows the resampled photo's edges (made once per frame and kept), and
                    // reads nothing of the edited photo (issue #175)
                    if held.guide.as_ref().is_none_or(|(geo, g)| *geo != plan.geo || (g.width, g.height) != (w, h)) {
                        let img = cx.read_rgb(sampled, w, h);
                        held.guide = Some((plan.geo, Arc::new(lightcraft_pipeline::masks::guide_plane(&img))));
                    }
                    let guide = held.guide.as_ref().map(|(_, g)| g.clone());
                    let (img, l) = (Rgb32f::new(0, 0), lightcraft_raster::Plane::new(0, 0));
                    let mut v = lightcraft_pipeline::masks::shape_alpha_guided(
                        &comp.shape,
                        frame,
                        w,
                        h,
                        (&img, &l, guide.as_deref()),
                        ev,
                        plan.mattes.as_deref(),
                    );
                    if inverted {
                        v.data.iter_mut().for_each(|x| *x = 1.0 - *x);
                    }
                    Some(cx.gpu.upload(&v.data))
                }
                None if matches!(comp.shape, MaskShape::Linear { .. } | MaskShape::Radial { .. }) => {
                    // a gradient too sharp for the grid's positions in `f32` (`gradient`): on the CPU, which reads no
                    // pixel of the image for it
                    let (img, l) = (Rgb32f::new(0, 0), lightcraft_raster::Plane::new(0, 0));
                    let mut v = lightcraft_pipeline::masks::shape_alpha(&comp.shape, frame, w, h, &img, &l, ev, plan.mattes.as_deref());
                    if inverted {
                        v.data.iter_mut().for_each(|x| *x = 1.0 - *x);
                    }
                    Some(cx.gpu.upload(&v.data))
                }
                None => {
                    // no kernel: evaluate on the CPU
                    let img = host.lin.get_or_insert_with(|| Arc::new(cx.read_rgb(lin, w, h))).clone();
                    let l = host.log_l.get_or_insert_with(|| Arc::new(cx.read_plane(&prep.log_l, w, h))).clone();
                    let mut v = lightcraft_pipeline::masks::shape_alpha(&comp.shape, frame, w, h, &img, &l, ev, plan.mattes.as_deref());
                    if inverted {
                        v.data.iter_mut().for_each(|x| *x = 1.0 - *x);
                    }
                    Some(cx.gpu.upload(&v.data))
                }
            };
            let cbuf = plane.as_ref().unwrap_or(&comp_plane);
            let k = [n as u32, 0, mi as u32, op];
            cx.run("combine", &k, &[None, None, None, Some(cbuf), Some(&alpha)], groups1(n));
        }
        if m.refine > 0.0 {
            // Refine Edges (`masks::evaluate_one`): the combined alpha follows the photo's edges,
            // before the mask's invert and amount
            let k = (m.refine / 100.0).clamp(0.0, 1.0) as f32;
            let sigma = (0.04 * frame.output_long(w, h) as f32 * k).max(1.0);
            let plane = cx.slice(&alpha, mi * n, n);
            let refined = guided_cross(cx, &prep.log_l, &plane, w, h, sigma, 0.02, k.sqrt());
            cx.copy_into(&refined, &alpha, mi * n);
        }
        // (the mask's Amount scales its effect terms, not its alpha: see `effect_terms`)
        if !used.iter().any(|(k, _)| *k == key) {
            used.push((key, Arc::new(cx.slice(&alpha, mi * n, n))));
        }
    }
    // (a render that failed is redone on the CPU: what it left on the device is not kept)
    if !crate::ctx::failed() {
        held.mattes = plan.mattes.clone();
        // and after this render's, the alphas of earlier windows and scales (most recent first) up to the caps: a pan back
        // or a zoom to a scale seen before finds its masks (issue #175)
        // (of this frame only what the render used: an edited mask's earlier alpha goes)
        let mut kept: Vec<(u64, u64, Arc<Buf>)> = used.into_iter().map(|(k, b)| (k, plan.geo, b)).collect();
        let mut bytes = kept.iter().map(|(_, _, b)| b.len * 4).fold(0usize, usize::saturating_add);
        for (k, g, b) in old {
            if kept.len() >= lightcraft_pipeline::masks::KEPT_ALPHAS {
                break;
            }
            if g == plan.geo || kept.iter().any(|(q, ..)| *q == k) || bytes.saturating_add(b.len * 4) > lightcraft_pipeline::masks::KEPT_ALPHA_BYTES {
                continue;
            }
            bytes += b.len * 4;
            kept.push((k, g, b));
        }
        held.slots = kept;
    }
    let terms = list
        .iter()
        .map(|m| {
            let r = plan.recolor.iter().find(|(id, _)| *id == m.id).map(|(_, r)| r);
            effect_terms_with(&m.adjust, (m.adjust.amount / 100.0) as f32, r)
        })
        .collect();
    (Some(alpha), terms)
}
