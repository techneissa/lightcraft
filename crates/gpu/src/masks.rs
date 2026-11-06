//! Mask evaluation on the device (`lightcraft_pipeline::masks`): each visible mask's alpha plane
//! and its effect terms.

use std::sync::Arc;

use lightcraft_pipeline::Plan;
use lightcraft_pipeline::finish::{MASK_TERMS, mask_terms};

use crate::ctx::{Buf, groups1, groups2};
use crate::render::{Cx, Host, Prep, gaussian, map};

/// Most brush dabs the mask kernel evaluates per pixel (more: the CPU rasterizes the brush).
const MAX_DABS: usize = 4096;

/// The `shape` kernel's brush data for `dabs`: stroke records (12 words: dab offset, dab count,
/// r, hard, flow, density, erase, bbox x0 y0 x1 y1, auto) then the dab centres.
fn brush_aux(dabs: &[&lightcraft_pipeline::masks::BrushDabs], erase: bool) -> Vec<f32> {
    let mut aux = Vec::new();
    let mut off = 12 * dabs.len();
    for d in dabs {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for q in &d.dabs {
            (x0, y0, x1, y1) = (x0.min(q.x - d.r), y0.min(q.y - d.r), x1.max(q.x + d.r), y1.max(q.y + d.r));
        }
        aux.extend([off as f32, d.dabs.len() as f32, d.r as f32, d.hard as f32, d.flow, d.density, (erase && d.erase) as u8 as f32]);
        aux.extend([x0, y0, x1, y1].map(|v| v as f32));
        aux.push(d.auto as u8 as f32);
        off += 2 * d.dabs.len();
    }
    for d in dabs {
        aux.extend(d.dabs.iter().flat_map(|q| [q.x as f32, q.y as f32]));
    }
    aux
}

/// How [`guided_cross`] combines the filtered plane with its input.
#[derive(Clone, Copy)]
enum Combine {
    /// The larger of the two (Auto Mask strokes).
    Max,
    /// The input moved towards the filtered plane by this much (Refine Edges).
    Mix(f32),
}

/// Cross guided filter (`masks::guided_cross`) of `p` steered by `guide`, combined with `p`.
#[allow(clippy::too_many_arguments)]
fn guided_cross(cx: &mut Cx<'_>, guide: &Buf, p: &Buf, w: usize, h: usize, sigma: f32, eps: f32, how: Combine) -> Buf {
    let n = w * h;
    let (m0, m1) = (cx.gpu.buffer(2 * n), cx.gpu.buffer(2 * n));
    map(cx, "xguided_pre", n, &[0], [Some(guide), Some(p), None], &m0);
    map(cx, "xguided_pre", n, &[1], [Some(guide), Some(p), None], &m1);
    let (b0, b1) = (gaussian(cx, &m0, w, h, 2, sigma), gaussian(cx, &m1, w, h, 2, sigma));
    let ab = cx.gpu.buffer(2 * n);
    map(cx, "xguided_ab", n, &[eps.to_bits()], [Some(&b0), Some(&b1), None], &ab);
    let ab = gaussian(cx, &ab, w, h, 2, sigma);
    let q = cx.gpu.buffer(n);
    let mode = match how {
        Combine::Max => [0, 0],
        Combine::Mix(k) => [1, k.to_bits()],
    };
    map(cx, "xguided_apply", n, &mode, [Some(guide), Some(&ab), Some(p)], &q);
    q
}

