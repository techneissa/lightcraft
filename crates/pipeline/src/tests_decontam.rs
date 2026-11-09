//! Fork feature (`fork/decontaminate.md`, issue #115): a background swap leaves no blue in the strands at a
//! decontaminated mask's soft edge; Decontaminate 0 renders exactly as before; a preview and an export agree.

use lightcraft_color::perceptual::oklab_from_2020;
use lightcraft_color::transfer::srgb_to_linear;
use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_geom::Point;
use lightcraft_raster::{Rgb32f, Rgba8};

use crate::{RenderRequest, SourceInfo, render};

const HAIR: [f32; 3] = [0.40, 0.16, 0.06];
const SKY: [f32; 3] = [0.06, 0.18, 0.60];
/// Yellow, far from both.
const YELLOW: [f64; 3] = [1.0, 0.82, 0.0];

/// A Background mask: a narrow gradient from hair (x < 0.46) to sky (x > 0.54), a strand-wide edge, recoloured yellow, decontaminated at `d`.
fn swapped(d: f64) -> DevelopSettings {
    let shape = MaskShape::Linear { start: Point::new(0.54, 0.5), end: Point::new(0.46, 0.5) };
    let adjust = LocalAdjustments { recolor: 100.0, recolor_r: YELLOW[0], recolor_g: YELLOW[1], recolor_b: YELLOW[2], ..Default::default() };
    let mut s = DevelopSettings::default();
    s.masks.push(Mask {
        id: 1,
        components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }],
        adjust,
        decontaminate: d,
        ..Default::default()
    });
    s
}

/// Hair and sky mixed by the mask's own alpha (as a matte describes a strand), with fine hair texture.
fn scene(w: usize, h: usize, s: &DevelopSettings) -> Rgb32f {
    scene_of(w, h, s, HAIR, SKY)
}

/// [`scene`] with these colours.
fn scene_of(w: usize, h: usize, s: &DevelopSettings, hair: [f32; 3], sky: [f32; 3]) -> Rgb32f {
    let frame = crate::geometry::Frame::new(w, h, s, true);
    let flat = Rgb32f::new(w, h);
    let log_l = lightcraft_raster::Plane::new(w, h);
    let a = crate::masks::evaluate_one(&s.masks[0], &frame, w, h, &flat, &log_l, 0.0, None);
    Rgb32f::from_fn(w, h, |x, y| {
        let t = a.get(x, y);
        let shade = 0.85 + 0.15 * ((x * 7 + y * 3) % 5) as f32 / 4.0;
        std::array::from_fn(|c| (1.0 - t) * hair[c] * shade + t * sky[c])
    })
}

fn shot(src: &Rgb32f, s: &DevelopSettings, w: usize, h: usize) -> Rgba8 {
    render(src, &SourceInfo::default(), s, &RenderRequest::fit(w, h)).image
}

/// OkLab b (blue is negative) of an 8-bit sRGB pixel.
fn ok_b(p: [u8; 4]) -> f32 {
    let lin = [p[0], p[1], p[2]].map(|v| srgb_to_linear(f32::from(v) / 255.0));
    oklab_from_2020(lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020).apply_f32(lin))[2]
}

/// The mean OkLab b across the soft edge (where the strands mix with the sky).
fn edge_blue(img: &Rgba8) -> f32 {
    let (w, h) = (img.width, img.height);
    let xs = (w * 47 / 100)..(w * 53 / 100);
    let n = xs.len() * h;
    xs.flat_map(|x| (0..h).map(move |y| (x, y))).map(|(x, y)| ok_b(img.get(x, y))).sum::<f32>() / n as f32
}

#[test]
fn a_background_swap_leaves_no_blue_in_the_strands() {
    let (w, h) = (240, 60);
    let src = scene(w, h, &swapped(0.0));
    let off = edge_blue(&shot(&src, &swapped(0.0), w, h));
    let on = edge_blue(&shot(&src, &swapped(100.0), w, h));
    assert!(on > off + 0.02, "decontaminated: OkLab b {on:.3} across the edge, {off:.3} without");
    assert!(on > 0.0, "no blue left at the edge: {on:.3}");
}

