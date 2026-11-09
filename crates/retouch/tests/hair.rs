//! Tests of the Stray Hair feature (`lightcraft_retouch::hair`), on a synthetic portrait: a pale sky, a dark hair
//! mass over the head, a face of textured skin with eyebrows, a stray strand across the forehead and a flyaway
//! against the sky beside the hair.

mod common;

use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::hair::{Hair, MIN_FACE_PX, with_strands, with_strands_on_crops};
use lightcraft_retouch::input::Searched;
use lightcraft_retouch::{Face, Feature, Input, NormRect, Part, PartMask, Retouch, Stage, View, WholePhoto};

#[test]
fn off_by_default_and_amount_zero_changes_nothing() {
    common::off_changes_nothing::<Hair>();
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<Hair>();
}

const SKY: [f32; 3] = [0.55, 0.65, 0.78];
const HAIR: [f32; 3] = [0.03, 0.02, 0.015];
const SKIN: [f32; 3] = [0.5, 0.36, 0.28];
const BOUNDS: NormRect = NormRect { x0: 0.35, y0: 0.35, x1: 0.65, y1: 0.75 };
/// A strand's half width, in fractions of the picture (a pixel and a fifth wide at 400 px).
const HALF: f64 = 0.0015;
/// The forehead strand and the flyaway, as segments (fractions of the picture).
const FOREHEAD: [(f64, f64); 2] = [(0.42, 0.4), (0.57, 0.44)];
const FLYAWAY: [(f64, f64); 2] = [(0.26, 0.42), (0.24, 0.28)];

fn ellipse(nx: f64, ny: f64, c: (f64, f64), rx: f64, ry: f64) -> bool {
    ((nx - c.0) / rx).powi(2) + ((ny - c.1) / ry).powi(2) <= 1.0
}

fn in_face(nx: f64, ny: f64) -> bool {
    ellipse(nx, ny, (0.5, 0.56), 0.15, 0.2)
}

fn in_hair(nx: f64, ny: f64) -> bool {
    ellipse(nx, ny, (0.5, 0.4), 0.22, 0.2) && ny < 0.5 && !in_face(nx, ny)
}

fn in_brow(nx: f64, ny: f64) -> bool {
    (0.47..0.49).contains(&ny) && ((0.41..0.47).contains(&nx) || (0.53..0.59).contains(&nx))
}

/// Distance from (`nx`, `ny`) to segment `s`.
fn to_segment(nx: f64, ny: f64, s: [(f64, f64); 2]) -> f64 {
    let [(ax, ay), (bx, by)] = s;
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((nx - ax) * dx + (ny - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((nx - ax - t * dx).powi(2) + (ny - ay - t * dy).powi(2)).sqrt()
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
}

/// The picture at a point, without the strands (`strands` false) or with them; `beard` covers the lower face with
/// dense short dark lines.
fn colour(nx: f64, ny: f64, strands: bool, beard: bool) -> [f32; 3] {
    let mut c = if in_hair(nx, ny) {
        HAIR
    } else if in_face(nx, ny) {
        // pores: a fine, fixed texture
        let t = 1.0 + 0.05 * ((nx * 2310.0).sin() * (ny * 1970.0).sin()) as f32;
        let skin = SKIN.map(|v| v * t);
        if in_brow(nx, ny) { HAIR } else { skin }
    } else {
        SKY
    };
    if beard && in_face(nx, ny) && ny > 0.64 && ((nx * 260.0).fract() < 0.35 || (ny * 230.0 + nx * 40.0).fract() < 0.3) {
        c = mix(c, HAIR, 0.85);
    }
    if strands {
        for s in [FOREHEAD, FLYAWAY] {
            if to_segment(nx, ny, s) < HALF {
                c = mix(c, HAIR, 0.9);
            }
        }
    }
    c
}

/// The picture at `w × h`, each pixel the mean of 4 × 4 samples (so a draft and an export draw the same strands).
fn picture(w: usize, h: usize, strands: bool, beard: bool) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let mut acc = [0.0f32; 3];
        for sy in 0..4 {
            for sx in 0..4 {
                let p = colour((x as f64 + (sx as f64 + 0.5) / 4.0) / w as f64, (y as f64 + (sy as f64 + 0.5) / 4.0) / h as f64, strands, beard);
                acc = std::array::from_fn(|k| acc[k] + p[k] / 16.0);
            }
        }
        acc
    })
}

