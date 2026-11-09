//! Tests of Stray Hair v2, the strands outside the hair taken away with a person matte and a fill
//! (`lightcraft_retouch::hair::with_removals`), on a synthetic portrait: a pale sky with a faint grain, a dark head, and a
//! flyaway leaving the top of it. The matte is the picture's difference from the sky and the fill paints the hole with
//! the sky (the engine's Apple Vision and LaMa stand behind the same trait).

use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::hair::{Fill, Remover, with_removals, with_strands};
use lightcraft_retouch::{Face, Input, NormRect, Part, PartMask, Retouch, Stage, View, WholePhoto};

const SKY: [f32; 3] = [0.55, 0.65, 0.78];
const HAIR: [f32; 3] = [0.03, 0.02, 0.015];
const SKIN: [f32; 3] = [0.5, 0.36, 0.28];
const BOUNDS: NormRect = NormRect { x0: 0.35, y0: 0.35, x1: 0.65, y1: 0.75 };
const HALF: f64 = 0.0012;
/// The flyaway: a segment leaving the top of the head (fractions of the picture).
const FLYAWAY: [(f64, f64); 2] = [(0.5, 0.2), (0.46, 0.07)];

fn ellipse(nx: f64, ny: f64, c: (f64, f64), rx: f64, ry: f64) -> bool {
    ((nx - c.0) / rx).powi(2) + ((ny - c.1) / ry).powi(2) <= 1.0
}

fn in_face(nx: f64, ny: f64) -> bool {
    ellipse(nx, ny, (0.5, 0.56), 0.15, 0.2)
}

fn in_hair(nx: f64, ny: f64) -> bool {
    ellipse(nx, ny, (0.5, 0.4), 0.22, 0.2) && ny < 0.5 && !in_face(nx, ny)
}

fn to_segment(nx: f64, ny: f64, s: [(f64, f64); 2]) -> f64 {
    let [(ax, ay), (bx, by)] = s;
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((nx - ax) * dx + (ny - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((nx - ax - t * dx).powi(2) + (ny - ay - t * dy).powi(2)).sqrt()
}

fn colour(nx: f64, ny: f64, strand: bool) -> [f32; 3] {
    let c = if in_hair(nx, ny) {
        HAIR
    } else if in_face(nx, ny) {
        SKIN
    } else {
        // a faint grain in the sky
        let g = 1.0 + 0.004 * ((nx * 3100.0).sin() * (ny * 2700.0).sin()) as f32;
        SKY.map(|v| v * g)
    };
    if strand && to_segment(nx, ny, FLYAWAY) < HALF {
        return std::array::from_fn(|k| c[k] + (HAIR[k] - c[k]) * 0.9);
    }
    c
}

/// The picture at `w × h`, each pixel the mean of 4 × 4 samples (so a draft and an export draw the same strand).
fn picture(w: usize, h: usize, strand: bool) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let mut acc = [0.0f32; 3];
        for sy in 0..4 {
            for sx in 0..4 {
                let p = colour((x as f64 + (sx as f64 + 0.5) / 4.0) / w as f64, (y as f64 + (sy as f64 + 0.5) / 4.0) / h as f64, strand);
                acc = std::array::from_fn(|k| acc[k] + p[k] / 16.0);
            }
        }
        acc
    })
}

fn mask(f: impl Fn(f64, f64) -> bool) -> PartMask {
    let n = 256;
    let plane = Plane::from_fn(n, n, |x, y| if f((x as f64 + 0.5) / n as f64, (y as f64 + 0.5) / n as f64) { 1.0 } else { 0.0 });
    PartMask { plane, area: NormRect::new(0.0, 0.0, 1.0, 1.0) }
}

fn face() -> Face {
    let masks =
        vec![(Part::Hair, mask(in_hair)), (Part::FacialSkin, mask(in_face)), (Part::EntirePerson, mask(|x, y| in_hair(x, y) || in_face(x, y)))];
    Face { masks, ..Face::boxed(BOUNDS) }
}

/// What a test lends the search: the matte is the picture's difference from the sky, the fill is the sky.
struct Fake {
    matte: bool,
    fill: bool,
    stop: bool,
    fills: usize,
}

impl Fake {
    fn new() -> Fake {
        Fake { matte: true, fill: true, stop: false, fills: 0 }
    }
}

impl Remover for Fake {
    fn matte(&mut self, crop: &Rgb32f) -> Option<Plane> {
        self.matte.then(|| {
            Plane::from_fn(crop.width, crop.height, |x, y| {
                let p = crop.get(x, y);
                ((0..3).map(|k| (p[k] - SKY[k]).abs()).fold(0.0, f32::max) / 0.25).clamp(0.0, 1.0)
            })
        })
    }