/// Decontaminated, an edge the mask leaves alone (no adjustment) renders as the photo does.
#[test]
fn an_edge_with_nothing_to_edit_stays_the_photo() {
    let (w, h) = (240, 60);
    let mut idle = swapped(100.0);
    idle.masks[0].adjust = LocalAdjustments::default();
    let src = scene(w, h, &idle);
    let plain = shot(&src, &DevelopSettings::default(), w, h);
    let on = shot(&src, &idle, w, h);
    let worst = plain.data.iter().zip(&on.data).flat_map(|(a, b)| (0..3).map(move |c| a[c].abs_diff(b[c]))).max().unwrap_or(0);
    assert!(worst <= 1, "an idle decontaminated mask changed a pixel by {worst}");
}

#[test]
fn decontaminate_zero_renders_exactly_as_before() {
    let (w, h) = (120, 40);
    let src = scene(w, h, &swapped(0.0));
    let plain = shot(&src, &swapped(0.0), w, h);
    let mut none = swapped(0.0);
    none.masks[0].decontaminate = f64::NAN;
    assert_eq!(shot(&src, &none, w, h).data, plain.data, "a NaN amount is off");
    // the pixels the mask selects wholly or not at all never change
    let on = shot(&src, &swapped(100.0), w, h);
    for y in 0..h {
        for x in (0..w * 44 / 100).chain(w * 56 / 100..w) {
            assert_eq!(on.get(x, y), plain.get(x, y), "({x}, {y})");
        }
    }
}

#[test]
fn a_preview_and_an_export_decontaminate_alike() {
    // the export, averaged down 4 × 4, against the preview across the edge: no further apart than the same render
    // without decontamination is (the gradient itself is drawn per pixel)
    let gap = |d: f64| {
        let s = swapped(d);
        let big = shot(&scene(960, 240, &s), &s, 960, 240);
        let small = shot(&scene(240, 60, &s), &s, 240, 60);
        (240 * 45 / 100..240 * 55 / 100)
            .map(|x| {
                let y = 30;
                (0..3)
                    .map(|c| {
                        let mean = (0..16).map(|k| f32::from(big.get(x * 4 + k % 4, y * 4 + k / 4)[c])).sum::<f32>() / 16.0;
                        (mean - f32::from(small.get(x, y)[c])).abs()
                    })
                    .fold(0.0f32, f32::max)
            })
            .fold(0.0f32, f32::max)
    };
    let (plain, on) = (gap(0.0), gap(100.0));
    eprintln!("preview vs export: {plain} without, {on} with");
    assert!(on <= plain + 4.0, "decontaminated preview and export {on} apart, {plain} without");
}

/// A raw's tone map (process V4 without a camera curve: `ToneMap::hdr`) with Contrast +100, on a source bright
/// enough to reach its shoulder.
fn raw_like(mut s: DevelopSettings) -> DevelopSettings {
    s.process = lightcraft_develop::Process::V4.version();
    s.light.contrast = 100.0;
    s
}

fn raw_shot(src: &Rgb32f, s: &DevelopSettings, w: usize, h: usize) -> Rgba8 {
    let info = SourceInfo { raw: true, ..Default::default() };
    render(src, &info, s, &RenderRequest::fit(w, h)).image
}

/// Dark hair against a sky three stops over mid grey, as a raw holds them.
fn bright(w: usize, h: usize, s: &DevelopSettings) -> Rgb32f {
    scene_of(w, h, s, [0.05, 0.025, 0.012], [0.9, 1.6, 3.2])
}

/// The largest difference of any channel of any pixel.
fn worst(a: &Rgba8, b: &Rgba8) -> u8 {
    a.data.iter().zip(&b.data).flat_map(|(a, b)| (0..3).map(move |c| a[c].abs_diff(b[c]))).max().unwrap_or(0)
}

