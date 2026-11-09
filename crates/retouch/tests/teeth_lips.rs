//! Tests of the teeth and lips feature (`lightcraft_retouch::teeth_lips`).

mod common;

use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::teeth_lips::{LIP_MARGIN, MAX_HUE_TURN, TEETH_CEILING, TeethLips, oklab, rgb};
use lightcraft_retouch::{Face, Input, NormRect, Part, PartMask, Retouch, Stage, WholePhoto};

#[test]
fn off_by_default_and_amount_zero_changes_nothing() {
    common::off_changes_nothing::<TeethLips>();
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<TeethLips>();
}

/// A mouth in the middle of a `w × h` picture: lips (a rose band) around teeth (yellowish, with dark gaps and a
/// shaded top row), on skin, drawn in fractions of the picture. Linear Rec.2020.
fn mouth(w: usize, h: usize) -> Rgb32f {
    let lips = rgb([0.55, 0.12, 0.04]);
    let skin = rgb([0.7, 0.04, 0.06]);
    Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = (x as f64 / w as f64, y as f64 / h as f64);
        let shade = 0.9 + 0.2 * u as f32; // the light falls off across the mouth
        if is_teeth(u, v) {
            let gap = (u * 10.0).fract() < 0.12;
            let l = if gap {
                0.3
            } else if v < 0.42 {
                0.6
            } else {
                0.78
            };
            rgb([l * shade.min(1.0), -0.005, 0.05])
        } else if is_lips(u, v) {
            lips.map(|c| c * shade)
        } else {
            skin
        }
    })
}

fn is_teeth(u: f64, v: f64) -> bool {
    (0.35..0.65).contains(&u) && (0.4..0.55).contains(&v)
}

fn is_lips(u: f64, v: f64) -> bool {
    (0.25..0.75).contains(&u) && (0.3..0.65).contains(&v)
}

/// A mask of the whole picture drawn at 64 × 64 (hard edged, so the test knows where it is 1 and 0).
fn mask_of(inside: fn(f64, f64) -> bool) -> PartMask {
    let plane = Plane::from_fn(64, 64, |x, y| if inside((x as f64 + 0.5) / 64.0, (y as f64 + 0.5) / 64.0) { 1.0 } else { 0.0 });
    PartMask { plane, area: NormRect::new(0.0, 0.0, 1.0, 1.0) }
}

/// The face around the mouth, with a Lips mask (taking in the open mouth, as parsers do) and, when `open`, a Teeth
/// mask.
fn face(open: bool) -> Face {
    let mut masks = vec![(Part::Lips, mask_of(is_lips))];
    if open {
        masks.push((Part::Teeth, mask_of(is_teeth)));
    }
    Face { masks, ..Face::boxed(NormRect::new(0.15, 0.0, 0.85, 1.0)) }
}

fn settings(values: &[(&str, f64)]) -> Retouch {
    let mut r = Retouch::default();
    for (id, v) in values {
        assert!(r.global.set(&format!("retouch.teethLips.{id}"), *v), "{id}");
    }
    r
}

fn retouched(img: &Rgb32f, r: &Retouch, faces: Vec<Face>) -> Rgb32f {
    let mut out = img.clone();
    let view = WholePhoto { width: img.width, height: img.height };
    lightcraft_retouch::run::apply(&mut out, r, &Input::new(faces), &view, Stage::Retouch);
    out
}

fn same(a: [f32; 3], b: [f32; 3]) -> bool {
    a.map(f32::to_bits) == b.map(f32::to_bits)
}

fn chroma(p: [f32; 3]) -> f32 {
    let [_, a, b] = oklab(p);
    (a * a + b * b).sqrt()
}

/// The pixel at fractions (`u`, `v`).
fn at(img: &Rgb32f, u: f64, v: f64) -> [f32; 3] {
    img.data[(v * img.height as f64) as usize * img.width + (u * img.width as f64) as usize]
}

