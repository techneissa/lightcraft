//! Tests of the background feature (`lightcraft_retouch::background`): Blur, Swap and Clean Backdrop.

mod common;

use std::sync::Arc;

use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::background::{Analysis, Backdrop, Background, Depth, Place, Swap};
use lightcraft_retouch::{Face, Features, Input, NormRect, Part, PartMask, Retouch, Stage, View};

#[test]
fn off_by_default_and_amount_zero_changes_nothing() {
    common::off_changes_nothing::<Background>();
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<Background>();
}

/// The photo seen through a window of `place` (the whole photo is `photo` pixels), with the pipeline's analysis
/// image when there is one.
struct Win {
    photo: (usize, usize),
    place: Place,
    analysis: Option<Arc<Analysis>>,
}

impl Win {
    fn whole(w: usize, h: usize) -> Win {
        Win { photo: (w, h), place: Place { x: 0, y: 0, width: w, height: h }, analysis: None }
    }

    fn with(mut self, a: &Arc<Analysis>) -> Win {
        self.analysis = Some(a.clone());
        self
    }
}

/// `img` reduced by 2 (box mean): an analysis image of the whole frame smaller than the render, as the pipeline's.
fn half(img: &Rgb32f) -> Rgb32f {
    Rgb32f::from_fn(img.width / 2, img.height / 2, |x, y| {
        let p = [img.get(2 * x, 2 * y), img.get(2 * x + 1, 2 * y), img.get(2 * x, 2 * y + 1), img.get(2 * x + 1, 2 * y + 1)];
        std::array::from_fn(|c| p.iter().map(|v| v[c]).sum::<f32>() / 4.0)
    })
}

impl View for Win {
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64) {
        ((x + self.place.x as f64) / self.photo.0 as f64, (y + self.place.y as f64) / self.photo.1 as f64)
    }

    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        Some((nx * self.photo.0 as f64 - self.place.x as f64, ny * self.photo.1 as f64 - self.place.y as f64))
    }

    fn place(&self) -> Option<Place> {
        Some(self.place)
    }

    fn analysis(&self) -> Option<&Analysis> {
        self.analysis.as_deref()
    }
}

/// The person's matte: a soft-edged ellipse centred at (`cx`, `cy`) with radii `rx`, `ry` (normalised), its edge
/// `soft` wide, on a 256² plane over the whole photo.
fn matte(cx: f64, cy: f64, rx: f64, ry: f64, soft: f64) -> Plane {
    matte_n(256, cx, cy, rx, ry, soft)
}

/// [`matte`] on an `n²` plane.
fn matte_n(n: usize, cx: f64, cy: f64, rx: f64, ry: f64, soft: f64) -> Plane {
    Plane::from_fn(n, n, |x, y| {
        let (nx, ny) = ((x as f64 + 0.5) / n as f64, (y as f64 + 0.5) / n as f64);
        let d = (((nx - cx) / rx).powi(2) + ((ny - cy) / ry).powi(2)).sqrt();
        (((1.0 - d) * rx.min(ry) / soft) + 0.5).clamp(0.0, 1.0) as f32
    })
}

/// A face (its box over the top of the ellipse) whose person is `plane`.
fn person(plane: Plane, cx: f64, cy: f64, rx: f64, ry: f64) -> Face {
    let mask = PartMask { plane, area: NormRect::new(0.0, 0.0, 1.0, 1.0) };
    Face { masks: vec![(Part::EntirePerson, mask)], ..Face::boxed(NormRect::new(cx - rx * 0.5, cy - ry, cx + rx * 0.5, cy - ry * 0.3)) }
}

/// One person, an ellipse in the middle of the photo.
fn one() -> Input {
    Input::new(vec![person(matte(0.5, 0.55, 0.18, 0.35, 0.01), 0.5, 0.55, 0.18, 0.35)])
}

