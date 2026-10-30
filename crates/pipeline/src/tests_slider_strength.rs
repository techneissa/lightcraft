//! Slider strength audit: how much each Develop slider changes a synthetic picture at its ends.
//!
//! Every number comes from a real render of a procedural image (flat patches, ramps, steps, noise,
//! multi-scale texture), never from reading constants. `slider_strength_report` prints the whole
//! table (`cargo test -p lightcraft-pipeline --release slider_strength_report -- --ignored --nocapture`);
//! it is the source of `docs/slider-strength.md`. The other tests here pin a minimum strength for the
//! sliders that were retuned, so they cannot go weak again.

use std::fmt::Write as _;

use lightcraft_color::perceptual::{hsv_to_rgb, lab_to_lch, oklab_from_2020, oklab_to_2020};
use lightcraft_color::transfer::srgb_to_linear;
use lightcraft_color::{REC2020, SRGB};
use lightcraft_develop::{DevelopSettings, MIXER_HUES, WbMode, WbScale, controls};
use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f, Rgba8};

use crate::{RenderRequest, SourceInfo, render};

// ---------------------------------------------------------------- helpers

/// A raw source whose as-shot white is 5500 K, no camera matrices (Bradford white balance).
fn raw_info() -> SourceInfo {
    SourceInfo { raw: true, as_shot_temp: 5500.0, as_shot_tint: 0.0, ..SourceInfo::default() }
}

fn settings(ids: &[(&str, f64)]) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    for (id, v) in ids {
        assert!(controls::set(&mut s, id, *v), "unknown control {id}");
    }
    s
}

fn render_with(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings) -> Rgba8 {
    render(src, info, s, &RenderRequest::fit(src.width, src.height)).image
}

/// A row of flat `PATCH`-px squares.
const PATCH: usize = 16;

fn patches(values: &[[f32; 3]]) -> Rgb32f {
    Rgb32f::from_fn(PATCH * values.len(), PATCH, |x, _| values.get(x / PATCH).copied().unwrap_or([0.0; 3]))
}

fn patch_px(img: &Rgba8, i: usize) -> [u8; 4] {
    img.data[(PATCH / 2) * img.width + i * PATCH + PATCH / 2]
}

/// OkLab of an 8-bit sRGB pixel.
fn lab8(p: [u8; 4]) -> [f32; 3] {
    let lin = [p[0], p[1], p[2]].map(|v| srgb_to_linear(v as f32 / 255.0));
    oklab_from_2020(SRGB.to_space(&REC2020).apply_f32(lin))
}

fn lch8(p: [u8; 4]) -> [f32; 3] {
    lab_to_lch(lab8(p))
}

/// Signed hue difference in degrees.
fn dhue(a: f32, b: f32) -> f32 {
    let d = (b - a).to_degrees().rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// Scene-linear Rec.2020 of an sRGB-hsv colour.
fn hsv_scene(h: f32, s: f32, v: f32) -> [f32; 3] {
    let lin = hsv_to_rgb(h, s, v).map(srgb_to_linear);
    SRGB.to_space(&REC2020).apply_f32(lin)
}

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut v = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    v ^= v >> 13;
    v = v.wrapping_mul(0x5bd1_e995);
    v ^= v >> 15;
    (v & 0xffff) as f32 / 32768.0 - 1.0
}

/// Roughly Gaussian, unit-variance noise.
fn gauss(x: usize, y: usize, seed: u32) -> f32 {
    let s: f32 = (0..4).map(|k| hash(x as i32, y as i32, seed.wrapping_add(k * 977))).sum();
    s * (3.0f32 / 4.0).sqrt()
}

