//! The brush stroke being painted, drawn on the photo while the button is held (issue #517).
//!
//! The committed stroke is rasterized by the pipeline ([`lightcraft_pipeline::masks::brush_dabs`]
//! and friends): dabs a quarter radius apart along the path, each with a hard core and a smooth
//! feather, flow accumulating up to density. The live stroke places and paints its dabs with the
//! same functions, at screen resolution, so letting go of the button doesn't change what's
//! covered. It's incremental: each frame stamps only the dabs of the samples added since the last
//! one and uploads only the rectangle they touched.

pub mod auto;
pub mod composite;
pub mod target;

use composite::{Blend, Mapping, PxRect};
use egui::{Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions, pos2};
use lightcraft_geom::Point;
use lightcraft_pipeline::masks::{accumulate, auto_similarity, dab_alpha, pressure_flow, pressure_radius, push_dabs_counted, push_pressures};
use std::sync::Arc;

/// The largest live-stroke buffer edge (pixels); a bigger canvas is painted at a lower resolution.
const MAX_SIDE: usize = 4096;

/// What a stroke is painted with; a change (a new canvas size, zoom, brush) repaints it from its
/// first sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveParams {
    /// The area the stroke may cover (screen points): the canvas.
    pub clip: Rect,
    /// Buffer pixels per screen point.
    pub scale: f32,
    /// Brush radius (screen points) and its hard core (no feather), as the pipeline computes them.
    pub r: f32,
    pub hard: f32,
    /// Flow and density, 0..1.
    pub flow: f32,
    pub density: f32,
    /// Overlay colour and its opacity (0..1) at full coverage.
    pub color: [u8; 3],
    pub opacity: f32,
}

/// What a live stroke is blended over: the photo as it is drawn now, and as it would look with the
/// mask covering all of it ([`target`]), both at the size the photo is drawn on `img_rect`.
#[derive(Clone)]
pub struct LiveImages {
    pub base: Arc<ColorImage>,
    pub target: Arc<ColorImage>,
    /// The render keys of the two pictures (a new pair starts the blend afresh).
    pub key: (u64, u64),
    pub img_rect: Rect,
}

/// The picture an Auto Mask stroke reads while it is painted, and where it is drawn.
pub struct AutoCtx<'a> {
    pub guide: &'a auto::Guide,
    pub img_rect: Rect,
}

/// The blended picture and the texture showing it.
struct BlendTex {
    blend: Blend,
    texture: Option<TextureHandle>,
    key: (u64, u64),
    rect: Rect,
    /// Picture pixels changed since the last upload.
    dirty: Option<PxRect>,
}

/// An in-progress stroke's coverage at screen resolution and the texture showing it.
pub struct LiveStroke {
    params: LiveParams,
    w: usize,
    h: usize,
    alpha: Vec<f32>,
    /// Samples stamped so far, and the last one (buffer pixels).
    placed: usize,
    /// Total dabs emitted across every stamp, including the first sample.
    emitted: usize,
    last: Option<Point>,
    /// The pen pressure of the last stamped sample.
    last_pressure: f32,
    /// Buffer pixels painted since the last upload: (x0, y0, x1, y1), exclusive ends.
    dirty: Option<(usize, usize, usize, usize)>,
    texture: Option<TextureHandle>,
    /// Buffer pixels painted since the blend last caught up, and everything painted so far.
    blend_dirty: Option<PxRect>,
    painted: Option<PxRect>,
    blend: Option<BlendTex>,
    /// The loupe render this stroke was committed over, once the button is up: the blend stays
    /// drawn until that render is replaced (no flash of the unadjusted photo while the committed
    /// stroke renders) or [`HOLD_SECS`] pass.
    pub held: Option<(u64, f64)>,
    /// UI-thread milliseconds the last frame spent stamping, blending and uploading.
    last_ms: f64,
}

/// The longest a released stroke's blend stays on screen waiting for the committed render.
pub const HOLD_SECS: f64 = 3.0;

