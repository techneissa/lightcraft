//! Match Look's residual Look (fork feature, `fork/match-look-followups.md`): the OkLab offsets of
//! [`lightcraft_develop::ResidualLook`] baked into a 3D lookup table on the display-encoded colour, the place a LUT
//! profile applies. It rides the finishing stage's one LUT slot ([`crate::finish::FinishParams::lut`]): the CPU loop
//! and the GPU kernel both read that slot, so they agree, and a preview and an export render it alike. With a LUT
//! profile too, the two are composed into one table (profile first).

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use lightcraft_develop::{DevelopSettings, LOOK_KNOTS, MIXER_HUES, ResidualLook, Treatment};

use crate::lut::Lut3d;

/// Grid points per axis of the Look's table.
const GRID: usize = 17;
/// Grid points per axis when a LUT profile is composed with the Look.
const GRID_COMPOSED: usize = 33;
/// OkLab chroma that counts as fully colourful for the hue bands (the look crate's measure uses the same).
pub const BAND_CHROMA: f32 = 0.06;

/// How far each kind of offset may go, whatever the settings say (they may come from a hand-edited file).
const MAX_L: f32 = 0.3;
const MAX_AB: f32 = 0.2;
const CHROMA_RANGE: (f32, f32) = (-0.9, 1.5);

/// Shadow / midtone / highlight membership of OkLab lightness `l` (sums to 1).
pub fn zones(l: f32) -> [f32; 3] {
    let s = ((0.55 - l) / 0.35).clamp(0.0, 1.0);
    let h = ((l - 0.55) / 0.35).clamp(0.0, 1.0);
    [s, (1.0 - s - h).max(0.0), h]
}

/// HSV hue of an encoded RGB colour (0..1) in degrees (the colour mixer's bands are placed on it); `None` for greys.
pub fn hsv_hue(c: [f32; 3]) -> Option<f32> {
    let [r, g, b] = c;
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    if d.is_nan() || d < 1.0 / 255.0 {
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

/// Linear sRGB → OkLab (Ottosson's published matrices).
pub fn oklab_from_linear_srgb([r, g, b]: [f32; 3]) -> [f32; 3] {
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).max(0.0).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).max(0.0).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).max(0.0).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OkLab → linear sRGB.
pub fn linear_srgb_from_oklab([l, a, b]: [f32; 3]) -> [f32; 3] {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3,
        -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3,
    ]
}

fn finite_clamp(v: f64, lo: f32, hi: f32) -> f32 {
    let v = v as f32;
    if v.is_finite() { v.clamp(lo, hi) } else { 0.0 }
}

/// The Look on one display-encoded colour (0..1 per channel), at full strength.
pub fn apply(look: &ResidualLook, c: [f32; 3]) -> [f32; 3] {
    use lightcraft_color::transfer::{linear_to_srgb, srgb_to_linear};
    let c = c.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
    let [l, a, b] = oklab_from_linear_srgb(c.map(srgb_to_linear));
    // lightness offset, linear between the knots
    let x = l.clamp(0.0, 1.0) * (LOOK_KNOTS - 1) as f32;
    let i = (x.floor() as usize).min(LOOK_KNOTS - 2);
    let t = x - i as f32;
    let at = |k: usize| finite_clamp(look.lightness.get(k).copied().unwrap_or(0.0), -MAX_L, MAX_L);
    let mut dl = at(i) + (at(i + 1) - at(i)) * t;
    // colourfulness around neutral, then the zones' casts
    let k = 1.0 + finite_clamp(look.chroma, CHROMA_RANGE.0, CHROMA_RANGE.1);
    let (mut na, mut nb) = (a * k, b * k);
    for (zw, cast) in zones(l).iter().zip(&look.cast) {
        na += zw * finite_clamp(cast[0], -MAX_AB, MAX_AB);
        nb += zw * finite_clamp(cast[1], -MAX_AB, MAX_AB);
    }
    // each hue band's own offset, on its colourful pixels
    let colourful = (a.hypot(b) / BAND_CHROMA).min(1.0);
    if colourful > 0.0
        && let Some(h) = hsv_hue(c)
    {
        for (w, band) in band_weights(h).iter().zip(&look.bands) {
            let k = w * colourful;
            if k > 0.0 {
                dl += k * finite_clamp(band[0], -MAX_L, MAX_L);
                na += k * finite_clamp(band[1], -MAX_AB, MAX_AB);
                nb += k * finite_clamp(band[2], -MAX_AB, MAX_AB);
            }
        }
    }
    let lin = linear_srgb_from_oklab([(l + dl).clamp(0.0, 1.0), na, nb]);
    lin.map(|v| if v.is_finite() { linear_to_srgb(v.clamp(0.0, 1.0)) } else { 0.0 })
}

