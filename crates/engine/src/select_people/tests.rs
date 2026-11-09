//! Select People (`fork/select-people.md`): each part makes a mask; several parts make one; one face or everyone;
//! no people, 50 people, a face cut by the edge and a turned photo; faces found again by name or place; the
//! commands; preview and export agree; the mask survives closing and reopening the library. A fake runner stands in
//! for the parser: no test needs a model.

use std::sync::Arc;

use lightcraft_develop::{MaskShape, SegMask};
use lightcraft_geom::Orientation;
use lightcraft_parsing::{NormRect, Parsing, Part, Person, Plane};
use serde_json::json;

use super::grid::{Selection, build};
use super::{Pick, face_key, find_person};
use crate::Session;

pub(super) fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(super) fn plane(r: NormRect, side: usize) -> Plane {
    Plane { rect: r, width: side, height: side, data: vec![1.0; side * side] }
}

/// A person whose every part is its own small square inside the box at (`x`, `y`) of side `s`: part `i` of
/// `Part::ALL` at column `i % 4`, row `i / 4`.
pub(super) fn person(x: f32, y: f32, s: f32, name: Option<&str>) -> Person {
    let mut q = Person {
        face: Some(NormRect::new(x, y, x + s, y + s)),
        face_score: 0.9,
        person_id: name.map(str::to_string),
        bounds: Some(NormRect::new(x, y, x + s, y + s)),
        ..Person::default()
    };
    let c = s / 4.0;
    for (i, part) in Part::ALL.into_iter().enumerate() {
        let (px, py) = (x + (i % 4) as f32 * c, y + (i / 4) as f32 * c);
        // (Entire Person: the whole box)
        let r = if part == Part::EntirePerson {
            NormRect::new(x, y, x + s, y + s)
        } else {
            NormRect::new(px + c * 0.2, py + c * 0.2, px + c * 0.8, py + c * 0.8)
        };
        q.parts.insert(part, vec![plane(r, 16)]);
    }
    q
}

pub(super) fn parse(people: Vec<Person>) -> Parsing {
    Parsing { model_set: "segformer".into(), model_version: "1".into(), width: 2048, height: 1365, people }
}

/// The coverage (0 to 1) the grids select at a point of the mask's frame: a finer grid inside its box, else the
/// main one (as a render samples them, without the edge filter).
pub(super) fn at(main: &SegMask, detail: &[SegMask], x: f64, y: f64) -> f32 {
    let sample = |g: &SegMask| -> Option<f32> {
        let r = g.bounds()?;
        if x < r[0] || x > r[2] || y < r[1] || y > r[3] {
            return None;
        }
        let side = g.side as usize;
        let l = g.logits()?;
        let i = (((x - r[0]) / (r[2] - r[0]) * side as f64) as usize).min(side - 1);
        let j = (((y - r[1]) / (r[3] - r[1]) * side as f64) as usize).min(side - 1);
        Some(1.0 / (1.0 + (-l[j * side + i]).exp()))
    };
    detail.iter().filter_map(sample).reduce(f32::max).or_else(|| sample(main)).unwrap_or(0.0)
}

/// The centre of part `part`'s square in [`person`].
pub(super) fn centre(x: f32, y: f32, s: f32, part: Part) -> (f64, f64) {
    let i = Part::ALL.iter().position(|p| *p == part).unwrap();
    let c = s / 4.0;
    (f64::from(x + (i % 4) as f32 * c + c / 2.0), f64::from(y + (i / 4) as f32 * c + c / 2.0))
}

#[test]
fn each_part_makes_a_mask_of_its_own_place() {
    let p = parse(vec![person(0.2, 0.2, 0.4, None)]);
    for pick in Pick::ALL {
        let sel = Selection::of(None, &[pick]);
        let (main, detail) = build(&p, &sel, Orientation::Normal).unwrap_or_else(|| panic!("{pick:?} made no mask"));
        assert!(main.model.as_deref().is_some_and(|m| m.starts_with("people:segformer")), "{pick:?}");
        for part in pick.parts() {
            let (x, y) = centre(0.2, 0.2, 0.4, *part);
            assert!(at(&main, &detail, x, y) > 0.9, "{pick:?} leaves out {part:?}");
        }
        // a part outside the pick is left out (Entire Person covers them all)
        if pick != Pick::EntirePerson && pick != Pick::Background {
            let other = Part::ALL.into_iter().find(|q| *q != Part::EntirePerson && !pick.parts().contains(q)).unwrap();
            let (x, y) = centre(0.2, 0.2, 0.4, other);
            assert!(at(&main, &detail, x, y) < 0.1, "{pick:?} takes {other:?} too");
        }
        // outside the person: only Background
        let outside = at(&main, &detail, 0.9, 0.9);
        assert_eq!(outside > 0.9, pick == Pick::Background, "{pick:?}: {outside}");
    }
    // Background leaves the person out
    let (main, detail) = build(&p, &Selection::of(None, &[Pick::Background]), Orientation::Normal).unwrap();
    assert!(at(&main, &detail, 0.4, 0.4) < 0.1);
}