fn grow(d: &mut Option<PxRect>, r: PxRect) {
    *d = Some(match *d {
        Some((a, b, c, e)) => (a.min(r.0), b.min(r.1), c.max(r.2), e.max(r.3)),
        None => r,
    });
}

impl LiveStroke {
    fn new(params: LiveParams) -> LiveStroke {
        let scale = buffer_scale(params.clip, params.scale);
        let params = LiveParams { scale, ..params };
        let (w, h) = buffer_size(params.clip, scale);
        LiveStroke {
            params,
            w,
            h,
            alpha: vec![0.0; w * h],
            placed: 0,
            emitted: 0,
            last: None,
            last_pressure: 1.0,
            dirty: None,
            texture: None,
            blend_dirty: None,
            painted: None,
            blend: None,
            held: None,
            last_ms: 0.0,
        }
    }

    /// Stroke coverage (0..1) at buffer pixel `(x, y)`; 0 outside.
    pub fn coverage(&self, x: usize, y: usize) -> f32 {
        if x >= self.w {
            return 0.0;
        }
        y.checked_mul(self.w).and_then(|i| i.checked_add(x)).and_then(|i| self.alpha.get(i)).copied().unwrap_or(0.0)
    }

    /// Buffer size (pixels) and pixels per screen point.
    pub fn size(&self) -> ([usize; 2], f32) {
        ([self.w, self.h], self.params.scale)
    }

    /// Stamp the dabs of `points` (screen positions of the stroke's samples so far) not yet
    /// painted, at the pen pressures `pressure` (one per point; empty: full pressure), as the
    /// pipeline scales them. Returns `true` when anything new was painted.
    pub fn stamp(&mut self, points: &[Pos2], pressure: &[f32]) -> bool {
        self.stamp_with(points, pressure, None)
    }

    /// [`Self::stamp`] with Auto Mask: each pixel takes a dab's paint by how much it looks like the
    /// pixel under the dab's centre in `auto`'s picture.
    pub fn stamp_with(&mut self, points: &[Pos2], pressure: &[f32], auto: Option<AutoCtx>) -> bool {
        let Some(new) = points.get(self.placed..) else { return false };
        if new.is_empty() {
            return false;
        }
        let LiveParams { clip, scale, r, hard, flow, density, .. } = self.params;
        let (r, hard) = ((r * scale) as f64, (hard * scale) as f64);
        let mut dabs = Vec::new();
        let mut pressures = Vec::new();
        let first = self.placed;
        for (k, q) in new.iter().enumerate() {
            let q = Point::new(((q.x - clip.left()) * scale) as f64, ((q.y - clip.top()) * scale) as f64);
            let before = dabs.len();
            if push_dabs_counted(&mut dabs, &mut self.emitted, self.last, q, r) {
                let pq = pressure.get(first + k).or(pressure.last()).copied().unwrap_or(1.0);
                let from = if self.last.is_some() { self.last_pressure } else { pq };
                push_pressures(&mut pressures, dabs.len() - before, from, pq);
                self.last = Some(q);
                self.last_pressure = pq;
            }
        }
        self.placed = points.len();
        let base = (r, hard);
        // buffer pixel → fraction of the guide's picture
        let to_frac = |b: f64, lo: f32, origin: f32, extent: f32| ((b as f32 / scale + lo) - origin) / extent.max(1e-6);
        for (d, p) in dabs.into_iter().zip(pressures) {
            if !(d.x.is_finite() && d.y.is_finite()) {
                continue;
            }
            let reference = auto.as_ref().map(|a| {
                a.guide.at(
                    to_frac(d.x, clip.left(), a.img_rect.left(), a.img_rect.width()),
                    to_frac(d.y, clip.top(), a.img_rect.top(), a.img_rect.height()),
                )
            });
            let r = pressure_radius(base.0, p);
            let hard = if base.0 > 0.0 { base.1 * r / base.0 } else { base.1 };
            let flow = pressure_flow(flow, p);
            let x0 = (d.x - r).floor().clamp(0.0, self.w as f64) as usize;
            let x1 = (d.x + r).ceil().clamp(0.0, self.w as f64) as usize;
            let y0 = (d.y - r).floor().clamp(0.0, self.h as f64) as usize;
            let y1 = (d.y + r).ceil().clamp(0.0, self.h as f64) as usize;
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            for y in y0..y1 {
                let Some(row) = self.alpha.get_mut(y * self.w + x0..y * self.w + x1) else { continue };
                for (x, v) in (x0..x1).zip(row.iter_mut()) {
                    let dd = Point::new(x as f64 + 0.5, y as f64 + 0.5).dist(d);
                    if dd > r {
                        continue;
                    }
                    let mut a = dab_alpha(dd, r, hard);
                    if let (Some(ctx), Some((rl, rch))) = (auto.as_ref(), reference) {
                        let fx = to_frac(x as f64 + 0.5, clip.left(), ctx.img_rect.left(), ctx.img_rect.width());
                        let fy = to_frac(y as f64 + 0.5, clip.top(), ctx.img_rect.top(), ctx.img_rect.height());
                        let (l, ch) = ctx.guide.at(fx, fy);
                        a *= auto_similarity(l, ch, rl, rch);
                    }
                    *v = accumulate(*v, a, flow, density);
                }
            }
            grow(&mut self.dirty, (x0, y0, x1, y1));
            grow(&mut self.blend_dirty, (x0, y0, x1, y1));
            grow(&mut self.painted, (x0, y0, x1, y1));
        }
        true
    }

