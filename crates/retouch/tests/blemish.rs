//! Tests of the blemish feature (`lightcraft_retouch::blemish`), on a synthetic face: skin with pores and soft
//! shading, red pimples, brown moles and a cluster of faint freckles, inside a Facial Skin disc.

mod common;

use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::blemish::{Blemish, CORE, FEATHER, MAX_RADIUS, find, with_spots};
use lightcraft_retouch::input::{Spot, SpotKind, Spots};
use lightcraft_retouch::{Face, Features, Input, NormRect, Part, PartMask, Retouch, Scene, Stage, WholePhoto};

/// The face's box: most of the picture.
const FACE: NormRect = NormRect::new(0.05, 0.05, 0.95, 0.95);
/// Pimples (red), moles (dark brown, sharp), and the freckle cluster's centre, in fractions of the picture.
const PIMPLES: [(f64, f64); 4] = [(0.32, 0.35), (0.68, 0.33), (0.35, 0.66), (0.62, 0.7)];
const MOLES: [(f64, f64); 2] = [(0.5, 0.25), (0.25, 0.5)];
const FRECKLES: (f64, f64) = (0.7, 0.52);

fn hash(x: usize, y: usize) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h % 10_000) as f32 / 10_000.0
}

/// The freckles: a field of 25 small faint spots around [`FRECKLES`] (a few in a group are age spots, not a
/// freckled face).
fn freckles() -> Vec<(f64, f64)> {
    (0..25).map(|i| (FRECKLES.0 + ((i % 5) as f64 - 2.0) * 0.03, FRECKLES.1 + ((i / 5) as f64 - 2.0) * 0.03)).collect()
}

/// The face at `n` × `n` pixels. Pores are a fixed grain of the picture's pixels (fine texture at any size); spot
/// sizes are fractions of the face, so every size shows the same spots.
fn skin(n: usize) -> Rgb32f {
    let size = n as f64 * FACE.width();
    let d =
        |x: usize, y: usize, at: (f64, f64)| ((x as f64 + 0.5 - at.0 * n as f64).powi(2) + (y as f64 + 0.5 - at.1 * n as f64).powi(2)).sqrt() / size;
    Rgb32f::from_fn(n, n, |x, y| {
        let shade = 0.9 + 0.2 * (x as f32 / n as f32);
        let pore = 1.0 + 0.04 * (hash(x, y) - 0.5);
        let mut p = [0.42 * shade * pore, 0.28 * shade * pore, 0.2 * shade * pore];
        for at in PIMPLES {
            // a soft red blob, 2 % of the face across: green and blue absorbed
            let g = (-(d(x, y, at) / 0.007).powi(2)).exp() as f32;
            p = [p[0] * (1.0 - 0.06 * g), p[1] * (1.0 - 0.45 * g), p[2] * (1.0 - 0.35 * g)];
        }
        for at in MOLES {
            // a sharp dark brown disc, 1.4 % across
            let k = ((0.007 - d(x, y, at)) * size + 0.5).clamp(0.0, 1.0) as f32;
            p = [p[0] * (1.0 - 0.55 * k), p[1] * (1.0 - 0.65 * k), p[2] * (1.0 - 0.68 * k)];
        }
        for at in freckles() {
            // small faint brown dots, about 1 % across
            let g = (-(d(x, y, at) / 0.004).powi(2)).exp() as f32;
            p = [p[0] * (1.0 - 0.17 * g), p[1] * (1.0 - 0.26 * g), p[2] * (1.0 - 0.28 * g)];
        }
        p
    })
}

/// A Facial Skin disc filling 90 % of the face's box, its edge one sample soft.
fn disc() -> Plane {
    Plane::from_fn(128, 128, |x, y| {
        let r = ((x as f32 - 63.5).powi(2) + (y as f32 - 63.5).powi(2)).sqrt();
        (57.6 - r).clamp(0.0, 1.0)
    })
}

fn face() -> Face {
    Face { masks: vec![(Part::FacialSkin, PartMask { plane: disc(), area: FACE })], ..Face::boxed(FACE) }
}