fn table(n: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> Lut3d {
    let s = (n - 1) as f32;
    let mut data = Vec::with_capacity(n * n * n);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                data.push(f([r as f32 / s, g as f32 / s, b as f32 / s]));
            }
        }
    }
    Lut3d { size: n, data, domain_min: [0.0; 3], domain_max: [1.0; 3], title: None }
}

type Key = (Vec<u32>, usize, u32, u32);

/// The last table built (a render reuses it while the Look and profile stay the same).
fn cache() -> &'static Mutex<Option<(Key, Arc<Lut3d>)>> {
    static C: OnceLock<Mutex<Option<(Key, Arc<Lut3d>)>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The finishing stage's LUT for `s`: the LUT profile `profile` (with its amount) as it is, the residual Look's
/// table with its Amount, or both composed into one table. B&W photos skip the Look.
pub fn finish_lut(s: &DevelopSettings, profile: Option<(Arc<Lut3d>, f32)>) -> Option<(Arc<Lut3d>, f32)> {
    let look = &s.look;
    if !look.is_active() || s.treatment == Treatment::Bw {
        return profile;
    }
    let k = (look.amount / 100.0).clamp(0.0, 1.0) as f32;
    let bits = look
        .lightness
        .iter()
        .chain(look.cast.iter().flatten())
        .chain(look.bands.iter().flatten())
        .chain([&look.chroma])
        .map(|v| (*v as f32).to_bits())
        .collect();
    let key: Key = (
        bits,
        profile.as_ref().map_or(0, |(l, _)| Arc::as_ptr(l) as usize),
        profile.as_ref().map_or(0, |(_, a)| a.to_bits()),
        if profile.is_some() { k.to_bits() } else { 0 },
    );
    let mut c = cache().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((ck, lut)) = c.as_ref()
        && *ck == key
    {
        let lut = lut.clone();
        return Some((lut, if profile.is_some() { 1.0 } else { k }));
    }
    let (lut, amount) = match &profile {
        None => (table(GRID, |c| apply(look, c)), k),
        Some((p, pk)) => {
            let composed = table(GRID_COMPOSED, |c| {
                let m = p.apply(c);
                let q = std::array::from_fn(|i| (c[i] + (m[i] - c[i]) * pk).clamp(0.0, 1.0));
                let o = apply(look, q);
                std::array::from_fn(|i| q[i] + (o[i] - q[i]) * k)
            });
            (composed, 1.0)
        }
    };
    let lut = Arc::new(lut);
    *c = Some((key, lut.clone()));
    Some((lut, amount))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(f: impl FnOnce(&mut ResidualLook)) -> ResidualLook {
        let mut l = ResidualLook::default();
        f(&mut l);
        l
    }

    #[test]
    fn oklab_round_trips() {
        for c in [[0.2f32, 0.5, 0.8], [1.0, 1.0, 1.0], [0.0, 0.0, 0.0], [0.9, 0.1, 0.3]] {
            let back = linear_srgb_from_oklab(oklab_from_linear_srgb(c));
            assert!(c.iter().zip(back).all(|(a, b)| (a - b).abs() < 1e-4), "{c:?} → {back:?}");
        }
    }

    #[test]
    fn an_empty_look_changes_nothing_and_is_not_in_the_lut_slot() {
        let s = DevelopSettings::default();
        assert!(s.look.is_identity());
        assert!(finish_lut(&s, None).is_none());
        for c in [[0.2f32, 0.5, 0.8], [0.5, 0.5, 0.5], [0.95, 0.3, 0.1]] {
            let o = apply(&s.look, c);
            assert!(c.iter().zip(o).all(|(a, b)| (a - b).abs() < 1e-3), "{c:?} → {o:?}");
        }
    }

    #[test]
    fn a_warm_cast_warms_the_greys_and_amount_scales_it() {
        let mut s = DevelopSettings { look: look(|l| l.cast = [[0.0, 0.03]; 3]), ..Default::default() };
        let (lut, k) = finish_lut(&s, None).unwrap();
        assert_eq!(k, 1.0);
        let o = lut.apply([0.5, 0.5, 0.5]);
        assert!(o[0] > o[2] + 0.03, "warmer: {o:?}");
        s.look.amount = 50.0;
        assert_eq!(finish_lut(&s, None).unwrap().1, 0.5);
        s.look.amount = 0.0;
        assert!(finish_lut(&s, None).is_none(), "Amount 0 is off");
        s.look.amount = 100.0;
        s.treatment = Treatment::Bw;
        assert!(finish_lut(&s, None).is_none(), "B&W skips the Look");
    }

    #[test]
    fn hostile_numbers_stay_in_range() {
        let l = look(|l| {
            l.lightness = [f64::MAX; LOOK_KNOTS];
            l.cast = [[f64::NAN, -f64::MAX]; 3];
            l.chroma = f64::INFINITY;
            l.bands = [[1e300, -1e300, f64::NAN]; 8];
        });
        for c in [[0.2f32, 0.5, 0.8], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [f32::NAN, 2.0, -1.0]] {
            let o = apply(&l, c);
            assert!(o.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)), "{o:?}");
        }
    }

    #[test]
    fn with_a_lut_profile_both_compose() {
        let swap = table(9, |c| [c[2], c[1], c[0]]);
        let s = DevelopSettings { look: look(|l| l.cast = [[0.0, 0.03]; 3]), ..Default::default() };
        let (lut, k) = finish_lut(&s, Some((Arc::new(swap), 1.0))).unwrap();
        assert_eq!(k, 1.0);
        let c = [0.2f32, 0.5, 0.8];
        let want = apply(&s.look, [0.8, 0.5, 0.2]);
        let got = lut.apply(c);
        assert!(want.iter().zip(got).all(|(a, b)| (a - b).abs() < 0.01), "{want:?} vs {got:?}");
    }

    #[test]
    fn json_and_hash_stay_the_same_without_a_look() {
        let s = DevelopSettings::default();
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("\"look\""), "{json}");
        let mut with = s.clone();
        with.look.cast[0][1] = 0.01;
        assert!(serde_json::to_string(&with).unwrap().contains("\"look\""));
        assert_ne!(with.hash64(), s.hash64());
        let back: DevelopSettings = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
        assert_eq!(back.look, with.look);
    }

    #[test]
    fn a_render_applies_the_look_through_the_finishing_lut() {
        let src = lightcraft_raster::Rgb32f::filled(32, 24, [0.18, 0.18, 0.18]);
        let info = crate::SourceInfo::default();
        let req = crate::RenderRequest::fit(32, 32);
        let plain = crate::render(&src, &info, &DevelopSettings::default(), &req).image;
        let warm = DevelopSettings { look: look(|l| l.cast = [[0.0, 0.04]; 3]), ..Default::default() };
        let out = crate::render(&src, &info, &warm, &req).image;
        let (p, o) = (plain.get(10, 10), out.get(10, 10));
        assert!(o[0] as i32 - o[2] as i32 > p[0] as i32 - p[2] as i32 + 5, "{p:?} → {o:?}");
    }
}