/// Its matte at pixel (`x`, `y`) of a `w × h` photo.
fn alpha_at(input: &Input, x: usize, y: usize, w: usize, h: usize) -> f32 {
    input.faces[0].mask(Part::EntirePerson).map_or(0.0, |m| m.sample((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64))
}

/// A background with detail (a fine checker and a gradient) and a person in a warm colour.
fn photo(w: usize, h: usize, input: &Input) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let a = alpha_at(input, x, y, w, h);
        // (in fractions of the photo: a draft and an export show the same picture)
        let checker = if (x * 80 / w + y * 80 / w).is_multiple_of(2) { 0.08 } else { 0.0 };
        let bg = [0.10 + checker + 0.2 * x as f32 / w as f32, 0.15 + checker, 0.30 + checker];
        let skin = [0.6, 0.35, 0.25];
        std::array::from_fn(|c| a * skin[c] + (1.0 - a) * bg[c])
    })
}

fn settings(blur: f64, swap: f64, clean: f64, with: Swap) -> Retouch {
    Retouch { global: Features { background: Background { blur, swap, clean, with }, ..Features::default() }, faces: Vec::new() }
}

fn run(img: &mut Rgb32f, r: &Retouch, input: &Input, view: &dyn View) {
    for stage in [Stage::Retouch, Stage::Geometry, Stage::Background] {
        lightcraft_retouch::run::apply(img, r, input, view, stage);
    }
}

fn rendered(r: &Retouch, input: &Input, w: usize, h: usize) -> (Rgb32f, Rgb32f) {
    let before = photo(w, h, input);
    let mut after = before.clone();
    run(&mut after, r, input, &Win::whole(w, h));
    (before, after)
}

/// The mean squared difference of each pixel from its right and lower neighbours over `pick`ed pixels: how much
/// fine detail there is.
fn detail(img: &Rgb32f, pick: impl Fn(usize, usize) -> bool) -> f64 {
    let (w, h) = (img.width, img.height);
    let (mut sum, mut n) = (0.0f64, 0usize);
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            if pick(x, y) {
                let (p, r, d) = (img.get(x, y), img.get(x + 1, y), img.get(x, y + 1));
                sum += (0..3).map(|c| f64::from((p[c] - r[c]).powi(2) + (p[c] - d[c]).powi(2))).sum::<f64>();
                n += 1;
            }
        }
    }
    sum / n.max(1) as f64
}

#[test]
fn the_person_never_changes_and_a_face_without_the_person_mask_is_left_alone() {
    let input = one();
    let (w, h) = (200, 150);
    for r in [settings(100.0, 0.0, 0.0, Swap::default()), settings(0.0, 100.0, 0.0, Swap::default()), settings(70.0, 60.0, 100.0, Swap::default())] {
        let (before, after) = rendered(&r, &input, w, h);
        let mut inside = 0;
        for y in 0..h {
            for x in 0..w {
                if alpha_at(&input, x, y, w, h) >= 1.0 {
                    inside += 1;
                    assert_eq!(before.get(x, y).map(f32::to_bits), after.get(x, y).map(f32::to_bits), "({x}, {y})");
                }
            }
        }
        assert!(inside > 1000);
        assert_ne!(before.data, after.data, "the background changed");
    }
    // a face found by YuNet only (a box, no masks)
    let boxed = Input::new(vec![Face::boxed(NormRect::new(0.4, 0.2, 0.6, 0.5))]);
    let before = photo(120, 90, &boxed);
    let mut after = before.clone();
    run(&mut after, &settings(100.0, 100.0, 100.0, Swap::default()), &boxed, &Win::whole(120, 90));
    assert_eq!(before.data, after.data);
}

#[test]
fn blur_blurs_the_background_more_far_from_the_person() {
    let input = one();
    let (w, h) = (300, 200);
    let (before, after) = rendered(&settings(100.0, 0.0, 0.0, Swap::default()), &input, w, h);
    let near = |x: usize, y: usize| {
        let a = alpha_at(&input, x, y, w, h);
        a == 0.0 && (alpha_at(&input, x + 6, y, w, h) > 0.5 || alpha_at(&input, x.saturating_sub(6), y, w, h) > 0.5)
    };
    let far = |x: usize, _: usize| !(30..270).contains(&x);
    let (dn, df) = (detail(&after, near) / detail(&before, near), detail(&after, far) / detail(&before, far));
    assert!(df < 0.05, "the far background loses its fine checker: {df}");
    assert!(dn > df * 2.0, "next to the person it is blurred less: near {dn}, far {df}");
}

