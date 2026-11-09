//! A spot edit draws only the spots it reaches, and the result is the one drawing every spot gives (issue #82).

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, Spot, SpotMode};
use lightcraft_geom::Point;
use lightcraft_raster::Rgb32f;

use super::{Drawn, Footprint, Step, redo, spots_drawn};
use crate::{RenderRequest, SourceInfo, StageCache, render, render_cached};

fn scene() -> Arc<Rgb32f> {
    Arc::new(lightcraft_scenes::demo_library()[0].render(480, 320))
}

fn spot(mode: SpotMode, x: f64, y: f64, source: Option<(f64, f64)>) -> Spot {
    Spot {
        mode,
        points: vec![Point::new(x, y)],
        size: 0.008,
        feather: 40.0,
        opacity: 100.0,
        source_offset: source.map(|(dx, dy)| Point::new(dx, dy)),
        ..Default::default()
    }
}

/// `n` spots on a grid, apart from each other, every mode and some with an automatic source.
fn grid(n: usize) -> Vec<Spot> {
    (0..n)
        .map(|i| {
            let (x, y) = (0.12 + 0.11 * (i % 8) as f64, 0.15 + 0.14 * (i / 8) as f64);
            let mode = [SpotMode::Remove, SpotMode::Heal, SpotMode::Clone][i % 3];
            spot(mode, x, y, (i % 5 != 4).then_some((0.0, 0.045)))
        })
        .collect()
}

/// Renders `s` through `cache`: the spots drawn, and whether the result is the one an uncached render gives.
fn edit(cache: &StageCache, src: &Arc<Rgb32f>, s: &DevelopSettings) -> (usize, bool) {
    let (info, req) = (SourceInfo::default(), RenderRequest::fit(480, 480));
    let before = spots_drawn();
    let kept = render_cached(src, &info, s, &req, cache).image;
    let drawn = spots_drawn() - before;
    let whole = render(src, &info, s, &req).image;
    (drawn, kept.data == whole.data)
}

#[test]
fn adding_a_spot_draws_only_that_spot() {
    let (src, cache) = (scene(), StageCache::default());
    let mut s = DevelopSettings { spots: grid(30), ..Default::default() };
    assert_eq!(edit(&cache, &src, &s), (30, true), "the first render draws them all");
    s.spots.push(spot(SpotMode::Heal, 0.5, 0.92, Some((0.05, -0.03))));
    assert_eq!(edit(&cache, &src, &s), (1, true), "the 31st spot alone");
    s.spots.push(spot(SpotMode::Remove, 0.3, 0.92, None));
    assert_eq!(edit(&cache, &src, &s), (1, true), "a 32nd with an automatic source");
}

#[test]
fn changing_one_spot_draws_it_alone_when_it_reaches_no_other() {
    let (src, cache) = (scene(), StageCache::default());
    let mut s = DevelopSettings { spots: grid(30), ..Default::default() };
    edit(&cache, &src, &s);
    let changes: [(&str, fn(&mut Spot)); 5] = [
        ("move", |sp| sp.points.iter_mut().for_each(|p| p.x += 0.01)),
        ("move its source", |sp| sp.source_offset = Some(Point::new(0.01, 0.04))),
        ("size", |sp| sp.size = 0.01),
        ("feather", |sp| sp.feather = 80.0),
        ("opacity", |sp| sp.opacity = 60.0),
    ];
    for (what, change) in changes {
        change(&mut s.spots[10]);
        assert_eq!(edit(&cache, &src, &s), (1, true), "{what}");
    }
    s.spots.remove(10);
    assert_eq!(edit(&cache, &src, &s), (0, true), "a deleted spot draws nothing: its box comes back");
    let unchanged = edit(&cache, &src, &DevelopSettings { red_eye: Vec::new(), ..s.clone() });
    assert_eq!(unchanged, (0, true), "the same spots draw nothing");
}

#[test]
fn a_change_draws_again_every_spot_that_reads_what_it_changed() {
    let (src, cache) = (scene(), StageCache::default());
    // a chain: each spot overlaps the next, the last one copies from the first one's box
    let mut s = DevelopSettings {
        spots: vec![
            spot(SpotMode::Clone, 0.300, 0.50, Some((0.0, 0.1))),
            spot(SpotMode::Heal, 0.325, 0.50, Some((0.0, -0.1))),
            spot(SpotMode::Clone, 0.350, 0.50, Some((0.0, 0.1))),
            spot(SpotMode::Heal, 0.60, 0.50, Some((-0.3, 0.0))),
            spot(SpotMode::Heal, 0.85, 0.20, Some((0.0, 0.1))),
        ],
        ..Default::default()
    };
    edit(&cache, &src, &s);
    s.spots[0].points[0].y += 0.005;
    assert_eq!(edit(&cache, &src, &s), (4, true), "the chain and the spot reading it, not the one apart");
    s.spots[4].opacity = 50.0;
    assert_eq!(edit(&cache, &src, &s), (1, true));
}