/// Evaluate the masks (`lightcraft_pipeline::masks::evaluate`): their alpha planes (concatenated,
/// followed by the blurred chromaticity when local Moiré / Noise need it) and their adjustment
/// terms. Shapes without a kernel (Sky, Subject, …) run on the CPU.
pub(crate) fn masks(cx: &mut Cx<'_>, lin: &Buf, prep: &Prep, plan: &Plan<'_>, host: &mut Host) -> (Option<Buf>, Vec<[f32; MASK_TERMS]>) {
    use lightcraft_develop::{MaskOp, MaskShape};
    let s = &*plan.settings;
    let list: Vec<_> = s.masks.iter().filter(|m| m.visible && !m.components.is_empty()).collect();
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
    for (mi, m) in list.iter().enumerate() {
        // its coordinates on the photo before the warp (`masks::evaluate_one`): through a lens or
        // perspective warp the kernels' affine positions don't hold, so its shapes run on the CPU
        let mframe = plan.frame.for_mask(m.on_photo);
        let frame = &*mframe;
        let warped = frame.mask_warp().is_some();
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
            let kind = match &comp.shape {
                MaskShape::Linear { .. } | MaskShape::Radial { .. } if warped => None,
                MaskShape::Linear { start, end } => {
                    let (a, b) = (frame.norm_to_long(*start), frame.norm_to_long(*end));
                    let d = b - a;
                    p.extend(f32s(&[a.x, a.y, d.x, d.y, d.dot(d).max(1e-12)]));
                    Some(0)
                }
                MaskShape::Radial { center, rx, ry, angle, feather, invert } => {
                    let c = frame.norm_to_long(*center);
                    let (sn, co) = (-angle.to_radians()).sin_cos();
                    p.extend(f32s(&[c.x, c.y, co, sn, rx.max(1e-6), ry.max(1e-6), (*feather / 100.0).clamp(0.0, 1.0)]));
                    p.push(*invert as u32);
                    Some(1)
                }
                MaskShape::LuminanceRange { lo, hi, lo_feather, hi_feather } => {
                    p.extend(f32s(&[*lo, *hi, lo_feather.max(1e-3), hi_feather.max(1e-3)]));
                    p.push(ev.to_bits());
                    Some(2)
                }
                MaskShape::ColorRange { samples, refine } => {
                    let tol = lightcraft_pipeline::masks::color_range_tolerance(*refine);
                    // at most the CPU's samples (`MAX_COLOR_RANGE_SAMPLES`): cost is pixels x samples
                    let samples = samples.iter().take(lightcraft_develop::MAX_COLOR_RANGE_SAMPLES);
                    p.extend([tol.to_bits(), ev.exp2().to_bits(), samples.len() as u32]);
                    aux.extend(samples.flat_map(|s| s.map(|v| v as f32)));
                    Some(3)
                }
                MaskShape::Brush { strokes } => {
                    // The kernel takes at most MAX_DABS across the whole mask, so no more than
                    // that (plus one, to tell "at it" from "past it") is ever built: each stroke
                    // may otherwise construct two million dabs before the sum below rejects them
                    // all (#1520).
                    let mut room = MAX_DABS + 1;
                    let dabs: Vec<_> = strokes
                        .iter()
                        .map(|st| {
                            let d = lightcraft_pipeline::masks::brush_dabs_within(st, frame, w, h, room);
                            room -= d.dabs.len().min(room);
                            d
                        })
                        .collect();
                    // pen pressure varies radius and flow per dab, which the kernel doesn't take: the CPU rasterizes it
                    if dabs.iter().map(|d| d.dabs.len()).sum::<usize>() > MAX_DABS || dabs.iter().any(|d| !d.pressure.is_empty()) {
                        None
                    } else if dabs.iter().any(|d| d.auto) {
                        // Auto Mask: stroke by stroke (each one refined before it's combined)
                        let brush = cx.zeroed(n);
                        let stroke = cx.gpu.buffer(n);
                        for d in &dabs {
                            let mut q = p.clone();
                            q[2] = 4;
                            q[3] = 0;
                            q.push(1);
                            let aux = cx.gpu.upload(&brush_aux(&[d], false));
                            cx.run("shape", &q, &[Some(lin), Some(&prep.log_l), Some(&aux), Some(&stroke), Some(&brush)], groups2(w, h, [16, 16]));
                            let refined = d.auto.then(|| {
                                let (sigma, eps) = lightcraft_pipeline::masks::auto_refine(d.r);
                                guided_cross(cx, &prep.log_l, &stroke, w, h, sigma, eps, Combine::Max)
                            });
                            let op = if d.erase { 2 } else { 1 };
                            cx.run(
                                "combine",
                                &[n as u32, 0, 0, op],
                                &[None, None, None, Some(refined.as_ref().unwrap_or(&stroke)), Some(&brush)],
                                groups1(n),
                            );
                        }
                        if comp.invert {
                            cx.run("finalize", &[n as u32, 0, 0, 1, 1f32.to_bits()], &[None, None, None, Some(&stroke), Some(&brush)], groups1(n));
                        }
                        own = Some(brush);
                        None
                    } else {
                        p.push(dabs.len() as u32);
                        aux = brush_aux(&dabs.iter().collect::<Vec<_>>(), true);
                        Some(4)
                    }
                }
                _ => None,
            };
            let plane = match kind {
                _ if own.is_some() => own,
                Some(k) => {
                    p[2] = k;
                    let aux = (!aux.is_empty()).then(|| cx.gpu.upload(&aux));
                    cx.run("shape", &p, &[Some(lin), Some(&prep.log_l), aux.as_ref(), Some(&comp_plane), Some(&alpha)], groups2(w, h, [16, 16]));
                    None
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
            let refined = guided_cross(cx, &prep.log_l, &plane, w, h, sigma, 0.02, Combine::Mix(k.sqrt()));
            cx.copy_into(&refined, &alpha, mi * n);
        }
        let amt = (m.adjust.amount / 100.0) as f32;
        cx.run("finalize", &[n as u32, 0, mi as u32, 0, amt.to_bits()], &[None, None, None, Some(&comp_plane), Some(&alpha)], groups1(n));
    }
    let terms = list.iter().map(|m| mask_terms(&m.adjust)).collect();
    (Some(alpha), terms)
}