fn settings(amount: f64, keep: bool) -> Retouch {
    let mut global = Features::default();
    assert!(global.set("retouch.blemish.amount", amount));
    assert!(global.set("retouch.blemish.keepMoles", if keep { 1.0 } else { 0.0 }));
    Retouch::with_global(global)
}

fn retouch(img: &Rgb32f, r: &Retouch, faces: Vec<Face>) -> Rgb32f {
    let mut out = img.clone();
    let view = WholePhoto { width: img.width, height: img.height };
    lightcraft_retouch::run::apply(&mut out, r, &Input::new(faces), &view, Stage::Retouch);
    out
}

/// How much darker the green channel is at `at` than the mean of a ring 4 % of the face out (natural log).
fn dip(img: &Rgb32f, at: (f64, f64)) -> f32 {
    let n = img.width as f64;
    let g = |x: f64, y: f64| img.data.get((y * n) as usize * img.width + (x * n) as usize).map_or(0.0, |p| p[1]);
    let r = 0.04 * FACE.width();
    let ring: f32 =
        (0..16).map(|k| f64::from(k) * std::f64::consts::TAU / 16.0).map(|a| g(at.0 + r * a.cos(), at.1 + r * a.sin())).sum::<f32>() / 16.0;
    (ring / g(at.0, at.1)).ln()
}

fn spots_of(img: &Rgb32f) -> Spots {
    let input = Input::new(vec![face()]);
    let view = WholePhoto { width: img.width, height: img.height };
    let scene = Scene::new(&input, &view, img.width, img.height);
    find(img, &scene, 0)
}

fn near(spots: &Spots, at: (f64, f64)) -> Option<SpotKind> {
    spots.list.iter().find(|s| ((s.at.0 - at.0).powi(2) + (s.at.1 - at.1).powi(2)).sqrt() < 0.012).map(|s| s.kind)
}

#[test]
fn off_by_default_and_amount_zero_changes_nothing() {
    common::off_changes_nothing::<Blemish>();
    // the switch alone (Amount 0) changes nothing either
    let img = skin(300);
    for keep in [true, false] {
        let out = retouch(&img, &settings(0.0, keep), vec![face()]);
        assert!(out.data.iter().zip(&img.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)));
    }
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<Blemish>();
}