#[test]
fn content_aware_spots_fill_only_the_one_added() {
    let (src, cache) = (scene(), StageCache::default());
    let mut s = DevelopSettings { spots: (0..6).map(|i| spot(SpotMode::Fill, 0.15 + 0.14 * i as f64, 0.3, None)).collect(), ..Default::default() };
    assert_eq!(edit(&cache, &src, &s), (6, true));
    s.spots.push(spot(SpotMode::Fill, 0.5, 0.75, None));
    assert_eq!(edit(&cache, &src, &s), (1, true));
}

#[test]
fn edits_in_any_order_match_drawing_every_spot() {
    let (src, cache) = (scene(), StageCache::default());
    let mut s = DevelopSettings { spots: grid(12), ..Default::default() };
    let mut seed = 7u64;
    let mut next = |n: usize| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % n.max(1)
    };
    for step in 0..40 {
        let n = s.spots.len();
        match next(5) {
            0 => s.spots.insert(
                next(n + 1),
                spot(SpotMode::Heal, 0.1 + 0.8 * next(100) as f64 / 100.0, 0.1 + 0.8 * next(100) as f64 / 100.0, Some((0.04, 0.0))),
            ),
            1 if n > 0 => {
                s.spots.remove(next(n));
            }
            2 if n > 0 => s.spots[next(n)].points[0].x += 0.02,
            3 if n > 0 => s.spots[next(n)].size = 0.005 + next(30) as f64 / 1000.0,
            _ if n > 0 => s.spots[next(n)].source_offset = None,
            _ => {}
        }
        assert!(edit(&cache, &src, &s).1, "step {step}: {:?}", s.spots);
    }
}

#[test]
fn redo_draws_the_changed_spot_and_the_later_ones_reading_it() {
    let d = |write: [usize; 4], read: [usize; 4], x: f64| Some(Drawn { spot: spot(SpotMode::Heal, x, 0.5, None), at: Footprint { write, read } });
    let first = d([0, 0, 10, 10], [0, 0, 10, 10], 0.05); // under the changed one, earlier
    let a = d([5, 0, 15, 10], [5, 0, 15, 10], 0.1);
    let b = d([20, 0, 30, 10], [8, 0, 30, 10], 0.2); // reads a's box
    let c = d([40, 0, 50, 10], [40, 0, 50, 10], 0.3); // apart
    let e = d([12, 0, 22, 10], [12, 0, 22, 10], 0.4); // over a's and b's boxes, reads neither's change
    let old = vec![first.clone(), a, b.clone(), c.clone(), e.clone()];
    let a2 = d([6, 0, 16, 10], [6, 0, 16, 10], 0.11);
    let r = redo(&old, &[first.clone(), a2.clone(), b.clone(), c.clone(), e.clone()]);
    assert_eq!(r.steps, vec![Step::Draw(1), Step::Draw(2), Step::Draw(4)], "e reads what a drew");
    assert_eq!(r.undo, vec![4, 2, 1], "taken off last first; the first spot and c stay");
    let moved_far = d([60, 0, 70, 10], [60, 0, 70, 10], 0.7);
    let r = redo(&old, &[first.clone(), old[1].clone(), b.clone(), c.clone(), moved_far]);
    assert_eq!((r.undo, r.steps), (vec![4], vec![Step::Draw(4)]), "the last spot alone");
    let r = redo(&old, &[first.clone(), old[1].clone(), c.clone(), e.clone()]);
    assert_eq!((r.undo.clone(), r.steps.clone()), (vec![4, 2], vec![Step::Draw(3)]), "b deleted: e reads its box, c is left alone");
    assert_eq!(r.same, vec![Some(0), Some(1), Some(3), Some(4)]);
    let over = d([30, 0, 38, 10], [30, 0, 38, 10], 0.25); // drawn later where c's new source is, reading nothing that changed
    let longer = vec![first.clone(), old[1].clone(), b.clone(), c.clone(), e.clone(), over.clone()];
    let c2 = d([41, 0, 51, 10], [33, 0, 51, 10], 0.31);
    let r = redo(&longer, &[first.clone(), old[1].clone(), b.clone(), c2, e.clone(), over]);
    assert_eq!(
        r.steps,
        vec![Step::Draw(3), Step::Paste { i: 5, old: 5 }],
        "c reads from under a later spot: it is taken off, then put back from its result"
    );
    let r = redo(&old, &old);
    assert_eq!((r.undo.len(), r.steps.len()), (0, 0), "nothing changed");
}