/// A part mask over the whole picture at 128 × 128, as a parser's (coarse, soft at the edge): the strands are not in
/// any mask (the parser misses them).
fn mask(f: impl Fn(f64, f64) -> bool) -> PartMask {
    let n = 128;
    let plane = Plane::from_fn(n, n, |x, y| if f((x as f64 + 0.5) / n as f64, (y as f64 + 0.5) / n as f64) { 1.0 } else { 0.0 });
    PartMask { plane, area: NormRect::new(0.0, 0.0, 1.0, 1.0) }
}

fn face(parts: &[Part]) -> Face {
    let all = [
        (Part::Hair, mask(in_hair)),
        (Part::FacialSkin, mask(|x, y| in_face(x, y) && !in_brow(x, y))),
        (Part::Eyebrows, mask(in_brow)),
        (Part::EntirePerson, mask(|x, y| in_hair(x, y) || in_face(x, y))),
    ];
    Face { masks: all.into_iter().filter(|(p, _)| parts.contains(p)).collect(), ..Face::boxed(BOUNDS) }
}

fn every_part() -> Vec<Part> {
    vec![Part::Hair, Part::FacialSkin, Part::Eyebrows, Part::EntirePerson]
}

fn settings(amount: f64) -> Retouch {
    let mut r = Retouch::default();
    assert!(r.global.set("retouch.hair.amount", amount));
    r
}

fn run_in(img: &mut Rgb32f, r: &Retouch, input: &Input, view: &dyn View) {
    for stage in [Stage::Retouch, Stage::Geometry, Stage::Background] {
        lightcraft_retouch::run::apply(img, r, input, view, stage);
    }
}

/// The faces with their strands searched on `picture` (as the provider searches the parse's picture).
fn searched(picture: &Rgb32f, faces: Vec<Face>) -> Input {
    Input::new(with_strands(picture, faces))
}

/// `src` retouched with `input`'s faces.
fn render(src: &Rgb32f, amount: f64, input: &Input) -> Rgb32f {
    let mut img = src.clone();
    let view = WholePhoto { width: img.width, height: img.height };
    run_in(&mut img, &settings(amount), input, &view);
    img
}

/// `src` retouched, its strands searched on itself.
fn retouched(src: &Rgb32f, amount: f64, parts: &[Part]) -> Rgb32f {
    render(src, amount, &searched(src, vec![face(parts)]))
}

fn diff(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max)
}

/// How much of the strands is left: the strands' pixels' distance from the picture without them, after over before.
fn left_of(after: &Rgb32f, before: &Rgb32f, clean: &Rgb32f, near: impl Fn(f64, f64) -> bool) -> f32 {
    let (mut now, mut was) = (0.0f32, 0.0f32);
    for y in 0..before.height {
        for x in 0..before.width {
            let (nx, ny) = ((x as f64 + 0.5) / before.width as f64, (y as f64 + 0.5) / before.height as f64);
            if near(nx, ny) {
                let c = clean.get(x, y);
                now += diff(after.get(x, y), c);
                was += diff(before.get(x, y), c);
            }
        }
    }
    assert!(was > 0.0, "the strands are in the picture");
    now / was
}

fn on(s: [(f64, f64); 2]) -> impl Fn(f64, f64) -> bool {
    move |nx, ny| to_segment(nx, ny, s) < HALF * 2.0
}

#[test]
fn strands_across_the_forehead_and_against_the_sky_go() {
    let (w, h) = (400, 400);
    let (src, clean) = (picture(w, h, true, false), picture(w, h, false, false));
    let after = retouched(&src, 100.0, &every_part());
    for (name, s) in [("forehead", FOREHEAD), ("flyaway", FLYAWAY)] {
        let left = left_of(&after, &src, &clean, on(s));
        assert!(left < 0.3, "{name}: {left:.2} of the strand is left");
    }
    // half the amount takes about half of it away
    let half = retouched(&src, 50.0, &every_part());
    let left = left_of(&half, &src, &clean, on(FOREHEAD));
    assert!((0.4..0.75).contains(&left), "at 50: {left:.2} left");
}

