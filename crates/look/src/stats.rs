//! What a picture's look is made of, measured in OkLab on its display-encoded (sRGB) pixels: the spread of its
//! lightness (quantiles), the colour cast of its shadows, midtones and highlights, how colourful it is, and the
//! lightness and colour of each of the colour mixer's eight hue bands. [`loss`] says how far two pictures' looks are
//! apart; the fitter moves sliders to make it small.

use lightcraft_develop::MIXER_HUES;
use lightcraft_raster::{Plane, Rgba8};

/// Lightness quantiles measured (fractions of the pixels).
pub const L_QUANTILES: [f32; 9] = [0.02, 0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95, 0.98];
/// Chroma quantiles measured.
pub const C_QUANTILES: [f32; 2] = [0.5, 0.9];
const BINS: usize = 1024;
/// What a fully colourful pixel still counts in a zone's cast, against 1 + this for a neutral one: the cast is the
/// colour of the walls, white clothes and greys (white balance), not of a big coloured object (that is for Vibrance,
/// Saturation and the mixer); a picture with no neutrals at all still has a cast, its colourful pixels' mean.
const NEUTRAL_FLOOR: f32 = 0.02;
/// OkLab chroma past which a pixel counts as colourful for the cast. Wide enough that walls under a strong white
/// balance (a warm 9500 K) still count, narrow enough that a saturated shirt does not.
const NEUTRAL_CHROMA: f32 = 0.16;
/// OkLab chroma that counts as fully colourful for the hue bands (below it a pixel is partly neutral).
const BAND_CHROMA: f32 = 0.06;

/// One hue band's share of the picture and its average colour.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Band {
    /// Share of the picture's weight that is colourful and in this band (0..1).
    pub share: f32,
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

/// The measured look of a picture (or of one region of it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub l_q: [f32; L_QUANTILES.len()],
    /// The cast of the shadows, midtones and highlights: mean OkLab a, b, neutral pixels weighted far above colourful
    /// ones.
    pub zone_ab: [[f32; 2]; 3],
    /// Share of the weight in each zone.
    pub zone_w: [f32; 3],
    /// Mean OkLab chroma around the zone's cast (how colourful, whatever the cast).
    pub c_mean: f32,
    pub c_q: [f32; C_QUANTILES.len()],
    pub bands: [Band; 8],
    /// Total weight measured (pixels, or the mask's sum); 0: nothing to compare.
    pub weight: f32,
}