    /// The overlay colour of coverage `a`.
    fn pixel(&self, a: f32) -> Color32 {
        let [r, g, b] = self.params.color;
        let alpha = (a * self.params.opacity * 255.0).round().clamp(0.0, 255.0) as u8;
        Color32::from_rgba_unmultiplied(r, g, b, alpha)
    }

    fn region(&self, (x0, y0, x1, y1): (usize, usize, usize, usize)) -> ColorImage {
        let mut px = Vec::with_capacity((x1 - x0) * (y1 - y0));
        for y in y0..y1 {
            for x in x0..x1 {
                px.push(self.pixel(self.coverage(x, y)));
            }
        }
        ColorImage::new([x1 - x0, y1 - y0], px)
    }

    /// Bring the texture up to date (the whole buffer the first time, then only what changed).
    fn upload(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        match (&mut self.texture, self.dirty.take()) {
            (None, _) => {
                let image = self.region((0, 0, self.w, self.h));
                self.texture = Some(ctx.load_texture("brush-live-stroke", image, TextureOptions::LINEAR));
            }
            (Some(_), Some(rect)) => {
                let image = self.region(rect);
                if let Some(t) = &mut self.texture {
                    t.set_partial([rect.0, rect.1], image, TextureOptions::LINEAR);
                }
            }
            (Some(_), None) => {}
        }
        self.texture.as_ref()
    }
}

/// Pixels per screen point of a buffer covering `clip`: `want`, lowered so no edge exceeds
/// [`MAX_SIDE`].
fn buffer_scale(clip: Rect, want: f32) -> f32 {
    let want = if want.is_finite() && want > 0.0 { want } else { 1.0 };
    let long = clip.width().max(clip.height());
    if long.is_finite() && long * want > MAX_SIDE as f32 { MAX_SIDE as f32 / long } else { want }
}

fn buffer_size(clip: Rect, scale: f32) -> (usize, usize) {
    let side = |v: f32| {
        let v = (v * scale).ceil();
        if v.is_finite() { (v.max(1.0) as usize).min(MAX_SIDE) } else { 1 }
    };
    (side(clip.width()), side(clip.height()))
}

/// How the live stroke is drawn this frame.
pub struct Look<'a> {
    /// The photo's pictures to blend the adjustment from (`None`: coverage colour only).
    pub images: Option<&'a LiveImages>,
    /// Draw the coverage colour over the blend too (Show Overlay).
    pub overlay: bool,
    /// Auto Mask: weigh the dabs by the picture under them.
    pub auto: bool,
    /// Egui time (seconds), for the hold after release.
    pub now: f64,
}