#[test]
fn only_strands_change_never_the_hair_mass_the_eyebrows_or_the_skin_beside() {
    let (w, h) = (400, 400);
    let src = picture(w, h, true, false);
    let after = retouched(&src, 100.0, &every_part());
    // (a strand's repair covers it and the sharpening halo beside it: a few pixels either side)
    let near_strand = |nx: f64, ny: f64| [FOREHEAD, FLYAWAY].iter().any(|s| to_segment(nx, ny, *s) < 6.0 / w as f64);
    let mut changed = 0;
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            if after.get(x, y) != src.get(x, y) {
                changed += 1;
                assert!(near_strand(nx, ny), "a pixel away from the strands changed at {x},{y}");
                assert!(!in_brow(nx, ny), "an eyebrow pixel changed at {x},{y}");
            }
        }
    }
    assert!(changed > 50, "the strands changed ({changed})");
    // the hair mass, well inside it, keeps every bit
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            let deep = [(-0.02, 0.0), (0.02, 0.0), (0.0, -0.02), (0.0, 0.02)].iter().all(|(dx, dy)| in_hair(nx + dx, ny + dy));
            if deep {
                assert_eq!(after.get(x, y).map(f32::to_bits), src.get(x, y).map(f32::to_bits), "hair mass at {x},{y}");
            }
        }
    }
}

#[test]
fn texture_beside_a_strand_is_kept_and_nothing_halos() {
    let (w, h) = (400, 400);
    let (src, clean) = (picture(w, h, true, false), picture(w, h, false, false));
    let after = retouched(&src, 100.0, &every_part());
    // skin pixels 2–4 px from the forehead strand: as in the picture without it (no dark halo, no smear)
    let mut worst = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            let d = to_segment(nx, ny, FOREHEAD) * w as f64;
            if (2.0..4.0).contains(&d) && in_face(nx, ny) && !in_brow(nx, ny) {
                worst = worst.max(diff(after.get(x, y), clean.get(x, y)) / SKIN[0]);
            }
        }
    }
    assert!(worst < 0.08, "skin beside the strand differs by {worst:.3}");
    // the repaired strand keeps the skin's fine texture: its pixels vary as the skin's around do
    let spread = |img: &Rgb32f| -> f32 {
        let v: Vec<f32> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|(x, y)| to_segment((*x as f64 + 0.5) / w as f64, (*y as f64 + 0.5) / h as f64, FOREHEAD) * (w as f64) < 0.7)
            .map(|(x, y)| img.get(x, y)[0])
            .collect();
        let mean = v.iter().sum::<f32>() / v.len().max(1) as f32;
        (v.iter().map(|a| (a - mean).powi(2)).sum::<f32>() / v.len().max(1) as f32).sqrt() / mean
    };
    assert!(spread(&after) < 0.12, "the repaired strand is not smeared: {:.3}", spread(&after));
}

#[test]
fn a_beard_is_left_alone() {
    let (w, h) = (400, 400);
    let src = picture(w, h, true, true);
    let after = retouched(&src, 100.0, &every_part());
    let (mut beard, mut changed) = (0, 0);
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            if in_face(nx, ny) && ny > 0.66 {
                beard += 1;
                changed += usize::from(after.get(x, y) != src.get(x, y));
            }
        }
    }
    assert!(changed * 100 < beard, "{changed} of {beard} beard pixels changed");
    let clean = picture(w, h, false, true);
    assert!(left_of(&after, &src, &clean, on(FOREHEAD)) < 0.3, "the forehead strand still goes");
}

#[test]
fn a_face_without_hair_or_skin_masks_is_left_alone() {
    let (w, h) = (300, 300);
    let src = picture(w, h, true, false);
    for parts in [vec![], vec![Part::Eyebrows, Part::EntirePerson]] {
        let after = retouched(&src, 100.0, &parts);
        assert!(after.data.iter().zip(&src.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)), "{parts:?}");
    }
    // without the Hair mask the face's strands still go, and nothing outside the face's skin changes
    let after = retouched(&src, 100.0, &[Part::FacialSkin, Part::Eyebrows]);
    let clean = picture(w, h, false, false);
    assert!(left_of(&after, &src, &clean, on(FOREHEAD)) < 0.35, "the forehead strand goes without a Hair mask");
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            // (a pixel on the face's edge mixes skin and what is beside it)
            if !ellipse(nx, ny, (0.5, 0.56), 0.16, 0.21) {
                assert_eq!(after.get(x, y), src.get(x, y), "outside the face at {x},{y}");
            }
        }
    }
}