#[test]
fn several_parts_make_one_combined_mask() {
    let p = parse(vec![person(0.2, 0.2, 0.4, None)]);
    let sel = Selection::of(None, &[Pick::Hair, Pick::Lips, Pick::Background]);
    let (main, detail) = build(&p, &sel, Orientation::Normal).unwrap();
    for part in [Part::Hair, Part::Lips] {
        let (x, y) = centre(0.2, 0.2, 0.4, part);
        assert!(at(&main, &detail, x, y) > 0.9, "{part:?}");
    }
    let (x, y) = centre(0.2, 0.2, 0.4, Part::Teeth);
    assert!(at(&main, &detail, x, y) < 0.1);
    assert!(at(&main, &detail, 0.95, 0.05) > 0.9, "the background is in");
}

#[test]
fn one_face_or_everyone() {
    let p = parse(vec![person(0.05, 0.3, 0.3, None), person(0.6, 0.3, 0.3, None)]);
    let (a, b) = (centre(0.05, 0.3, 0.3, Part::Hair), centre(0.6, 0.3, 0.3, Part::Hair));
    let (main, detail) = build(&p, &Selection::of(Some(1), &[Pick::Hair]), Orientation::Normal).unwrap();
    assert!(at(&main, &detail, a.0, a.1) < 0.1 && at(&main, &detail, b.0, b.1) > 0.9);
    let (main, detail) = build(&p, &Selection::of(None, &[Pick::Hair]), Orientation::Normal).unwrap();
    assert!(at(&main, &detail, a.0, a.1) > 0.9 && at(&main, &detail, b.0, b.1) > 0.9);
    // one person's background still holds the other person
    let (main, detail) = build(&p, &Selection::of(Some(1), &[Pick::Background]), Orientation::Normal).unwrap();
    assert!(at(&main, &detail, a.0, a.1) > 0.9 && at(&main, &detail, b.0, b.1) < 0.1);
    // a person who is not there
    assert!(build(&p, &Selection::of(Some(7), &[Pick::Hair]), Orientation::Normal).is_none());
}

#[test]
fn no_people_fifty_people_and_a_face_cut_by_the_edge() {
    // nobody: no hair, and the background is the whole photo
    let none = parse(Vec::new());
    assert!(build(&none, &Selection::of(None, &[Pick::Hair]), Orientation::Normal).is_none());
    let (main, detail) = build(&none, &Selection::of(None, &[Pick::Background]), Orientation::Normal).unwrap();
    assert!(at(&main, &detail, 0.5, 0.5) > 0.9);
    // fifty small faces: everyone's hair, at most four finer grids, every one of them selected
    let many: Vec<Person> = (0..50).map(|i| person(0.02 + (i % 10) as f32 * 0.097, 0.05 + (i / 10) as f32 * 0.19, 0.06, None)).collect();
    let p = parse(many);
    let (main, detail) = build(&p, &Selection::of(None, &[Pick::Hair, Pick::Iris]), Orientation::Normal).unwrap();
    assert!(detail.len() <= lightcraft_develop::segmask::MAX_DETAIL);
    assert!(main.side as usize <= lightcraft_develop::segmask::MAX_SIDE);
    for i in 0..50 {
        let (x, y) = centre(0.02 + (i % 10) as f32 * 0.097, 0.05 + (i / 10) as f32 * 0.19, 0.06, Part::Hair);
        assert!(at(&main, &detail, x, y) > 0.5, "person {i}");
    }
    // a face past the photo's edge: what is inside is drawn
    let cut = parse(vec![person(-0.1, -0.1, 0.4, None)]);
    let (main, detail) = build(&cut, &Selection::of(Some(0), &[Pick::EntirePerson]), Orientation::Normal).unwrap();
    assert!(at(&main, &detail, 0.1, 0.1) > 0.9);
    // damaged planes are skipped, never a panic
    let mut bad = person(0.2, 0.2, 0.4, None);
    bad.parts.insert(Part::Hair, vec![Plane { rect: NormRect::new(f32::NAN, 0.0, 1.0, 1.0), width: 2, height: 2, data: vec![1.0; 3] }]);
    assert!(build(&parse(vec![bad]), &Selection::of(None, &[Pick::Hair]), Orientation::Normal).is_none());
}