#[test]
fn keep_moles_and_freckles_is_on_by_default_and_a_control() {
    let b = Blemish::default();
    assert!(b.keep_moles && b.amount == 0.0);
    let old: Blemish = serde_json::from_str(r#"{"amount": 40}"#).unwrap();
    assert!(old.keep_moles, "settings saved before the switch keep moles");
    let mut f = Features::default();
    assert_eq!(f.get("retouch.blemish.keepMoles"), Some(1.0));
    assert!(f.set("retouch.blemish.keepMoles", 0.0));
    assert!(!f.blemish.keep_moles);
}

/// Pimples, moles and freckles are found and told apart; nothing else on the skin is a spot.
#[test]
fn finds_pimples_moles_and_freckles() {
    let spots = spots_of(&skin(400));
    for at in PIMPLES {
        assert_eq!(near(&spots, at), Some(SpotKind::Blemish), "pimple at {at:?}: {:?}", spots.list);
    }
    for at in MOLES {
        assert_eq!(near(&spots, at), Some(SpotKind::Mole), "mole at {at:?}: {:?}", spots.list);
    }
    let freckled = freckles().iter().filter(|at| near(&spots, **at) == Some(SpotKind::Freckle)).count();
    assert!(freckled >= 21, "{freckled} of 25 freckles: {:?}", spots.list);
    assert!(spots.list.len() <= PIMPLES.len() + MOLES.len() + 25, "pores are not spots: {:?}", spots.list);
    assert!(spots.skin > 0.5 && spots.skin < 1.0, "skin area in face sizes²: {}", spots.skin);
}

#[test]
fn removes_pimples_and_keeps_moles_and_freckles_while_the_switch_is_on() {
    let img = skin(400);
    let out = retouch(&img, &settings(100.0, true), vec![face()]);
    for at in PIMPLES {
        let (before, after) = (dip(&img, at), dip(&out, at));
        assert!(before > 0.3 && after < before * 0.15, "pimple at {at:?}: {before} → {after}");
    }
    for at in MOLES.iter().chain(&freckles()) {
        assert!((dip(&out, *at) - dip(&img, *at)).abs() < 1e-6, "kept at {at:?}");
    }
    // with the switch off they go too
    let all = retouch(&img, &settings(100.0, false), vec![face()]);
    for at in MOLES {
        let (before, after) = (dip(&img, at), dip(&all, at));
        assert!(after < before * 0.3, "mole removed at {at:?}: {before} → {after}");
    }
    let gone = freckles().iter().filter(|at| dip(&all, **at) < dip(&img, **at) * 0.3).count();
    assert!(gone >= 21, "{gone} of 25 freckles removed");
}

/// Healing keeps the skin's texture: inside a pimple's core the pores' fine variation is about that of clean skin.
#[test]
fn the_repair_keeps_the_texture() {
    let img = skin(400);
    let out = retouch(&img, &settings(100.0, true), vec![face()]);
    // fine variation: each pixel against the mean of its 4 neighbours, over 6 × 6 pixels around `at`
    let fine = |img: &Rgb32f, at: (f64, f64)| {
        let (cx, cy) = ((at.0 * 400.0) as usize, (at.1 * 400.0) as usize);
        let g = |x: usize, y: usize| img.data[y * img.width + x][1];
        let mut sum = 0.0;
        for y in cy - 3..cy + 3 {
            for x in cx - 3..cx + 3 {
                let m = (g(x - 1, y) + g(x + 1, y) + g(x, y - 1) + g(x, y + 1)) / 4.0;
                sum += ((g(x, y) - m) / m).abs();
            }
        }
        sum / 36.0
    };
    let clean = fine(&img, (0.5, 0.5));
    for at in PIMPLES {
        let healed = fine(&out, at);
        assert!(healed > clean * 0.6 && healed < clean * 1.6, "pores in the healed pimple at {at:?}: {healed}, clean skin {clean}");
    }
}

#[test]
fn pixels_outside_the_mask_never_change() {
    let img = skin(300);
    let out = retouch(&img, &settings(100.0, false), vec![face()]);
    let mask = disc();
    let mut changed_inside = 0;
    for y in 0..300 {
        for x in 0..300 {
            let (nx, ny) = ((x as f64 + 0.5) / 300.0, (y as f64 + 0.5) / 300.0);
            let m = PartMask { plane: mask.clone(), area: FACE }.sample(nx, ny);
            let (a, b) = (img.data[y * 300 + x], out.data[y * 300 + x]);
            if m == 0.0 {
                assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits), "({x}, {y}) is outside the mask");
            } else if a != b {
                changed_inside += 1;
            }
        }
    }
    assert!(changed_inside > 100, "the spots were repaired");
    // a face without a Facial Skin mask is left alone
    let bare = retouch(&img, &settings(100.0, false), vec![Face::boxed(FACE)]);
    assert!(bare.data.iter().zip(&img.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)));
}