fn value_noise(x: f32, y: f32, period: f32, seed: u32) -> f32 {
    let (u, v) = (x / period, y / period);
    let (x0, y0) = (u.floor(), v.floor());
    let (fx, fy) = (u - x0, v - y0);
    let (fx, fy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let h = |i: i32, j: i32| hash(x0 as i32 + i, y0 as i32 + j, seed);
    let a = h(0, 0) + (h(1, 0) - h(0, 0)) * fx;
    let b = h(0, 1) + (h(1, 1) - h(0, 1)) * fx;
    a + (b - a) * fy
}

fn encoded_luma(img: &Rgba8) -> Plane {
    Plane::from_fn(img.width, img.height, |x, y| {
        let p = img.data[y * img.width + x];
        (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0
    })
}

fn std_dev(v: &[f32]) -> f32 {
    let n = v.len().max(1) as f32;
    let m = v.iter().sum::<f32>() / n;
    (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / n).sqrt()
}

/// Values of `p` inside the rectangle.
fn region(p: &Plane, x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<f32> {
    (y0..y1).flat_map(|y| (x0..x1).map(move |x| p.data[y * p.width + x])).collect()
}

fn band_energy(luma: &Plane, fine: f32, coarse: f32) -> f32 {
    let (a, b) = (gaussian(luma, fine), gaussian(luma, coarse));
    let m = 40; // stay clear of the borders
    let d: Vec<f32> =
        region(&a, m, m, a.width - m, a.height - m).iter().zip(region(&b, m, m, b.width - m, b.height - m)).map(|(p, q)| p - q).collect();
    std_dev(&d)
}

// ---------------------------------------------------------------- colour

/// OkLab (L, a, b) of a mid-grey patch after a white balance, a raw source shot at 5500 K.
pub(crate) fn wb_grey(temp: f64, tint: f64) -> [f32; 3] {
    let mut s = settings(&[("wb.temp", temp), ("wb.tint", tint)]);
    s.wb.mode = WbMode::Custom;
    s.wb.scale = WbScale::Kelvin;
    let img = render_with(&patches(&[[0.18; 3]]), &raw_info(), &s);
    lab8(patch_px(&img, 0))
}

/// Chroma of `colour` after `ids`, relative to its chroma with nothing set.
pub(crate) fn chroma_ratio(colour: [f32; 3], ids: &[(&str, f64)]) -> f32 {
    let src = patches(&[colour]);
    let base = lch8(patch_px(&render_with(&src, &SourceInfo::default(), &DevelopSettings::default()), 0));
    let out = lch8(patch_px(&render_with(&src, &SourceInfo::default(), &settings(ids)), 0));
    out[1] / base[1].max(1e-6)
}

/// OkLCh colour of a scene-linear colour patch, after nothing / after `ids`.
fn lch_pair(colour: [f32; 3], ids: &[(&str, f64)]) -> ([f32; 3], [f32; 3]) {
    let src = patches(&[colour]);
    let base = lch8(patch_px(&render_with(&src, &SourceInfo::default(), &DevelopSettings::default()), 0));
    let out = lch8(patch_px(&render_with(&src, &SourceInfo::default(), &settings(ids)), 0));
    (base, out)
}

/// A colour of OkLab lightness `l`, chroma `c` and hue `h` (degrees) as scene-linear Rec.2020.
fn lch_scene(l: f32, c: f32, h: f32) -> [f32; 3] {
    oklab_to_2020([l, c * h.to_radians().cos(), c * h.to_radians().sin()])
}

/// (hue shift in degrees, chroma ratio, lightness change) of the colour mixer's `band` at `v`.
pub(crate) fn mixer_effect(band: usize, field: &str, v: f64) -> (f32, f32, f32) {
    let name = ["red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta"][band];
    let colour = hsv_scene(MIXER_HUES[band] as f32, 0.6, 0.75);
    let id = format!("mixer.{name}.{field}");
    let (b, o) = lch_pair(colour, &[(id.as_str(), v)]);
    (dhue(b[2], o[2]), o[1] / b[1].max(1e-6), o[0] - b[0])
}

/// Chroma added (OkLab units) and hue of a colour-grading wheel on a neutral patch of scene luminance `y`.
pub(crate) fn grading_tint(wheel: &str, sat: f64, y: f32, extra: &[(&str, f64)]) -> (f32, f32) {
    let mut ids = vec![(format!("grading.{wheel}.hue"), 220.0), (format!("grading.{wheel}.sat"), sat)];
    ids.extend(extra.iter().map(|(k, v)| (k.to_string(), *v)));
    let ids: Vec<(&str, f64)> = ids.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let (_, o) = lch_pair([y; 3], &ids);
    (o[1], o[2].to_degrees())
}

/// Lightness change of a neutral patch of scene luminance `y` from a grading wheel's Luminance.
pub(crate) fn grading_lum(wheel: &str, v: f64, y: f32) -> f32 {
    let id = format!("grading.{wheel}.lum");
    let (b, o) = lch_pair([y; 3], &[(id.as_str(), v)]);
    o[0] - b[0]
}

/// Encoded-value change (0..255 scale) of a grey ramp from `ids`: (largest rise, largest fall).
pub(crate) fn ramp_shift(ids: &[(&str, f64)]) -> (f32, f32) {
    let ramp: Vec<[f32; 3]> = (0..32).map(|i| [0.003 * 1.2f32.powi(i); 3]).collect();
    let src = patches(&ramp);
    let base = render_with(&src, &SourceInfo::default(), &DevelopSettings::default());
    let out = render_with(&src, &SourceInfo::default(), &settings(ids));
    let d: Vec<f32> = (0..ramp.len()).map(|i| patch_px(&out, i)[1] as f32 - patch_px(&base, i)[1] as f32).collect();
    (d.iter().cloned().fold(0.0, f32::max), -d.iter().cloned().fold(0.0, f32::min))
}

// ---------------------------------------------------------------- detail

/// A multi-scale texture: value noise of periods 3, 12 and 48 px, 0.3 EV each, around scene grey.
pub(crate) fn texture_scene(w: usize, h: usize) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let ev = 0.3 * (value_noise(fx, fy, 3.0, 1) + value_noise(fx, fy, 12.0, 2) + value_noise(fx, fy, 48.0, 3));
        [0.18 * ev.exp2(); 3]
    })
}