#[test]
fn a_flat_grey_photo_stays_flat() {
    let src = Rgb32f::filled(320, 320, [0.18; 3]);
    let after = retouched(&src, 100.0, &every_part());
    assert!(after.data.iter().zip(&src.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)));
}

/// A draft and an export four times its size remove the same strands: they are searched once, on the parse's
/// picture, and kept in the photo's frame.
#[test]
fn a_draft_and_an_export_agree() {
    let (dw, ew) = (400, 1600);
    let draft = (picture(dw, dw, true, false), picture(dw, dw, false, false));
    let export = (picture(ew, ew, true, false), picture(ew, ew, false, false));
    // the strands are searched once, on the parse's picture, and every render repairs them
    let input = searched(&picture(800, 800, true, false), vec![face(&every_part())]);
    let d_after = render(&draft.0, 100.0, &input);
    let e_after = render(&export.0, 100.0, &input);
    for (name, s) in [("forehead", FOREHEAD), ("flyaway", FLYAWAY)] {
        let d = left_of(&d_after, &draft.0, &draft.1, on(s));
        let e = left_of(&e_after, &export.0, &export.1, on(s));
        assert!(d < 0.3 && e < 0.3, "{name}: draft {d:.2}, export {e:.2} left");
    }
    // the export scaled down looks like the draft around the strands
    let k = ew / dw;
    let mut worst = 0.0f32;
    for y in 0..dw {
        for x in 0..dw {
            let (nx, ny) = ((x as f64 + 0.5) / dw as f64, (y as f64 + 0.5) / dw as f64);
            if !(on(FOREHEAD)(nx, ny) || on(FLYAWAY)(nx, ny)) {
                continue;
            }
            let mut acc = [0.0f32; 3];
            for sy in 0..k {
                for sx in 0..k {
                    let p = e_after.get(x * k + sx, y * k + sy);
                    acc = std::array::from_fn(|c| acc[c] + p[c] / (k * k) as f32);
                }
            }
            worst = worst.max(diff(acc, d_after.get(x, y)));
        }
    }
    assert!(worst < 0.12, "the export scaled down differs from the draft by {worst:.3}");
}

/// A window of the photo (a zoomed render): pixel (`x`, `y`) of the window is (`x + ox`, `y + oy`) of the photo.
struct Window {
    ox: usize,
    oy: usize,
    w: usize,
    h: usize,
}

impl View for Window {
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64) {
        ((x + self.ox as f64) / self.w as f64, (y + self.oy as f64) / self.h as f64)
    }

    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        let p = (nx * self.w as f64 - self.ox as f64, ny * self.h as f64 - self.oy as f64);
        (p.0.is_finite() && p.1.is_finite()).then_some(p)
    }
}