/// A spot next to the skin's soft edge, black hair just past it, its donor patch and its ring of skin samples reaching
/// over the edge: the hair's colour never comes into the repair. The same repair with skin where the
/// hair is gives the same pixels.
#[test]
fn no_halo_next_to_dark_hair() {
    let n = 400;
    let edge = 0.3;
    let hair_rows = (edge * n as f64) as usize;
    let skin_img = skin(n);
    let mut hair_img = skin_img.clone();
    for p in hair_img.data.iter_mut().take(hair_rows * n) {
        *p = [0.02, 0.015, 0.012];
    }
    // the mask starts at the hair's edge and rises to 1 over 2.5 % of the picture, as a parse's soft edge does
    let mask = Plane::from_fn(128, 128, |_, y| (((y as f32 + 0.5) / 128.0 * 0.9 + 0.05 - edge as f32) / 0.025).clamp(0.0, 1.0));
    // a spot 2.2 radii below the edge; its donor 3.4 radii away, 30° up, so the patch it lends reaches over the edge
    let size = FACE.width();
    let r = 0.012;
    let at = (0.5, edge + 2.2 * r * size);
    let (dx, dy) = (3.4 * r * size * 30f64.to_radians().cos(), -3.4 * r * size * 30f64.to_radians().sin());
    let spot = Spot { at, radius: r, donor: Some((at.0 + dx, at.1 + dy)), kind: SpotKind::Blemish, strength: 1.0 };
    let f = Face {
        masks: vec![(Part::FacialSkin, PartMask { plane: mask, area: FACE })],
        spots: Some(Spots { list: vec![spot], skin: 0.5 }),
        ..Face::boxed(FACE)
    };
    let r100 = settings(100.0, false);
    let (with_hair, with_skin) = (retouch(&hair_img, &r100, vec![f.clone()]), retouch(&skin_img, &r100, vec![f]));
    let mut changed = 0;
    let mut worst = 0.0f32;
    for y in hair_rows..n {
        for x in 0..n {
            let i = y * n + x;
            changed += usize::from(with_skin.data[i] != skin_img.data[i]);
            for c in 0..3 {
                worst = worst.max((with_hair.data[i][c] / with_skin.data[i][c]).ln().abs());
            }
        }
    }
    assert!(changed > 50, "the spot was repaired ({changed} pixels)");
    assert!(worst < 0.01, "the hair's colour came into the repair: a pixel {:.1} % off", worst * 100.0);
}

/// Spots from a provider are held to what the repair may read (`Feature::REACH`): a spot larger than any found is
/// repaired at the largest radius, and a donor farther than `MAX_DONOR` radii is not used.
#[test]
fn a_huge_spot_or_a_far_donor_stays_within_the_reach() {
    let img = skin(300);
    let size = FACE.width() * 300.0;
    let at = PIMPLES[0];
    let face = |radius: f64, donor: Option<(f64, f64)>| Face {
        spots: Some(Spots { list: vec![Spot { at, radius, donor, kind: SpotKind::Blemish, strength: 1.0 }], skin: 0.5 }),
        ..face()
    };
    let out = retouch(&img, &settings(100.0, false), vec![face(0.5, None)]);
    let far = (CORE.max(FEATHER) * MAX_RADIUS * size + 2.0).powi(2);
    let mut changed = 0;
    for y in 0..300 {
        for x in 0..300 {
            let d2 = (x as f64 + 0.5 - at.0 * 300.0).powi(2) + (y as f64 + 0.5 - at.1 * 300.0).powi(2);
            if out.data[y * 300 + x] != img.data[y * 300 + x] {
                changed += 1;
                assert!(d2 <= far, "({x}, {y}) changed, past the largest spot's repair");
            }
        }
    }
    assert!(changed > 20, "the spot is repaired at the largest radius ({changed})");
    // a donor far across the face is dropped: the same repair as without one
    let r = 0.01;
    let (a, b) =
        (retouch(&img, &settings(100.0, false), vec![face(r, Some((0.9, 0.9)))]), retouch(&img, &settings(100.0, false), vec![face(r, None)]));
    assert_eq!(a.data, b.data);
}