const ALL_ON: [(&str, f64); 4] = [("teethWhitening", 100.0), ("lipColorAmount", 100.0), ("lipHue", 60.0), ("lipSaturation", -40.0)];

#[test]
fn a_colour_with_every_amount_at_zero_is_off_and_changes_nothing() {
    let r = settings(&[("lipColorR", 20.0), ("lipColorG", 200.0), ("lipColorB", 90.0)]);
    assert!(r.is_off());
    let img = mouth(80, 60);
    let out = retouched(&img, &r, vec![face(true)]);
    assert!(img.data.iter().zip(&out.data).all(|(a, b)| same(*a, *b)));
}

#[test]
fn nothing_outside_the_masks_changes() {
    let (w, h) = (120, 90);
    let img = mouth(w, h);
    let f = face(true);
    let out = retouched(&img, &settings(&ALL_ON), vec![f.clone()]);
    let lips = f.mask(Part::Lips).cloned();
    let mut changed = 0;
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            let (a, b) = (img.data[y * w + x], out.data[y * w + x]);
            // (the Teeth mask lies inside the Lips mask)
            if lips.as_ref().map_or(0.0, |m| m.sample(u, v)) == 0.0 {
                assert!(same(a, b), "({x}, {y}) is outside the lips and teeth: {a:?} → {b:?}");
            } else if !same(a, b) {
                changed += 1;
            }
        }
    }
    assert!(changed > 1000, "{changed} pixels changed");
}

#[test]
fn teeth_whitening_takes_the_yellow_out_lifts_the_teeth_and_keeps_the_gaps() {
    let img = mouth(160, 120);
    let out = retouched(&img, &settings(&[("teethWhitening", 100.0)]), vec![face(true)]);
    let tooth = (0.42, 0.5);
    let ([l0, _, b0], [l1, _, b1]) = (oklab(at(&img, tooth.0, tooth.1)), oklab(at(&out, tooth.0, tooth.1)));
    assert!(b1 < b0 * 0.3, "yellow {b0} → {b1}");
    assert!(l1 > l0 + 0.02 && l1 < TEETH_CEILING, "lightness {l0} → {l1}: lighter, never paper white");
    // a gap between teeth stays dark: lifted far less than the tooth beside it
    let gap_u = (0..160).map(|x| x as f64 / 160.0).find(|u| is_teeth(*u, 0.5) && oklab(at(&img, *u, 0.5))[0] < 0.4).unwrap_or(0.0);
    let (g0, g1) = (oklab(at(&img, gap_u, 0.5))[0], oklab(at(&out, gap_u, 0.5))[0]);
    assert!(g1 - g0 < (l1 - l0) * 0.3, "gap {g0} → {g1}, tooth {l0} → {l1}");
    // the shaded row stays darker than the lit one
    assert!(oklab(at(&out, tooth.0, 0.41))[0] < l1);
    // the lips around are not teeth
    assert!(same(at(&img, 0.3, 0.6), at(&out, 0.3, 0.6)));
}

#[test]
fn a_closed_mouth_has_no_teeth_to_whiten() {
    let img = mouth(80, 60);
    let out = retouched(&img, &settings(&[("teethWhitening", 100.0)]), vec![face(false)]);
    assert!(img.data.iter().zip(&out.data).all(|(a, b)| same(*a, *b)));
}

#[test]
fn lip_color_takes_the_picked_colour_keeps_the_shading_and_leaves_the_teeth() {
    let img = mouth(160, 120);
    // a plum
    let r = settings(&[("lipColorAmount", 100.0), ("lipColorR", 110.0), ("lipColorG", 30.0), ("lipColorB", 120.0)]);
    let out = retouched(&img, &r, vec![face(true)]);
    let target = lightcraft_retouch::teeth_lips::oklab_of_srgb8([110.0, 30.0, 120.0]);
    let hue = |p: [f32; 3]| {
        let [_, a, b] = oklab(p);
        b.atan2(a)
    };
    let (dim, lit) = (at(&out, 0.27, 0.6), at(&out, 0.73, 0.6));
    for p in [dim, lit] {
        assert!((hue(p) - target[2].atan2(target[1])).abs() < 0.05, "hue {} vs {}", hue(p), target[2].atan2(target[1]));
    }
    assert!(oklab(dim)[0] < oklab(lit)[0], "the shading across the lips stays");
    // the teeth inside the lips mask keep their colour
    assert!(same(at(&img, 0.5, 0.5), at(&out, 0.5, 0.5)));
}