/// Zoom never changes the result: a window holding the face and its masks' areas grown by [`Hair::REACH`] (what the
/// pipeline renders for a zoomed window, `window_for_reads`) gives the whole frame's pixels. The portrait is drawn at
/// half size in the middle of the photo, so that window is smaller than the photo.
#[test]
fn a_zoomed_window_gives_the_whole_frames_result() {
    let (w, h) = (600, 600);
    // photo coordinates → the portrait's
    let at = |n: f64| 0.5 + (n - 0.5) * 2.0;
    let back = |n: f64| 0.5 + (n - 0.5) / 2.0;
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let mut acc = [0.0f32; 3];
        for sy in 0..4 {
            for sx in 0..4 {
                let (nx, ny) = ((x as f64 + (sx as f64 + 0.5) / 4.0) / w as f64, (y as f64 + (sy as f64 + 0.5) / 4.0) / h as f64);
                let p = colour(at(nx), at(ny), true, false);
                acc = std::array::from_fn(|k| acc[k] + p[k] / 16.0);
            }
        }
        acc
    });
    let area = NormRect::new(back(0.15), back(0.1), back(0.85), back(0.9));
    let local = |f: fn(f64, f64) -> bool| {
        let n = 96;
        let plane = Plane::from_fn(n, n, |x, y| {
            if f(at(area.x0 + (x as f64 + 0.5) / n as f64 * area.width()), at(area.y0 + (y as f64 + 0.5) / n as f64 * area.height())) {
                1.0
            } else {
                0.0
            }
        });
        PartMask { plane, area }
    };
    let bounds = NormRect::new(back(BOUNDS.x0), back(BOUNDS.y0), back(BOUNDS.x1), back(BOUNDS.y1));
    let f = Face {
        masks: vec![
            (Part::Hair, local(in_hair)),
            (Part::FacialSkin, local(|x, y| in_face(x, y) && !in_brow(x, y))),
            (Part::Eyebrows, local(in_brow)),
        ],
        ..Face::boxed(bounds)
    };
    let input = searched(&src, vec![f]);
    let r = settings(100.0);
    let mut full = src.clone();
    run_in(&mut full, &r, &input, &WholePhoto { width: w, height: h });
    // the read box: the face's box and the masks' area, grown by REACH × the face's size (and a pixel)
    let size = bounds.width().max(bounds.height()) * w as f64;
    let grow = Hair::REACH * size + 2.0;
    let rb = [area.x0 * w as f64 - grow, area.y0 * h as f64 - grow, area.x1 * w as f64 + grow, area.y1 * h as f64 + grow];
    let (x0, y0) = (rb[0].floor().max(0.0) as usize, rb[1].floor().max(0.0) as usize);
    let (x1, y1) = ((rb[2].ceil() as usize).min(w), (rb[3].ceil() as usize).min(h));
    assert!(x0 > 0 && y0 > 0 && x1 < w && y1 < h, "the read box is inside the photo: {x0},{y0} {x1},{y1}");
    let mut win = src.crop(x0, y0, x1 - x0, y1 - y0);
    run_in(&mut win, &r, &input, &Window { ox: x0, oy: y0, w, h });
    let mut worst = 0.0f32;
    let mut changed = 0;
    for y in 0..win.height {
        for x in 0..win.width {
            worst = worst.max(diff(win.get(x, y), full.get(x0 + x, y0 + y)));
            changed += usize::from(full.get(x0 + x, y0 + y) != src.get(x0 + x, y0 + y));
        }
    }
    assert!(changed > 50, "the strands are in the window ({changed})");
    assert!(worst <= 1.0 / 255.0, "the window differs from the whole frame by {worst}");
}

/// A render only repairs what the search found: faces not searched yet get no Stray Hair, the same input gives the
/// same pixels, and a photo without strands is nearly untouched.
#[test]
fn a_render_never_searches_and_repairs_the_same_strands_every_time() {
    let (w, h) = (300, 300);
    let src = picture(w, h, true, false);
    let not_yet = render(&src, 100.0, &Input::new(vec![face(&every_part())]));
    assert!(not_yet.data.iter().zip(&src.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)), "no search, no change");
    let input = searched(&src, vec![face(&every_part())]);
    let (first, again) = (render(&src, 100.0, &input), render(&src, 100.0, &input));
    assert!(first.data.iter().zip(&src.data).any(|(a, b)| a != b), "the strands are repaired");
    assert!(first.data.iter().zip(&again.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)));
    let clean = picture(w, h, false, false);
    let after = retouched(&clean, 100.0, &every_part());
    let changed = after.data.iter().zip(&clean.data).filter(|(a, b)| a != b).count();
    assert!(changed < 20, "{changed} pixels changed on a photo without strands");
}

/// The repair reads the pixels the render has: a brighter render (an exposure or a Skin change before the step)
/// still loses the strands, with no new search.
#[test]
fn a_change_before_the_step_needs_no_new_search() {
    let (w, h) = (400, 400);
    let input = searched(&picture(w, h, true, false), vec![face(&every_part())]);
    let brighter = |img: Rgb32f| -> Rgb32f { Rgb32f::from_fn(img.width, img.height, |x, y| img.get(x, y).map(|v| v * 1.6)) };
    let (src, clean) = (brighter(picture(w, h, true, false)), brighter(picture(w, h, false, false)));
    let after = render(&src, 100.0, &input);
    for (name, s) in [("forehead", FOREHEAD), ("flyaway", FLYAWAY)] {
        let left = left_of(&after, &src, &clean, on(s));
        assert!(left < 0.3, "{name}: {left:.2} left on the brighter render");
    }
}