/// Band energy of the render relative to the unedited render: (fine 1-2 px, medium 4-8 px, coarse 16-32 px, overall std).
pub(crate) fn texture_bands(src: &Rgb32f, ids: &[(&str, f64)]) -> [f32; 4] {
    let info = SourceInfo::default();
    let (b, o) = (encoded_luma(&render_with(src, &info, &DevelopSettings::default())), encoded_luma(&render_with(src, &info, &settings(ids))));
    let scales = [(1.0, 2.0), (4.0, 8.0), (16.0, 32.0)];
    let mut out = [0.0; 4];
    for (k, (f, c)) in scales.iter().enumerate() {
        out[k] = band_energy(&o, *f, *c) / band_energy(&b, *f, *c).max(1e-9);
    }
    out[3] = std_dev(&o.data) / std_dev(&b.data).max(1e-9);
    out
}

/// A hazy scene: the texture scene seen through a veil (transmission 0.55, airlight 0.7) with colour variety.
pub(crate) fn hazy_scene(w: usize, h: usize) -> Rgb32f {
    let t = texture_scene(w, h);
    Rgb32f::from_fn(w, h, |x, y| {
        let v = t.data[y * w + x][0];
        let tint = [1.0 + 0.5 * value_noise(x as f32, y as f32, 90.0, 9), 1.0, 1.0 - 0.5 * value_noise(x as f32, y as f32, 90.0, 9)];
        [0, 1, 2].map(|k| 1.5 * v * tint[k] * 0.55 + 0.7 * 0.45)
    })
}

/// (5th–95th percentile spread of encoded luma, mean encoded OkLab chroma) of the render.
pub(crate) fn spread_and_chroma(src: &Rgb32f, ids: &[(&str, f64)]) -> (f32, f32) {
    let img = render_with(src, &SourceInfo::default(), &settings(ids));
    let mut l: Vec<f32> = encoded_luma(&img).data;
    l.sort_by(f32::total_cmp);
    let spread = l[l.len() * 95 / 100] - l[l.len() * 5 / 100];
    let chroma = img.data.iter().map(|p| lch8(*p)[1]).sum::<f32>() / img.data.len() as f32;
    (spread, chroma)
}

/// Step edge (soft, σ 1.2 px) in the top half, noise-only flat area at the bottom.
fn edge_scene(w: usize, h: usize, noise: f32) -> Rgb32f {
    let soft = Plane::from_fn(w, h, |x, y| if y < h / 2 && x > w / 2 { 0.4 } else { 0.1 });
    let soft = gaussian(&soft, 1.2);
    Rgb32f::from_fn(w, h, |x, y| {
        let n = (noise * gauss(x, y, 5)).exp2();
        [soft.data[y * w + x] * n; 3]
    })
}

