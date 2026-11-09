//! Where a feature works ([`Feature::area`], the map the Portrait tab's ⌥-drag shows): by default the union of its
//! part masks, and what each feature with a different reach says.

use lightcraft_raster::Plane;
use lightcraft_retouch::input::{Spot, Spots};
use lightcraft_retouch::{FEATURES, Face, Input, NormRect, Part, PartMask, Scene, WholePhoto};

const W: usize = 100;
const H: usize = 100;

fn mask(area: NormRect) -> PartMask {
    PartMask { plane: Plane::filled(4, 4, 1.0), area }
}

fn area_of(key: &str, face: Face, faces: &[usize]) -> Option<Plane> {
    area_in(key, face, faces, W)
}

/// The area over a `side × side` picture.
fn area_in(key: &str, face: Face, faces: &[usize], side: usize) -> Option<Plane> {
    let info = FEATURES.iter().find(|f| f.key == key).expect("a feature");
    let input = Input::new(vec![face]);
    let view = WholePhoto { width: side, height: side };
    let scene = Scene::new(&input, &view, side, side);
    (info.area)(&scene, faces)
}

fn at(p: &Plane, x: usize, y: usize) -> f32 {
    p.data[y * p.width + x]
}

/// A feature with no override of its own gives the union of its parts' masks: white inside them (a face's skin, the
/// teeth and the lips), black outside, and `None` for a face without any of them.
#[test]
fn by_default_the_area_is_the_union_of_the_parts_masks() {
    let b = NormRect::new(0.2, 0.2, 0.8, 0.8);
    let face = |parts: Vec<(Part, PartMask)>| Face { masks: parts, ..Face::boxed(b) };
    let skin = area_of("skin", face(vec![(Part::FacialSkin, mask(b))]), &[0]).expect("a map");
    assert_eq!((at(&skin, 50, 50), at(&skin, 5, 5)), (1.0, 0.0));
    let teeth = mask(NormRect::new(0.4, 0.6, 0.5, 0.7));
    let lips = mask(NormRect::new(0.6, 0.6, 0.7, 0.7));
    let mouth = area_of("teethLips", face(vec![(Part::Teeth, teeth), (Part::Lips, lips), (Part::FacialSkin, mask(b))]), &[0]).expect("a map");
    assert_eq!((at(&mouth, 45, 65), at(&mouth, 65, 65), at(&mouth, 25, 25)), (1.0, 1.0, 0.0), "teeth and lips, not the skin");
    assert!(area_of("skin", face(vec![(Part::Teeth, mask(b))]), &[0]).is_none(), "no skin mask: nowhere");
    assert!(area_of("skin", face(vec![(Part::FacialSkin, mask(b))]), &[]).is_none(), "no face asked: nowhere");
}

#[test]
fn eyes_are_the_whites_and_the_iris() {
    let b = NormRect::new(0.2, 0.2, 0.8, 0.8);
    let face = Face {
        masks: vec![(Part::EyeWhites, mask(NormRect::new(0.3, 0.3, 0.4, 0.35))), (Part::Iris, mask(NormRect::new(0.6, 0.3, 0.7, 0.35)))],
        ..Face::boxed(b)
    };
    let p = area_of("eyes", face, &[0]).expect("a map");
    assert_eq!((at(&p, 35, 32), at(&p, 65, 32), at(&p, 50, 70)), (1.0, 1.0, 0.0));
}

/// Blemishes: a disc on each spot at the radius the repair works at (`CORE` spot radii, the spot's radius held to
/// `MAX_RADIUS`), none for a spot too small to see or outside the skin mask's box.
#[test]
fn blemishes_are_discs_where_the_repair_works() {
    use lightcraft_retouch::blemish::{CORE, MAX_RADIUS};
    const SIDE: usize = 1000;
    let b = NormRect::new(0.2, 0.2, 0.8, 0.8);
    let face = |spots: Vec<Spot>| Face { masks: vec![(Part::FacialSkin, mask(b))], spots: Some(Spots { list: spots, skin: 1.0 }), ..Face::boxed(b) };
    let size = 0.6 * SIDE as f64;
    let spot = |at: (f64, f64), radius: f64| Spot { at, radius, ..Spot::default() };
    // a spot of radius 0.05 is held to MAX_RADIUS: the disc is the repair's, not the spot's
    let p = area_in("blemish", face(vec![spot((0.5, 0.5), 0.05)]), &[0], SIDE).expect("a map");
    let core = MAX_RADIUS * size * CORE;
    let at_dist = |d: f64| at(&p, 500 + d as usize, 500);
    assert_eq!(at_dist(core - 3.0), 1.0, "inside the repair's core");
    assert_eq!(at_dist(core + 3.0), 0.0, "outside it (the unclamped radius would reach {})", 0.05 * size * CORE);
    // smaller than a pixel's third: the repair skips it, so no disc
    let tiny = area_in("blemish", face(vec![spot((0.5, 0.5), 0.0005)]), &[0], SIDE).expect("a map");
    assert!(tiny.data.iter().all(|v| *v == 0.0), "a spot the repair skips shows no disc");
    // the centre outside the skin mask's box: skipped too
    let outside = area_in("blemish", face(vec![spot((0.1, 0.1), 0.01)]), &[0], SIDE).expect("a map");
    assert!(outside.data.iter().all(|v| *v == 0.0));
    assert!(area_in("blemish", Face::boxed(b), &[0], SIDE).is_none(), "spots not looked for, or no skin mask: nowhere");
}