#[test]
fn lip_hue_turns_and_lip_saturation_scales_the_lips() {
    let img = mouth(80, 60);
    let p0 = oklab(at(&img, 0.3, 0.6));
    let out = retouched(&img, &settings(&[("lipHue", 100.0)]), vec![face(true)]);
    let p1 = oklab(at(&out, 0.3, 0.6));
    let turn = p1[2].atan2(p1[1]) - p0[2].atan2(p0[1]);
    assert!((turn - MAX_HUE_TURN).abs() < 0.01, "turned {turn}");
    assert!((chroma(at(&out, 0.3, 0.6)) - chroma(at(&img, 0.3, 0.6))).abs() < 1e-3, "a turn keeps chroma");
    let less = retouched(&img, &settings(&[("lipSaturation", -100.0)]), vec![face(true)]);
    let more = retouched(&img, &settings(&[("lipSaturation", 100.0)]), vec![face(true)]);
    let c = chroma(at(&img, 0.3, 0.6));
    assert!(chroma(at(&less, 0.3, 0.6)) < c * 0.5 && chroma(at(&more, 0.3, 0.6)) > c * 1.3);
}

const INNER: [usize; 20] = [78, 191, 80, 81, 82, 13, 312, 311, 310, 415, 308, 324, 318, 402, 317, 14, 87, 178, 88, 95];
const OUTER_IDS: [usize; 20] = [61, 185, 40, 39, 37, 0, 267, 269, 270, 409, 291, 375, 321, 405, 314, 17, 84, 181, 91, 146];
/// The outer lip outline the mesh tests use (an ellipse a little inside the Lips mask's box).
const OUTER: (f64, f64, f64, f64) = (0.27, 0.32, 0.73, 0.63);

/// Point `i` of `n` around the ellipse in box (x0, y0, x1, y1).
fn around(i: usize, n: usize, (x0, y0, x1, y1): (f64, f64, f64, f64)) -> (f64, f64) {
    let t = i as f64 / n as f64 * std::f64::consts::TAU;
    ((x0 + x1) / 2.0 + (x1 - x0) / 2.0 * t.cos(), (y0 + y1) / 2.0 - (y1 - y0) / 2.0 * t.sin())
}

/// Landmarks as the app gives them: five stand-ins, then 478 mesh points, all at the mouth's centre but the outer and
/// inner lip outlines on ellipses in boxes `outer` and `inner`.
fn mesh(outer: (f64, f64, f64, f64), inner: (f64, f64, f64, f64)) -> Vec<(f64, f64)> {
    let mut m = vec![(0.5, 0.47); 478];
    for (k, i) in INNER.iter().enumerate() {
        m[*i] = around(k, INNER.len(), inner);
    }
    for (k, i) in OUTER_IDS.iter().enumerate() {
        m[*i] = around(k, OUTER_IDS.len(), outer);
    }
    let mut l = vec![(0.4, 0.2); 5];
    l.extend(m);
    l
}

/// Whether (`u`, `v`) lies inside the polygon through `pts`.
fn in_polygon(pts: &[(f64, f64)], u: f64, v: f64) -> bool {
    let mut inside = false;
    for (i, a) in pts.iter().enumerate() {
        let b = pts[(i + 1) % pts.len()];
        if (a.1 > v) != (b.1 > v) && u < a.0 + (v - a.1) * (b.0 - a.0) / (b.1 - a.1) {
            inside = !inside;
        }
    }
    inside
}