/// Edge acutance and noise gain of sharpening: (peak edge slope ratio, overshoot as share of the step, noise std ratio).
pub(crate) fn sharpen_effect(ids: &[(&str, f64)]) -> (f32, f32, f32) {
    let (w, h) = (400, 400);
    let src = edge_scene(w, h, 0.0);
    let noisy = edge_scene(w, h, 0.06);
    let info = SourceInfo::default();
    let row = |img: &Rgba8| -> Vec<f32> { (0..w).map(|x| img.data[(h / 4) * w + x][1] as f32).collect() };
    let slope = |r: &[f32]| r.windows(2).map(|p| (p[1] - p[0]).abs()).fold(0.0, f32::max);
    let base = row(&render_with(&src, &info, &DevelopSettings::default()));
    let out = row(&render_with(&src, &info, &settings(ids)));
    let hi = base[w - 20];
    let step = hi - base[20];
    let over = (out.iter().cloned().fold(0.0, f32::max) - hi) / step.max(1.0);
    let nb = encoded_luma(&render_with(&noisy, &info, &DevelopSettings::default()));
    let no = encoded_luma(&render_with(&noisy, &info, &settings(ids)));
    let flat = |p: &Plane| std_dev(&region(p, 40, h / 2 + 40, w - 40, h - 40));
    (slope(&out) / slope(&base).max(1e-6), over.max(0.0), flat(&no) / flat(&nb).max(1e-9))
}

/// Noise std of a flat patch (luma, chroma) and the edge contrast kept, relative to NR off.
pub(crate) fn nr_effect(ids: &[(&str, f64)]) -> (f32, f32, f32) {
    let (w, h) = (400, 400);
    let info = SourceInfo::default();
    // luma noise 0.2 EV, colour noise 0.25 EV per channel, on a flat grey, and on a step edge
    let noisy = Rgb32f::from_fn(w, h, |x, y| {
        let base = if y < h / 2 && x > w / 2 { 0.36 } else { 0.18 };
        let l = (0.2 * gauss(x, y, 11)).exp2();
        [0, 1, 2].map(|k| base * l * (0.25 * gauss(x, y, 21 + k as u32)).exp2())
    });
    let b = render_with(&noisy, &info, &DevelopSettings::default());
    let o = render_with(&noisy, &info, &settings(ids));
    let luma_std = |img: &Rgba8| std_dev(&region(&encoded_luma(img), 40, h / 2 + 40, w - 40, h - 40));
    let chroma_std = |img: &Rgba8| {
        let v: Vec<f32> = (h / 2 + 40..h - 40).flat_map(|y| (40..w - 40).map(move |x| (x, y))).map(|(x, y)| lab8(img.data[y * w + x])[1]).collect();
        std_dev(&v)
    };
    let contrast = |img: &Rgba8| {
        let l = encoded_luma(img);
        let hi = region(&l, w / 2 + 20, 40, w - 40, h / 2 - 40);
        let lo = region(&l, 40, 40, w / 2 - 20, h / 2 - 40);
        hi.iter().sum::<f32>() / hi.len() as f32 - lo.iter().sum::<f32>() / lo.len() as f32
    };
    (luma_std(&o) / luma_std(&b), chroma_std(&o) / chroma_std(&b), contrast(&o) / contrast(&b))
}

// ---------------------------------------------------------------- effects

/// Corner and long-edge-middle change in stops of display luminance, of a flat grey picture.
pub(crate) fn vignette_effect(ids: &[(&str, f64)]) -> (f32, f32) {
    let (w, h) = (600, 400);
    let src = Rgb32f::from_fn(w, h, |_, _| [0.18; 3]);
    let info = SourceInfo::default();
    let base = render_with(&src, &info, &DevelopSettings::default());
    let out = render_with(&src, &info, &settings(ids));
    let lin = |p: [u8; 4]| srgb_to_linear(p[1] as f32 / 255.0).max(1e-4);
    let stops = |x: usize, y: usize| (lin(out.data[y * w + x]) / lin(base.data[y * w + x])).log2();
    (stops(2, 2), stops(2, h / 2))
}

