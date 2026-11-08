//! Fork feature (`fork/lama-fill.md`): Remove and Content-Aware spots whose fill is LaMa.
//!
//! [`apply`] runs before the white balance (so the cached fill does not depend on the white balance sliders) and
//! hands a learned fill ([`LearnedFill`]) the pixels around each LaMa spot: the spot's own box and a margin of one
//! box side, which holds everything LaMa's crop reads, so a windowed render (the loupe) and the whole frame fill
//! the same. Only pixels under the spot (its alpha, with feather and opacity as for every spot) change.
//!
//! The crop is handed over at the size it has in the original's pixels (at most [`FILL_MAX_SIDE`]), not at the
//! render's: a preview, the loupe and an export then show the model the same picture and get the same fill, which
//! is scaled back into the render. All the learned spots of a render share one [`FillRun`], so stopping the fill
//! stops the render's remaining spots too.
//!
//! A spot that gets no learned fill (no provider, no model, cancelled, a draft render without a cached fill, a
//! provider that answers nonsense) is not marked as done: the Classic fill draws it ([`crate::spots::apply_except`]).

use lightcraft_develop::{Spot, SpotFill, SpotMode};
use lightcraft_raster::Rgb32f;

use crate::geometry::Frame;
use crate::learned_fill::{FillRun, LearnedFill};
use crate::learned_resample::{fill_size, scale, scale_hole};
use crate::spots::{radius_px, smooth};

/// Whether a render draws `spot` with the learned fill: LaMa is the fill of a Remove or Content-Aware spot; Heal and
/// Clone copy a source and ignore it.
pub fn is_learned(spot: &Spot) -> bool {
    spot.fill == SpotFill::Lama && matches!(spot.mode, SpotMode::Remove | SpotMode::Fill)
}

/// Pixels more a learned spot's window must hold around the box a Classic spot works in, for a stroke whose points
/// span `span` pixels along their longer axis (a radius `r` spot, `ppl` output pixels to the long edge): the margin of
/// one box side the fill reads. Zero for every other spot.
pub fn extra_pad(spot: &Spot, r: f64, span: f64, ppl: f64) -> f64 {
    if !is_learned(spot) {
        return 0.0;
    }
    let side = if span.is_finite() { span.max(0.0) } else { 0.0 } + 2.0 * r;
    side + min_context(ppl)
}

/// Least margin around the spot's box handed to the fill, in the long edge's fraction (so a preview and an export
/// take the same crop), and never under two pixels.
const MIN_CONTEXT_FRAC: f64 = 0.002;

fn min_context(ppl: f64) -> f64 {
    let ppl = if ppl.is_finite() { ppl.max(0.0) } else { 0.0 };
    (ppl * MIN_CONTEXT_FRAC).max(2.0)
}

/// Longest side of the crop handed to the fill, pixels (the model works on 512 and reads less than this).
pub const FILL_MAX_SIDE: usize = 1536;

/// A pixel box `[x0, x1) × [y0, y1)`.
#[derive(Clone, Copy)]
struct PixelBox {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl PixelBox {
    fn w(self) -> usize {
        self.x1 - self.x0
    }
    fn h(self) -> usize {
        self.y1 - self.y0
    }
}

/// The stroke's box as the Classic fill takes it (one radius around the points), kept inside the `w × h` picture.
fn stroke_box(pts: &[(f32, f32)], r: f32, w: usize, h: usize) -> Option<PixelBox> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pts {
        x0 = x0.min(p.0 - r);
        y0 = y0.min(p.1 - r);
        x1 = x1.max(p.0 + r);
        y1 = y1.max(p.1 + r);
    }
    let b = PixelBox {
        x0: x0.floor().max(0.0) as usize,
        y0: y0.floor().max(0.0) as usize,
        x1: (x1.ceil() as usize).min(w),
        y1: (y1.ceil() as usize).min(h),
    };
    (b.x1 > b.x0 && b.y1 > b.y0).then_some(b)
}

