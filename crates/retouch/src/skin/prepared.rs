//! Skin's run in two parts (issue #234): what no slider changes, made once for the pixels, the mask and the place
//! of the image and kept between renders ([`Prepared`]: the pore-free skin, Shine's map of the skin around and of
//! the shine, Smoothness's fade from the mask's edge, and the last grid Smoothness evened with), and the pass a
//! slider tick runs ([`finish`]). A drag of Texture or Shine reruns only that pass; a drag of Smoothness also
//! rebuilds its grid, which is small. Each part is the arithmetic the single run did, so the result is the same,
//! bit for bit.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use lightcraft_raster::{Image, Rgb32f, par_rows};

use super::grid::{Grid, Reduced};
use super::shine::{self, Shine};

use super::{COLOUR_EDGE, FEATHER, PORES, SHINE_AROUND, SHINE_SOFT, SMOOTH, Strength, TONE, TONE_CUT, curve, pixel, soft, split};
use crate::memo::{self, Memo};
use crate::{Part, Scene};

/// The memory Skin's kept work may hold, for every face of the photo, the draft's and the loupe's (about 40 bytes a
/// pixel of a face's box: two 2.5 MP draft boxes, or many head-and-shoulders faces; a box past half of it, such as a
/// large export's, is made and not kept).
const KEPT_BYTES: usize = 256 << 20;

static MEMO: Memo<Prepared> = Memo::new(KEPT_BYTES);

/// Drop the kept work (another photo is open).
pub(super) fn forget() {
    MEMO.clear();
}

/// What no Skin slider changes, for one face's box.
pub(super) struct Prepared {
    /// The box of the image it covers: `[x0, y0, x1, y1)`.
    b: [usize; 4],
    /// The skin without its pores over the box, per pixel `[log luminance, red, blue, mask]` (not weighted by the
    /// mask; 0 where the mask is).
    base: Image<[f32; 4]>,
    /// [`SMOOTH`], [`TONE`], [`FEATHER`], Shine's skin around and its soft edge, in pixels.
    px: Sizes,
    /// Per pixel, how far in from the mask's edge it is: 0 at the edge, 1 a [`FEATHER`] inside it (made with the
    /// first Smoothness above 0).
    fade: OnceLock<Vec<f32>>,
    /// Shine's map per pixel ([`Shine::map`]; made with the first Shine above 0, `None` when it can't be made).
    shine: OnceLock<Option<Image<[f32; 4]>>>,
    /// The grid of the last Smoothness evened with (its bits; `None` with no skin to grid): a drag of Texture or
    /// Shine reuses it.
    even: Mutex<Option<(u32, Option<Arc<Grid>>)>>,
}

#[derive(Clone, Copy)]
struct Sizes {
    smooth: f32,
    tone: f32,
    feather: f32,
    around: f32,
    soft: f32,
}

/// The kept work for face `face`'s box `b` of `img`, made now when none is kept for these pixels, mask and place.
/// `None` when the mask is 0 over the whole box.
pub(super) fn prepared(img: &Rgb32f, scene: &Scene<'_>, face: usize, b: [usize; 4], size: f64) -> Option<Arc<Prepared>> {
    let mut h = memo::Hash::new(0x736b_696e);
    h.u64(memo::pixels(img, b));
    h.u64(memo::mask(scene, face, Part::FacialSkin, b)?);
    MEMO.get_or(h.finish(), || Prepared::new(img, scene, face, b, size), Prepared::bytes)
}

impl Prepared {
    /// The work for face `face`'s box `b` of `img` (face size `size` pixels), made without the cache.
    pub(super) fn new(img: &Rgb32f, scene: &Scene<'_>, face: usize, b: [usize; 4], size: f64) -> Option<Prepared> {
        let mask = scene.faces.get(face)?.face.mask(Part::FacialSkin)?;
        let view = scene.view();
        let [x0, y0, x1, y1] = b;
        let w = x1.checked_sub(x0)?;
        let px = |f: f64| {
            let v = (f * size) as f32;
            if v.is_finite() { v.max(0.0) } else { 0.0 }
        };
        // the skin (split) weighted by the mask, then without its pores: [log luminance, red, blue, mask]
        let mut base = Image::<[f32; 4]>::new(w, y1.checked_sub(y0)?);
        par_rows(&mut base.data, w.max(1), |y, row| {
            let start = (y0 + y) * img.width + x0;
            let src = img.data.get(start..start + w).unwrap_or(&[]);
            for (x, o) in row.iter_mut().enumerate() {
                let (nx, ny) = view.to_norm((x0 + x) as f64 + 0.5, (y0 + y) as f64 + 0.5);
                let mv = mask.sample(nx, ny).clamp(0.0, 1.0);
                let l = split(src.get(x).copied().unwrap_or_default());
                *o = [l[0] * mv, l[1] * mv, l[2] * mv, mv];
            }
        });
        if !base.data.iter().any(|p| p[3] > 0.0) {
            return None;
        }
        let pores = Reduced::blurred(&base, px(PORES));
        par_rows(&mut base.data, w.max(1), |y, row| {
            for (x, o) in row.iter_mut().enumerate() {
                if o[3] <= 0.0 {
                    continue;
                }
                let own = [o[0] / o[3], o[1] / o[3], o[2] / o[3]];
                let p = pores.read(&pores.taps(x, y), own);
                *o = [p[0], p[1], p[2], o[3]];
            }
        });
        drop(pores);
        let px = Sizes { smooth: px(SMOOTH), tone: px(TONE), feather: px(FEATHER), around: px(SHINE_AROUND), soft: px(SHINE_SOFT) };
        Some(Prepared { b, base, px, fade: OnceLock::new(), shine: OnceLock::new(), even: Mutex::new(None) })
    }