/// Standard deviation (8-bit levels) of the grain on a flat mid-grey picture.
pub(crate) fn grain_std(ids: &[(&str, f64)]) -> f32 {
    let (w, h) = (800, 600);
    let src = Rgb32f::from_fn(w, h, |_, _| [0.18; 3]);
    let out = encoded_luma(&render_with(&src, &SourceInfo::default(), &settings(ids)));
    std_dev(&out.data) * 255.0
}

// ---------------------------------------------------------------- the report

fn row(r: &mut String, name: &str, cells: &str) {
    let _ = writeln!(r, "| {name} | {cells} |");
}

fn report_colour(r: &mut String) {
    r.push_str(
        "\n### Temp / Tint (raw source, as shot 5500 K, mid grey; OkLab a, b of the grey)\n\n| setting | a | b | chroma |\n|---|---|---|---|\n",
    );
    for (t, ti) in [(5500.0, 0.0), (3000.0, 0.0), (4000.0, 0.0), (7500.0, 0.0), (10000.0, 0.0), (5500.0, 150.0), (5500.0, -150.0), (5500.0, 75.0)] {
        let l = wb_grey(t, ti);
        row(r, &format!("{t} K / {ti}"), &format!("{:+.3} | {:+.3} | {:.3}", l[1], l[2], (l[1] * l[1] + l[2] * l[2]).sqrt()));
    }
    r.push_str("\n### Vibrance / Saturation (chroma ratio vs unedited, colour of OkLab chroma 0.03 / 0.08 / 0.15 at hue 40 deg (skin) and 250 deg (blue))\n\n| slider | value | low | mid | high |\n|---|---|---|---|---|\n");
    for id in ["color.vibrance", "color.saturation"] {
        for v in [-100.0, 100.0] {
            for hue in [40.0, 250.0] {
                let c = |ch: f32| chroma_ratio(lch_scene(0.6, ch, hue), &[(id, v)]);
                row(r, &format!("{id} (hue {hue})"), &format!("{v:+} | {:.2} | {:.2} | {:.2}", c(0.03), c(0.08), c(0.15)));
            }
        }
    }
    r.push_str("\n### Colour mixer (8 bands; at +-100: hue shift deg / chroma ratio / lightness change, min..max of |effect| over the bands)\n\n| field | value | min | max |\n|---|---|---|---|\n");
    for (field, pick) in [("hue", 0usize), ("sat", 1), ("lum", 2)] {
        for v in [-100.0, 100.0] {
            let vals: Vec<f32> = (0..8)
                .map(|b| {
                    let e = mixer_effect(b, field, v);
                    match pick {
                        0 => e.0.abs(),
                        1 => e.1,
                        _ => e.2,
                    }
                })
                .collect();
            let min = vals.iter().cloned().fold(f32::MAX, f32::min);
            let max = vals.iter().cloned().fold(f32::MIN, f32::max);
            row(r, field, &format!("{v:+} | {min:.2} | {max:.2}"));
        }
    }
    r.push_str("\n### Colour grading (wheel sat 100, hue 220 deg; chroma added to a neutral at scene luminance 0.01 / 0.18 / 0.8)\n\n| wheel | dark | mid | light |\n|---|---|---|---|\n");
    for wheel in ["shadows", "midtones", "highlights", "global"] {
        let c = |y: f32| grading_tint(wheel, 100.0, y, &[]).0;
        row(r, wheel, &format!("{:.3} | {:.3} | {:.3}", c(0.01), c(0.18), c(0.8)));
    }
    for (name, extra) in [
        ("shadows, blending 0", ("grading.blending", 0.0)),
        ("shadows, blending 100", ("grading.blending", 100.0)),
        ("shadows, balance -100", ("grading.balance", -100.0)),
        ("shadows, balance +100", ("grading.balance", 100.0)),
    ] {
        let c = |y: f32| grading_tint("shadows", 100.0, y, &[extra]).0;
        row(r, name, &format!("{:.3} | {:.3} | {:.3}", c(0.01), c(0.18), c(0.8)));
    }
    for wheel in ["shadows", "midtones", "highlights", "global"] {
        let y = match wheel {
            "shadows" => 0.01,
            "highlights" => 0.8,
            _ => 0.18,
        };
        row(
            r,
            &format!("{wheel} luminance -100 / +100 (OkLab L)"),
            &format!("{:+.3} | {:+.3} | -", grading_lum(wheel, -100.0, y), grading_lum(wheel, 100.0, y)),
        );
    }
}