/// The composite of the two colours is tone-mapped once: on a raw with Contrast +100, a decontaminated mask with
/// nothing (or next to nothing) to edit apart changes no pixel (each colour tone-mapped on its own, then mixed, shifted
/// the edge's colour).
#[test]
fn an_idle_mask_on_a_raw_with_contrast_changes_nothing() {
    let (w, h) = (240, 60);
    let idle = [
        ("nothing", LocalAdjustments::default()),
        ("a trace of Exposure", LocalAdjustments { exposure: 0.002, ..Default::default() }),
        ("a trace of Saturation", LocalAdjustments { saturation: 0.2, ..Default::default() }),
    ];
    for (name, adjust) in idle {
        let mut s = raw_like(swapped(100.0));
        s.masks[0].adjust = adjust;
        let src = bright(w, h, &s);
        let plain = raw_shot(&src, &raw_like(DevelopSettings::default()), w, h);
        let worst = worst(&plain, &raw_shot(&src, &s, w, h));
        assert!(worst <= 1, "{name}: a decontaminated mask changed a pixel by {worst}");
    }
}

/// Highlights, Clarity and Texture read the mixed pixel's neighbourhood: they run once on the composite, so a
/// decontaminated edge draws no bright rim with them (with a trace of Exposure, so the edge is split).
#[test]
fn tone_edits_draw_no_rim_at_a_decontaminated_edge() {
    let (w, h) = (240, 60);
    let trace = LocalAdjustments { exposure: 0.002, ..Default::default() };
    let edits = [
        ("highlights", LocalAdjustments { highlights: -100.0, ..trace }),
        ("clarity", LocalAdjustments { clarity: 100.0, ..trace }),
        ("texture", LocalAdjustments { texture: 100.0, ..trace }),
        ("highlights alone", LocalAdjustments { highlights: -100.0, ..Default::default() }),
    ];
    for (name, adjust) in edits {
        let with = |d: f64| {
            let mut s = raw_like(swapped(d));
            s.masks[0].adjust = adjust;
            s
        };
        let src = bright(w, h, &with(0.0));
        let worst = worst(&raw_shot(&src, &with(0.0), w, h), &raw_shot(&src, &with(100.0), w, h));
        assert!(worst <= 1, "{name}: decontaminating moved a pixel by {worst}");
    }
}

/// Two decontaminated masks on one edge: each is split, not only the first. The second swaps the background as it
/// does with the first one not decontaminated.
#[test]
fn every_decontaminated_mask_at_an_edge_is_split() {
    let (w, h) = (240, 60);
    let both = |first_d: f64, d: f64| {
        let mut s = swapped(d);
        let mut first = s.masks[0].clone();
        first.id = 2;
        // (the first barely edits; the second swaps the colour)
        first.adjust = LocalAdjustments { exposure: 0.05, ..Default::default() };
        first.decontaminate = first_d;
        s.masks.insert(0, first);
        s
    };
    let src = scene(w, h, &swapped(0.0));
    let off = edge_blue(&shot(&src, &both(0.0, 0.0), w, h));
    let second = edge_blue(&shot(&src, &both(0.0, 100.0), w, h));
    let on = edge_blue(&shot(&src, &both(100.0, 100.0), w, h));
    assert!(second > off + 0.02, "the second alone: OkLab b {second:.3} across the edge, {off:.3} without");
    assert!((on - second).abs() < 0.005, "both decontaminated: OkLab b {on:.3} across the edge, {second:.3} with the second alone");
}

/// The split is kept with the render's other stages: an edit that doesn't change the mask's alpha (its Amount, its
/// sliders, the global sliders) doesn't estimate it again.
#[test]
fn the_split_is_kept_while_only_the_edits_change() {
    let (w, h) = (240, 60);
    let mut s = swapped(100.0);
    let src = std::sync::Arc::new(scene(w, h, &s));
    let cache = crate::StageCache::default();
    let req = RenderRequest::fit(w, h);
    let before = crate::decontam::estimates();
    let first = crate::render_cached(&src, &SourceInfo::default(), &s, &req, &cache).image;
    s.masks[0].adjust.amount = 60.0;
    s.masks[0].adjust.exposure = 0.5;
    s.light.contrast = 30.0;
    let _ = crate::render_cached(&src, &SourceInfo::default(), &s, &req, &cache);
    assert_eq!(crate::decontam::estimates() - before, 1, "the split was estimated again");
    // and a kept split renders as a fresh one
    let fresh = shot(&src, &s, w, h);
    assert_eq!(crate::render_cached(&src, &SourceInfo::default(), &s, &req, &cache).image.data, fresh.data);
    assert_ne!(first.data, fresh.data);
}
