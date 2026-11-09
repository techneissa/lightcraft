//! Fork feature (`fork/ai-culling.md`): what the eyes of a photo's faces say, read from each face's mesh (MediaPipe's
//! 478 points) and the photo's pixels. No model runs here: the caller hands in the landmarks.
//!
//! - **Eyes closed**: each eye's opening is its eye aspect ratio (the lids' distance at three points over the
//!   corners' distance, in 3-D so a face turned a little reads the same). A face whose eyes average under
//!   [`CLOSED_EAR`] has them closed ([`CLOSED_ONE_EAR`] when only one eye can be read). An eye too small to read, or the far eye of a face turned away, is unknown, never
//!   open.
//! - **Eye focus** (open eyes only): the detail (Laplacian energy) in a box around each eye, against the sharpest parts
//!   of the upper face (down to the nose tip: a beard has more fine detail than any eye) and of the photo, measured at a scale where the eye is about [`FOCUS_EYE_PX`] wide so faces of any size compare.
//!   A shot focused on an ear or on the background has eyes with less detail than those, even when the photo's own
//!   focus score is high.
//!
//! Every position is a fraction of the photo (x of its width, y of its height; z of its width), so the scores do not
//! depend on the size of the picture handed in beyond the detail it holds.

/// The landmarks a face mesh gives: 468 face points, then 5 per iris.
pub const MESH_POINTS: usize = 468;
/// A face whose eyes' mean aspect ratio is below this has them closed. Measured on real portraits: open eyes read
/// 0.24–0.41, eyes looking down 0.13–0.22, blinks and eyes squeezed shut 0.02–0.10.
pub const CLOSED_EAR: f32 = 0.11;
/// The same with only one eye readable (less to go on: a lower line, so a lid lowered by a downward look is not a
/// blink).
pub const CLOSED_ONE_EAR: f32 = 0.08;
/// An eye narrower than this many pixels of the picture is too small to read.
pub const MIN_EYE_PX: f32 = 6.0;
/// The far eye is unknown when it is narrower (in the picture) than this share of the near one: the face is turned.
const TURNED: f32 = 0.6;
/// Both eyes are unknown below this share: the face is in profile.
const PROFILE: f32 = 0.35;
/// The eye width the focus is measured at (the picture is shrunk to it, never enlarged).
pub const FOCUS_EYE_PX: f32 = 32.0;
/// Eyes narrower than this many pixels get no focus score.
pub const MIN_FOCUS_EYE_PX: f32 = 10.0;
/// The photo's sharpest areas count this much against the eyes (the face's count fully): a sharp, busy background
/// alone does not make sharp eyes look soft.
const PHOTO_WEIGHT: f32 = 0.5;
/// The share of a box's strongest values its detail is read from.
const PEAK_SHARE: f32 = 0.1;
/// The upper face's blocks the eyes are measured against: this quantile of them (the brows and the hairline are
/// the top few).
const FACE_QUANTILE: f32 = 0.75;
/// A face is a main face when it is at least this share of the largest face's width.
pub const MAIN_SHARE: f32 = 0.6;

/// Each eye's outer and inner corner, then three pairs of points on the upper and lower lid (the subject's right eye,
/// on the left of the picture, first).
const EYES: [([usize; 2], [[usize; 2]; 3]); 2] =
    [([33, 133], [[160, 144], [159, 145], [158, 153]]), ([263, 362], [[387, 373], [386, 374], [385, 380]])];
/// The nose tip.
const NOSE_TIP: usize = 1;
/// Each eye's contour (16 points round the opening), for its centre.
const OPENINGS: [[usize; 16]; 2] = [
    [33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246],
    [263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466],
];

/// A sRGB-encoded luminance picture, values 0..1, row-major.
pub struct Luma<'a> {
    pub data: &'a [f32],
    pub width: usize,
    pub height: usize,
}

impl Luma<'_> {
    fn valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.width.checked_mul(self.height) == Some(self.data.len())
    }
}

/// One face handed in: its box (fractions, `[x0, y0, x1, y1]`) and its mesh (empty without one).
pub struct FaceIn<'a> {
    pub rect: [f32; 4],
    pub landmarks: &'a [[f32; 3]],
}

/// What one eye says.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Eye {
    /// Its aspect ratio; `None` when it could not be read (too small, the far eye of a turned face, no mesh).
    pub ear: Option<f32>,
    /// Its width in pixels of the picture.
    pub width: f32,
}