/// The spot's alpha (feather, opacity) in `b`; the hole is where it is above zero.
fn stroke_alpha(spot: &Spot, pts: &[(f32, f32)], r: f32, b: PixelBox) -> Vec<f32> {
    let feather = (spot.feather / 100.0).clamp(0.0, 1.0) as f32;
    let opacity = (spot.opacity / 100.0).clamp(0.0, 1.0) as f32;
    let inner = r * (1.0 - feather * 0.8);
    let mut alpha = vec![0.0f32; b.w() * b.h()];
    for y in 0..b.h() {
        for x in 0..b.w() {
            let (px, py) = ((b.x0 + x) as f32 + 0.5, (b.y0 + y) as f32 + 0.5);
            let d = pts.iter().map(|p| ((px - p.0).powi(2) + (py - p.1).powi(2)).sqrt()).fold(f32::MAX, f32::min);
            alpha[y * b.w() + x] = (1.0 - smooth(inner, r, d)) * opacity;
        }
    }
    alpha
}

/// Draw every learned spot of `spots` on `img` (output resolution, `ppl` output pixels per long edge) before the
/// white balance. `reference_long` is the long edge of the original in pixels: the fill sees each crop at the size it
/// has there. Says, per spot, whether it is done (a spot that is not learned, or got no learned fill, is not).
pub fn apply(img: &mut Rgb32f, spots: &[Spot], frame: &Frame, ppl: f64, reference_long: f64, provider: &dyn LearnedFill, draft: bool) -> Vec<bool> {
    let run = FillRun::new();
    let at = Where { frame, ppl, reference_long, draft };
    spots.iter().map(|spot| apply_one(img, spot, &at, provider, &run)).collect()
}

/// Where a render draws its learned spots: its frame, output pixels per long edge, the original's long edge (pixels)
/// and whether it follows a drag (see [`apply`]).
#[derive(Clone, Copy)]
pub struct Where<'a> {
    pub frame: &'a Frame,
    pub ppl: f64,
    pub reference_long: f64,
    pub draft: bool,
}

/// The stroke of `spot` in a `w × h` render, its box and the crop its fill reads (`None`: it draws nothing there).
fn boxes(spot: &Spot, frame: &Frame, w: usize, h: usize, ppl: f64) -> Option<(Vec<(f32, f32)>, f32, PixelBox, PixelBox)> {
    let to_out = frame.norm_to_out(w, h);
    let r = radius_px(spot.size, ppl) as f32;
    let pts: Vec<(f32, f32)> =
        spot.points.iter().map(|p| to_out.apply(*p)).filter(|q| q.x.is_finite() && q.y.is_finite()).map(|q| (q.x as f32, q.y as f32)).collect();
    let b = (!pts.is_empty()).then(|| stroke_box(&pts, r, w, h)).flatten()?;
    // the box and a margin of one box side (at least a few pixels), kept inside the picture
    let ctx = b.w().max(b.h()).max(min_context(ppl).ceil() as usize);
    let c = PixelBox { x0: b.x0.saturating_sub(ctx), y0: b.y0.saturating_sub(ctx), x1: (b.x1 + ctx).min(w), y1: (b.y1 + ctx).min(h) };
    Some((pts, r, b, c))
}

/// Where a learned spot draws (its box) and what its fill reads (the crop), in a `w × h` render: `[x0, y0, x1, y1)`.
pub fn footprint(spot: &Spot, frame: &Frame, w: usize, h: usize, ppl: f64) -> Option<([usize; 4], [usize; 4])> {
    if !is_learned(spot) {
        return None;
    }
    let (_, _, b, c) = boxes(spot, frame, w, h, ppl)?;
    Some(([b.x0, b.y0, b.x1, b.y1], [c.x0, c.y0, c.x1, c.y1]))
}