/// A white person on a dark background: with Blur at 100, the background next to them does not light up.
#[test]
fn no_halo_round_the_person() {
    let input = Input::new(vec![person(matte(0.5, 0.5, 0.2, 0.3, 0.005), 0.5, 0.5, 0.2, 0.3)]);
    let (w, h) = (300, 200);
    let before = Rgb32f::from_fn(w, h, |x, y| {
        let a = alpha_at(&input, x, y, w, h);
        let bg = if (x / 4 + y / 4).is_multiple_of(2) { 0.05 } else { 0.02 };
        [a + (1.0 - a) * bg; 3]
    });
    for r in [settings(100.0, 0.0, 0.0, Swap::default()), settings(0.0, 0.0, 100.0, Swap::default()), settings(100.0, 0.0, 100.0, Swap::default())] {
        let mut after = before.clone();
        run(&mut after, &r, &input, &Win::whole(w, h));
        for y in 0..h {
            for x in 0..w {
                if alpha_at(&input, x, y, w, h) <= 0.0 {
                    let v = after.get(x, y)[0];
                    assert!(v <= 0.06, "a halo at ({x}, {y}): {v}");
                }
            }
        }
    }
}

/// A strand of brown hair half over a blue sky, swapped for red: the strand keeps no blue.
#[test]
fn swap_puts_the_colour_behind_the_person_and_their_edge_keeps_no_old_background() {
    let input = Input::new(vec![person(matte(0.5, 0.6, 0.25, 0.4, 0.01), 0.5, 0.6, 0.25, 0.4)]);
    let (w, h) = (300, 200);
    let hair = [0.30f32, 0.18, 0.08];
    let sky = [0.10f32, 0.30, 0.80];
    let before = Rgb32f::from_fn(w, h, |x, y| {
        let a = alpha_at(&input, x, y, w, h);
        std::array::from_fn(|c| a * hair[c] + (1.0 - a) * sky[c])
    });
    let red = Swap::from_hex("#FF0000").unwrap_or_default();
    let mut after = before.clone();
    run(&mut after, &settings(0.0, 100.0, 0.0, red), &input, &Win::whole(w, h));
    let (mut edges, mut checked, mut left, mut naive) = (0, 0, 0.0f32, 0.0f32);
    for y in 0..h {
        for x in 0..w {
            let a = alpha_at(&input, x, y, w, h);
            let p = after.get(x, y);
            if a == 0.0 {
                // (sRGB red in linear Rec.2020, the render's working space)
                let want = [0.6274f32, 0.0691, 0.0164];
                assert!((0..3).all(|c| (p[c] - want[c]).abs() < 1e-3), "the background is red: {p:?}");
                checked += 1;
            } else if (0.3..0.7).contains(&a) {
                // drawn as one colour, the strand would keep `a · (pixel − hair)` of the sky's blue
                let red_b = 0.0164;
                let pixel_b = a * hair[2] + (1.0 - a) * sky[2];
                left += p[2] - (a * hair[2] + (1.0 - a) * red_b);
                naive += a * (pixel_b - hair[2]);
                assert!(p[2] < a * pixel_b, "less blue at every strand ({a}): {p:?}");
                edges += 1;
            }
        }
    }
    assert!(edges > 50 && checked > 1000, "{edges} edge pixels, {checked} background");
    assert!(left < 0.3 * naive, "at most 30 % of the sky's blue stays in the strands: {left} of {naive}");
}

/// A Swap picture covers the frame (centred, its shape cut), and is blurred with the background when Blur is on.
#[test]
fn swap_with_a_picture_covers_the_frame() {
    let input = one();
    let (w, h) = (240, 160);
    // a square picture: green on the left, magenta on the right
    let pic = Rgb32f::from_fn(100, 100, |x, _| if x < 50 { [0.0, 0.8, 0.0] } else { [0.8, 0.0, 0.8] });
    let view = Win::whole(w, h);
    let loaded = input.with_backdrop(Some(Backdrop { image: Arc::new(pic), key: 1 }));
    let before = photo(w, h, &input);
    let mut after = before.clone();
    run(&mut after, &settings(0.0, 100.0, 0.0, Swap::Picture { hash: "pic".into() }), &loaded, &view);
    let (l, r) = (after.get(5, 80), after.get(w - 5, 80));
    assert!(l[1] > 0.7 && l[0] < 0.05, "{l:?}");
    assert!(r[0] > 0.7 && r[1] < 0.05, "{r:?}");
    // not loaded yet: Swap waits for it and changes nothing
    let mut waiting = before.clone();
    run(&mut waiting, &settings(0.0, 100.0, 0.0, Swap::Picture { hash: "pic".into() }), &input, &Win::whole(w, h));
    assert_eq!(waiting.data, before.data);
    // blurred: the two halves meet in a soft ramp
    let mut soft = before.clone();
    run(&mut soft, &settings(100.0, 100.0, 0.0, Swap::Picture { hash: "pic".into() }), &loaded, &view);
    let mid = soft.get(w / 2, 4);
    assert!(mid[0] > 0.1 && mid[1] > 0.1, "a blurred seam: {mid:?}");
}