fn report_curve(r: &mut String) {
    r.push_str("\n### Parametric tone curve (largest rise / fall of the encoded grey ramp, 8-bit levels)\n\n| slider | -100 rise/fall | +100 rise/fall |\n|---|---|---|\n");
    for id in ["curve.shadows", "curve.darks", "curve.lights", "curve.highlights"] {
        let (a, b) = (ramp_shift(&[(id, -100.0)]), ramp_shift(&[(id, 100.0)]));
        row(r, id, &format!("{:.0} / {:.0} | {:.0} / {:.0}", a.0, a.1, b.0, b.1));
    }
}

fn report_spatial(r: &mut String) {
    let tex = texture_scene(1024, 683);
    r.push_str("\n### Texture / Clarity / Dehaze (band energy ratio vs unedited: fine 1-2 px / medium 4-8 px / coarse 16-32 px / whole-image std)\n\n| slider | value | fine | medium | coarse | std |\n|---|---|---|---|---|---|\n");
    for id in ["effects.texture", "effects.clarity"] {
        for v in [-100.0, 100.0] {
            let b = texture_bands(&tex, &[(id, v)]);
            row(r, id, &format!("{v:+} | {:.2} | {:.2} | {:.2} | {:.2}", b[0], b[1], b[2], b[3]));
        }
    }
    let hazy = hazy_scene(1024, 683);
    r.push_str(
        "\n### Dehaze on a hazy scene (5-95 percentile spread of encoded luma, mean OkLab chroma)\n\n| value | spread | chroma |\n|---|---|---|\n",
    );
    for v in [-100.0, 0.0, 50.0, 100.0] {
        let (s, c) = spread_and_chroma(&hazy, &[("effects.dehaze", v)]);
        row(r, &format!("{v:+}"), &format!("{s:.3} | {c:.3}"));
    }
    r.push_str("\n### Sharpening (peak edge slope ratio / overshoot share of step / flat-noise std ratio)\n\n| setting | slope | overshoot | noise |\n|---|---|---|---|\n");
    let cases: [(&str, Vec<(&str, f64)>); 8] = [
        ("amount 40 (raw default: r 1.0, detail 25)", vec![("detail.sharpenAmount", 40.0)]),
        ("amount 150", vec![("detail.sharpenAmount", 150.0)]),
        ("amount 100, radius 0.5", vec![("detail.sharpenAmount", 100.0), ("detail.sharpenRadius", 0.5)]),
        ("amount 100, radius 3.0", vec![("detail.sharpenAmount", 100.0), ("detail.sharpenRadius", 3.0)]),
        ("amount 100, detail 0", vec![("detail.sharpenAmount", 100.0), ("detail.sharpenDetail", 0.0)]),
        ("amount 100, detail 100", vec![("detail.sharpenAmount", 100.0), ("detail.sharpenDetail", 100.0)]),
        ("amount 100, masking 0", vec![("detail.sharpenAmount", 100.0)]),
        ("amount 100, masking 100", vec![("detail.sharpenAmount", 100.0), ("detail.sharpenMasking", 100.0)]),
    ];
    for (name, ids) in cases {
        let (s, o, n) = sharpen_effect(&ids);
        row(r, name, &format!("{s:.2} | {o:.2} | {n:.2}"));
    }
    r.push_str("\n### Noise reduction (luma noise std ratio / colour noise std ratio / edge contrast kept)\n\n| setting | luma | colour | edge |\n|---|---|---|---|\n");
    let nr: [(&str, Vec<(&str, f64)>); 9] = [
        ("luminance 50", vec![("detail.nrLuminance", 50.0)]),
        ("luminance 100", vec![("detail.nrLuminance", 100.0)]),
        ("luminance 50, detail 0", vec![("detail.nrLuminance", 50.0), ("detail.nrDetail", 0.0)]),
        ("luminance 50, detail 100", vec![("detail.nrLuminance", 50.0), ("detail.nrDetail", 100.0)]),
        ("luminance 50, contrast 100", vec![("detail.nrLuminance", 50.0), ("detail.nrContrast", 100.0)]),
        ("colour 50", vec![("detail.nrColor", 50.0)]),
        ("colour 100", vec![("detail.nrColor", 100.0)]),
        ("colour 50, detail 0", vec![("detail.nrColor", 50.0), ("detail.nrColorDetail", 0.0)]),
        ("colour 50, smoothness 100", vec![("detail.nrColor", 50.0), ("detail.nrColorSmoothness", 100.0)]),
    ];
    for (name, ids) in nr {
        let (l, c, e) = nr_effect(&ids);
        row(r, name, &format!("{l:.2} | {c:.2} | {e:.2}"));
    }
}