/// With the face mesh, the lip colour stays within the outer lip outline's margin ([`LIP_MARGIN`] of the face): a
/// lip-mask pixel past it keeps its bits.
#[test]
fn the_lip_colour_stays_within_the_outer_outlines_margin() {
    let (w, h) = (320, 240);
    let img = mouth(w, h);
    let mut f = face(true);
    f.landmarks = mesh(OUTER, (0.36, 0.42, 0.64, 0.53));
    // the face's size in pixels (the longer side of its box), and the margin in pixels
    let margin = (0.7 * w as f64).max(1.0 * h as f64) * LIP_MARGIN;
    let outline: Vec<(f64, f64)> = (0..20).map(|k| around(k, 20, OUTER)).collect();
    let dist = |u: f64, v: f64| {
        let (px, py) = (u * w as f64, v * h as f64);
        (0..outline.len())
            .map(|i| {
                let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
                let (ax, ay, bx, by) = (a.0 * w as f64, a.1 * h as f64, b.0 * w as f64, b.1 * h as f64);
                let (ex, ey) = (bx - ax, by - ay);
                let t = (((px - ax) * ex + (py - ay) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
                (px - ax - t * ex).hypot(py - ay - t * ey)
            })
            .fold(f64::INFINITY, f64::min)
    };
    let out = retouched(&img, &settings(&[("lipColorAmount", 100.0), ("lipSaturation", 100.0)]), vec![f]);
    let (mut past, mut inside_changed) = (0, 0);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            let i = y * w + x;
            if in_polygon(&outline, u, v) {
                inside_changed += usize::from(!same(img.data[i], out.data[i]));
            } else if dist(u, v) > margin {
                // (the Lips mask reaches past the outline and its margin here)
                past += usize::from(is_lips(u, v));
                assert!(same(img.data[i], out.data[i]), "({x}, {y}) is past the outer lip outline's margin");
            }
        }
    }
    assert!(past > 500 && inside_changed > 1000, "{past} lip-mask pixels past the margin, {inside_changed} changed inside");
}

/// No halo: lips and teeth next to a mask's edge take the same change as their middles; nothing from the other side
/// of the edge bleeds in.
#[test]
fn no_halo_at_the_edges_of_the_lips_and_teeth() {
    let (w, h) = (160, 120);
    let (lip, tooth, skin) = (rgb([0.55, 0.12, 0.04]), rgb([0.75, -0.005, 0.05]), rgb([0.7, 0.04, 0.06]));
    let img = Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
        if is_teeth(u, v) {
            tooth
        } else if is_lips(u, v) {
            lip
        } else {
            skin
        }
    });
    let out = retouched(&img, &settings(&[("teethWhitening", 100.0), ("lipColorAmount", 100.0)]), vec![face(true)]);
    let f = face(true);
    let full = |part: Part, u: f64, v: f64| f.mask(part).map_or(0.0, |m| m.sample(u, v)) == 1.0;
    let (mid_lip, mid_tooth) = (at(&out, 0.3, 0.6), at(&out, 0.5, 0.5));
    assert!(!same(mid_lip, lip) && !same(mid_tooth, tooth), "both changed");
    let mut edge = 0;
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            let p = out.data[y * w + x];
            let lip_px = full(Part::Lips, u, v) && f.mask(Part::Teeth).map_or(0.0, |m| m.sample(u, v)) == 0.0 && same(img.data[y * w + x], lip);
            let tooth_px = full(Part::Teeth, u, v) && same(img.data[y * w + x], tooth);
            for (is, mid) in [(lip_px, mid_lip), (tooth_px, mid_tooth)] {
                if is {
                    edge += 1;
                    assert!((0..3).all(|k| (p[k] - mid[k]).abs() < 1e-5), "({x}, {y}): {p:?} vs {mid:?}");
                }
            }
        }
    }
    assert!(edge > 1000, "{edge} pixels checked");
}