/// A creased backdrop with grain and a light stand: Clean Backdrop takes away the creases, keeps the stand, and
/// leaves a fine grain (its dither) of about the paper's own strength.
#[test]
fn clean_backdrop_removes_creases_and_keeps_grain_and_objects() {
    let input = one();
    let (w, h) = (400, 300);
    let grain = |x: usize, y: usize| ((x * 7919 + y * 104_729) % 13) as f32 / 13.0 * 0.02 - 0.01;
    let crease = |x: usize, y: usize| {
        let d = (x as f32 - 0.6 * y as f32 - 20.0).abs();
        if d < 4.0 { -0.08 * (1.0 - d / 4.0) } else { 0.0 }
    };
    let stand = |x: usize, y: usize| (20..26).contains(&x) && y > 40;
    let before = Rgb32f::from_fn(w, h, |x, y| {
        let a = alpha_at(&input, x, y, w, h);
        let paper = if stand(x, y) { 0.05 } else { 0.7 + crease(x, y) + grain(x, y) };
        [a * 0.5 + (1.0 - a) * paper; 3]
    });
    let mut after = before.clone();
    run(&mut after, &settings(0.0, 0.0, 100.0, Swap::default()), &input, &Win::whole(w, h));
    let on_crease = |x: usize, y: usize| crease(x, y) < -0.05 && alpha_at(&input, x, y, w, h) == 0.0 && !(15..32).contains(&x);
    let (mut n, mut was, mut now) = (0, 0.0f32, 0.0f32);
    for y in 0..h {
        for x in 0..w {
            if on_crease(x, y) {
                n += 1;
                was += 0.7 - before.get(x, y)[0];
                now += 0.7 - after.get(x, y)[0];
            }
        }
    }
    assert!(n > 100 && now < was * 0.3, "the crease is mostly gone: {was} → {now} over {n}");
    let plain = |x: usize, y: usize| {
        crease(x, y) == 0.0
            && alpha_at(&input, x + 20, y, w, h) == 0.0
            && x > 40
            && x + 40 < w
            && alpha_at(&input, x.saturating_sub(20), y, w, h) == 0.0
    };
    let kept = detail(&after, plain) / detail(&before, plain);
    assert!((0.3..1.5).contains(&kept), "a grain of about the paper's stays: {kept}");
    assert!(after.get(22, 200)[0] < 0.1, "the stand stays: {:?}", after.get(22, 200));
}

#[test]
fn a_flat_grey_photo_stays_flat() {
    let input = one();
    let before = Rgb32f::filled(160, 120, [0.18; 3]);
    let mut after = before.clone();
    run(&mut after, &settings(100.0, 0.0, 100.0, Swap::default()), &input, &Win::whole(160, 120));
    assert!(after.data.iter().flatten().all(|v| (v - 0.18).abs() < 1e-4));
}

/// A draft and an export four times its size make the same change once the export is scaled down.
#[test]
fn a_draft_and_an_export_make_the_same_change() {
    let input = one();
    let r = settings(60.0, 30.0, 50.0, Swap::default());
    let (b1, a1) = rendered(&r, &input, 160, 120);
    let (b4, a4) = rendered(&r, &input, 640, 480);
    let (mut diff, mut change) = (0.0f64, 0.0f64);
    for y in 0..120 {
        for x in 0..160 {
            let mut d4 = [0.0f64; 3];
            for yy in 0..4 {
                for xx in 0..4 {
                    let (a, b) = (a4.get(x * 4 + xx, y * 4 + yy), b4.get(x * 4 + xx, y * 4 + yy));
                    for c in 0..3 {
                        d4[c] += f64::from(a[c] - b[c]) / 16.0;
                    }
                }
            }
            let (a, b) = (a1.get(x, y), b1.get(x, y));
            for c in 0..3 {
                let d1 = f64::from(a[c] - b[c]);
                diff += (d1 - d4[c]).abs();
                change += d1.abs();
            }
        }
    }
    assert!(change > 0.0 && diff / change < 0.15, "{diff} against a change of {change}");
}

