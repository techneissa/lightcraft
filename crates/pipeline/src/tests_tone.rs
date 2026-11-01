//! Highlights / Shadows keep the local contrast of the tones they move (issue #632), from process
//! V2; V1 keeps the base it always had (`docs/process-versions.md`).

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, Process, ProcessVersion, controls};
use lightcraft_raster::{Plane, Rgb32f, Rgba8};

use crate::{Quality, RenderRequest, SourceInfo, StageCache, local, render, render_cached};

const W: usize = 1200;
const H: usize = 600;

/// Left half: a bright sky crossed by 1 px twigs (≈ 2.5 EV darker) and a little texture; right
/// half: a dark ground with 1 px light stalks. The two halves meet at x = W/2 (a strong edge).
fn scene() -> Rgb32f {
    Rgb32f::from_fn(W, H, |x, y| {
        let t = 1.0 + 0.06 * (((x * 7 + y * 13) % 5) as f32 - 2.0) / 2.0;
        let line = on_line(x, y);
        let v = if x < W / 2 {
            if line { 0.14 } else { 0.85 }
        } else if line {
            0.06
        } else {
            0.011
        };
        [v * t; 3]
    })
}

/// Thin lines in a chequer of 48 px cells: twigs where a branch crosses the sky, open sky between.
fn on_line(x: usize, y: usize) -> bool {
    (x / 48 + y / 48).is_multiple_of(2) && (x % 4 == 1 || y % 5 == 2)
}

/// log2 of (mean luma off the lines / mean luma on them) over columns `x0..x1`: the twigs' (or
/// stalks') contrast against their surroundings.
fn line_contrast(img: &Rgba8, x0: usize, x1: usize) -> f32 {
    let (mut on, mut off) = ((0.0f64, 0.0f64), (0.0f64, 0.0f64));
    for y in 40..H - 40 {
        for x in x0..x1 {
            let v = luma(img.data[y * W + x]) as f64;
            let acc = if on_line(x, y) { &mut on } else { &mut off };
            acc.0 += v;
            acc.1 += 1.0;
        }
    }
    ((off.0 / off.1) / (on.0 / on.1)).log2().abs() as f32
}

fn shot(s: &DevelopSettings) -> Rgba8 {
    render(&scene(), &SourceInfo::default(), s, &RenderRequest::fit(W, H)).image
}

/// Settings on `process`.
fn on(process: Process) -> DevelopSettings {
    DevelopSettings { process: process.version(), ..DevelopSettings::default() }
}

/// V2 settings with control `id` at `v`.
fn with(id: &str, v: f64) -> DevelopSettings {
    with_on(Process::V2, id, v)
}

fn with_on(process: Process, id: &str, v: f64) -> DevelopSettings {
    let mut s = on(process);
    controls::set(&mut s, id, v);
    s
}