#[test]
fn a_change_never_draws_the_spots_before_it() {
    let (src, cache) = (scene(), StageCache::default());
    let mut s = DevelopSettings {
        spots: vec![
            spot(SpotMode::Clone, 0.300, 0.50, Some((0.0, 0.1))),
            spot(SpotMode::Heal, 0.325, 0.50, Some((0.0, -0.1))),
            spot(SpotMode::Clone, 0.350, 0.50, Some((0.0, 0.1))),
        ],
        ..Default::default()
    };
    edit(&cache, &src, &s);
    s.spots[1].opacity = 70.0;
    assert_eq!(edit(&cache, &src, &s), (2, true), "the second and the third it overlaps, not the first under it");
    s.spots[0].feather = 0.0;
    assert_eq!(edit(&cache, &src, &s), (3, true), "the first and the two drawn over it");
    // a spot copying from where a later spot drew: that one is taken off and put back, not drawn
    s.spots.push(spot(SpotMode::Clone, 0.7, 0.3, Some((0.0, 0.2))));
    s.spots.push(spot(SpotMode::Heal, 0.7, 0.5, Some((0.1, 0.0))));
    edit(&cache, &src, &s);
    s.spots[3].opacity = 50.0;
    assert_eq!(edit(&cache, &src, &s), (1, true));
}

#[test]
fn the_full_size_keeps_its_spots_through_a_drags_drafts() {
    let (src, cache) = (scene(), StageCache::default());
    let info = SourceInfo::default();
    let mut s = DevelopSettings { spots: grid(30), ..Default::default() };
    edit(&cache, &src, &s);
    // a drag renders drafts at two other sizes: the stage cache holds two sizes, so the full size's entry goes
    s.spots[4].opacity = 70.0;
    for size in [400, 300] {
        let before = spots_drawn();
        render_cached(&src, &info, &s, &RenderRequest::fit(size, size), &cache);
        assert_eq!(spots_drawn() - before, 30, "a size drawn the first time draws them all");
    }
    s.spots[4].opacity = 60.0;
    assert_eq!(edit(&cache, &src, &s), (1, true), "the full render after the drag draws the spot it changed");
}

/// A spot's eye (`Spot::hidden`) is an edit like any other: hiding one of ten redraws only the later spot drawn over it,
/// with the result of drawing the ten without it; showing it again draws it and that spot.
#[test]
fn hiding_a_spot_redraws_only_the_spots_drawn_over_it() {
    let (src, cache) = (scene(), StageCache::default());
    let mut spots = grid(9);
    // the tenth spot is drawn over the fifth
    let mut over = spots[4].clone();
    over.points[0].x += 0.01;
    over.mode = SpotMode::Clone;
    spots.push(over);
    let mut s = DevelopSettings { spots, ..Default::default() };
    edit(&cache, &src, &s);
    s.spots[4].hidden = true;
    assert_eq!(edit(&cache, &src, &s), (1, true), "the spot drawn over it, not the other eight");
    let mut without = s.clone();
    without.spots.remove(4);
    let (info, req) = (SourceInfo::default(), RenderRequest::fit(480, 480));
    assert_eq!(render_cached(&src, &info, &s, &req, &cache).image.data, render(&src, &info, &without, &req).image.data, "as if it were not there");
    s.spots[2].hidden = true;
    assert_eq!(edit(&cache, &src, &s), (0, true), "a spot nothing is drawn over: its box comes back");
    s.spots[4].hidden = false;
    assert_eq!(edit(&cache, &src, &s), (2, true), "shown again: it and the spot drawn over it");
}