/// A window of the render (the zoomed loupe) draws what the whole render draws inside it, reading nothing around
/// itself: both read the analysis image of the whole frame (here half the render's size), and the window reuses the
/// work the whole render made.
#[test]
fn a_window_draws_what_the_whole_render_draws() {
    let input = one();
    let (w, h) = (360, 240);
    let pic = Rgb32f::from_fn(64, 48, |x, y| [x as f32 / 64.0, y as f32 / 48.0, 0.3]);
    let backdrop = Backdrop { image: Arc::new(pic), key: 9 };
    let whole = photo(w, h, &input);
    let analysis = Arc::new(Analysis::new(Arc::new(half(&whole)), 77));
    let input = input.with_backdrop(Some(backdrop));
    for r in [settings(80.0, 40.0, 70.0, Swap::Picture { hash: "p".into() }), settings(100.0, 0.0, 100.0, Swap::default())] {
        let mut full = whole.clone();
        run(&mut full, &r, &input, &Win::whole(w, h).with(&analysis));
        assert_ne!(full.data, whole.data);
        for (wx, wy, ww, wh) in [(120usize, 70usize, 60, 50), (0, 0, 50, 40), (250, 150, 110, 90), (175, 33, 9, 7)] {
            let mut part = whole.crop(wx, wy, ww, wh);
            let view = Win { photo: (w, h), place: Place { x: wx as i64, y: wy as i64, width: w, height: h }, analysis: Some(analysis.clone()) };
            run(&mut part, &r, &input, &view);
            for y in wy..wy + wh {
                for x in wx..wx + ww {
                    let (a, b) = (part.get(x - wx, y - wy), full.get(x, y));
                    assert!((0..3).all(|c| (a[c] - b[c]).abs() < 1e-4), "({x}, {y}) in the window at {wx},{wy}: {a:?} ≠ {b:?}");
                }
            }
        }
    }
    assert_eq!(analysis.kept(), 1, "every window and slider reused the whole render's work");
}

/// With a depth map, the background at the person's depth stays sharper than the far background.
#[test]
fn a_depth_map_sets_the_blur() {
    let input = one();
    let (w, h) = (300, 200);
    // near (the person's depth) on the left third, far on the right third; the photo's shape
    let depth = Plane::from_fn(96, 64, |x, _| {
        if x < 32 {
            1.0
        } else if x > 64 {
            0.0
        } else {
            1.0 - (x - 32) as f32 / 32.0
        }
    });
    let mut person_depth = depth.clone();
    for (i, v) in person_depth.data.iter_mut().enumerate() {
        let (nx, ny) = (((i % 96) as f64 + 0.5) / 96.0, ((i / 96) as f64 + 0.5) / 64.0);
        if input.faces[0].mask(Part::EntirePerson).map_or(0.0, |m| m.sample(nx, ny)) > 0.5 {
            *v = 1.0;
        }
    }
    let deep = input.with_depth(Some(Depth { plane: Arc::new(person_depth), key: 3 }));
    let before = photo(w, h, &input);
    let mut after = before.clone();
    run(&mut after, &settings(100.0, 0.0, 0.0, Swap::default()), &deep, &Win::whole(w, h));
    let left = |x: usize, _: usize| x < 60;
    let right = |x: usize, _: usize| x > 240;
    let (dl, dr) = (detail(&after, left) / detail(&before, left), detail(&after, right) / detail(&before, right));
    assert!(dl > dr * 3.0 && dr < 0.05, "near {dl}, far {dr}");
    // a map of another shape (a square one on this 3:2 photo) is another picture's: the distance stands in
    let square = input.with_depth(Some(Depth { plane: Arc::new(Plane::filled(64, 64, 1.0)), key: 4 }));
    let mut other = before.clone();
    run(&mut other, &settings(100.0, 0.0, 0.0, Swap::default()), &square, &Win::whole(w, h));
    let mut plain = before.clone();
    run(&mut plain, &settings(100.0, 0.0, 0.0, Swap::default()), &input, &Win::whole(w, h));
    assert_eq!(other.data, plain.data);
}