fn report_effects(r: &mut String) {
    r.push_str("\n### Vignette (stops of luminance change at the corner / at the middle of the long edge, flat grey)\n\n| setting | corner | edge |\n|---|---|---|\n");
    let cases: [(&str, Vec<(&str, f64)>); 8] = [
        ("-100 (mid 50, feather 50)", vec![("vignette.amount", -100.0)]),
        ("-50", vec![("vignette.amount", -50.0)]),
        ("+100", vec![("vignette.amount", 100.0)]),
        ("-100, midpoint 0", vec![("vignette.amount", -100.0), ("vignette.midpoint", 0.0)]),
        ("-100, midpoint 100", vec![("vignette.amount", -100.0), ("vignette.midpoint", 100.0)]),
        ("-100, feather 0", vec![("vignette.amount", -100.0), ("vignette.feather", 0.0)]),
        ("-100, roundness -100", vec![("vignette.amount", -100.0), ("vignette.roundness", -100.0)]),
        ("-100, roundness +100", vec![("vignette.amount", -100.0), ("vignette.roundness", 100.0)]),
    ];
    for (name, ids) in cases {
        let (c, e) = vignette_effect(&ids);
        row(r, name, &format!("{c:+.2} | {e:+.2}"));
    }
    r.push_str("\n### Grain (std dev of a flat mid grey, 8-bit levels)\n\n| setting | std |\n|---|---|\n");
    let grain: [(&str, Vec<(&str, f64)>); 6] = [
        ("amount 25 (size 25, rough 50)", vec![("grain.amount", 25.0)]),
        ("amount 50", vec![("grain.amount", 50.0)]),
        ("amount 100", vec![("grain.amount", 100.0)]),
        ("amount 50, size 0", vec![("grain.amount", 50.0), ("grain.size", 0.0)]),
        ("amount 50, size 100", vec![("grain.amount", 50.0), ("grain.size", 100.0)]),
        ("amount 50, roughness 100", vec![("grain.amount", 50.0), ("grain.roughness", 100.0)]),
    ];
    for (name, ids) in grain {
        row(r, name, &format!("{:.1}", grain_std(&ids)));
    }
}

#[test]
#[ignore = "prints the slider strength table (see the module docs); slow"]
fn slider_strength_report() {
    let mut r = String::new();
    report_colour(&mut r);
    report_curve(&mut r);
    report_spatial(&mut r);
    report_effects(&mut r);
    println!("{r}");
}

// ---------------------------------------------------------------- minimum strengths

/// Noise Reduction's Contrast slider keeps contrast and detail at the price of noisy blotches: it used to do
/// nothing at all (no code read `detail.nr_contrast`).
#[test]
fn nr_contrast_keeps_more_of_the_noise_and_detail() {
    let (plain, _, edge_plain) = nr_effect(&[("detail.nrLuminance", 60.0)]);
    let (kept, _, edge_kept) = nr_effect(&[("detail.nrLuminance", 60.0), ("detail.nrContrast", 100.0)]);
    assert!(kept > plain * 1.15, "luma noise std ratio {plain} -> {kept}");
    assert!(edge_kept >= edge_plain - 0.01, "edge contrast kept {edge_plain} -> {edge_kept}");
    // half way sits between
    let (half, _, _) = nr_effect(&[("detail.nrLuminance", 60.0), ("detail.nrContrast", 50.0)]);
    assert!(plain < half && half < kept, "{plain} < {half} < {kept}");
    // Contrast does nothing without luminance noise reduction
    let (off, _, _) = nr_effect(&[("detail.nrContrast", 100.0)]);
    assert!((off - 1.0).abs() < 1e-6, "{off}");
}