/// What one face's eyes say.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FaceEyes {
    /// The subject's right eye, then the left.
    pub eyes: [Eye; 2],
    /// `Some(true)`: closed; `None`: neither eye could be read.
    pub closed: Option<bool>,
    /// Eye focus 0..100 (100: the sharpest eye has the detail of the sharpest parts of the face and photo).
    pub focus: Option<f32>,
    /// One of the photo's largest faces.
    pub main: bool,
}

/// What a photo's main faces say.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    pub faces: u32,
    pub main: u32,
    /// A main face's eyes are closed (`None`: no main face's eyes could be read).
    pub closed: Option<bool>,
    /// The best eye focus of the main faces (`None`: none could be measured).
    pub focus: Option<f32>,
}

/// A landmark in picture pixels (z in the x unit); `None` when it is missing or not a number.
fn point(lm: &[[f32; 3]], k: usize, w: f32, h: f32) -> Option<[f32; 3]> {
    let p = lm.get(k)?;
    let q = [p[0] * w, p[1] * h, p[2] * w];
    q.iter().all(|v| v.is_finite()).then_some(q)
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Both eyes' aspect ratios and widths for a mesh in a `w × h` picture (`None` per eye when it can't be read).
pub fn eyes_of(lm: &[[f32; 3]], w: f32, h: f32) -> [Eye; 2] {
    let mut out = [Eye::default(); 2];
    if lm.len() < MESH_POINTS {
        return out;
    }
    // (aspect ratio in 3-D, width in the picture)
    let read = |(corners, lids): &([usize; 2], [[usize; 2]; 3])| -> Option<(f32, f32)> {
        let (a, b) = (point(lm, corners[0], w, h)?, point(lm, corners[1], w, h)?);
        let across = dist(a, b);
        let mut open = 0.0;
        for [up, down] in lids {
            open += dist(point(lm, *up, w, h)?, point(lm, *down, w, h)?);
        }
        let ear = open / 3.0 / across;
        (ear.is_finite() && across > 0.0).then_some((ear, dist2(a, b)))
    };
    let got = [read(&EYES[0]), read(&EYES[1])];
    let widths = got.map(|g| g.map_or(0.0, |(_, w)| w));
    let wide = widths[0].max(widths[1]);
    for (k, g) in got.iter().enumerate() {
        let Some((ear, width)) = *g else { continue };
        let Some(eye) = out.get_mut(k) else { continue };
        eye.width = width;
        let share = if wide > 0.0 { width / wide } else { 0.0 };
        let readable = width >= MIN_EYE_PX && share >= TURNED && widths.iter().all(|w| *w >= PROFILE * wide);
        eye.ear = readable.then_some(ear);
    }
    out
}

/// Whether a face with these eyes has them closed (`None`: neither could be read).
pub fn closed(eyes: &[Eye; 2]) -> Option<bool> {
    let read: Vec<f32> = eyes.iter().filter_map(|e| e.ear).collect();
    let limit = if read.len() == 1 { CLOSED_ONE_EAR } else { CLOSED_EAR };
    (!read.is_empty()).then(|| read.iter().sum::<f32>() / (read.len() as f32) < limit)
}

/// The detail map of `img` shrunk by `scale` (≤ 1): squared Laplacian of the smoothed luminance.
struct Detail {
    e: Vec<f32>,
    w: usize,
    h: usize,
    /// Picture pixels per map pixel.
    step: f32,
}

/// Most pixels a detail map holds (a 4096² picture at full scale).
const MAX_DETAIL: usize = 1 << 24;