    /// Memory it holds once Smoothness and Shine have both been used (its four values a pixel, Shine's four, the
    /// fade and the grid, which is small beside the box).
    fn bytes(&self) -> usize {
        self.base.data.len() * (2 * std::mem::size_of::<[f32; 4]>() + std::mem::size_of::<f32>()) * 9 / 8
    }

    /// The grid Smoothness `smooth` (0…1) evens with: the grid at [`SMOOTH`] with [`TONE_CUT`] of the grid at
    /// [`TONE`] mixed in, its bins as wide as the difference of light it evens across ([`curve`]).
    fn grid(&self, smooth: f32) -> Option<Arc<Grid>> {
        if let Some((s, g)) = &*self.even.lock().unwrap_or_else(PoisonError::into_inner)
            && *s == smooth.to_bits()
        {
            return g.clone();
        }
        let (_, range) = curve(smooth);
        let (sigma, tone) = (self.px.smooth, self.px.tone);
        let g = Grid::new(&self.base, sigma, range).map(|g| Arc::new(g.blurred(sigma, range).mix(&g.blurred(tone, range), TONE_CUT)));
        *self.even.lock().unwrap_or_else(PoisonError::into_inner) = Some((smooth.to_bits(), g.clone()));
        g
    }

    fn fade(&self) -> &[f32] {
        self.fade.get_or_init(|| {
            let blur = Reduced::of_skin(&self.base, self.px.feather).blur(self.px.feather);
            let w = self.base.width.max(1);
            let mut fade = vec![0.0f32; self.base.data.len()];
            par_rows(&mut fade, w, |y, row| {
                for (x, o) in row.iter_mut().enumerate() {
                    let inside = blur.weight(&blur.taps(x, y));
                    *o = ((inside - 0.5) / 0.4).clamp(0.0, 1.0);
                }
            });
            fade
        })
    }

    fn shine(&self) -> Option<&Image<[f32; 4]>> {
        self.shine.get_or_init(|| Shine::new(&self.base, [self.px.around, self.px.soft]).map(|s| s.map(&self.base))).as_ref()
    }
}

/// The even skin Smoothness gives at one setting ([`Prepared::grid`]) and how much of it to take.
struct Even<'a> {
    grid: Arc<Grid>,
    /// How much of the even skin the slider takes ([`curve`]).
    amount: f32,
    /// Per pixel, how far in from the mask's edge it is ([`Prepared::fade`]).
    fade: &'a [f32],
}

impl Even<'_> {
    /// The even skin at (`x`, `y`) of the box (`i` its place in the box) and how much of it to take there (0 at the
    /// mask's edge, the slider's amount a [`FEATHER`] inside it).
    fn at(&self, x: usize, y: usize, i: usize, own: [f32; 3]) -> ([f32; 3], f32) {
        let even = self.grid.read(x, y, own);
        // colour: the grid tells edges by light only, so skin of another colour but the same light (a lit nose
        // wing beside a cheek) can be averaged in; a change of colour past COLOUR_EDGE is held back
        let ce = COLOUR_EDGE;
        let colour = |c: usize| {
            let d = even[c] - own[c];
            d * soft(d.abs() / own[c].abs().max(0.05), ce)
        };
        let fade = self.fade.get(i).copied().unwrap_or(0.0);
        ([even[0], own[1] + colour(1), own[2] + colour(2)], self.amount * fade * fade * (3.0 - 2.0 * fade))
    }
}