    fn fill(&mut self, rgb: &Rgb32f, hole: &Plane) -> Fill {
        self.fills += 1;
        if !self.fill {
            return Fill::Unavailable;
        }
        Fill::Done(Rgb32f::from_fn(rgb.width, rgb.height, |x, y| if hole.get(x, y) > 0.0 { SKY } else { rgb.get(x, y) }))
    }

    fn cancelled(&self) -> bool {
        self.stop
    }
}

fn searched(src: &Rgb32f, tools: &mut Fake) -> Vec<Face> {
    let mut cut = |r: [usize; 4]| Some(Rgb32f::from_fn(r[2] - r[0], r[3] - r[1], |x, y| src.get(r[0] + x, r[1] + y)));
    with_removals((src.width, src.height), vec![face()], &mut cut, tools)
}

fn settings(amount: f64) -> Retouch {
    let mut r = Retouch::default();
    assert!(r.global.set("retouch.hair.amount", amount));
    r
}

fn render_in(src: &Rgb32f, amount: f64, input: &Input, view: &dyn View) -> Rgb32f {
    let mut img = src.clone();
    for stage in [Stage::Retouch, Stage::Geometry, Stage::Background] {
        lightcraft_retouch::run::apply(&mut img, &settings(amount), input, view, stage);
    }
    img
}

fn render(src: &Rgb32f, amount: f64, input: &Input) -> Rgb32f {
    render_in(src, amount, input, &WholePhoto { width: src.width, height: src.height })
}

fn diff(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max)
}

/// How much of the strand is left: its pixels' distance from the picture without it, after over before.
fn left_of(after: &Rgb32f, before: &Rgb32f, clean: &Rgb32f) -> f32 {
    let (mut now, mut was) = (0.0f32, 0.0f32);
    for y in 0..before.height {
        for x in 0..before.width {
            let (nx, ny) = ((x as f64 + 0.5) / before.width as f64, (y as f64 + 0.5) / before.height as f64);
            if to_segment(nx, ny, FLYAWAY) < HALF * 2.0 + 1.5 / before.width as f64 && !in_hair(nx, ny) {
                let c = clean.get(x, y);
                now += diff(after.get(x, y), c);
                was += diff(before.get(x, y), c);
            }
        }
    }
    assert!(was > 0.0, "the strand is in the picture");
    now / was
}

#[test]
fn the_flyaway_goes_and_the_amount_scales_it() {
    let (src, clean) = (picture(800, 800, true), picture(800, 800, false));
    let mut tools = Fake::new();
    let input = Input::new(searched(&src, &mut tools));
    let strands = input.faces[0].strands.as_ref().expect("searched");
    let removal = strands.removal.as_ref().expect("the face has its change");
    assert!(!removal.is_empty() && removal.count >= 1, "the flyaway is taken: {}", removal.count);
    assert!(strands.count >= 1, "the readout counts it");
    assert_eq!(tools.fills, 1, "one fill for the face");
    let full = left_of(&render(&src, 100.0, &input), &src, &clean);
    let half = left_of(&render(&src, 50.0, &input), &src, &clean);
    assert!(full < 0.25, "{full:.2} of the flyaway is left at 100");
    assert!((0.35..0.65).contains(&half), "half the amount leaves about half: {half:.2}");
    // amount 0 is the original, bit for bit
    assert_eq!(render(&src, 0.0, &input), src);
}

#[test]
fn only_the_strand_changes_never_the_head() {
    let (src, _) = (picture(800, 800, true), ());
    let mut tools = Fake::new();
    let input = Input::new(searched(&src, &mut tools));
    let after = render(&src, 100.0, &input);
    let mut changed = 0;
    for y in 0..800 {
        for x in 0..800 {
            if after.get(x, y) == src.get(x, y) {
                continue;
            }
            changed += 1;
            let (nx, ny) = ((x as f64 + 0.5) / 800.0, (y as f64 + 0.5) / 800.0);
            assert!(to_segment(nx, ny, FLYAWAY) < 0.01, "pixel ({x}, {y}) changed away from the strand");
            assert!(!in_face(nx, ny), "the face changed at ({x}, {y})");
        }
    }
    assert!(changed > 50, "the strand's pixels changed ({changed})");
    // the hair mass (away from its edge) is exactly as it was
    for (x, y) in [(400, 250), (330, 300), (470, 280)] {
        assert_eq!(after.get(x, y), src.get(x, y));
    }
}