/// Stray Hair: the cells of the strand search that hold a strand, by weight; nowhere without a search.
#[test]
fn hair_is_the_strand_cells() {
    use lightcraft_retouch::input::Strands;
    let b = NormRect::new(0.2, 0.2, 0.8, 0.8);
    // 32 × 32 cells over the box, in 2 × 2 tiles of 16: only the first tile is stored, with one strand cell at (3, 3)
    let mut cells = vec![[0u8; 3]; Strands::TILE * Strands::TILE];
    cells[3 * Strands::TILE + 3] = [255, 0, 1];
    let strands = Strands { area: b, width: 32, height: 32, tiles: vec![0, u32::MAX, u32::MAX, u32::MAX], cells, ..Strands::default() };
    let face = Face { strands: Some(strands), ..Face::boxed(b) };
    let p = area_of("hair", face, &[0]).expect("a map");
    assert_eq!((at(&p, 26, 26), at(&p, 50, 50), at(&p, 5, 5)), (1.0, 0.0, 0.0), "the strand's cell only");
    assert!(area_of("hair", Face::boxed(b), &[0]).is_none(), "strands not searched: nowhere");
}

/// A face of box `b` (fractions of a `W` picture) with YuNet's five points and the 478 mesh points of a frontal
/// face (`wrinkles::areas::frontal`), Facial Skin over the box and Body Skin under it.
fn meshed(b: NormRect) -> Face {
    use lightcraft_retouch::wrinkles::areas;
    let at = |(fx, fy): (f64, f64)| (b.x0 + fx * b.width(), b.y0 + fy * b.height());
    let five = [(0.33, 0.45), (0.67, 0.45), (0.5, 0.62), (0.38, 0.78), (0.62, 0.78)];
    let landmarks = five.into_iter().map(at).chain((0..areas::MESH).map(|i| at(areas::frontal(i)))).collect();
    let skin = |area: NormRect| PartMask { plane: Plane::filled(8, 8, 1.0), area };
    Face {
        landmarks,
        masks: vec![
            (Part::FacialSkin, skin(NormRect::new(b.x0, b.y0 - 0.1 * b.height(), b.x1, b.y1))),
            (Part::BodySkin, skin(NormRect::new(b.x0, b.y1, b.x1, b.y1 + 0.8 * b.height()))),
        ],
        ..Face::boxed(b)
    }
}

/// Wrinkles: the face's areas only (not all of its skin), the neck cut by Body Skin; nowhere without the mesh.
#[test]
fn wrinkles_are_the_faces_areas_not_all_its_skin() {
    let b = NormRect::new(0.3, 0.1, 0.7, 0.5);
    let p = area_of("wrinkles", meshed(b), &[0]).expect("a map");
    let lit = p.data.iter().filter(|v| **v > 0.5).count();
    let skin = (0.4 * W as f64 * 0.5 * W as f64) as usize;
    assert!(lit > 100 && lit < skin * 3 / 4, "some of the skin, not all: {lit} of {skin}");
    assert_eq!((at(&p, 50, 12), at(&p, 50, 40)), (1.0, 0.0), "the forehead in; between the cheeks' lines out, on skin the mask covers");
    let no_mesh = Face { masks: meshed(b).masks, ..Face::boxed(b) };
    assert!(area_of("wrinkles", no_mesh, &[0]).is_none_or(|p| p.data.iter().all(|v| *v == 0.0)), "no mesh: no areas");
}

/// Eyes: the crescents under the lower lids as well as the whites and irises, and not the rest of the skin.
#[test]
fn eyes_include_the_crescents_under_the_lids_and_not_all_the_skin() {
    let b = NormRect::new(0.2, 0.2, 0.8, 0.8);
    // YuNet's five points (right eye, left eye, nose, mouth corners) in the box, and Facial Skin over all of it
    let pt = |fx: f64, fy: f64| (b.x0 + fx * b.width(), b.y0 + fy * b.height());
    let landmarks = vec![pt(0.33, 0.45), pt(0.67, 0.45), pt(0.5, 0.62), pt(0.38, 0.78), pt(0.62, 0.78)];
    let face = Face { landmarks, masks: vec![(Part::FacialSkin, mask(b))], ..Face::boxed(b) };
    let p = area_of("eyes", face, &[0]).expect("a map");
    let (ex, ey) = (b.x0 * W as f64 + 0.33 * b.width() * W as f64, (b.y0 + 0.45 * b.height()) * W as f64);
    let below = (ey + 4.0) as usize;
    assert!(at(&p, ex as usize, below) > 0.5, "under the lid");
    assert_eq!(at(&p, 50, 30), 0.0, "the forehead is not an eye area");
    assert_eq!(at(&p, 50, 75), 0.0, "nor the chin");
}

