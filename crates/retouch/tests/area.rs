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
    let info = FEATURES.iter().find(|f| f.key == key).expect("a feature");
    let input = Input::new(vec![face]);
    let view = WholePhoto { width: W, height: H };
    let scene = Scene::new(&input, &view, W, H);
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

#[test]
fn blemishes_are_discs_on_the_spots() {
    let b = NormRect::new(0.2, 0.2, 0.8, 0.8);
    let spot = Spot { at: (0.5, 0.5), radius: 0.05, ..Spot::default() };
    let face = Face { spots: Some(Spots { list: vec![spot], skin: 1.0 }), ..Face::boxed(b) };
    let p = area_of("blemish", face, &[0]).expect("a map");
    assert_eq!((at(&p, 50, 50), at(&p, 30, 30)), (1.0, 0.0));
    assert!(area_of("blemish", Face::boxed(b), &[0]).is_none(), "spots not looked for: nowhere");
}

/// Glasses Glare: before a repair, the glasses (Others).
#[test]
fn glare_is_the_glasses_before_a_repair() {
    let b = NormRect::new(0.2, 0.2, 0.8, 0.8);
    let face = Face { masks: vec![(Part::Others, mask(NormRect::new(0.3, 0.3, 0.7, 0.4)))], ..Face::boxed(b) };
    let p = area_of("glare", face, &[0]).expect("a map");
    assert_eq!((at(&p, 50, 35), at(&p, 50, 70)), (1.0, 0.0));
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