#[test]
fn a_turned_photo_gets_its_mask_where_the_part_is_on_screen() {
    // hair in the parse's top-left; after a quarter turn clockwise it is at the top right
    let p = parse(vec![person(0.0, 0.0, 0.4, None)]);
    let (hx, hy) = centre(0.0, 0.0, 0.4, Part::Hair);
    let (tx, ty) = Orientation::Rotate90.map(hx, hy, 1.0, 1.0);
    let (main, detail) = build(&p, &Selection::of(None, &[Pick::Hair]), Orientation::Rotate90).unwrap();
    assert!(at(&main, &detail, tx, ty) > 0.9, "({tx}, {ty})");
    assert!(at(&main, &detail, hx, hy) < 0.1);
}

#[test]
fn a_mask_finds_its_face_by_name_or_by_place() {
    let p = parse(vec![person(0.1, 0.1, 0.2, Some("Ann Lee")), person(0.6, 0.1, 0.2, None)]);
    let ann = face_key(&p.people[0]);
    assert_eq!(ann.person.as_deref(), Some("ann lee"));
    let other = face_key(&p.people[1]);
    // found again in another order
    let swapped = parse(vec![p.people[1].clone(), p.people[0].clone()]);
    assert_eq!(find_person(&swapped, &ann), Some(1));
    assert_eq!(find_person(&swapped, &other), Some(0));
    // the named face moved: still found by name; an unnamed key never goes far
    let moved = parse(vec![person(0.6, 0.6, 0.2, Some("ann lee")), person(0.1, 0.6, 0.2, None)]);
    assert_eq!(find_person(&moved, &ann), Some(0));
    assert_eq!(find_person(&moved, &other), None);
    // a face named since is still the face at its place; a named one never goes to someone else
    let named = parse(vec![person(0.6, 0.1, 0.2, Some("bo")), person(0.1, 0.1, 0.2, Some("cy"))]);
    assert_eq!(find_person(&named, &other), Some(0));
    assert_eq!(find_person(&named, &ann), None);
    assert_eq!(find_person(&parse(Vec::new()), &ann), None);
}

// ---- the session and its commands

pub(super) fn fake_people() -> Parsing {
    parse(vec![person(0.08, 0.2, 0.3, Some("Ann")), person(0.6, 0.2, 0.3, None)])
}