/// Draw one learned spot on `img` (see [`apply`]), sharing `run` with the render's other spots: whether it is done.
pub fn apply_one(img: &mut Rgb32f, spot: &Spot, at: &Where<'_>, provider: &dyn LearnedFill, run: &FillRun) -> bool {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || !is_learned(spot) || run.stopped() {
        // (stopped: ✕ on the fill's row; the rest of this render's spots show the Classic fill)
        return false;
    }
    let ppl = at.ppl;
    // render pixels to the original's: the fill sees the crop at the original's size, whatever the render's
    let k = if ppl.is_finite() && ppl > 0.0 && at.reference_long.is_finite() && at.reference_long > 0.0 { at.reference_long / ppl } else { 1.0 };
    let Some((pts, r, b, c)) = boxes(spot, at.frame, w, h, ppl) else { return false };
    let alpha = stroke_alpha(spot, &pts, r, b);
    if !alpha.iter().any(|a| *a > 0.0) {
        // nothing to rebuild (zero opacity): nothing for the Classic fill to draw either
        return true;
    }
    let job = Job { alpha: &alpha, b, c, k };
    let Some(filled) = fill_crop(img, &job, run, provider, at.draft) else { return false };
    blend(img, &job, &filled);
    true
}

/// One spot's crop `c` of the picture, the box `b` inside it that `alpha` covers, and the scale `k` from render pixels
/// to the original's.
#[derive(Clone, Copy)]
struct Job<'a> {
    alpha: &'a [f32],
    b: PixelBox,
    c: PixelBox,
    k: f64,
}

/// The learned fill of the job's crop (hole: its alpha), as a picture of the crop's size; handed over and taken back at
/// the size `k` gives (see the module notes).
fn fill_crop(img: &Rgb32f, job: &Job<'_>, run: &FillRun, provider: &dyn LearnedFill, draft: bool) -> Option<Vec<f32>> {
    let Job { alpha, b, c, k } = *job;
    let (cw, ch) = (c.w(), c.h());
    let mut rgb = Vec::with_capacity(cw * ch * 3);
    let mut hole = vec![0.0f32; cw * ch];
    for y in 0..ch {
        for x in 0..cw {
            rgb.extend_from_slice(&img.get(c.x0 + x, c.y0 + y));
        }
    }
    for y in 0..b.h() {
        for x in 0..b.w() {
            if alpha[y * b.w() + x] > 0.0 {
                hole[(b.y0 - c.y0 + y) * cw + (b.x0 - c.x0 + x)] = 1.0;
            }
        }
    }
    let (fw, fh) = fill_size(cw, ch, k, FILL_MAX_SIDE);
    let same = (fw, fh) == (cw, ch);
    let (rgb, hole) = if same { (rgb, hole) } else { (scale(&rgb, (cw, ch), (fw, fh), 3), scale_hole(&hole, (cw, ch), (fw, fh))) };
    let theirs = provider.fill(run, &rgb, &hole, fw, fh, draft)?;
    if theirs.len() != fw * fh * 3 || theirs.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let back = if same { theirs.as_ref().clone() } else { scale(&theirs, (fw, fh), (cw, ch), 3) };
    (back.len() == cw * ch * 3).then_some(back)
}

/// Draw `filled` (the job's crop) over `img` under the job's alpha.
fn blend(img: &mut Rgb32f, job: &Job<'_>, filled: &[f32]) {
    let Job { alpha, b, c, .. } = *job;
    let cw = c.w();
    for y in 0..b.h() {
        for x in 0..b.w() {
            let a = alpha[y * b.w() + x];
            if a <= 0.0 {
                continue;
            }
            let i = ((b.y0 - c.y0 + y) * cw + (b.x0 - c.x0 + x)) * 3;
            let t = img.get(b.x0 + x, b.y0 + y);
            let Some(v) = filled.get(i..i + 3) else { continue };
            img.set(b.x0 + x, b.y0 + y, [t[0] + (v[0] - t[0]) * a, t[1] + (v[1] - t[1]) * a, t[2] + (v[2] - t[2]) * a].map(|c| c.max(0.0)));
        }
    }
}