/// Retouch `img`'s pixels in the box `p` covers with the sliders `k`, weighted by the mask (pixels where it is 0 keep
/// their bits).
pub(super) fn finish(img: &mut Rgb32f, p: &Prepared, k: Strength) {
    let [x0, y0, x1, y1] = p.b;
    let (w, iw) = (p.base.width, img.width);
    if x1 > iw || w != x1 - x0 || img.data.len() != iw.saturating_mul(img.height) {
        return;
    }
    let even = if k.smooth > 0.0 { p.grid(k.smooth).map(|grid| Even { grid, amount: curve(k.smooth).0, fade: p.fade() }) } else { None };
    let shine = if k.shine > 0.0 { p.shine().map(|s| (s, shine::curve(k.shine))) } else { None };
    let Some(rows) = img.data.get_mut(y0 * iw..y1.min(img.height) * iw) else { return };
    par_rows(rows, iw.max(1), |y, row| {
        let Some(row) = row.get_mut(x0..x1) else { return };
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let Some(b) = p.base.data.get(i) else { continue };
            if b[3] <= 0.0 {
                continue;
            }
            let (src, smooth) = (*o, [b[0], b[1], b[2]]);
            let even = even.as_ref().map(|e| e.at(x, y, i, smooth));
            let shine = shine.and_then(|(s, take)| s.data.get(i).map(|v| shine::change(*v, take)));
            let new = pixel(src, smooth, even, shine, k);
            *o = std::array::from_fn(|c| src[c] + b[3] * (new[c] - src[c]));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Face, Input, NormRect, PartMask};
    use crate::skin::Skin;
    use crate::{Feature, WholePhoto};
    use lightcraft_raster::Plane;

    /// Skin with blotches, pores and a pale highlight, 240 px square, and a face whose Facial Skin mask is a soft
    /// disc over most of it.
    fn photo() -> (Rgb32f, Input) {
        let n = 240;
        let img = Rgb32f::from_fn(n, n, |x, y| {
            let (u, v) = (x as f32 / n as f32, y as f32 / n as f32);
            let pore = ((x * 7919 + y * 104_729) % 97) as f32 / 97.0 - 0.5;
            let blotch = (u * 19.0).sin() * (v * 23.0).cos();
            let shine = (-((u - 0.4).powi(2) + (v - 0.35).powi(2)) / 0.004).exp();
            let k = 1.0 + 0.08 * blotch + 0.05 * pore;
            [0.45 * k + 0.4 * shine, 0.3 * k + 0.42 * shine, 0.24 * k + 0.45 * shine]
        });
        let disc = Plane::from_fn(64, 64, |x, y| (28.0 - ((x as f32 - 31.5).powi(2) + (y as f32 - 31.5).powi(2)).sqrt()).clamp(0.0, 2.0) / 2.0);
        let area = NormRect::new(0.05, 0.05, 0.95, 0.95);
        let face = Face { masks: vec![(Part::FacialSkin, PartMask { plane: disc, area })], ..Face::boxed(area) };
        (img, Input::new(vec![face]))
    }

    /// A drag over all three sliders on the kept work gives the bits a fresh run gives at each step, and a changed
    /// pixel is not answered from the work kept for the old ones.
    #[test]
    fn a_tick_on_kept_work_equals_a_fresh_run() {
        let (mut img, input) = photo();
        let view = WholePhoto { width: img.width, height: img.height };
        let ticks = [(60.0, 0.0, 0.0), (60.0, 40.0, 0.0), (61.0, 40.0, 70.0), (20.0, -50.0, 30.0), (0.0, 0.0, 30.0), (20.0, -50.0, 30.0)];
        for change in [false, true] {
            if change && let Some(p) = img.data.get_mut(120 * 240 + 120) {
                *p = [0.9, 0.1, 0.1];
            }
            let scene = Scene::new(&input, &view, img.width, img.height);
            let (Some(b), Some(size)) = (scene.mask_box(0, Part::FacialSkin), scene.faces.first().map(|f| f.size)) else {
                panic!("the face maps into the photo");
            };
            for (smoothness, texture, shine) in ticks {
                let s = Skin { smoothness, texture, shine };
                let mut kept = img.clone();
                s.apply(&mut kept, &scene, Some(0));
                let mut fresh = img.clone();
                let Some(p) = Prepared::new(&img, &scene, 0, b, size) else { panic!("the face has skin") };
                finish(&mut fresh, &p, Strength::of(&s));
                assert!(kept.data.iter().zip(&fresh.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)), "{s:?}");
                assert!(kept.data != img.data, "{s:?} retouches the skin");
            }
            let (Some(a), Some(b)) = (prepared(&img, &scene, 0, b, size), prepared(&img, &scene, 0, b, size)) else { panic!("kept") };
            assert!(Arc::ptr_eq(&a, &b), "the second tick reuses the work");
        }
    }
}