/// The app finds spots once on the parse's picture and repairs them at every size: a draft and an export four times
/// its size make the same change once the export is scaled down, and spots found at both sizes are the same.
#[test]
fn a_draft_and_an_export_repair_the_same_spots() {
    let (small, big) = (300, 1200);
    let (draft_img, export_img) = (skin(small), skin(big));
    let faces = with_spots(&draft_img, vec![face()]);
    let r = settings(100.0, false);
    let draft = retouch(&draft_img, &r, faces.clone());
    let export = retouch(&export_img, &r, faces.clone());
    for at in PIMPLES.iter().chain(&MOLES) {
        let (d, e) = (dip(&draft, *at), dip(&export, *at));
        let (d0, e0) = (dip(&draft_img, *at), dip(&export_img, *at));
        assert!(d < d0 * 0.3 && e < e0 * 0.3, "both repair {at:?}: {d0} → {d}, {e0} → {e}");
    }
    // the change, the export scaled down by 4, matches the draft's
    let k = big / small;
    let down = |img: &Rgb32f| {
        Rgb32f::from_fn(small, small, |x, y| {
            let mut acc = [0.0f32; 3];
            for yy in y * k..(y + 1) * k {
                for xx in x * k..(x + 1) * k {
                    let p = img.data[yy * big + xx];
                    acc = std::array::from_fn(|c| acc[c] + p[c]);
                }
            }
            acc.map(|v| v / (k * k) as f32)
        })
    };
    let (eo, ed) = (down(&export_img), down(&export));
    let (mut diff, mut total) = (0.0f64, 0.0f64);
    for i in 0..small * small {
        let dc = f64::from(draft.data[i][1] - draft_img.data[i][1]);
        let ec = f64::from(ed.data[i][1] - eo.data[i][1]);
        diff += (dc - ec).abs();
        total += dc.abs();
    }
    assert!(total > 0.0 && diff / total < 0.35, "the changes differ by {:.0} %", diff / total * 100.0);
    // spots found on the export are the draft's
    let (a, b) = (spots_of(&draft_img), spots_of(&export_img));
    // (the draft's face is 270 pixels: it sees only the darkest few of the faint freckles, too few to make a
    // freckled face, so those few read as marks there; spots are found once, on the people parse's picture)
    for s in &a.list {
        let on_export = near(&b, s.at);
        let same = on_export == Some(s.kind) || (s.kind == SpotKind::Blemish && on_export == Some(SpotKind::Freckle));
        assert!(same, "{s:?} found on the draft, not on the export ({on_export:?}): {:?}", b.list);
    }
}

/// The provider's spots are used as they are: a face whose provider found no spots is left alone even with spots in
/// the picture (no detection in the render), and Amount only weighs the cached spots.
#[test]
fn amount_blends_the_cached_spots_only() {
    let img = skin(300);
    let none = Face { spots: Some(Spots::default()), ..face() };
    let out = retouch(&img, &settings(100.0, false), vec![none]);
    assert!(out.data.iter().zip(&img.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)));
    let faces = with_spots(&img, vec![face()]);
    let mut last = f32::INFINITY;
    for amount in [10.0, 40.0, 70.0, 100.0] {
        let out = retouch(&img, &settings(amount, true), faces.clone());
        let left: f32 = PIMPLES.iter().map(|at| dip(&out, *at)).sum();
        assert!(left <= last + 1e-4, "Amount {amount} removes at least as much as less: {left} > {last}");
        last = left;
    }
}

#[test]
fn the_report_counts_spots_and_the_repaired_area() {
    let spots = spots_of(&skin(400));
    let keep = Blemish { amount: 100.0, keep_moles: true }.report(&spots);
    assert_eq!(keep.removed, PIMPLES.len());
    assert_eq!(keep.moles, MOLES.len());
    assert!(keep.freckles >= 21);
    assert!(keep.area > 0.0 && keep.area < 0.05, "{}", keep.area);
    let all = Blemish { amount: 100.0, keep_moles: false }.report(&spots);
    assert_eq!(all.removed, spots.list.len());
    assert_eq!((all.moles, all.freckles), (0, 0));
    assert!(all.area > keep.area);
    assert_eq!(Blemish::default().report(&spots).removed, 0);
}

#[test]
fn a_flat_grey_photo_stays_flat() {
    let img = Rgb32f::filled(300, 300, [0.18, 0.18, 0.18]);
    assert!(spots_of(&img).list.is_empty());
    let out = retouch(&img, &settings(100.0, false), vec![face()]);
    assert!(out.data.iter().all(|p| p.map(f32::to_bits) == [0.18f32.to_bits(); 3]));
}