impl Detail {
    fn new(img: &Luma, scale: f32) -> Option<Self> {
        let s = if scale.is_finite() { scale.clamp(0.01, 1.0) } else { 1.0 };
        let (w, h) = (((img.width as f32 * s).round() as usize).max(3), ((img.height as f32 * s).round() as usize).max(3));
        if w.checked_mul(h)? > MAX_DETAIL {
            return None;
        }
        // box-shrink (each map pixel the mean of the picture pixels it covers)
        let mut l = vec![0f32; w * h];
        for (y, row) in l.chunks_mut(w).enumerate() {
            let (y0, y1) = (y * img.height / h, ((y + 1) * img.height / h).max(y * img.height / h + 1).min(img.height));
            for (x, v) in row.iter_mut().enumerate() {
                let (x0, x1) = (x * img.width / w, ((x + 1) * img.width / w).max(x * img.width / w + 1).min(img.width));
                let mut acc = 0.0f32;
                let mut n = 0u32;
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        acc += img.data.get(yy * img.width + xx).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
                        n += 1;
                    }
                }
                *v = acc / n.max(1) as f32;
            }
        }
        // a [1 2 1] smoothing takes most of the pixel noise out before the Laplacian
        let at = |v: &[f32], x: usize, y: usize| v.get(y.min(h - 1) * w + x.min(w - 1)).copied().unwrap_or(0.0);
        let mut sm = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (xl, yl) = (x.saturating_sub(1), y.saturating_sub(1));
                let row = |yy: usize| at(&l, xl, yy) + 2.0 * at(&l, x, yy) + at(&l, x + 1, yy);
                sm[y * w + x] = (row(yl) + 2.0 * row(y) + row(y + 1)) / 16.0;
            }
        }
        let mut e = vec![0f32; w * h];
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let c = sm[y * w + x];
                let lap = 4.0 * c - sm[y * w + x - 1] - sm[y * w + x + 1] - sm[(y - 1) * w + x] - sm[(y + 1) * w + x];
                e[y * w + x] = lap * lap;
            }
        }
        Some(Self { e, w, h, step: img.width as f32 / w as f32 })
    }

    /// The strongest detail in a box of picture pixels: the mean of its top [`PEAK_SHARE`] of values, so the lashes,
    /// the iris's rim and a catchlight count and the plain skin round them does not (`None`: no inner map pixel).
    fn peak(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<f32> {
        let to = |v: f32, n: usize| ((v / self.step).round().max(1.0) as usize).min(n.saturating_sub(1));
        let (ax, bx, ay, by) = (to(x0, self.w), to(x1, self.w), to(y0, self.h), to(y1, self.h));
        if bx <= ax || by <= ay {
            return None;
        }
        let mut v: Vec<f32> = Vec::with_capacity((bx - ax) * (by - ay));
        for y in ay..by {
            v.extend(self.e.get(y * self.w + ax..y * self.w + bx).unwrap_or_default());
        }
        let k = ((v.len() as f32 * PEAK_SHARE).ceil() as usize).clamp(1, v.len());
        let at = v.len() - k;
        v.select_nth_unstable_by(at, f32::total_cmp);
        let top = v.get(at..).unwrap_or_default();
        Some(top.iter().map(|x| f64::from(*x)).sum::<f64>() as f32 / top.len().max(1) as f32)
    }

    /// The `q` quantile of the strongest detail ([`Self::peak`]) of `side`-pixel blocks (picture pixels) inside a box of picture pixels.
    fn blocks(&self, (x0, y0, x1, y1): (f32, f32, f32, f32), side: f32, q: f32) -> Option<f32> {
        let side = side.max(self.step * 3.0);
        let mut v = Vec::new();
        let mut y = y0.max(0.0);
        while y + side <= y1 && v.len() < 1 << 16 {
            let mut x = x0.max(0.0);
            while x + side <= x1 && v.len() < 1 << 16 {
                if let Some(m) = self.peak(x, y, x + side, y + side) {
                    v.push(m);
                }
                x += side;
            }
            y += side;
        }
        if v.is_empty() {
            return None;
        }
        let k = ((v.len() as f32 * q) as usize).min(v.len() - 1);
        let (_, m, _) = v.select_nth_unstable_by(k, f32::total_cmp);
        Some(*m)
    }
}

/// An eye's centre (the mean of its contour) in picture pixels.
fn centre(lm: &[[f32; 3]], ring: &[usize; 16], w: f32, h: f32) -> Option<(f32, f32)> {
    let mut acc = (0.0, 0.0);
    for &k in ring {
        let p = point(lm, k, w, h)?;
        acc = (acc.0 + p[0], acc.1 + p[1]);
    }
    Some((acc.0 / 16.0, acc.1 / 16.0))
}

/// The box round a face's mesh in picture pixels, grown by `grow` of its size on each side.
fn mesh_box(lm: &[[f32; 3]], w: f32, h: f32, grow: f32) -> Option<(f32, f32, f32, f32)> {
    let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in lm.iter().take(MESH_POINTS) {
        let (x, y) = (p[0] * w, p[1] * h);
        if x.is_finite() && y.is_finite() {
            b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
        }
    }
    let (bw, bh) = (b.2 - b.0, b.3 - b.1);
    (bw > 0.0 && bh > 0.0).then_some((b.0 - grow * bw, b.1 - grow * bh, b.2 + grow * bw, b.3 + grow * bh))
}