fn luma(p: [u8; 4]) -> f32 {
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

/// (mean, rel. local contrast: std of luma minus its 9×9 mean, over the mean) of the block
/// `x0..x1 × 40..H-40`.
fn stats(img: &Rgba8, x0: usize, x1: usize) -> (f32, f32) {
    let l = |x: usize, y: usize| luma(img.data[y * W + x]);
    let (mut sum, mut hp, mut n) = (0.0f64, Vec::new(), 0.0f64);
    for y in 40..H - 40 {
        for x in x0..x1 {
            let mut m = 0.0;
            for dy in 0..9 {
                for dx in 0..9 {
                    m += l(x + dx - 4, y + dy - 4);
                }
            }
            sum += l(x, y) as f64;
            hp.push(l(x, y) - m / 81.0);
            n += 1.0;
        }
    }
    let mean = (sum / n) as f32;
    let var = hp.iter().map(|v| v * v).sum::<f32>() / hp.len() as f32;
    (mean, var.sqrt() / mean)
}

#[test]
fn highlights_keep_the_local_contrast_of_bright_areas() {
    let (plain, hl, exp) = (shot(&on(Process::V2)), shot(&with("light.highlights", -100.0)), shot(&with("light.exposure", -1.0)));
    let (bright, flat_dark) = ((40, 540), (700, 1160));
    let ((m0, c0), (mh, ch), (_, ce)) = (stats(&plain, bright.0, bright.1), stats(&hl, bright.0, bright.1), stats(&exp, bright.0, bright.1));
    let (k0, kh, ke) = (line_contrast(&plain, bright.0, bright.1), line_contrast(&hl, bright.0, bright.1), line_contrast(&exp, bright.0, bright.1));
    // the sky comes down as much as with −1 EV or more…
    assert!(mh < m0 - 25.0, "highlights −100 mean {mh} vs {m0}");
    // …and keeps its twigs as −1 EV does (the edge-aware base alone followed them: 0.73 of
    // Exposure's twig contrast and 0.79 of its local contrast were left)
    assert!(kh > 0.9 * ke && kh > 0.9 * k0, "highlights −100 twig contrast {kh} EV vs exposure −1 {ke}, plain {k0}");
    assert!(ch > 0.9 * ce, "highlights −100 local contrast {ch} vs exposure −1 {ce}, plain {c0}");
    // the dark half is not touched
    let (d0, dh) = (stats(&plain, flat_dark.0, flat_dark.1).0, stats(&hl, flat_dark.0, flat_dark.1).0);
    assert!((dh - d0).abs() < 1.5, "dark half {dh} vs {d0}");
}

#[test]
fn shadows_keep_the_local_contrast_of_dark_areas() {
    let (plain, sh, exp) = (shot(&on(Process::V2)), shot(&with("light.shadows", 100.0)), shot(&with("light.exposure", 1.0)));
    let dark = (640, 1160);
    let ((m0, _), (ms, cs), (_, ce)) = (stats(&plain, dark.0, dark.1), stats(&sh, dark.0, dark.1), stats(&exp, dark.0, dark.1));
    let (k0, ks, ke) = (line_contrast(&plain, dark.0, dark.1), line_contrast(&sh, dark.0, dark.1), line_contrast(&exp, dark.0, dark.1));
    assert!(ms > m0 + 10.0, "shadows +100 mean {ms} vs {m0}");
    // (the edge-aware base alone: 0.75 of Exposure's stalk contrast, 0.72 of its local contrast)
    assert!(ks > 0.9 * ke, "shadows +100 stalk contrast {ks} EV vs exposure +1 {ke}, plain {k0}");
    assert!(cs > 0.9 * ce, "shadows +100 local contrast {cs} vs exposure +1 {ce}");
    // the bright half is not lifted
    let (b0, bs) = (stats(&plain, 40, 540).0, stats(&sh, 40, 540).0);
    assert!((bs - b0).abs() < 1.5, "bright half {bs} vs {b0}");
}

#[test]
fn highlights_add_no_halo_at_a_strong_edge() {
    // a flat bright field against a flat dark one: the bright pixels next to the edge come down
    // (nearly) as much as those far from it, the dark ones stay where they are (the guided
    // filter's own softness leaves up to 4 levels within a few px of the edge, as before)
    let src = Rgb32f::from_fn(W, H, |x, _| if x < W / 2 { [0.85; 3] } else { [0.011; 3] });
    let shot = |s: &DevelopSettings| render(&src, &SourceInfo::default(), s, &RenderRequest::fit(W, H)).image;
    let (plain, hl) = (shot(&on(Process::V2)), shot(&with("light.highlights", -100.0)));
    let at = |img: &Rgba8, x: usize| luma(img.data[(H / 2) * W + x]);
    let far = at(&hl, 100) - at(&plain, 100);
    for x in (W / 2 - 40..W / 2).chain(W / 2..W / 2 + 40) {
        let d = at(&hl, x) - at(&plain, x);
        let want = if x < W / 2 { far } else { 0.0 };
        assert!((d - want).abs() <= 5.0, "x {x}: change {d}, far from the edge {want}");
    }
}

#[test]
fn zero_highlights_and_shadows_render_without_the_base() {
    let mut s = on(Process::V2);
    s.light.highlights = 0.0;
    s.light.shadows = 0.0;
    controls::set(&mut s, "effects.clarity", 40.0);
    // no base plane is computed or read (the per-pixel stage sees log luminance): the render is
    // the one it always was
    assert_eq!(local::plane_sigmas(&s, 1200.0, 1.0, Quality::Full).base, None);
    assert!(local::plane_sigmas(&with("light.shadows", 1.0), 1200.0, 1.0, Quality::Full).base.is_some());
}

#[test]
fn median_keeps_steps_and_drops_thin_lines() {
    let p = Plane::from_fn(40, 30, |x, y| if x < 20 { -2.0 } else { 2.0 } + if y == 15 || x == 8 { -3.0 } else { 0.0 });
    let m = local::median_hv(&p, 2);
    for y in 0..30 {
        for x in 0..40 {
            let want = if x < 20 { -2.0 } else { 2.0 };
            assert_eq!(m.get(x, y), want, "({x}, {y})");
        }
    }
    assert_eq!(local::median_hv(&p, 0).data, p.data);
    assert_eq!(local::base_detail_radius(5568.0), 8);
    assert_eq!(local::base_detail_radius(300.0), 0);
    assert_eq!(local::base_detail_radius(1e9), local::BASE_DETAIL_MAX);
}

#[test]
fn median_matches_a_plain_per_pixel_median() {
    let p =
        Plane::from_fn(37, 23, |x, y| (((x * 31 + y * 17) * 2_654_435_761usize) % 1000) as f32 / 250.0 - 2.0 + if x % 5 == 0 { 0.5 } else { 0.0 });
    for r in [1, 2, 4, 9] {
        let med = |v: &mut Vec<f32>| {
            v.sort_by(f32::total_cmp);
            v[r]
        };
        let rows = Plane::from_fn(37, 23, |x, y| med(&mut (0..=2 * r).map(|k| p.get((x + k).saturating_sub(r).min(36), y)).collect()));
        let want = Plane::from_fn(37, 23, |x, y| med(&mut (0..=2 * r).map(|k| rows.get(x, (y + k).saturating_sub(r).min(22))).collect()));
        assert_eq!(local::median_hv(&p, r).data, want.data, "r {r}");
    }
}

#[test]
fn median_orders_special_values_and_ties_like_total_cmp() {
    // the running window keeps keys in `f32::total_cmp` order: NaNs of either sign, signed zeros,
    // infinities and long runs of equal values come out bit for bit as a sorted per-pixel window
    // does, also on planes narrower or shorter than the window
    let special = [f32::NAN, -f32::NAN, 0.0, -0.0, f32::INFINITY, f32::NEG_INFINITY, f32::MIN_POSITIVE, -1.0, 1.0, 1.0, 1.0];
    let bits = |p: &Plane| p.data.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    for (w, h) in [(1, 1), (2, 3), (5, 1), (19, 7), (61, 9)] {
        let p = Plane::from_fn(w, h, |x, y| special[(x * 7 + y * 3 + (x * y) % 5) % special.len()]);
        for r in [1, 2, 9, 24] {
            let med = |v: &mut Vec<f32>| {
                v.sort_by(f32::total_cmp);
                v[r]
            };
            let rows = Plane::from_fn(w, h, |x, y| med(&mut (0..=2 * r).map(|k| p.get((x + k).saturating_sub(r).min(w - 1), y)).collect()));
            let want = Plane::from_fn(w, h, |x, y| med(&mut (0..=2 * r).map(|k| rows.get(x, (y + k).saturating_sub(r).min(h - 1))).collect()));
            assert_eq!(bits(&local::median_hv(&p, r)), bits(&want), "{w}x{h} r {r}");
        }
    }
}

#[test]
fn v1_keeps_the_base_it_always_had() {
    // V1's base is the edge-aware filter of log luminance itself, as before process V2: no median
    // radius, and `tone_base` at 0 is that filter bit for bit
    let v1 = with_on(Process::V1, "light.highlights", -100.0);
    assert_eq!(ProcessVersion::legacy().process(), Process::V1);
    for ppl in [300.0, 1200.0, 5568.0, 1e6] {
        for q in [Quality::Full, Quality::Draft] {
            let sig = local::plane_sigmas(&v1, ppl, 1.0, q);
            assert!(sig.base.is_some());
            assert_eq!(sig.base_detail, 0, "V1 at {ppl} px");
            assert_eq!(local::tone_base_detail(&v1, ppl), 0);
        }
    }
    let l =
        Plane::from_fn(301, 157, |x, y| (((x * 31 + y * 17) * 2_654_435_761usize) % 1000) as f32 / 250.0 - 2.0 + if x % 5 == 0 { 0.5 } else { 0.0 });
    for sg in [1.0f32, 4.5, 18.0] {
        assert_eq!(local::tone_base(&l, sg, 0).data, local::guided_fast(&l, sg, local::BASE_EPS).data, "sigma {sg}");
    }
    // V2 takes the median wherever the image is big enough for one
    let v2 = with("light.highlights", -100.0);
    assert_eq!(local::plane_sigmas(&v2, 5568.0, 1.0, Quality::Full).base_detail, local::base_detail_radius(5568.0));
    assert_eq!(local::plane_sigmas(&v2, 300.0, 1.0, Quality::Full).base_detail, 0);
}

#[test]
fn only_v2_keeps_the_twigs_under_highlights() {
    let bright = (40, 540);
    let (p1, p2) = (shot(&on(Process::V1)), shot(&on(Process::V2)));
    // without Highlights / Shadows the processes render alike
    assert!(p1 == p2, "plain renders differ between V1 and V2");
    let (h1, h2, e) =
        (shot(&with_on(Process::V1, "light.highlights", -100.0)), shot(&with("light.highlights", -100.0)), shot(&with("light.exposure", -1.0)));
    let (k1, k2, ke) = (line_contrast(&h1, bright.0, bright.1), line_contrast(&h2, bright.0, bright.1), line_contrast(&e, bright.0, bright.1));
    // V1 is the base from before (≈ 0.73 of Exposure's twig contrast), V2 keeps them
    assert!(k1 < 0.85 * ke && k2 > 0.9 * ke, "twig contrast V1 {k1} EV, V2 {k2} EV, exposure −1 {ke} EV");
}

#[test]
fn the_base_plane_is_cached_per_process() {
    let s1 = with_on(Process::V1, "light.highlights", -100.0);
    let s2 = with("light.highlights", -100.0);
    // moving a photo between processes (Update to Current Process, then undo) with warm caches
    // renders what a fresh render does
    let src = Arc::new(scene());
    let (info, req) = (SourceInfo::default(), RenderRequest::fit(W, H));
    let cache = StageCache::default();
    for s in [&s1, &s2, &s1, &s2] {
        let warm = render_cached(&src, &info, s, &req, &cache).image;
        assert!(warm == render(&src, &info, s, &req).image, "process {:?}", s.process);
    }
    let sig = local::plane_sigmas(&s2, W as f64, 1.0, Quality::Full);
    let sg = sig.base.unwrap();
    assert_ne!(local::base_key(&s1, sg, 0), local::base_key(&s2, sg, 0), "the process is part of the key");
    assert_ne!(local::base_key(&s2, sg, 0), local::base_key(&s2, sg, sig.base_detail));
}

/// Large soft structure (a ramp, a bright and a dark blob) with a hard edge, 2400 px long: past the
/// size where the draft base used to be capped to 24 px.
fn big_scene() -> Rgb32f {
    const BW: usize = 2400;
    const BH: usize = 1200;
    Rgb32f::from_fn(BW, BH, |x, y| {
        let (u, v) = (x as f32 / BW as f32, y as f32 / BH as f32);
        let ramp = 0.004 * (7.0 * u).exp2() * (1.0 + 0.5 * v);
        let blob = |cx: f32, cy: f32, r: f32, amp: f32| amp * (-(((u - cx) * 2.0).powi(2) + (v - cy).powi(2)) / (r * r)).exp();
        let edge = if u > 0.62 { 0.35 } else { 0.0 };
        let t = 1.0 + 0.03 * (((x * 7 + y * 13) % 5) as f32 - 2.0);
        let c = (ramp + blob(0.3, 0.4, 0.12, 0.5) + blob(0.8, 0.7, 0.1, 0.02) + edge) * t;
        [c, c * 0.95, c * 0.9]
    })
}

fn max_and_mean_diff(a: &Rgba8, b: &Rgba8) -> (u8, f64) {
    let mut max = 0u8;
    let mut sum = 0.0f64;
    for (p, q) in a.data.iter().zip(&b.data) {
        for c in 0..3 {
            let d = p[c].abs_diff(q[c]);
            max = max.max(d);
            sum += d as f64;
        }
    }
    (max, sum / (3 * a.data.len()) as f64)
}

#[test]
fn draft_and_full_agree_for_local_tone_on_large_images() {
    let src = big_scene();
    let info = SourceInfo::default();
    for (name, settings) in [
        ("highlights -100", with("light.highlights", -100.0)),
        ("shadows +100", with("light.shadows", 100.0)),
        ("clarity +100", with("effects.clarity", 100.0)),
        ("all three", {
            let mut s = with("light.highlights", -100.0);
            controls::set(&mut s, "light.shadows", 100.0);
            controls::set(&mut s, "effects.clarity", 100.0);
            s
        }),
    ] {
        let full = render(&src, &info, &settings, &RenderRequest::fit(2400, 1200)).image;
        let draft = render(&src, &info, &settings, &RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(2400, 1200) }).image;
        let (max, mean) = max_and_mean_diff(&full, &draft);
        eprintln!("{name}: max {max}, mean {mean:.4}");
        assert!(max <= 1 && mean < 0.05, "{name}: the draft differs from the export (max {max}, mean {mean:.4})");
    }
}

