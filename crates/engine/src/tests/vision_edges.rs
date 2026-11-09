//! Fork feature (`fork/apple-vision-masks.md`, issue #115): Apple Vision as the third hair edge. Vision people are
//! matched to the parsed people; where two meet, each keeps their own; the person segmentation draws the edge in the
//! band only; the choice is kept; a parse made with it is its own cache version; a Subject part it drew is marked
//! `+vision`. Synthetic mattes stand in for Vision where the logic is tested; the tests that call Vision itself skip
//! on a Mac without it.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use lightcraft_coreml::vision::Matte as VMatte;
use lightcraft_parsing::{ModelSet, NormRect, Parsing, Part, Person, Plane, RgbImage};

use super::find_removals::temp_dir;
use crate::activity::Activity;
use crate::hair_edges::vision::{People, assign, refine_people, subject_people, to_matte, version, with_hair};
use crate::hair_edges::{Edge, EdgeKind, HairEdges};
use crate::people_parsing::{Outcome, PeopleParsing};

/// A `w × h` coverage matte that is `v` inside the disc `(cx, cy, r)` (fractions of the side), 0 outside.
fn disc(w: usize, h: usize, cx: f32, cy: f32, r: f32, v: f32) -> VMatte {
    let data = (0..w * h)
        .map(|i| {
            let (x, y) = (((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32);
            if (x - cx).hypot(y - cy) < r { v } else { 0.0 }
        })
        .collect();
    VMatte { width: w, height: h, data }
}

/// A parsed person whose Entire Person is the disc `(cx, cy, r)` over the whole photo (32² cells), Hair on its top.
fn person(cx: f32, cy: f32, r: f32) -> Person {
    let n = 32;
    let entire = disc(n, n, cx, cy, r, 1.0).data;
    let mut p = Person { face: Some(NormRect::new(cx - 0.1, cy - 0.1, cx + 0.1, cy + 0.1)), face_score: 0.9, ..Person::default() };
    p.bounds = Some(NormRect::new(cx - r, cy - r, cx + r, cy + r));
    p.parts.insert(Part::EntirePerson, vec![Plane { rect: NormRect::new(0.0, 0.0, 1.0, 1.0), width: n, height: n, data: entire }]);
    p.parts
        .insert(Part::Hair, vec![Plane { rect: NormRect::new(cx - 0.2, cy - r, cx + 0.2, cy - r + 0.13), width: 8, height: 4, data: vec![1.0; 32] }]);
    p
}

fn parse_of(people: Vec<Person>) -> Parsing {
    let set = ModelSet::Segformer;
    Parsing { model_set: set.id().into(), model_version: set.version(), width: 32, height: 32, people }
}

#[test]
fn ids_round_trip_and_each_edge_has_its_own_cache_version() {
    assert_eq!(Edge::parse("apple-vision"), Some(Edge::Vision));
    assert_eq!(Edge::Vision.label(), "Apple Vision");
    assert_eq!(version(Edge::Current), None);
    assert_ne!(version(Edge::Matting), version(Edge::Vision));
    assert!(version(Edge::Vision).is_some());
}

#[test]
fn vision_people_are_matched_to_the_parsed_people_they_overlap() {
    let people = vec![person(0.3, 0.5, 0.18), person(0.72, 0.5, 0.18)];
    // Vision found them in the other order, and a third person the parse does not have
    let each = vec![disc(64, 64, 0.72, 0.5, 0.2, 1.0), disc(64, 64, 0.3, 0.5, 0.2, 1.0), disc(64, 64, 0.5, 0.1, 0.05, 1.0)];
    assert_eq!(assign(&each, &people), vec![Some(1), Some(0)]);
    // nobody from Vision: nobody matched
    assert_eq!(assign(&[], &people), vec![None, None]);
}

#[test]
fn the_segmentation_draws_the_edge_and_people_keep_their_own() {
    // two people side by side; the segmentation covers both with their strands (r 0.2 against the parse's 0.18) and
    // the gap between them, where Vision's second person is
    let mut parse = parse_of(vec![person(0.3, 0.5, 0.18), person(0.72, 0.5, 0.18)]);
    let (w, h) = (128, 128);
    let mut all = disc(w, h, 0.3, 0.5, 0.2, 1.0);
    for (v, o) in all.data.iter_mut().zip(disc(w, h, 0.72, 0.5, 0.22, 1.0).data) {
        *v = v.max(o);
    }
    let each = vec![disc(w, h, 0.3, 0.5, 0.2, 1.0), disc(w, h, 0.72, 0.5, 0.22, 1.0)];
    refine_people(&mut parse, &People { all, each });
    assert!(parse.model_version.ends_with("+vision1"), "{}", parse.model_version);
    let first = &parse.people[0];
    assert!(first.sample(Part::EntirePerson, 0.3, 0.5 - 0.19) > 0.5, "the first person's strand comes in");
    assert!(first.sample(Part::Hair, 0.3, 0.5 - 0.19) > 0.5, "and joins their hair");
    assert!(first.sample(Part::EntirePerson, 0.3, 0.5) > 0.9, "the inside stays in");
    // 0.51 is the second Vision person's, within the first's band: never the first's
    assert!(first.sample(Part::EntirePerson, 0.51, 0.5) < 0.5, "the first does not take the second's edge");
    assert!(parse.people[1].sample(Part::EntirePerson, 0.72 + 0.21, 0.5) > 0.5, "the second gains their strand");
}

#[test]
fn a_coverage_matte_becomes_a_logit_grid() {
    let m = to_matte(&disc(60, 40, 0.5, 0.5, 0.25, 1.0), 64);
    assert!(m.is_valid() && m.side == 64);
    assert!(m.coverage(0.5, 0.5) > 0.99 && m.coverage(0.05, 0.05) < 0.01);
    // damaged sizes give an empty, valid grid rather than a panic
    assert!(to_matte(&VMatte { width: 0, height: 0, data: vec![] }, 0).is_valid());
}

#[cfg(feature = "sam")]
#[test]
fn a_subject_part_vision_drew_is_marked_and_named() {
    let base = lightcraft_develop::SegMask::from_logits(32, &vec![-6.0; 32 * 32]).made_by("birefnet");
    let mut m = to_matte(&disc(64, 64, 0.5, 0.5, 0.3, 1.0), 64);
    assert!(crate::hair_edges::vision::subject_edge(Edge::Vision, &base, &m).is_none(), "an empty base stays empty");
    let base = lightcraft_develop::SegMask::from_logits(32, &to_matte(&disc(32, 32, 0.5, 0.5, 0.27, 1.0), 32).logits).made_by("birefnet");
    let out = crate::hair_edges::vision::subject_edge(Edge::Vision, &base, &m).expect("a selection");
    assert_eq!(out.model.as_deref(), Some("birefnet+vision"));
    assert_eq!(crate::segment::birefnet::label_of(Some("birefnet+vision")), "BiRefNet, Apple Vision Edges");
    // BiRefNet Matting keeps its own mark
    m.side = 64;
    let matted = crate::hair_edges::vision::subject_edge(Edge::Matting, &base, &m).expect("a selection");
    assert_eq!(matted.model.as_deref(), Some("birefnet+matting"));
}

fn edges(tag: &str) -> (HairEdges, std::path::PathBuf, Activity) {
    let dir = temp_dir(&format!("vision-{tag}"));
    let activity = Activity::default();
    let h = HairEdges::new(activity.clone());
    h.set_paths(Some(dir.join("models")), Some(dir.join("mask-edges.json")));
    (h, dir, activity)
}

#[test]
fn apple_vision_is_chosen_and_kept_where_it_runs() {
    let (h, dir, _) = edges("choice");
    let r = h.set_edge(EdgeKind::People, Edge::Vision);
    if !crate::hair_edges::vision::available() {
        assert!(r.unwrap_err().contains("macOS 14"));
        assert_eq!(h.edge(EdgeKind::People), Edge::Current);
        lightcraft_testkit::skip!(System, "Apple Vision masks need macOS 14 or later on a Mac");
        return;
    }
    r.unwrap();
    assert_eq!((h.chosen(EdgeKind::People), h.edge(EdgeKind::People)), (Edge::Vision, Edge::Vision));
    assert_eq!(h.edge(EdgeKind::Subject), Edge::Current, "the other kind is untouched");
    let saved = std::fs::read_to_string(dir.join("mask-edges.json")).unwrap();
    assert!(saved.contains("apple-vision"), "{saved}");
    // read back by a new session
    let again = HairEdges::new(Activity::default());
    again.set_paths(None, Some(dir.join("mask-edges.json")));
    assert_eq!(again.chosen(EdgeKind::People), Edge::Vision);
    let _ = std::fs::remove_dir_all(dir);
}

/// A portrait-like picture: a light backdrop, a dark round head with a body.
fn picture(w: usize, h: usize) -> RgbImage {
    let mut px = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let head = (fx - 0.5).hypot(fy - 0.35) < 0.16;
            let body = (fx - 0.5).abs() < 0.25 && fy > 0.5;
            px.extend_from_slice(if head {
                &[200, 150, 120]
            } else if body {
                &[40, 40, 60]
            } else {
                &[120, 170, 230]
            });
        }
    }
    RgbImage::new(w, h, px).unwrap()
}