/// Review of #96 (F1): the Optics eye switches defringe off where it runs, before the spots. With only Defringe set,
/// toggling the eye kept the image before the spots (and with no spot edit the whole stage): defringe stayed on, or
/// off, outside what an edit redrew.
#[test]
fn the_optics_eye_redraws_the_image_under_the_spots() {
    let (src, cache) = (scene(), StageCache::default());
    let mut s = DevelopSettings { spots: grid(10), ..Default::default() };
    s.optics.defringe_purple_amount = 20.0;
    s.optics.defringe_green_amount = 20.0;
    s.optics.defringe_purple_hue_lo = 0.0;
    s.optics.defringe_purple_hue_hi = 100.0;
    s.optics.defringe_green_hue_lo = 0.0;
    s.optics.defringe_green_hue_hi = 100.0;
    let (info, req) = (SourceInfo::default(), RenderRequest::fit(480, 480));
    let without = DevelopSettings { optics: Default::default(), ..s.clone() };
    assert_ne!(render(&src, &info, &s, &req).image.data, render(&src, &info, &without, &req).image.data, "defringe changes the picture");
    assert!(edit(&cache, &src, &s).1);
    for on in [false, true] {
        if on {
            s.disabled_sections.retain(|x| x != "optics");
        } else {
            s.disabled_sections.push("optics".to_string());
        }
        assert!(edit(&cache, &src, &s).1, "the eye alone (on: {on})");
        s.spots[3].opacity -= 10.0;
        assert!(edit(&cache, &src, &s).1, "then a spot edit (on: {on})");
    }
}

/// Review of #96 (F3): a view's kept spots stay under their byte cap, however long the strokes; past it a render draws
/// more spots again and still gives what drawing every spot gives.
#[test]
fn kept_spots_stay_under_their_byte_cap() {
    let src = scene();
    // long strokes across the photo: each spot's boxes are most of the picture
    let long = |i: usize| Spot {
        mode: [SpotMode::Heal, SpotMode::Clone][i % 2],
        points: (0..20).map(|k| Point::new(0.05 + 0.045 * k as f64, 0.1 + 0.04 * i as f64)).collect(),
        size: 0.01,
        source_offset: Some(Point::new(0.0, 0.02)),
        ..Default::default()
    };
    let mut s = DevelopSettings { spots: (0..20).map(long).collect(), ..Default::default() };
    // a cap of about three images of the largest size here: the boxes of twenty long strokes go past it
    let cap = 480 * 320 * 12 * 3;
    let cache = StageCache { spots: super::KeptStore::with_cap(cap), ..Default::default() };
    let mut drawn = Vec::new();
    for size in [480, 400, 300, 480] {
        s.spots[size % 7].opacity -= 5.0;
        let (info, req) = (SourceInfo::default(), RenderRequest::fit(size, size));
        let before = spots_drawn();
        let kept = render_cached(&src, &info, &s, &req, &cache).image;
        drawn.push(spots_drawn() - before);
        assert_eq!(kept.data, render(&src, &info, &s, &req).image.data, "{size}");
        assert!(cache.spots.bytes() <= cap, "{} bytes kept, cap {cap}", cache.spots.bytes());
    }
    assert_eq!(drawn[3], 20, "the full size was forgotten to stay under the cap, so it drew every spot: {drawn:?}");
    let store = super::KeptStore::<u8>::with_cap(250);
    let a = Arc::new(Rgb32f::new(1, 1));
    store.put(&a, 1, Some(Arc::new(1)), 100);
    store.put(&a, 2, Some(Arc::new(2)), 100);
    store.put(&a, 3, Some(Arc::new(3)), 100);
    assert_eq!((store.get(&a, 1), store.bytes()), (None, 200), "the oldest size goes");
    store.put(&a, 4, Some(Arc::new(4)), 300);
    assert_eq!((store.get(&a, 4), store.bytes()), (None, 200), "one over the cap alone is not kept");
}

/// A photo with a Remove spot is still retouched in the cached render (which the GPU's scene-linear stage also
/// runs): the cached render matches the uncached one, and both show the retouch.
#[test]
fn a_spot_does_not_skip_portrait_retouching() {
    use lightcraft_develop::retouch::{Face, Input, NormRect, Part, PartMask};
    let (src, cache) = (scene(), StageCache::default());
    let area = NormRect::new(0.3, 0.2, 0.7, 0.8);
    let face = Face { masks: vec![(Part::FacialSkin, PartMask { plane: lightcraft_raster::Plane::filled(4, 4, 1.0), area })], ..Face::boxed(area) };
    let info = SourceInfo { retouch: Some(Arc::new(Input::new(vec![face]))), ..SourceInfo::default() };
    let mut s = DevelopSettings { spots: vec![spot(SpotMode::Remove, 0.1, 0.1, Some((0.0, 0.05)))], ..Default::default() };
    let req = RenderRequest::fit(480, 480);
    let plain = render(&src, &info, &s, &req).image;
    lightcraft_develop::controls::set(&mut s, "retouch.skinTone.faceLight", 100.0);
    let whole = render(&src, &info, &s, &req).image;
    assert_ne!(whole.data, plain.data, "the retouch shows");
    let kept = render_cached(&src, &info, &s, &req, &cache).image;
    assert!(kept.data == whole.data, "the cached render with a spot is retouched too");
}
