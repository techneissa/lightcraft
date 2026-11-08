//! A spot edit draws only the spots it reaches, and the result is the one drawing every spot gives (issue #82).

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, Spot, SpotMode};
use lightcraft_geom::Point;
use lightcraft_raster::Rgb32f;

use super::{Drawn, Footprint, redo, spots_drawn};
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
fn redo_follows_what_each_spot_reads() {
    let d = |write: [usize; 4], read: [usize; 4], x: f64| Some(Drawn { spot: spot(SpotMode::Heal, x, 0.5, None), at: Footprint { write, read } });
    let a = d([0, 0, 10, 10], [0, 0, 10, 10], 0.1);
    let b = d([20, 0, 30, 10], [5, 0, 30, 10], 0.2); // reads a's box
    let c = d([40, 0, 50, 10], [40, 0, 50, 10], 0.3);
    let old = vec![a.clone(), b.clone(), c.clone()];
    let a2 = d([1, 0, 11, 10], [1, 0, 11, 10], 0.11);
    let r = redo(&old, &[a2.clone(), b.clone(), c.clone()]).unwrap_or_else(|| panic!("a and b only"));
    assert_eq!(r.draw, vec![true, true, false]);
    assert_eq!(redo(&old, &old).map(|r| r.drawn()), Some(0), "nothing changed");
    assert_eq!(redo(&old, &[a2, b, d([5, 0, 45, 10], [5, 0, 45, 10], 0.31)]), None, "every spot: from the image before them");
    assert_eq!(redo(&[], &[c]), None);
}