/// The work a slider does not change is kept with the analysis image: the next slider move finds it, and other
/// faces make a set of their own.
#[test]
fn the_work_is_kept_for_the_next_slider_move() {
    let input = Input::new(vec![person(matte(0.4, 0.5, 0.15, 0.3, 0.01), 0.4, 0.5, 0.15, 0.3)]);
    let (w, h) = (211, 137);
    let before = photo(w, h, &input);
    let analysis = Arc::new(Analysis::new(Arc::new(before.clone()), 5));
    let view = Win::whole(w, h).with(&analysis);
    assert_eq!(analysis.kept(), 0);
    let mut a = before.clone();
    run(&mut a, &settings(30.0, 0.0, 0.0, Swap::default()), &input, &view);
    let mut b = before.clone();
    run(&mut b, &settings(90.0, 0.0, 40.0, Swap::default()), &input, &view);
    assert_ne!(a.data, b.data);
    assert_eq!(analysis.kept(), 1);
    assert!(analysis.bytes() > before.data.len() * 12);
    let other = Input::new(vec![person(matte(0.6, 0.5, 0.15, 0.3, 0.01), 0.6, 0.5, 0.15, 0.3)]);
    run(&mut before.clone(), &settings(30.0, 0.0, 0.0, Swap::default()), &other, &view);
    assert_eq!(analysis.kept(), 2);
}

/// A person whose hair covers the top of the ellipse (above `ny` 0.4), with a gap of `gap` mask pixels at
/// (`gx`, `gy`) (normalised), on a 1024² mask.
fn with_gap(gx: f64, gy: f64, gap: usize) -> Input {
    const N: usize = 1024;
    let mut plane = matte_n(N, 0.5, 0.5, 0.3, 0.4, 0.01);
    let hair = Plane::from_fn(N, N, |x, y| if (y as f64 + 0.5) / (N as f64) < 0.4 { plane.get(x, y) } else { 0.0 });
    let (x0, y0) = ((gx * N as f64) as usize - gap / 2, (gy * N as f64) as usize - gap / 2);
    for y in y0..y0 + gap {
        for x in x0..x0 + gap {
            plane.data[y * N + x] = 0.0;
        }
    }
    let mut face = person(plane, 0.5, 0.5, 0.3, 0.4);
    face.masks.push((Part::Hair, PartMask { plane: hair, area: NormRect::new(0.0, 0.0, 1.0, 1.0) }));
    Input::new(vec![face])
}

/// A small gap in the hair (between curls) is the person: Swap leaves it as it is; the background outside is swapped.
#[test]
fn a_small_gap_in_the_hair_is_not_swapped() {
    let input = with_gap(0.5, 0.25, 16);
    let (w, h) = (256, 256);
    let before = Rgb32f::filled(w, h, [0.2, 0.3, 0.4]);
    let mut after = before.clone();
    run(&mut after, &settings(0.0, 100.0, 0.0, Swap::from_hex("#FF0000").unwrap_or_default()), &input, &Win::whole(w, h));
    assert_eq!(after.get(128, 64), before.get(128, 64), "the gap keeps its pixels");
    assert_ne!(after.get(5, 5), before.get(5, 5), "the background is swapped");
}

/// A closed pocket of background away from the hair (between an arm and the body, 30 × 30 mask pixels, small enough
/// to be a gap between curls) is background: Swap swaps it.
#[test]
fn a_pocket_between_arm_and_body_is_swapped() {
    let input = with_gap(0.5, 0.75, 30);
    let (w, h) = (256, 256);
    let before = Rgb32f::filled(w, h, [0.2, 0.3, 0.4]);
    let mut after = before.clone();
    run(&mut after, &settings(0.0, 100.0, 0.0, Swap::from_hex("#FF0000").unwrap_or_default()), &input, &Win::whole(w, h));
    let p = after.get(128, 192);
    assert!(p[0] > 0.6 && p[2] < 0.05, "the pocket is swapped: {p:?}");
    assert_eq!(after.get(128, 150), before.get(128, 150), "the body is not");
}