/// A new parse with another Hair mask (another hair edge, another parts set) gives other strands: here the flyaway
/// is inside the new mask's hair, so it is hair and stays.
#[test]
fn a_new_parse_with_another_hair_mask_finds_other_strands() {
    let (w, h) = (400, 400);
    let (src, clean) = (picture(w, h, true, false), picture(w, h, false, false));
    let first = searched(&src, vec![face(&every_part())]);
    let wider = |nx: f64, ny: f64| (in_hair(nx, ny) || ((0.2..0.3).contains(&nx) && (0.25..0.45).contains(&ny))) && !in_face(nx, ny);
    let mut f = face(&every_part());
    for (p, m) in &mut f.masks {
        if *p == Part::Hair {
            *m = mask(wider);
        }
    }
    let second = searched(&src, vec![f]);
    assert_ne!(first.fingerprint(), second.fingerprint(), "another parse renders anew");
    assert!(left_of(&render(&src, 100.0, &first), &src, &clean, on(FLYAWAY)) < 0.3, "the first parse's flyaway goes");
    assert!(left_of(&render(&src, 100.0, &second), &src, &clean, on(FLYAWAY)) > 0.8, "inside the new hair mask it stays");
}

/// A face under the least size searched gets no strands, and no change.
#[test]
fn a_face_too_small_to_search_is_left_alone() {
    let side = (MIN_FACE_PX / 0.4 * 0.8) as usize;
    let src = picture(side, side, true, false);
    let input = searched(&src, vec![face(&every_part())]);
    assert!(input.faces.iter().all(|f| f.strands.as_ref().is_some_and(|s| s.is_empty())));
    let after = render(&src, 100.0, &input);
    assert!(after.data.iter().zip(&src.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)));
}

const SWEATER: [f32; 3] = [0.03, 0.03, 0.04];

fn in_sweater(nx: f64, ny: f64) -> bool {
    (0.12..0.24).contains(&nx) && (0.5..0.8).contains(&ny)
}

/// A flyaway across the sky that runs into a dark sweater's edge (clothes: the person's pixels that are neither hair
/// nor skin) is repaired, the sweater keeps every bit, and no strand found reaches into it: the strands' grown,
/// softened edge is cut at the clothes' margin (it reached two cells in before the cut).
#[test]
fn a_flyaway_beside_a_sweater_leaves_the_sweater_alone() {
    let (w, h) = (400, 400);
    let strand = |nx: f64, ny: f64| (ny - 0.6).abs() < HALF && (0.2..0.3).contains(&nx);
    let draw = |strands: bool| {
        Rgb32f::from_fn(w, h, move |x, y| {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            if in_sweater(nx, ny) {
                SWEATER
            } else if strands && strand(nx, ny) {
                mix(SKY, HAIR, 0.9)
            } else {
                colour(nx, ny, false, false)
            }
        })
    };
    let (src, clean) = (draw(true), draw(false));
    let mut f = face(&every_part());
    for (p, m) in &mut f.masks {
        if *p == Part::EntirePerson {
            *m = mask(|x, y| in_hair(x, y) || in_face(x, y) || in_sweater(x, y));
        }
    }
    let input = searched(&src, vec![f]);
    let after = render(&src, 100.0, &input);
    let left = left_of(&after, &src, &clean, |nx, ny| (ny - 0.6).abs() < HALF * 2.0 && (0.27..0.3).contains(&nx));
    assert!(left < 0.3, "the flyaway goes: {left:.2} left");
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            if in_sweater(nx, ny) {
                assert_eq!(after.get(x, y).map(f32::to_bits), src.get(x, y).map(f32::to_bits), "the sweater changed at {x},{y}");
            }
        }
    }
    let Some(s) = input.faces.first().and_then(|f| f.strands.as_ref()) else { panic!("searched") };
    let (cw, ch) = (s.area.width() / s.width as f64, s.area.height() / s.height as f64);
    for cy in 0..s.height {
        for cx in 0..s.width {
            let (nx, ny) = (s.area.x0 + (cx as f64 + 0.5) * cw, s.area.y0 + (cy as f64 + 0.5) * ch);
            let weight = s.cell(cx, cy).map_or(0, |c| c[0]);
            assert!(!(in_sweater(nx, ny) && weight > 0), "a strand cell reaches into the sweater at {nx:.3},{ny:.3} ({weight})");
        }
    }
}