#[test]
fn a_draft_and_an_export_remove_the_same_strand() {
    let big = picture(800, 800, true);
    let mut tools = Fake::new();
    let input = Input::new(searched(&big, &mut tools));
    // (a render finer than the search's picture, which is the original, takes out less: a strand is thinner in its pixels)
    for side in [200, 400, 800] {
        let (src, clean) = (picture(side, side, true), picture(side, side, false));
        let left = left_of(&render(&src, 100.0, &input), &src, &clean);
        assert!(left < 0.5, "at {side} px {left:.2} of the flyaway is left");
    }
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

#[test]
fn a_zoomed_window_gives_the_whole_frames_result() {
    let src = picture(800, 800, true);
    let mut tools = Fake::new();
    let input = Input::new(searched(&src, &mut tools));
    let full = render(&src, 100.0, &input);
    // a window across the flyaway
    let (x0, y0, w, h) = (300, 20, 200, 160);
    let win = src.crop(x0, y0, w, h);
    let got = render_in(&win, 100.0, &input, &Window { ox: x0, oy: y0, w: 800, h: 800 });
    let mut changed = 0;
    for y in 0..h {
        for x in 0..w {
            assert_eq!(got.get(x, y), full.get(x0 + x, y0 + y), "pixel ({x}, {y}) of the window");
            changed += usize::from(full.get(x0 + x, y0 + y) != src.get(x0 + x, y0 + y));
        }
    }
    assert!(changed > 30, "the strand is in the window ({changed})");
}

#[test]
fn a_face_without_a_matte_a_fill_or_with_a_stop_keeps_no_change_and_v1_looks_for_its_strands() {
    let src = picture(800, 800, true);
    for (name, mut tools) in [
        ("no matte", Fake { matte: false, ..Fake::new() }),
        ("no fill", Fake { fill: false, ..Fake::new() }),
        ("stopped", Fake { stop: true, ..Fake::new() }),
    ] {
        let faces = searched(&src, &mut tools);
        assert!(faces[0].strands.is_none(), "{name}: nothing kept");
        // v1 searches the whole face as before and finds the flyaway
        let v1 = with_strands(&src, faces);
        let s = v1[0].strands.as_ref().expect("searched");
        assert!(s.removal.is_none() && !s.is_empty(), "{name}: v1's strands");
    }
}

#[test]
fn a_face_with_a_change_is_left_to_v1_over_its_skin_only() {
    // a matte that sees nobody thin: the face has an empty change, so v1 does not look outside the hair
    let src = picture(800, 800, true);
    let clean = picture(800, 800, false);
    let mut tools = Fake { matte: true, ..Fake::new() };
    let faces = {
        // (the strand is in the matte's picture but the search's picture is the clean one)
        let mut cut = |r: [usize; 4]| Some(Rgb32f::from_fn(r[2] - r[0], r[3] - r[1], |x, y| clean.get(r[0] + x, r[1] + y)));
        with_removals((800, 800), vec![face()], &mut cut, &mut tools)
    };
    assert!(faces[0].strands.as_ref().and_then(|s| s.removal.as_ref()).is_some_and(|r| r.is_empty()));
    let v1 = with_strands(&src, faces);
    let s = v1[0].strands.as_ref().expect("searched");
    assert!(s.removal.is_some());
    assert!(s.is_empty(), "v1 found nothing outside the hair: the flyaway is not its to take");
    assert_eq!(render(&src, 100.0, &Input::new(v1)), src);
}

#[test]
fn odd_faces_and_odd_pictures_do_not_panic() {
    let mut tools = Fake::new();
    let tiny = Face { masks: face().masks, ..Face::boxed(NormRect::new(0.4, 0.4, 0.42, 0.42)) };
    let bare = Face::boxed(BOUNDS);
    let src = picture(800, 800, true);
    let mut cut = |r: [usize; 4]| Some(Rgb32f::from_fn(r[2] - r[0], r[3] - r[1], |x, y| src.get(r[0] + x, r[1] + y)));
    let faces = with_removals((800, 800), vec![tiny, bare], &mut cut, &mut tools);
    assert!(faces.iter().all(|f| f.strands.is_none()), "too small or without parts: not searched");
    // a crop that can't be cut, a crop of the wrong size, a photo of no size, a matte of no size
    assert!(with_removals((800, 800), vec![face()], &mut |_| None, &mut tools)[0].strands.is_none());
    assert!(with_removals((800, 800), vec![face()], &mut |_| Some(Rgb32f::new(3, 3)), &mut tools)[0].strands.is_none());
    assert!(with_removals((0, 0), vec![face()], &mut |_| None, &mut tools)[0].strands.is_none());
    struct Empty;
    impl Remover for Empty {
        fn matte(&mut self, _: &Rgb32f) -> Option<Plane> {
            Some(Plane::new(0, 0))
        }
        fn fill(&mut self, _: &Rgb32f, _: &Plane) -> Fill {
            Fill::Unavailable
        }
    }
    assert!(with_removals((800, 800), vec![face()], &mut cut, &mut Empty)[0].strands.is_none());
}