/// Sharpening Detail (process V3) was read by nothing: 0 sharpens edges only (little noise gain), 100 also the
/// finest detail and noise.
#[test]
fn sharpening_detail_trades_edges_for_noise() {
    let (slope0, _, noise0) = sharpen_effect(&[("detail.sharpenAmount", 100.0), ("detail.sharpenDetail", 0.0)]);
    let (slope100, _, noise100) = sharpen_effect(&[("detail.sharpenAmount", 100.0), ("detail.sharpenDetail", 100.0)]);
    assert!(noise100 > noise0 * 1.3, "noise {noise0} -> {noise100}");
    assert!(slope100 > slope0 + 0.05, "edge slope {slope0} -> {slope100}");
    assert!(slope0 > 1.02 && noise0 < 1.3, "Detail 0 still sharpens edges without much noise: {slope0} {noise0}");
    // Amount 40 (the raw default) is visible on an edge
    assert!(sharpen_effect(&[("detail.sharpenAmount", 40.0)]).0 > 1.03);
    // V2 keeps its look: Detail does nothing there
    let v2 = |d: f64| {
        let mut s = settings(&[("detail.sharpenAmount", 100.0), ("detail.sharpenDetail", d)]);
        s.process = lightcraft_develop::Process::V2.version();
        let src = edge_scene(200, 200, 0.06);
        render_with(&src, &SourceInfo::default(), &s)
    };
    assert_eq!(v2(0.0).data, v2(100.0).data);
}

/// Floors for sliders the audit found to be at Lightroom strength: a later retune must not weaken them.
#[test]
fn strong_sliders_stay_strong() {
    // white balance: 5500 K to 3000 K is a clearly blue grey, Tint +-150 a clearly magenta / green one
    let chroma = |l: [f32; 3]| (l[1] * l[1] + l[2] * l[2]).sqrt();
    let blue = wb_grey(3000.0, 0.0);
    assert!(blue[2] < -0.08 && chroma(blue) > 0.09, "{blue:?}");
    assert!(wb_grey(5500.0, 150.0)[1] > 0.06 && wb_grey(5500.0, -150.0)[1] < -0.07);
    // saturation -100 is grey, +100 doubles chroma
    let c = lch_scene(0.6, 0.08, 40.0);
    assert!(chroma_ratio(c, &[("color.saturation", -100.0)]) < 0.02);
    assert!(chroma_ratio(c, &[("color.saturation", 100.0)]) > 1.8);
    // colour mixer: every band turns by more than 20 degrees at +-100
    for b in 0..8 {
        assert!(mixer_effect(b, "hue", 100.0).0.abs() > 20.0 && mixer_effect(b, "hue", -100.0).0.abs() > 20.0, "band {b}");
        assert!(mixer_effect(b, "sat", -100.0).1 < 0.15 && mixer_effect(b, "sat", 100.0).1 > 1.15, "band {b}");
    }
    // parametric curve regions move the curve by at least a tenth of the range
    for id in ["curve.shadows", "curve.darks", "curve.lights", "curve.highlights"] {
        assert!(ramp_shift(&[(id, 100.0)]).0 > 25.0 && ramp_shift(&[(id, -100.0)]).1 > 25.0, "{id}");
    }
    // texture and clarity
    let tex = texture_scene(1024, 683);
    assert!(texture_bands(&tex, &[("effects.texture", 100.0)])[0] > 1.6);
    assert!(texture_bands(&tex, &[("effects.clarity", 100.0)])[1] > 1.5);
    // noise reduction and grain
    assert!(nr_effect(&[("detail.nrLuminance", 100.0)]).0 < 0.5);
    assert!(nr_effect(&[("detail.nrColor", 100.0)]).1 < 0.4);
    assert!(grain_std(&[("grain.amount", 100.0)]) > 8.0);
    // vignette: strong darkening at -100, clearly lighter corners at +100
    assert!(vignette_effect(&[("vignette.amount", -100.0)]).0 < -3.0);
    assert!(vignette_effect(&[("vignette.amount", 100.0)]).0 > 1.0);
}