/// Face Reshape: the mesh's outline when there is a mesh (not the box's ellipse); without a mesh the ellipse.
#[test]
fn reshape_follows_the_meshs_outline() {
    use lightcraft_retouch::wrinkles::FACE_OVAL;
    let b = NormRect::new(0.2, 0.2, 0.8, 0.8);
    // a small circle (a quarter of the box's width across) of oval points, in the oval's order
    let (c, r) = ((0.5, 0.5), 0.1);
    let mut mesh = vec![c; 478];
    for (k, &i) in FACE_OVAL.iter().enumerate() {
        let t = k as f64 * std::f64::consts::TAU / FACE_OVAL.len() as f64;
        mesh[i] = (c.0 + r * t.cos(), c.1 + r * t.sin());
    }
    let landmarks: Vec<(f64, f64)> = vec![c; 5].into_iter().chain(mesh).collect();
    let face = Face { landmarks, ..Face::boxed(b) };
    let p = area_of("reshape", face, &[0]).expect("a map");
    assert_eq!((at(&p, 50, 50), at(&p, 62, 50), at(&p, 38, 50)), (1.0, 0.0, 0.0), "the mesh's small circle, not the box's ellipse");
}

/// Glasses Glare: before a repair, the glasses (Others).
#[test]
fn glare_is_the_glasses_before_a_repair() {
    let b = NormRect::new(0.2, 0.2, 0.8, 0.8);
    let face = Face { masks: vec![(Part::Others, mask(NormRect::new(0.3, 0.3, 0.7, 0.4)))], ..Face::boxed(b) };
    let p = area_of("glare", face, &[0]).expect("a map");
    assert_eq!((at(&p, 50, 35), at(&p, 50, 70)), (1.0, 0.0));
}

/// Glasses Glare after a repair: the repair's lens mask (its alpha), not the glasses mask.
#[test]
fn glare_is_the_repairs_lens_mask() {
    use lightcraft_retouch::input::Patch;
    use std::sync::Arc;
    let b = NormRect::new(0.2, 0.2, 0.8, 0.8);
    let lens = NormRect::new(0.3, 0.5, 0.45, 0.6);
    let patch = Patch { area: lens, gain: lightcraft_raster::Rgb32f::filled(4, 4, [1.0; 3]), alpha: Plane::filled(4, 4, 1.0), stamp: 1 };
    let face = Face { masks: vec![(Part::Others, mask(NormRect::new(0.3, 0.3, 0.7, 0.4)))], patch: Some(Arc::new(patch)), ..Face::boxed(b) };
    let p = area_of("glare", face, &[0]).expect("a map");
    assert_eq!((at(&p, 37, 55), at(&p, 50, 35), at(&p, 70, 55)), (1.0, 0.0, 0.0), "the lens, not the glasses mask");
}

/// Face Reshape without the mesh: the ellipse of the face's box.
#[test]
fn reshape_is_the_face_outline() {
    let p = area_of("reshape", Face::boxed(NormRect::new(0.2, 0.2, 0.8, 0.8)), &[0]).expect("a map");
    assert_eq!((at(&p, 50, 50), at(&p, 22, 22), at(&p, 5, 5)), (1.0, 0.0, 0.0), "the middle in, the box's corners and the rest out");
}

/// Background: everything but the person, and nowhere without a person mask.
#[test]
fn background_is_everything_but_the_person() {
    let b = NormRect::new(0.2, 0.2, 0.8, 0.8);
    let person = Face { masks: vec![(Part::EntirePerson, mask(b))], ..Face::boxed(b) };
    let p = area_of("background", person, &[0]).expect("a map");
    assert_eq!((at(&p, 50, 50), at(&p, 5, 5)), (0.0, 1.0));
    assert!(area_of("background", Face::boxed(b), &[0]).is_none());
}

/// The faces asked for are the ones that show: a map of one face leaves the other out.
#[test]
fn only_the_faces_asked_for_are_in_the_map() {
    let (a, b) = (NormRect::new(0.1, 0.1, 0.4, 0.4), NormRect::new(0.6, 0.6, 0.9, 0.9));
    let face = |r: NormRect| Face { masks: vec![(Part::FacialSkin, mask(r))], ..Face::boxed(r) };
    let input = Input::new(vec![face(a), face(b)]);
    let view = WholePhoto { width: W, height: H };
    let scene = Scene::new(&input, &view, W, H);
    let info = FEATURES.iter().find(|f| f.key == "skin").expect("skin");
    let one = (info.area)(&scene, &[1]).expect("a map");
    assert_eq!((at(&one, 25, 25), at(&one, 75, 75)), (0.0, 1.0));
    let both = (info.area)(&scene, &[0, 1]).expect("a map");
    assert_eq!((at(&both, 25, 25), at(&both, 75, 75)), (1.0, 1.0));
}