#[test]
fn base_radius_is_the_same_fraction_of_the_image_at_every_quality() {
    let s = with("light.shadows", 100.0);
    for ppl in [800.0, 1600.0, 2400.0, 5568.0, 8000.0] {
        let (d, f) = (local::plane_sigmas(&s, ppl, 1.0, Quality::Draft).base, local::plane_sigmas(&s, ppl, 1.0, Quality::Full).base);
        assert_eq!(d, f, "base sigma at {ppl} px");
    }
}

/// Lift in stops of Shadows +100 / Highlights -100 at a pixel of `linear` luminance (flat area).
fn lift_ev(shadows: f32, highlights: f32, linear: f32, v3: bool) -> f32 {
    crate::finish::shadows_highlights_delta(shadows, highlights, (linear / crate::tone::GREY).log2(), v3)
}

#[test]
fn shadows_open_the_whole_lower_half_but_not_to_grey() {
    // (linear level, least and most lift in stops at Shadows +100): clearly visible, never extreme
    for (level, least, most) in [(0.005, 1.5, 1.8), (0.02, 1.2, 1.6), (0.05, 0.8, 1.3), (0.1, 0.4, 0.9), (0.18, 0.15, 0.4)] {
        let lift = lift_ev(1.0, 0.0, level, true);
        assert!((least..=most).contains(&lift), "Shadows +100 lifts {level} by {lift:.2} EV, want {least}..{most}");
    }
    // ... leaves the highlights alone, and the lift only falls as the tone gets brighter (no flat grey)
    for level in [0.7, 1.0] {
        assert!(lift_ev(1.0, 0.0, level, true).abs() < 0.05, "{level}");
    }
    let lifts: Vec<f32> = [0.005, 0.02, 0.05, 0.1, 0.18, 0.36].map(|l| lift_ev(1.0, 0.0, l, true)).to_vec();
    assert!(lifts.windows(2).all(|w| w[0] >= w[1]), "{lifts:?}");
}