/// People parsing with the Apple Vision edge calls Vision once on the parse's picture, with an activity row, marks
/// the parse as its own version, and the second ask is the cache's. A parse whose people Vision does not see keeps
/// its parse inside the band's far side. Skips without Vision.
#[test]
fn a_people_parse_with_apple_vision_edges_is_made_once_and_cached() {
    if !crate::hair_edges::vision::available() {
        lightcraft_testkit::skip!(System, "Apple Vision masks need macOS 14 or later on a Mac");
        return;
    }
    let (h, dir, _) = edges("parse");
    h.set_edge(EdgeKind::People, Edge::Vision).unwrap();
    let p = PeopleParsing::new(Activity::default()).with_hair_edges(h.clone());
    p.set_paths(Some(dir.join("models")), Some(dir.join("cache")), None);
    p.set_runner(Some(Arc::new(|_, _, _, _, _| Ok(parse_of(vec![person(0.5, 0.55, 0.3)])))));
    let img = picture(192, 128);
    let parsing = match p.parse(img.clone(), Vec::new(), true) {
        Outcome::Ready { parsing, cached: false } => parsing,
        other => panic!("no parse: {other:?}"),
    };
    assert!(parsing.model_version.ends_with("+vision1"), "{}", parsing.model_version);
    assert!(parsing.people[0].sample(Part::EntirePerson, 0.5, 0.55) > 0.5, "the person's middle, far from the edge, stays");
    assert_eq!(h.runs(), 1);
    assert!(matches!(p.parse(img.clone(), Vec::new(), true), Outcome::Ready { cached: true, .. }));
    assert_eq!(h.runs(), 1, "the second ask is the cache's");
    // the Subject matte too: made once, then remembered
    let flag = Arc::new(AtomicBool::new(false));
    let found = h.vision_subject(&img.data, img.width, img.height, &flag).unwrap();
    assert_eq!(h.runs(), 2);
    if let Some(m) = found {
        assert!(m.is_valid());
        assert!(h.vision_subject(&img.data, img.width, img.height, &flag).unwrap().is_some());
        assert_eq!(h.runs(), 2, "the second ask is remembered");
    }
    // a damaged picture is a message
    assert!(h.vision_subject(&img.data[..10], img.width, img.height, &flag).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

/// A parse that splits one person (a hand found as a second "person" inside the first) while Vision sees one: the
/// extra parsed person keeps its parse edge. Before, it got the segmentation less the first person's share, which is
/// all of it, and a ring of background opened inside the person (hair-sky-3).
#[test]
fn a_parsed_person_vision_does_not_see_keeps_its_edge() {
    let mut parse = parse_of(vec![person(0.5, 0.5, 0.35), person(0.5, 0.7, 0.08)]);
    let hand_before = parse.people[1].clone();
    let (w, h) = (128, 128);
    let all = disc(w, h, 0.5, 0.5, 0.37, 1.0);
    let each = vec![disc(w, h, 0.5, 0.5, 0.37, 1.0)];
    refine_people(&mut parse, &People { all, each });
    let hand = &parse.people[1];
    // just inside the hand's own edge: still the hand's
    assert!(hand.sample(Part::EntirePerson, 0.5, 0.7 - 0.07) > 0.5, "the hand's edge stays");
    assert!(hand.sample(Part::EntirePerson, 0.5, 0.7) > 0.5, "the hand stays");
    assert_eq!(hand.parts, hand_before.parts, "the hand keeps its parse");
}

/// A parsed person Vision does not see where the segmentation is empty keeps its edge. Before, an unmatched person
/// took the segmentation as it was, and an empty one cut away their whole edge band (PR #120 review).
#[test]
fn a_parsed_person_vision_misses_keeps_its_edge_band() {
    let mut parse = parse_of(vec![person(0.5, 0.5, 0.3)]);
    let all = VMatte { width: 128, height: 128, data: vec![0.0; 128 * 128] };
    refine_people(&mut parse, &People { all, each: Vec::new() });
    assert!(parse.model_version.ends_with("+vision1"), "{}", parse.model_version);
    let p = &parse.people[0];
    // (the parse's 32-cell disc has its edge about 0.31 from the centre: 0.30 is within half the band)
    assert!(p.sample(Part::EntirePerson, 0.5, 0.5 - 0.30) > 0.5, "just inside the edge stays in");
    assert!(p.sample(Part::EntirePerson, 0.5 + 0.30, 0.5) > 0.5, "on every side");
}

/// Someone Vision sees beside a parsed person, whom the parse does not have, is still someone else: the parsed person
/// does not take their edge. Before, only the Vision people matched to other parsed people were subtracted, and the
/// parsed person grew into the neighbour (PR #120 review).
#[test]
fn a_neighbour_only_vision_sees_is_not_taken() {
    let mut parse = parse_of(vec![person(0.3, 0.5, 0.18)]);
    let (w, h) = (128, 128);
    let mut all = disc(w, h, 0.3, 0.5, 0.17, 1.0);
    let neighbour = disc(w, h, 0.73, 0.5, 0.245, 1.0);
    for (v, o) in all.data.iter_mut().zip(&neighbour.data) {
        *v = v.max(*o);
    }
    let each = vec![disc(w, h, 0.3, 0.5, 0.17, 1.0), neighbour];
    refine_people(&mut parse, &People { all, each });
    let p = &parse.people[0];
    assert!(p.sample(Part::EntirePerson, 0.3, 0.5) > 0.9, "the inside stays in");
    // 0.49 is within half the band of the parsed edge (0.48), and the neighbour's
    assert!(p.sample(Part::EntirePerson, 0.49, 0.5) < 0.5, "the person does not take the neighbour's edge");
}

/// A full-frame person at a large photo's size: the Vision edge's planes stay within what the parse cache reads back.
/// Before, the matte side was 2048 and the parse file was refused on every open, so the parse was made again each
/// time (PR #120 review).
#[test]
fn a_full_frame_parse_with_vision_edges_reads_back_from_the_cache() {
    let mut whole = person(0.5, 0.5, 0.2);
    whole.parts.insert(Part::EntirePerson, vec![Plane { rect: NormRect::new(0.0, 0.0, 1.0, 1.0), width: 32, height: 32, data: vec![1.0; 32 * 32] }]);
    let mut parse = parse_of(vec![whole]);
    let (w, h) = (2048, 1536);
    let all = VMatte { width: w, height: h, data: vec![1.0; w * h] };
    let each = vec![VMatte { width: 64, height: 48, data: vec![1.0; 64 * 48] }];
    refine_people(&mut parse, &People { all, each });
    assert!(parse.model_version.ends_with("+vision1"));
    let bytes = lightcraft_parsing::cache::encode(&parse).unwrap();
    let back = lightcraft_parsing::cache::decode(&bytes).expect("the parse reads back");
    assert_eq!(back.people.len(), 1);
    assert!(back.people[0].sample(Part::EntirePerson, 0.5, 0.5) > 0.9);
}

/// Only the people who are part of the subject add their hair to a Subject matte. Before, the person segmentation of
/// everyone was added, so a bystander beside a dog became part of the subject's edge (PR #120 review).
#[test]
fn only_subject_people_add_their_hair_to_the_subject() {
    let (w, h) = (128, 128);
    // the subject: one person whose subject mask drops their strands (r 0.18 against the segmentation's 0.21)
    let subject = disc(w, h, 0.3, 0.5, 0.18, 1.0);
    let each = vec![disc(w, h, 0.3, 0.5, 0.2, 1.0), disc(w, h, 0.75, 0.5, 0.15, 1.0)];
    assert_eq!(subject_people(&each, &subject), vec![0], "the bystander is not part of the subject");
    let mut seg = disc(w, h, 0.3, 0.5, 0.21, 1.0);
    for (v, o) in seg.data.iter_mut().zip(disc(w, h, 0.75, 0.5, 0.16, 1.0).data) {
        *v = v.max(o);
    }
    let out = with_hair(subject.clone(), &People { all: seg, each: each.clone() }, &[0]);
    assert!(out.at(0.3, 0.5 - 0.195) > 0.5, "the subject's strand comes in");
    assert!(out.at(0.75, 0.5) < 0.01, "the bystander stays out");
    assert_eq!(with_hair(subject.clone(), &People { all: subject.clone(), each }, &[]), subject, "nobody: the subject as it is");
}