/// A face cut by the photo's edge: its spots inside the photo are repaired, nothing panics.
#[test]
fn a_face_cut_by_the_edge() {
    let img = skin(400);
    let cut = Face {
        bounds: NormRect::new(0.4, 0.05, 1.3, 0.95),
        masks: vec![(Part::FacialSkin, PartMask { plane: disc(), area: NormRect::new(0.4, 0.05, 1.3, 0.95) })],
        ..Face::default()
    };
    let out = retouch(&img, &settings(100.0, false), vec![cut]);
    assert!(out.data.iter().flatten().all(|v| v.is_finite()));
    let at = PIMPLES[1];
    assert!(dip(&out, at) < dip(&img, at) * 0.3, "the pimple inside the photo is repaired");
}

/// Dark skin (#204): a group of brown marks (what acne leaves on dark skin, or an older face's age spots), one red
/// pimple, and a field of freckles away from them. On dark skin every spot is brown, so colour can't tell a mark
/// from a freckle; at 100 with Keep Moles and Freckles on, the marks and the pimple go and the freckle field stays.
#[test]
fn on_dark_skin_marks_go_and_a_freckle_field_stays() {
    let marks: Vec<(f64, f64)> = (0..8).map(|i| (0.265 + (i % 2) as f64 * 0.07, 0.395 + (i / 2) as f64 * 0.07)).collect();
    const PIMPLE: (f64, f64) = (0.6, 0.24);
    let field: Vec<(f64, f64)> = (0..36).map(|i| (0.665 + ((i % 6) as f64 - 2.5) * 0.03, 0.55 + ((i / 6) as f64 - 2.5) * 0.03)).collect();
    let n = 400;
    let size = n as f64 * FACE.width();
    let d =
        |x: usize, y: usize, at: (f64, f64)| ((x as f64 + 0.5 - at.0 * n as f64).powi(2) + (y as f64 + 0.5 - at.1 * n as f64).powi(2)).sqrt() / size;
    let img = Rgb32f::from_fn(n, n, |x, y| {
        let shade = 0.9 + 0.2 * (y as f32 / n as f32);
        let pore = 1.0 + 0.04 * (hash(x, y) - 0.5);
        let mut p = [0.1 * shade * pore, 0.06 * shade * pore, 0.04 * shade * pore];
        for at in &marks {
            // a soft brown mark about 1.4 % of the face across
            let g = (-(d(x, y, *at) / 0.005).powi(2)).exp() as f32;
            p = [p[0] * (1.0 - 0.2 * g), p[1] * (1.0 - 0.28 * g), p[2] * (1.0 - 0.3 * g)];
        }
        for at in &field {
            let g = (-(d(x, y, *at) / 0.004).powi(2)).exp() as f32;
            p = [p[0] * (1.0 - 0.2 * g), p[1] * (1.0 - 0.28 * g), p[2] * (1.0 - 0.3 * g)];
        }
        // the pimple: blood darkens green and blue and leaves red
        let g = (-(d(x, y, PIMPLE) / 0.007).powi(2)).exp() as f32;
        [p[0] * (1.0 - 0.04 * g), p[1] * (1.0 - 0.4 * g), p[2] * (1.0 - 0.3 * g)]
    });
    let spots = spots_of(&img);
    for at in marks.iter().chain(std::iter::once(&PIMPLE)) {
        assert_eq!(near(&spots, *at), Some(SpotKind::Blemish), "{at:?}: {:?}", spots.list);
    }
    let freckled = field.iter().filter(|at| near(&spots, **at) == Some(SpotKind::Freckle)).count();
    assert!(freckled >= 24, "{freckled} of 36 freckles: {:?}", spots.list);
    let out = retouch(&img, &settings(100.0, true), vec![face()]);
    for at in marks.iter().chain(std::iter::once(&PIMPLE)) {
        let (before, after) = (dip(&img, *at), dip(&out, *at));
        assert!(before > 0.15 && after < before * 0.3, "{at:?}: {before} → {after}");
    }
    let kept = field.iter().filter(|at| (dip(&out, **at) - dip(&img, **at)).abs() < 1e-6).count();
    assert!(kept >= 30, "{kept} of 36 freckles kept");
}