#[test]
fn highlights_pull_down_the_upper_half_and_spare_the_shadows() {
    for (level, least, most) in [(0.18, 0.3, 0.7), (0.36, 0.9, 1.4), (0.7, 1.3, 1.8)] {
        let pull = -lift_ev(0.0, -1.0, level, true);
        assert!((least..=most).contains(&pull), "Highlights -100 pulls {level} by {pull:.2} EV, want {least}..{most}");
    }
    for level in [0.005, 0.02, 0.05] {
        assert!(lift_ev(0.0, -1.0, level, true).abs() < 0.02, "{level}");
    }
}

#[test]
fn local_tone_keeps_tones_in_order_on_a_flat_ramp() {
    // output log luminance (input + delta) never decreases as the input rises
    for v3 in [false, true] {
        for (sh, hl) in [(1.0, 0.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 1.0)] {
            let mut prev = f32::NEG_INFINITY;
            for i in 0..=1400 {
                let base = -9.0 + i as f32 * 0.01;
                let out = base + crate::finish::shadows_highlights_delta(sh, hl, base, v3);
                assert!(out >= prev - 1e-6, "crosses at base {base} (shadows {sh}, highlights {hl}, v3 {v3})");
                prev = out;
            }
        }
    }
}

#[test]
fn v1_and_v2_keep_their_shadows_and_highlights_weights() {
    // the weights before process V3, spelled out: frozen
    let smooth = |e0: f32, e1: f32, x: f32| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    for i in 0..=1200 {
        let base = -9.0 + i as f32 * 0.01;
        let ws = 1.0 - smooth(-4.8, 0.3, base);
        let want = 0.7 * 1.7 * ws * ws.sqrt() - 0.4 * 1.7 * smooth(-1.0, 2.8, base);
        assert_eq!(crate::finish::shadows_highlights_delta(0.7, -0.4, base, false), want, "{base}");
    }
    // end to end: V2 and V3 renders differ under Shadows, agree without it
    let (v2, v3) = (with_on(Process::V2, "light.shadows", 100.0), with_on(Process::V3, "light.shadows", 100.0));
    assert_ne!(shot(&v2).data, shot(&v3).data);
    assert_eq!(shot(&on(Process::V2)).data, shot(&on(Process::V3)).data);
}

