//! Fork feature (`fork/lama-fill.md`): Remove and Content-Aware spots whose fill is LaMa.
//!
//! [`apply`] runs before the white balance (so the cached fill does not depend on the white balance sliders) and
//! hands a learned fill ([`LearnedFill`]) the pixels around each LaMa spot: the spot's own box and a margin of one
//! box side, which holds everything LaMa's crop reads, so a windowed render (the loupe) and the whole frame fill
//! the same. Only pixels under the spot (its alpha, with feather and opacity as for every spot) change.
//!
//! A spot that gets no learned fill (no provider, no model, cancelled, a draft render without a cached fill, a
//! provider that answers nonsense) is not marked as done: the Classic fill draws it ([`crate::spots::apply_except`]).

use lightcraft_develop::{Spot, SpotFill, SpotMode};
use lightcraft_raster::Rgb32f;

use crate::geometry::Frame;
use crate::learned_fill::LearnedFill;
use crate::spots::{radius_px, smooth};

/// Whether a render draws `spot` with the learned fill: LaMa is the fill of a Remove or Content-Aware spot; Heal and
/// Clone copy a source and ignore it.
pub fn is_learned(spot: &Spot) -> bool {
    spot.fill == SpotFill::Lama && matches!(spot.mode, SpotMode::Remove | SpotMode::Fill)
}

/// Pixels more a learned spot's window must hold around the box a Classic spot works in, for a stroke whose points
/// span `span` pixels along their longer axis (a radius `r` spot): the margin of one box side the fill reads. Zero
/// for every other spot.
pub fn extra_pad(spot: &Spot, r: f64, span: f64) -> f64 {
    if !is_learned(spot) {
        return 0.0;
    }
    let side = if span.is_finite() { span.max(0.0) } else { 0.0 } + 2.0 * r;
    side + MIN_CONTEXT
}

/// Least margin around the spot's box handed to the fill, pixels.
const MIN_CONTEXT: f64 = 8.0;

/// Draw every learned spot of `spots` on `img` (output resolution, `ppl` output pixels per long edge) before the
/// white balance. Says, per spot, whether it is done (a spot that is not learned, or got no learned fill, is not).
pub fn apply(img: &mut Rgb32f, spots: &[Spot], frame: &Frame, ppl: f64, provider: &dyn LearnedFill, draft: bool) -> Vec<bool> {
    let mut done = vec![false; spots.len()];
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || !spots.iter().any(is_learned) {
        return done;
    }
    let to_out = frame.norm_to_out(w, h);
    for (spot, done) in spots.iter().zip(done.iter_mut()) {
        if !is_learned(spot) {
            continue;
        }
        let r = radius_px(spot.size, ppl) as f32;
        let pts: Vec<(f32, f32)> =
            spot.points.iter().map(|p| to_out.apply(*p)).filter(|q| q.x.is_finite() && q.y.is_finite()).map(|q| (q.x as f32, q.y as f32)).collect();
        if pts.is_empty() {
            continue;
        }
        // the stroke's box, as the Classic fill takes it (one radius around the points)
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &pts {
            x0 = x0.min(p.0 - r);
            y0 = y0.min(p.1 - r);
            x1 = x1.max(p.0 + r);
            y1 = y1.max(p.1 + r);
        }
        let (bx0, by0) = ((x0.floor().max(0.0)) as usize, (y0.floor().max(0.0)) as usize);
        let (bx1, by1) = ((x1.ceil() as usize).min(w), (y1.ceil() as usize).min(h));
        if bx1 <= bx0 || by1 <= by0 {
            continue;
        }
        let (bw, bh) = (bx1 - bx0, by1 - by0);
        // the spot's alpha (feather, opacity) in the box; the hole is where it is above zero
        let feather = (spot.feather / 100.0).clamp(0.0, 1.0) as f32;
        let opacity = (spot.opacity / 100.0).clamp(0.0, 1.0) as f32;
        let inner = r * (1.0 - feather * 0.8);
        let mut alpha = vec![0.0f32; bw * bh];
        for y in 0..bh {
            for x in 0..bw {
                let (px, py) = ((bx0 + x) as f32 + 0.5, (by0 + y) as f32 + 0.5);
                let d = pts.iter().map(|p| ((px - p.0).powi(2) + (py - p.1).powi(2)).sqrt()).fold(f32::MAX, f32::min);
                alpha[y * bw + x] = (1.0 - smooth(inner, r, d)) * opacity;
            }
        }
        if !alpha.iter().any(|a| *a > 0.0) {
            // nothing to rebuild (zero opacity): nothing for the Classic fill to draw either
            *done = true;
            continue;
        }
        // the box and a margin of one box side (at least a few pixels), kept inside the picture
        let ctx = bw.max(bh).max(MIN_CONTEXT as usize);
        let (cx0, cy0, cx1, cy1) = (bx0.saturating_sub(ctx), by0.saturating_sub(ctx), (bx1 + ctx).min(w), (by1 + ctx).min(h));
        let (cw, ch) = (cx1 - cx0, cy1 - cy0);
        let mut rgb = Vec::with_capacity(cw * ch * 3);
        let mut hole = vec![0.0f32; cw * ch];
        for y in 0..ch {
            for x in 0..cw {
                rgb.extend_from_slice(&img.get(cx0 + x, cy0 + y));
            }
        }
        for y in 0..bh {
            for x in 0..bw {
                if alpha[y * bw + x] > 0.0 {
                    hole[(by0 - cy0 + y) * cw + (bx0 - cx0 + x)] = 1.0;
                }
            }
        }
        let Some(filled) = provider.fill(&rgb, &hole, cw, ch, draft) else { continue };
        if filled.len() != rgb.len() || filled.iter().any(|v| !v.is_finite()) {
            continue;
        }
        for y in 0..bh {
            for x in 0..bw {
                let a = alpha[y * bw + x];
                if a <= 0.0 {
                    continue;
                }
                let i = ((by0 - cy0 + y) * cw + (bx0 - cx0 + x)) * 3;
                let t = img.get(bx0 + x, by0 + y);
                let v = [filled[i], filled[i + 1], filled[i + 2]];
                img.set(bx0 + x, by0 + y, [t[0] + (v[0] - t[0]) * a, t[1] + (v[1] - t[1]) * a, t[2] + (v[2] - t[2]) * a].map(|c| c.max(0.0)));
            }
        }
        *done = true;
    }
    done
}