/// A session with a 1200 × 800 photo, a fake parser and (with `dir`) a library on disk.
pub(super) fn session(tag: &str) -> (Session, std::path::PathBuf) {
    let dir = temp_dir(&format!("select-people-{tag}"));
    let (w, h) = (1200usize, 800usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| if (i % w) * 100 < w * 55 { [60, 50, 40, 255] } else { [200, 190, 180, 255] }).collect();
    let png = lightcraft_codecs::encode_png(
        &lightcraft_codecs::EncodeImage::rgba8(&lightcraft_raster::Rgba8 { width: w, height: h, data }),
        &Default::default(),
    )
    .unwrap();
    std::fs::write(dir.join("people.png"), png).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(dir.join("library"), false).unwrap();
    s.execute("library.import", &json!({"paths": [dir.join("people.png").to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.parsing.set_paths(Some(dir.join("models")), Some(dir.join("cache")), None);
    s.parsing.set_runner(Some(Arc::new(|_, _, _, _, _| Ok(fake_people()))));
    (s, dir)
}

pub(super) fn people_part(s: &Session) -> MaskShape {
    let id = s.active().unwrap();
    let d = s.develop_of(id).unwrap();
    let m = d.masks.iter().find(|m| Some(m.id) == s.active_mask).unwrap();
    m.components[0].shape.clone()
}

#[test]
fn the_commands_make_and_change_a_mask() {
    let (mut s, dir) = session("commands");
    let id = s.active().unwrap();
    // found on a worker, with a row while the photo is prepared
    let r = s.execute("selectPeople.people", &json!({})).unwrap();
    assert!(r["state"] == "running" || r["state"] == "ready", "{r}");
    let r = s.execute("selectPeople.people", &json!({"wait": true})).unwrap();
    assert_eq!(r["people"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(r["people"][0]["name"], "Ann");
    assert!(s.people_of(id).ready.is_some());
    // a new mask: Hair of everyone
    let r = s.execute("selectPeople.add", &json!({"parts": ["hair"]})).unwrap();
    let mid = r["mask"].as_u64().unwrap() as u32;
    let d = s.develop_of(id).unwrap();
    let m = d.masks.iter().find(|m| m.id == mid).unwrap();
    assert_eq!(m.name, "Hair Mask 1");
    let MaskShape::People { parts, face: None, seg: Some(_), .. } = &m.components[0].shape else { panic!("{:?}", m.components[0].shape) };
    assert_eq!(parts, &["hair"]);
    // changed to Ann's hair and lips: one mask, its generated name follows
    s.execute("selectPeople.set", &json!({"parts": ["hair", "lips"], "face": 0})).unwrap();
    let MaskShape::People { parts, face: Some(face), .. } = people_part(&s) else { panic!() };
    assert_eq!(parts, &["hair", "lips"]);
    assert_eq!(face.person.as_deref(), Some("ann"));
    assert_eq!(s.develop_of(id).unwrap().masks.len(), 1);
    assert_eq!(s.develop_of(id).unwrap().masks[0].name, "Hair + Lips Mask 1");
    // undo puts the hair of everyone back
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(matches!(people_part(&s), MaskShape::People { face: None, .. }));
    // what is refused
    assert!(s.execute("selectPeople.add", &json!({"parts": ["elbows"]})).is_err());
    assert!(s.execute("selectPeople.add", &json!({"parts": []})).is_err());
    assert!(s.execute("selectPeople.add", &json!({"face": 9})).is_err());
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    assert!(s.execute("selectPeople.set", &json!({"parts": ["hair"]})).is_err(), "a radial is not a Select People mask");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn preview_and_export_draw_the_same_mask() {
    let (mut s, dir) = session("sizes");
    let id = s.active().unwrap();
    s.execute("selectPeople.add", &json!({"parts": ["entirePerson"], "face": 0})).unwrap();
    s.execute("mask.adjust", &json!({"values": {"exposure": 3.0}})).unwrap();
    let d = (*s.develop_of(id).unwrap()).clone();
    let render = |s: &mut Session, edge: usize| {
        let img = s.preview_job(id, edge, edge, true, &d).unwrap().run().rendered.unwrap().image;
        (img.width, img.height, img.data.iter().map(|p| f32::from(p[1]) / 255.0).collect::<Vec<f32>>())
    };
    let (bw, bh, big) = render(&mut s, 1200);
    let (sw, sh, small) = render(&mut s, 300);
    assert_eq!((bw / 4, bh / 4), (sw, sh));
    let f = 4;
    let down: Vec<f32> =
        (0..sw * sh).map(|j| (0..f * f).map(|k| big[((j / sw) * f + k / f) * bw + (j % sw) * f + k % f]).sum::<f32>() / (f * f) as f32).collect();
    let mean = down.iter().zip(&small).map(|(a, b)| (a - b).abs()).sum::<f32>() / small.len() as f32;
    assert!(mean < 0.02, "preview and export differ by {mean}");
    // the mask brightened Ann's box and left the same tone below it as it was
    let inside = small[(sh * 35 / 100) * sw + sw * 23 / 100];
    let outside = small[(sh * 85 / 100) * sw + sw * 23 / 100];
    assert!(inside > outside + 0.15, "{inside} {outside}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_mask_survives_closing_and_reopening_the_library() {
    let (mut s, dir) = session("reopen");
    let id = s.active().unwrap();
    s.execute("selectPeople.add", &json!({"parts": ["hair", "background"], "face": 1})).unwrap();
    let before = s.develop_of(id).unwrap().masks.clone();
    s.close_library().unwrap();
    drop(s);
    let mut again = Session::new().with_fs();
    again.open_library(dir.join("library"), false).unwrap();
    let after = again.catalog.photo(id).unwrap().develop.masks.clone();
    assert_eq!(after, before);
    assert!(matches!(&after[0].components[0].shape, MaskShape::People { seg: Some(_), face: Some(_), .. }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn two_people_side_by_side_make_one_top_without_a_line_where_they_meet() {
    // one shirt across the middle of the photo, the parse's cells 0..4 the first person's and 4..8 the second's, as a
    // parse shares a part out between people (#124)
    let half = |mine: &dyn Fn(usize) -> bool| {
        let data = (0..8 * 4).map(|i| if mine(i % 8) { 1.0 } else { 0.0 }).collect();
        let mut q = Person::default();
        q.parts.insert(Part::ClothesTop, vec![Plane { rect: NormRect::new(0.1, 0.4, 0.9, 0.6), width: 8, height: 4, data }]);
        q
    };
    let p = parse(vec![half(&|x| x < 4), half(&|x| x >= 4)]);
    let (main, detail) = build(&p, &Selection::of(None, &[Pick::Top]), Orientation::Normal).unwrap();
    for k in 0..=40 {
        let x = 0.15 + 0.7 * f64::from(k) / 40.0;
        assert!(at(&main, &detail, x, 0.5) > 0.95, "x {x}: {}", at(&main, &detail, x, 0.5));
    }
}