/// OkLab of an 8-bit sRGB pixel (Ottosson's published matrices for linear sRGB).
pub fn oklab_srgb8(p: [u8; 4]) -> [f32; 3] {
    use lightcraft_color::transfer::decode_srgb8;
    let (r, g, b) = (decode_srgb8(p[0]), decode_srgb8(p[1]), decode_srgb8(p[2]));
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).max(0.0).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).max(0.0).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).max(0.0).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// HSV hue of an 8-bit pixel in degrees (the colour mixer's bands are placed on it), `None` for greys.
fn hsv_hue(p: [u8; 4]) -> Option<f32> {
    let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    if d < 1.0 {
        return None;
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    Some(h * 60.0)
}

/// How much of hue `h` (degrees) belongs to each mixer band: a tent between neighbouring band centres.
pub fn band_weights(h: f32) -> [f32; 8] {
    let mut w = [0.0f32; 8];
    let n = MIXER_HUES.len();
    for i in 0..n {
        let c0 = MIXER_HUES.get(i).copied().unwrap_or(0.0) as f32;
        let c1 = MIXER_HUES.get((i + 1) % n).copied().unwrap_or(0.0) as f32;
        let span = (c1 - c0).rem_euclid(360.0).max(1.0);
        let t = (h - c0).rem_euclid(360.0) / span;
        if t <= 1.0 {
            if let Some(v) = w.get_mut(i) {
                *v += 1.0 - t;
            }
            if let Some(v) = w.get_mut((i + 1) % n) {
                *v += t;
            }
            break;
        }
    }
    w
}

/// Shadow / midtone / highlight membership of lightness `l` (sums to 1).
fn zones(l: f32) -> [f32; 3] {
    let s = ((0.55 - l) / 0.35).clamp(0.0, 1.0);
    let h = ((l - 0.55) / 0.35).clamp(0.0, 1.0);
    [s, (1.0 - s - h).max(0.0), h]
}

fn quantile(hist: &[f32], total: f32, q: f32, max: f32) -> f32 {
    let target = q * total;
    let mut acc = 0.0f32;
    for (i, &h) in hist.iter().enumerate() {
        if acc + h >= target && h > 0.0 {
            let frac = ((target - acc) / h).clamp(0.0, 1.0);
            return (i as f32 + frac) / hist.len() as f32 * max;
        }
        acc += h;
    }
    max
}

/// The weight of pixel (`x`, `y`) of a `w × h` picture under `mask` (sampled at the same relative place, so the mask
/// may have any size); 1 without one.
fn mask_at(mask: Option<&Plane>, x: usize, y: usize, w: usize, h: usize) -> f32 {
    let Some(m) = mask else { return 1.0 };
    if m.width == 0 || m.height == 0 {
        return 0.0;
    }
    let mx = ((x as f64 + 0.5) / w.max(1) as f64 * m.width as f64) as usize;
    let my = ((y as f64 + 0.5) / h.max(1) as f64 * m.height as f64) as usize;
    let v = m.data.get(my.min(m.height - 1) * m.width + mx.min(m.width - 1)).copied().unwrap_or(0.0);
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// The cast at a lightness with zone memberships `zl`: the zones' casts blended.
fn blend(zl: &[f32; 3], cast: &[[f32; 2]]) -> [f32; 2] {
    let mut c0 = [0.0f32; 2];
    for (zw, m) in zl.iter().zip(cast) {
        c0[0] += zw * m[0];
        c0[1] += zw * m[1];
    }
    c0
}

/// Measure `img`, each pixel weighted by `mask` (none: the whole picture).
pub fn measure(img: &Rgba8, mask: Option<&Plane>) -> Stats {
    let (w, h) = (img.width, img.height);
    let mut l_hist = vec![0.0f32; BINS];
    let mut c_hist = vec![0.0f32; BINS];
    const C_MAX: f32 = 0.4;
    let mut zone_ab = [[0.0f32; 2]; 3];
    let mut zone_w = [0.0f32; 3];
    // the cast's own weights: neutral pixels count, colourful ones barely (see `NEUTRAL_FLOOR`)
    let mut zone_n = [0.0f32; 3];
    let mut c_sum = 0.0f32;
    let mut bands = [[0.0f32; 4]; 8];
    let mut band_w = [0.0f32; 8];
    let mut total = 0.0f32;
    // first pass: lightness, the casts and the hue bands; the colourfulness is measured in a second pass, around each
    // zone's own cast, so a cast is matched by white balance and grading and never by draining the colour
    let mut px: Vec<([f32; 3], f32)> = Vec::with_capacity(w.saturating_mul(h).min(1 << 24));
    for y in 0..h {
        let Some(row) = img.data.get(y * w..(y + 1) * w) else { break };
        for (x, &p) in row.iter().enumerate() {
            let wt = mask_at(mask, x, y, w, h);
            if wt <= 0.0 {
                continue;
            }
            let [l, a, b] = oklab_srgb8(p);
            let c = a.hypot(b);
            px.push(([l, a, b], wt));
            total += wt;
            let li = ((l.clamp(0.0, 1.0) * BINS as f32) as usize).min(BINS - 1);
            if let Some(v) = l_hist.get_mut(li) {
                *v += wt;
            }
            let neutral = (1.0 - c / NEUTRAL_CHROMA).max(0.0) + NEUTRAL_FLOOR;
            for (z, zw) in zones(l).iter().enumerate() {
                let k = zw * wt;
                if let (Some(acc), Some(zwacc), Some(nacc)) = (zone_ab.get_mut(z), zone_w.get_mut(z), zone_n.get_mut(z)) {
                    acc[0] += a * k * neutral;
                    acc[1] += b * k * neutral;
                    *zwacc += k;
                    *nacc += k * neutral;
                }
            }
            let colourful = (c / BAND_CHROMA).min(1.0) * wt;
            if colourful > 0.0
                && let Some(hue) = hsv_hue(p)
            {
                for (i, bw) in band_weights(hue).iter().enumerate() {
                    let k = bw * colourful;
                    if k > 0.0
                        && let (Some(acc), Some(bwacc)) = (bands.get_mut(i), band_w.get_mut(i))
                    {
                        acc[0] += l * k;
                        acc[1] += a * k;
                        acc[2] += b * k;
                        *bwacc += k;
                    }
                }
            }
        }
    }
    let cast: Vec<[f32; 2]> = zone_ab.iter().zip(&zone_n).map(|(acc, zw)| if *zw > 0.0 { [acc[0] / zw, acc[1] / zw] } else { [0.0, 0.0] }).collect();
    for ([l, a, b], wt) in &px {
        let c0 = blend(&zones(*l), &cast);
        let c = (a - c0[0]).hypot(b - c0[1]);
        let ci = ((c.clamp(0.0, C_MAX) / C_MAX * BINS as f32) as usize).min(BINS - 1);
        if let Some(v) = c_hist.get_mut(ci) {
            *v += wt;
        }
        c_sum += c * wt;
    }
    let mut s = Stats { weight: total, ..Stats::default() };
    if total <= 0.0 {
        return s;
    }
    for (q, out) in L_QUANTILES.iter().zip(s.l_q.iter_mut()) {
        *out = quantile(&l_hist, total, *q, 1.0);
    }
    for (q, out) in C_QUANTILES.iter().zip(s.c_q.iter_mut()) {
        *out = quantile(&c_hist, total, *q, C_MAX);
    }
    s.c_mean = c_sum / total;
    for z in 0..3 {
        let (Some(acc), Some(zw), Some(zn)) = (zone_ab.get(z), zone_w.get(z), zone_n.get(z)) else { continue };
        if let (Some(out), Some(ow)) = (s.zone_ab.get_mut(z), s.zone_w.get_mut(z)) {
            *ow = zw / total;
            if *zn > 0.0 {
                *out = [acc[0] / zn, acc[1] / zn];
            }
        }
    }
    for i in 0..8 {
        let (Some(acc), Some(bw), Some(out)) = (bands.get(i), band_w.get(i), s.bands.get_mut(i)) else { continue };
        if *bw > 0.0 {
            *out = Band { share: bw / total, l: acc[0] / bw, a: acc[1] / bw, b: acc[2] / bw };
        }
    }
    s
}

/// How much each part of the look counts in [`loss`].
const W_L: f32 = 0.6;
const W_ZONE: f32 = 6.0;
const W_CHROMA: f32 = 16.0;
const W_BAND: f32 = 4.0;
/// A band counts once both pictures have at least this share in it.
pub const MIN_BAND_SHARE: f32 = 0.01;

/// How far the look of `t` is from `r` (0: the same). Squared differences in OkLab units, weighted so that a
/// visible difference in each part (lightness, cast, colourfulness, a hue band) costs about the same.
pub fn loss(t: &Stats, r: &Stats) -> f64 {
    if t.weight <= 0.0 || r.weight <= 0.0 {
        return 0.0;
    }
    let sq = |x: f32| (x * x) as f64;
    let mut e = 0.0f64;
    for (a, b) in t.l_q.iter().zip(&r.l_q) {
        e += W_L as f64 * sq(a - b);
    }
    for z in 0..3 {
        let (Some(ta), Some(ra), Some(tw), Some(rw)) = (t.zone_ab.get(z), r.zone_ab.get(z), t.zone_w.get(z), r.zone_w.get(z)) else {
            continue;
        };
        let k = (tw * rw).sqrt();
        e += (W_ZONE * k) as f64 * (sq(ta[0] - ra[0]) + sq(ta[1] - ra[1]));
    }
    e += W_CHROMA as f64 * sq(t.c_mean - r.c_mean);
    for (a, b) in t.c_q.iter().zip(&r.c_q) {
        e += W_CHROMA as f64 * 0.5 * sq(a - b);
    }
    for (tb, rb) in t.bands.iter().zip(&r.bands) {
        if tb.share < MIN_BAND_SHARE || rb.share < MIN_BAND_SHARE {
            continue;
        }
        let k = (tb.share * rb.share).sqrt().min(0.5) * 2.0;
        e += (W_BAND * k) as f64 * (sq(tb.l - rb.l) + 2.0 * (sq(tb.a - rb.a) + sq(tb.b - rb.b)));
    }
    e
}