#[test]
fn the_feature_reads_its_parts_and_reach() {
    for p in [Part::Hair, Part::FacialSkin, Part::Eyebrows, Part::EyeWhites, Part::Lips, Part::Others] {
        assert!(Hair::PARTS.contains(&p), "{p:?}");
    }
    // (the search's band past the hair and its reads past that; the zoom test checks it is enough)
    const { assert!(Hair::REACH > 0.3 && Hair::REACH < 0.6) };
}

/// The strands searched on a crop of the head cut from the full-size photo are in the photo's frame, found as on the
/// whole picture, and counted: the readout's numbers.
#[test]
fn a_head_crop_cut_from_the_full_size_photo_finds_the_strands_and_counts_them() {
    let (w, h) = (800, 800);
    let (src, clean) = (picture(w, h, true, false), picture(w, h, false, false));
    let mut asked = Vec::new();
    let mut cut = |r: [usize; 4]| {
        asked.push(r);
        Some(Rgb32f::from_fn(r[2] - r[0], r[3] - r[1], |x, y| src.get(r[0] + x, r[1] + y)))
    };
    let faces = with_strands_on_crops((w, h), vec![face(&every_part())], &mut cut);
    let strands = faces[0].strands.as_ref().expect("searched");
    assert_eq!(strands.searched, Searched::Yes);
    assert!(strands.count >= 2, "the forehead strand and the flyaway: {}", strands.count);
    assert_eq!(strands.face_px, 320);
    assert_eq!(asked.len(), 1, "one crop for the one face");
    assert!(asked.iter().all(|r| r[0] < r[2] && r[1] < r[3] && r[2] <= w && r[3] <= h), "{asked:?}");
    assert!(strands.area.x0 >= 0.0 && strands.area.x1 <= 1.0 && strands.area.y0 >= 0.0 && strands.area.y1 <= 1.0, "{:?}", strands.area);
    let after = render(&src, 100.0, &Input::new(faces));
    for (name, s) in [("forehead", FOREHEAD), ("flyaway", FLYAWAY)] {
        let left = left_of(&after, &src, &clean, on(s));
        assert!(left < 0.3, "{name}: {left:.2} of the strand is left");
    }
}

/// Why a face was not searched is kept with it, for the readout; a crop that cannot be cut leaves the face with none.
#[test]
fn a_face_not_searched_says_why() {
    let side = 800usize;
    let src = picture(side, side, true, false);
    let tiny = Face { masks: face(&every_part()).masks, ..Face::boxed(NormRect::new(0.4, 0.4, 0.4 + 60.0 / side as f64, 0.4 + 60.0 / side as f64)) };
    let bare = Face::boxed(BOUNDS);
    let mut cut = |r: [usize; 4]| Some(Rgb32f::from_fn(r[2] - r[0], r[3] - r[1], |x, y| src.get(r[0] + x, r[1] + y)));
    let faces = with_strands_on_crops((side, side), vec![tiny.clone(), bare.clone(), face(&every_part())], &mut cut);
    let got: Vec<(Searched, u32)> = faces.iter().map(|f| f.strands.as_ref().map(|s| (s.searched, s.face_px)).unwrap_or((Searched::Yes, 0))).collect();
    assert_eq!(got, vec![(Searched::TooSmall, 60), (Searched::NoParts, 320), (Searched::Yes, 320)]);
    // the same on the whole picture
    let whole = with_strands(&src, vec![tiny, bare]);
    let got: Vec<Searched> = whole.iter().filter_map(|f| f.strands.as_ref().map(|s| s.searched)).collect();
    assert_eq!(got, vec![Searched::TooSmall, Searched::NoParts]);
    // a crop that fails: nothing found, nothing wrong
    let none = with_strands_on_crops((side, side), vec![face(&every_part())], &mut |_| None);
    assert!(none[0].strands.as_ref().is_some_and(|s| s.is_empty() && s.count == 0));
}