/// Output linear luminance of a flat area at `linear` under Highlights `hl` (-1..1), process V3.
fn highlights_out(hl: f32, linear: f32) -> f32 {
    linear * lift_ev(0.0, hl, linear, true).exp2()
}

#[test]
fn positive_highlights_brighten_the_upper_tones_but_roll_off_before_white() {
    // clearly visible, even at +30
    for (hl, level, least) in [(0.3, 0.18, 0.1), (0.3, 0.36, 0.25), (1.0, 0.18, 0.3), (1.0, 0.36, 0.5)] {
        let lift = lift_ev(0.0, hl, level, true);
        assert!(lift >= least, "Highlights {hl} lifts {level} by {lift:.2} EV, want at least {least}");
    }
    // not clipping: a 0.5..0.9 ramp (a bright sky with cloud texture) stays below white at +100 ...
    let ramp: Vec<f32> = (0..=40).map(|i| 0.5 + 0.4 * i as f32 / 40.0).collect();
    let out: Vec<f32> = ramp.iter().map(|&l| highlights_out(1.0, l)).collect();
    let clipped = out.iter().filter(|&&v| v >= 0.99).count() as f32 / out.len() as f32;
    assert!(clipped <= 0.05, "{:.0} % of the ramp reaches white at Highlights +100", clipped * 100.0);
    // ... still brighter than it was, in order, and the texture across it survives
    assert!(out.iter().zip(&ramp).all(|(o, r)| o >= r), "{out:?}");
    assert!(out.windows(2).all(|w| w[1] >= w[0]), "{out:?}");
    assert!(out[40] - out[0] >= 0.1, "ramp contrast {:.3} -> {:.3}", out[0], out[40]);
    // tones already at white are left alone
    assert!((highlights_out(1.0, 1.0) - 1.0).abs() < 0.02);
}