/// With the face mesh, the open mouth inside the lips (here: no Teeth mask) is left out of the lip edits.
#[test]
fn the_mesh_keeps_the_open_mouth_out_of_the_lips() {
    // the open mouth is a dark red (its shade and the gums), close to the lips' own hue: only the mesh tells it apart
    let mut img = mouth(160, 120);
    let (w, h) = (img.width, img.height);
    for (i, p) in img.data.iter_mut().enumerate() {
        if is_teeth((i % w) as f64 / w as f64, (i / w) as f64 / h as f64) {
            *p = rgb([0.3, 0.08, 0.03]);
        }
    }
    let mut f = face(false);
    f.landmarks = mesh(OUTER, (0.33, 0.38, 0.67, 0.57));
    let out = retouched(&img, &settings(&[("lipColorAmount", 100.0), ("lipSaturation", 50.0)]), vec![f]);
    assert!(same(at(&img, 0.5, 0.47), at(&out, 0.5, 0.47)), "inside the mouth");
    assert!(!same(at(&img, 0.3, 0.47), at(&out, 0.3, 0.47)), "on the lips");
}

/// A face cut by the edge, a 1 × 1 and a flat grey photo: no panic, numbers stay numbers.
#[test]
fn a_face_cut_by_the_edge_a_one_pixel_and_a_flat_grey_photo() {
    let cut = Face {
        masks: vec![(Part::Lips, mask_of(|_, _| true)), (Part::Teeth, mask_of(|u, _| u < 0.5))],
        ..Face::boxed(NormRect::new(-0.4, -0.3, 0.3, 0.4))
    };
    for img in [mouth(64, 48), Rgb32f::from_fn(1, 1, |_, _| [0.4, 0.3, 0.2]), Rgb32f::from_fn(50, 50, |_, _| [0.18; 3])] {
        let out = retouched(&img, &settings(&ALL_ON), vec![cut.clone(), face(true)]);
        assert!(out.data.iter().flatten().all(|v| v.is_finite()));
    }
    // flat grey: the lips have no colour to measure, the teeth no yellow; nothing turns a colour from nothing
    let grey = Rgb32f::from_fn(50, 50, |_, _| [0.18; 3]);
    let out = retouched(&grey, &settings(&[("lipHue", 100.0), ("lipSaturation", 100.0)]), vec![face(true)]);
    assert!(out.data.iter().all(|p| chroma(*p) < 1e-3));
}

/// The masks are in fractions of the photo and sizes in fractions of the face: a draft and an export four times its
/// size agree once the export is scaled down by averaging.
#[test]
fn a_draft_and_a_bigger_export_agree_after_scaling() {
    let (w, h) = (60, 40);
    let r = settings(&ALL_ON);
    let draft = retouched(&mouth(w, h), &r, vec![face(true)]);
    let big = retouched(&mouth(w * 4, h * 4), &r, vec![face(true)]);
    let (mut worst, mut sum) = (0.0f32, 0.0f32);
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let mut s = 0.0;
                for dy in 0..4 {
                    for dx in 0..4 {
                        s += big.data[(y * 4 + dy) * w * 4 + x * 4 + dx][c];
                    }
                }
                let d = (s / 16.0 - draft.data[y * w + x][c]).abs();
                worst = worst.max(d);
                sum += d;
            }
        }
    }
    // (edges of the hard-edged test masks and the gaps resolve differently; the mean shows the rest agrees)
    let mean = sum / (w * h * 3) as f32;
    assert!(mean < 0.01 && worst < 0.35, "mean {mean} worst {worst}");
}

/// The face search keeps only the masks some feature lists: Teeth and Lips list theirs.
#[test]
fn the_teeth_and_lips_masks_are_kept_for_this_feature() {
    use lightcraft_retouch::Feature;
    assert_eq!(TeethLips::PARTS, &[Part::Lips, Part::Teeth]);
    let kept = lightcraft_retouch::parts_read();
    assert!(kept.contains(&Part::Lips) && kept.contains(&Part::Teeth));
}