impl LiveStroke {
    /// The picture pixels every painted buffer pixel maps to, for the picture drawn on `img_rect`.
    fn mapping(&self, img_rect: Rect, size: [usize; 2]) -> Mapping {
        Mapping { clip: self.params.clip, scale: self.params.scale, img_rect, size }
    }

    /// Bring the blended picture up to date and draw it over the photo. `false` when there is
    /// nothing to draw it from (no pictures of the same size, nothing painted yet).
    fn draw_blend(&mut self, ctx: &egui::Context, painter: &egui::Painter, images: &LiveImages) -> bool {
        let Some(painted) = self.painted else { return false };
        let size = images.base.size;
        let map = self.mapping(images.img_rect, size);
        let stale = self.blend.as_ref().is_none_or(|b| b.key != images.key || b.rect != images.img_rect);
        if stale {
            let Some(blend) = Blend::new(images.base.clone(), images.target.clone()) else {
                self.blend = None;
                return false;
            };
            // a new pair of pictures: everything painted so far is mixed in again
            self.blend = Some(BlendTex { blend, texture: None, key: images.key, rect: images.img_rect, dirty: None });
            self.blend_dirty = Some(painted);
        }
        let Some(mut bt) = self.blend.take() else { return false };
        if let Some(d) = self.blend_dirty.take() {
            let rect = map.picture_rect(d);
            if let Some(done) = bt.blend.update(rect, |x, y| {
                let (bx, by) = map.buffer_px(x, y);
                self.coverage(bx, by)
            }) {
                grow(&mut bt.dirty, done);
            }
        }
        match (&mut bt.texture, bt.dirty.take()) {
            (None, _) => {
                // (it starts as the photo: no copy, the pixels the loupe was made from are shared)
                let mut tex = ctx.load_texture("brush-live-blend", images.base.clone(), TextureOptions::LINEAR);
                // …with what is painted so far mixed in
                if let Some(rect) = self.painted.map(|p| map.picture_rect(p)) {
                    let rect = (rect.0, rect.1, rect.2.min(size[0]), rect.3.min(size[1]));
                    if rect.0 < rect.2 && rect.1 < rect.3 {
                        tex.set_partial([rect.0, rect.1], bt.blend.region(rect), TextureOptions::LINEAR);
                    }
                }
                bt.texture = Some(tex);
            }
            (Some(t), Some(rect)) => t.set_partial([rect.0, rect.1], bt.blend.region(rect), TextureOptions::LINEAR),
            (Some(_), None) => {}
        }
        if let Some(t) = &bt.texture {
            painter.image(t.id(), images.img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        self.blend = Some(bt);
        true
    }

    /// Draw the blend of a stroke whose button is up while the committed render is on its way.
    fn draw_held(&mut self, painter: &egui::Painter, img_rect: Rect) -> bool {
        match &self.blend {
            Some(BlendTex { texture: Some(t), rect, .. }) if *rect == img_rect => {
                painter.image(t.id(), img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                true
            }
            _ => false,
        }
    }

    /// The blended picture drawn under the stroke.
    #[cfg(test)]
    pub(crate) fn blend_image(&self) -> Option<&ColorImage> {
        self.blend.as_ref().map(|b| b.blend.image())
    }

    /// UI-thread milliseconds the last frame spent on this stroke, when it shows the adjustment.
    pub fn live_ms(&self) -> Option<f64> {
        self.blend.as_ref().map(|_| self.last_ms)
    }
}

/// Paint the live stroke whose samples so far are `points` (screen positions) into `painter`,
/// keeping its buffer in `slot` (a new stroke, or new parameters, start it afresh). With
/// `look.images` the stroke shows the adjustment under the brush, not only its coverage colour.
pub fn paint(
    slot: &mut Option<LiveStroke>,
    ctx: &egui::Context,
    painter: &egui::Painter,
    params: LiveParams,
    points: &[Pos2],
    pressure: &[f32],
    look: &Look,
) {
    let fresh = match slot {
        Some(s) => {
            // the same stroke when painted with the same parameters (at the scale it settled on)
            let same = s.params == LiveParams { scale: buffer_scale(params.clip, params.scale), ..params };
            !same || points.len() < s.placed || s.held.is_some()
        }
        None => true,
    };
    if fresh {
        *slot = Some(LiveStroke::new(params));
    }
    let Some(s) = slot else { return };
    let t0 = std::time::Instant::now();
    let guide = look.images.filter(|_| look.auto).map(|i| (auto::guide_for(i.key.0, &i.base), i.img_rect));
    let new_dabs = s.stamp_with(points, pressure, guide.as_ref().map(|(guide, img_rect)| AutoCtx { guide, img_rect: *img_rect }));
    let blended = look.images.is_some_and(|images| s.draw_blend(ctx, painter, images));
    if look.overlay || !blended {
        let clip = s.params.clip;
        // the buffer's pixels may overhang the canvas by a fraction of a pixel: map them exactly
        let (w, h, scale) = (s.w as f32, s.h as f32, s.params.scale);
        let rect = Rect::from_min_size(clip.min, egui::vec2(w / scale, h / scale));
        if let Some(t) = s.upload(ctx) {
            painter.image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
    }
    // (a frame with nothing new to paint says nothing about the cost of painting)
    if new_dabs {
        s.last_ms = t0.elapsed().as_secs_f64() * 1e3;
    }
}

/// Keep drawing the adjustment of a stroke whose button just came up, until the render with the
/// committed stroke replaces the loupe (`main_key` changes) or [`HOLD_SECS`] pass. Returns whether
/// the stroke is still held; when not, drop it.
pub fn paint_held(slot: &mut Option<LiveStroke>, painter: &egui::Painter, img_rect: Rect, main_key: Option<u64>, now: f64) -> bool {
    let Some(s) = slot else { return false };
    let Some((key, since)) = s.held else { return false };
    if Some(key) != main_key || now - since > HOLD_SECS || !s.draw_held(painter, img_rect) {
        *slot = None;
        return false;
    }
    true
}

/// The stroke was committed over the loupe render `main_key`: keep showing it (see [`paint_held`]).
pub fn hold(slot: &mut Option<LiveStroke>, main_key: Option<u64>, now: f64) {
    match (slot.as_mut(), main_key) {
        (Some(s), Some(k)) if s.blend.is_some() => s.held = Some((k, now)),
        _ => *slot = None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    fn params() -> LiveParams {
        LiveParams {
            clip: Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0)),
            scale: 1.0,
            r: 10.0,
            hard: 5.0,
            flow: 1.0,
            density: 1.0,
            color: [230, 30, 40],
            opacity: 0.5,
        }
    }

    /// Issue #517: two samples far apart (a fast stroke) paint a continuous band between them,
    /// not two separate dabs.
    #[test]
    fn a_fast_segment_is_painted_continuously() {
        let mut s = LiveStroke::new(params());
        s.stamp(&[pos2(20.0, 100.0)], &[]);
        s.stamp(&[pos2(20.0, 100.0), pos2(380.0, 100.0)], &[]);
        for x in 20..380 {
            assert!(s.coverage(x, 100) > 0.99, "a gap at x = {x}: {}", s.coverage(x, 100));
        }
        // the feather: full inside the core, fading to nothing at the radius
        assert!(s.coverage(200, 104) > 0.99);
        assert!((0.01..0.99).contains(&s.coverage(200, 108)), "{}", s.coverage(200, 108));
        assert_eq!(s.coverage(200, 111), 0.0);
    }

    /// Painting the samples one frame at a time gives the same coverage as all at once (what the
    /// committed stroke does).
    #[test]
    fn incremental_stamping_matches_painting_the_whole_path() {
        let pts = [pos2(30.0, 40.0), pos2(200.0, 150.0), pos2(210.0, 152.0), pos2(390.0, 20.0)];
        let mut once = LiveStroke::new(LiveParams { flow: 0.4, ..params() });
        once.stamp(&pts, &[]);
        let mut steps = LiveStroke::new(LiveParams { flow: 0.4, ..params() });
        for n in 1..=pts.len() {
            steps.stamp(&pts[..n], &[]);
        }
        assert_eq!(once.alpha, steps.alpha);
    }

    /// Crossing the budget in separate frames must match one stamp, including pressure state.
    #[test]
    fn incremental_stamping_respects_the_whole_stroke_budget() {
        use lightcraft_pipeline::masks::MAX_STROKE_DABS;

        let pts = [pos2(20.0, 20.0), pos2(80.0, 20.0), pos2(200.0, 80.0), pos2(21.0, 20.0)];
        let pressure = [0.2, 0.4, 1.0, 0.9];
        let mut once = LiveStroke::new(LiveParams { flow: 0.4, ..params() });
        once.emitted = MAX_STROKE_DABS - 3;
        once.stamp(&pts, &pressure);
        let mut steps = LiveStroke::new(once.params);
        steps.emitted = MAX_STROKE_DABS - 3;
        for n in 1..=pts.len() {
            steps.stamp(&pts[..n], &pressure[..n]);
        }
        assert_eq!(once.alpha, steps.alpha);
        assert_eq!(once.emitted, MAX_STROKE_DABS);
        assert_eq!(steps.emitted, once.emitted);
        assert_eq!(once.last, Some(Point::new(80.0, 20.0)));
        assert_eq!(steps.last, once.last);
        assert_eq!(once.last_pressure, pressure[1]);
        assert_eq!(steps.last_pressure, once.last_pressure);
        assert!(once.coverage(80, 20) > 0.0);
        assert_eq!(once.coverage(200, 80), 0.0);
        let alpha = steps.alpha.clone();
        let more = [pts[0], pts[1], pts[2], pts[3], pos2(300.0, 100.0)];
        steps.stamp(&more, &[1.0; 5]);
        assert_eq!(steps.alpha, alpha, "later frames cannot paint or reconnect discarded samples");
        assert_eq!(steps.last, once.last);
        assert_eq!(steps.last_pressure, once.last_pressure);
        assert_eq!(steps.emitted, MAX_STROKE_DABS);
    }

    #[test]
    fn hostile_input_paints_nothing_and_does_not_panic() {
        let mut s = LiveStroke::new(params());
        s.stamp(&[pos2(f32::NAN, 3.0), pos2(f32::INFINITY, -1e30), pos2(-5000.0, 9000.0)], &[]);
        assert!(s.alpha.iter().all(|a| *a == 0.0));
        // an enormous canvas is painted at a capped resolution
        let big = LiveStroke::new(LiveParams { clip: Rect::from_min_size(Pos2::ZERO, vec2(1e6, 10.0)), scale: 2.0, ..params() });
        assert!(big.w <= MAX_SIDE && big.h >= 1);
    }

    /// The live stroke scales its dabs by pen pressure as the committed one does: a light touch paints a thinner,
    /// fainter line, and a pressure list shorter than the points (or hostile values) is handled.
    #[test]
    fn pen_pressure_thins_and_fades_the_live_stroke() {
        let pts = [pos2(20.0, 100.0), pos2(380.0, 100.0)];
        let mut full = LiveStroke::new(params());
        full.stamp(&pts, &[]);
        let mut light = LiveStroke::new(params());
        light.stamp(&pts, &[0.2, 0.2]);
        let rows = |s: &LiveStroke| (0..s.h).filter(|y| s.coverage(200, *y) > 0.01).count();
        assert!(rows(&light) < rows(&full), "{} vs {}", rows(&light), rows(&full));
        assert!(light.coverage(200, 100) < full.coverage(200, 100));
        let mut odd = LiveStroke::new(params());
        odd.stamp(&pts, &[f32::NAN]);
        assert!(odd.alpha.iter().all(|v| v.is_finite()));
    }

    /// The live stroke, painted frame by frame at the output's resolution, is the mask the pipeline
    /// builds from the committed stroke (feather, flow, density and pen pressure included).
    #[test]
    fn incremental_stamping_equals_the_committed_mask() {
        use lightcraft_develop::{BrushStroke, MaskShape};
        use lightcraft_pipeline::{
            geometry::Frame,
            masks::{brush_dabs, shape_alpha},
        };
        use lightcraft_raster::{Plane, Rgb32f};

        let (w, h) = (160, 90);
        let frame = Frame::new(w, h, &Default::default(), true);
        let pts = [(0.1, 0.2), (0.3, 0.7), (0.32, 0.72), (0.6, 0.3), (0.9, 0.8)];
        for (feather, flow, density, pressure) in [(0.0, 100.0, 100.0, vec![]), (70.0, 35.0, 80.0, vec![0.3, 0.5, 0.6, 1.0, 0.8])] {
            let stroke = BrushStroke {
                points: pts.iter().map(|(x, y)| Point::new(*x, *y)).collect(),
                size: 0.06,
                feather,
                flow,
                density,
                pressure,
                ..Default::default()
            };
            let committed =
                shape_alpha(&MaskShape::Brush { strokes: vec![stroke.clone()] }, &frame, w, h, &Rgb32f::new(w, h), &Plane::new(w, h), 0.0, None);
            let bd = brush_dabs(&stroke, &frame, w, h);
            let live = LiveParams {
                clip: Rect::from_min_size(Pos2::ZERO, vec2(w as f32, h as f32)),
                scale: 1.0,
                r: bd.r as f32,
                hard: bd.hard as f32,
                flow: bd.flow,
                density: bd.density,
                color: [255, 0, 0],
                opacity: 1.0,
            };
            let to_out = frame.norm_to_out(w, h);
            let screen: Vec<Pos2> = stroke.points.iter().map(|p| to_out.apply(*p)).map(|q| pos2(q.x as f32, q.y as f32)).collect();
            let mut steps = LiveStroke::new(live);
            for n in 1..=screen.len() {
                steps.stamp(&screen[..n], &stroke.pressure[..n.min(stroke.pressure.len())]);
            }
            let worst = steps.alpha.iter().zip(&committed.data).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(worst < 1e-5, "feather {feather}: live and committed coverage differ by {worst}");
            assert!(committed.data.iter().any(|v| *v > 0.5));
        }
    }

    /// Auto Mask while painting: a stroke started on one surface does not paint the surface next to
    /// it, however far the brush reaches over.
    #[test]
    fn auto_mask_keeps_the_live_stroke_on_its_surface() {
        // a 400 x 200 picture: grey on the left half, red on the right
        let px = (0..200)
            .flat_map(|_| (0..400).map(|x| if x < 200 { Color32::from_rgb(120, 120, 120) } else { Color32::from_rgb(200, 40, 40) }))
            .collect();
        let guide = auto::Guide::new(&ColorImage::new([400, 200], px));
        let img_rect = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0));
        let ctx = || Some(AutoCtx { guide: &guide, img_rect });
        // dabs of radius 30 along a grey line that ends 10 px short of the border at x = 200: the
        // last dab reaches 20 px over it
        let pts = [pos2(120.0, 100.0), pos2(190.0, 100.0)];
        let mut plain = LiveStroke::new(LiveParams { r: 30.0, hard: 20.0, ..params() });
        plain.stamp(&pts, &[]);
        let mut auto_stroke = LiveStroke::new(LiveParams { r: 30.0, hard: 20.0, ..params() });
        auto_stroke.stamp_with(&pts, &[], ctx());
        assert!(plain.coverage(205, 100) > 0.9, "without Auto Mask the brush paints over the border");
        assert!(auto_stroke.coverage(180, 100) > 0.9, "the starting surface is painted");
        assert!(auto_stroke.coverage(205, 100) < 0.05, "the next surface is not: {}", auto_stroke.coverage(205, 100));
        // a dab centred on the other surface takes that one as its reference
        auto_stroke.stamp_with(&[pts[0], pts[1], pos2(260.0, 100.0)], &[], ctx());
        assert!(auto_stroke.coverage(250, 100) > 0.9);
        assert!(auto_stroke.coverage(195, 100) > 0.9, "(the grey side stays painted)");
    }
}