/// Read the eyes of every face of `img`: closed or open, eye focus, and which faces are the main ones.
pub fn read(img: &Luma, faces: &[FaceIn]) -> Vec<FaceEyes> {
    let mut out = vec![FaceEyes::default(); faces.len()];
    if !img.valid() || faces.is_empty() {
        return out;
    }
    let (w, h) = (img.width as f32, img.height as f32);
    // main faces by the width of their box
    let width = |f: &FaceIn| {
        let v = (f.rect[2] - f.rect[0]) * w;
        if v.is_finite() { v.max(0.0) } else { 0.0 }
    };
    let widest = faces.iter().map(width).fold(0.0f32, f32::max);
    for (o, f) in out.iter_mut().zip(faces) {
        o.main = widest > 0.0 && width(f) >= MAIN_SHARE * widest;
        o.eyes = eyes_of(f.landmarks, w, h);
        o.closed = closed(&o.eyes);
    }
    // focus: one detail map, at the scale where the main faces' widest eye is about `FOCUS_EYE_PX`
    let eye_w = out.iter().filter(|o| o.main).flat_map(|o| o.eyes.iter().filter(|e| e.ear.is_some()).map(|e| e.width)).fold(0.0f32, f32::max);
    if eye_w < MIN_FOCUS_EYE_PX {
        return out;
    }
    let Some(detail) = Detail::new(img, FOCUS_EYE_PX / eye_w) else { return out };
    let photo = detail.blocks((0.0, 0.0, w, h), eye_w, 0.9);
    // (closed eyes have no detail of their own to measure)
    for (o, f) in out.iter_mut().zip(faces).filter(|(o, _)| o.closed == Some(false)) {
        o.focus = focus(&detail, f.landmarks, &o.eyes, photo, (w, h));
    }
    out
}

/// One face's eye focus (see [`read`]).
fn focus(detail: &Detail, lm: &[[f32; 3]], eyes: &[Eye; 2], photo: Option<f32>, (w, h): (f32, f32)) -> Option<f32> {
    let mut best: Option<f32> = None;
    let mut size = 0.0f32;
    for (eye, ring) in eyes.iter().zip(&OPENINGS) {
        if eye.ear.is_none() || eye.width < MIN_FOCUS_EYE_PX {
            continue;
        }
        let Some((cx, cy)) = centre(lm, ring, w, h) else { continue };
        let (hw, hh) = (0.75 * eye.width, 0.45 * eye.width);
        if let Some(m) = detail.peak(cx - hw, cy - hh, cx + hw, cy + hh) {
            best = Some(best.map_or(m, |b| b.max(m)));
            size = size.max(eye.width);
        }
    }
    let eye = best?;
    // the upper face: forehead, brows, eyes, the nose's bridge and the ears' height, down to the nose tip; a beard
    // or a moustache has more fine detail than any eye and is left out
    let (x0, y0, x1, _) = mesh_box(lm, w, h, 0.15)?;
    let nose = point(lm, NOSE_TIP, w, h)?[1];
    let face = detail.blocks((x0, y0, x1, nose), size, FACE_QUANTILE).unwrap_or(0.0);
    let reference = face.max(PHOTO_WEIGHT * photo.unwrap_or(0.0));
    if reference.is_nan() || reference <= 1e-9 {
        return None;
    }
    let f = 100.0 * (eye / reference).clamp(0.0, 1.0);
    f.is_finite().then_some(f)
}

/// The photo's result from its faces' (main faces only; see [`Summary`]).
pub fn summary(faces: &[FaceEyes]) -> Summary {
    let main: Vec<&FaceEyes> = faces.iter().filter(|f| f.main).collect();
    let read: Vec<bool> = main.iter().filter_map(|f| f.closed).collect();
    Summary {
        faces: u32::try_from(faces.len()).unwrap_or(u32::MAX),
        main: u32::try_from(main.len()).unwrap_or(u32::MAX),
        closed: (!read.is_empty()).then(|| read.iter().any(|c| *c)),
        focus: main.iter().filter_map(|f| f.focus).reduce(f32::max),
    }
}

/// The sRGB-encoded luminance of 8-bit RGB pixels (`rgb.len()` = 3 × width × height).
pub fn luma_of_rgb8(rgb: &[u8]) -> Vec<f32> {
    rgb.as_chunks::<3>().0.iter().map(|p| (0.2627 * f32::from(p[0]) + 0.678 * f32::from(p[1]) + 0.0593 * f32::from(p[2])) / 255.0).collect()
}

#[cfg(test)]
mod tests;
