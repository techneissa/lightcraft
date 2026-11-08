//! Fork feature (`fork/lama-fill.md`): spots filled by a learned fill. A fake fill stands in for LaMa (the real one
//! lives in the engine): what it gets and when, what happens when it says nothing, and that a window of a render matches
//! the whole render.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_develop::{DevelopSettings, Spot, SpotFill, SpotMode};
use lightcraft_geom::Point;
use lightcraft_raster::Rgb32f;

use crate::geometry::Frame;
use crate::learned_fill::{self, FillRun, LearnedFill};
use crate::{PixelWindow, Quality, RenderRequest, SourceInfo, render, spots, spots_learned};

/// The installed fill is process-wide: tests that install one take turns.
static TURN: Mutex<()> = Mutex::new(());

/// Fills the hole with the mean of the crop's other pixels, and keeps what it was asked.
#[derive(Default)]
struct MeanFill {
    asked: Mutex<Vec<(usize, usize, bool)>>,
    /// What to answer instead of a fill.
    reply: Mutex<Option<Option<Vec<f32>>>>,
    /// Press ✕ on the render's row as soon as the first fill is asked for.
    stop_at_first: std::sync::atomic::AtomicBool,
}

impl LearnedFill for MeanFill {
    fn fill(&self, run: &FillRun, rgb: &[f32], hole: &[f32], w: usize, h: usize, draft: bool) -> Option<Arc<Vec<f32>>> {
        self.asked.lock().unwrap_or_else(PoisonError::into_inner).push((w, h, draft));
        if self.stop_at_first.load(std::sync::atomic::Ordering::Relaxed) {
            run.stop_flag().store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(r) = self.reply.lock().unwrap_or_else(PoisonError::into_inner).clone() {
            return r.map(Arc::new);
        }
        let mut sum = [0.0f64; 3];
        let mut n = 0.0f64;
        for (px, m) in rgb.as_chunks::<3>().0.iter().zip(hole) {
            if *m <= 0.0 {
                for c in 0..3 {
                    sum[c] += px[c] as f64;
                }
                n += 1.0;
            }
        }
        let mean = sum.map(|s| (s / n.max(1.0)) as f32);
        let mut out = rgb.to_vec();
        for (px, m) in out.as_chunks_mut::<3>().0.iter_mut().zip(hole) {
            if *m > 0.0 {
                *px = mean;
            }
        }
        Some(Arc::new(out))
    }
}

fn install<T>(fill: &Arc<MeanFill>, run: impl FnOnce() -> T) -> T {
    let _turn = TURN.lock().unwrap_or_else(PoisonError::into_inner);
    let provider: Arc<dyn LearnedFill> = fill.clone();
    learned_fill::install(Some(provider));
    let out = run();
    learned_fill::install(None);
    out
}

fn without_fill<T>(run: impl FnOnce() -> T) -> T {
    let _turn = TURN.lock().unwrap_or_else(PoisonError::into_inner);
    learned_fill::install(None);
    run()
}

fn spot(mode: SpotMode, fill: SpotFill) -> Spot {
    Spot { mode, points: vec![Point::new(0.5, 0.5)], size: 0.06, feather: 0.0, fill, source_offset: Some(Point::new(0.2, 0.0)), ..Default::default() }
}

fn with(spots: Vec<Spot>) -> DevelopSettings {
    DevelopSettings { spots, ..Default::default() }
}

/// A picture with a stripe pattern and a black blot where the spot is.
fn scene() -> Rgb32f {
    Rgb32f::from_fn(200, 150, |x, y| {
        if (x as f32 - 100.0).hypot(y as f32 - 75.0) < 8.0 {
            [0.0; 3]
        } else if (x / 10) % 2 == 0 {
            [0.2, 0.3, 0.4]
        } else {
            [0.6, 0.5, 0.3]
        }
    })
}

fn out(s: &DevelopSettings, req: &RenderRequest) -> lightcraft_raster::Rgba8 {
    render(&scene(), &SourceInfo::default(), s, req).image
}

#[test]
fn a_lama_spot_takes_the_learned_fill_and_leaves_the_rest_alone() {
    let fill = Arc::new(MeanFill::default());
    let req = RenderRequest::fit(400, 300);
    let (plain, learned) = (out(&with(vec![]), &req), install(&fill, || out(&with(vec![spot(SpotMode::Fill, SpotFill::Lama)]), &req)));
    // the blot is replaced by a flat colour, the picture far from it is the same
    assert_ne!(plain.get(200, 150), learned.get(200, 150));
    assert_eq!(plain.get(10, 10), learned.get(10, 10));
    assert_eq!(plain.get(390, 290), learned.get(390, 290));
    let asked = fill.asked.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(asked.len(), 1, "one spot asks once: {asked:?}");
    assert!(!asked[0].2, "a full render is not a draft");
}

#[test]
fn a_classic_spot_never_asks_the_learned_fill() {
    let fill = Arc::new(MeanFill::default());
    let req = RenderRequest::fit(400, 300);
    install(&fill, || out(&with(vec![spot(SpotMode::Fill, SpotFill::Classic)]), &req));
    assert!(fill.asked.lock().unwrap_or_else(PoisonError::into_inner).is_empty());
}

#[test]
fn heal_and_clone_ignore_the_fill_choice() {
    let fill = Arc::new(MeanFill::default());
    let req = RenderRequest::fit(400, 300);
    for mode in [SpotMode::Heal, SpotMode::Clone] {
        let (a, b) = (out(&with(vec![spot(mode, SpotFill::Lama)]), &req), out(&with(vec![spot(mode, SpotFill::Classic)]), &req));
        let again = install(&fill, || out(&with(vec![spot(mode, SpotFill::Lama)]), &req));
        assert_eq!(a.data, b.data, "{mode:?} with LaMa is the same spot");
        assert_eq!(a.data, again.data);
    }
    assert!(fill.asked.lock().unwrap_or_else(PoisonError::into_inner).is_empty());
}

#[test]
fn without_a_fill_the_spot_shows_the_classic_one() {
    let req = RenderRequest::fit(400, 300);
    let classic = out(&with(vec![spot(SpotMode::Fill, SpotFill::Classic)]), &req);
    let lama = without_fill(|| out(&with(vec![spot(SpotMode::Fill, SpotFill::Lama)]), &req));
    assert_eq!(classic.data, lama.data);
}

#[test]
fn a_fill_that_says_nothing_or_nonsense_leaves_the_classic_fill() {
    let req = RenderRequest::fit(400, 300);
    let classic = out(&with(vec![spot(SpotMode::Fill, SpotFill::Classic)]), &req);
    for reply in [None, Some(vec![0.5; 3]), Some(vec![f32::NAN; 3 * 40 * 40]), Some(Vec::new())] {
        let fill = Arc::new(MeanFill::default());
        *fill.reply.lock().unwrap_or_else(PoisonError::into_inner) = Some(reply.clone());
        let lama = install(&fill, || out(&with(vec![spot(SpotMode::Fill, SpotFill::Lama)]), &req));
        assert_eq!(classic.data, lama.data, "reply {:?}", reply.map(|v| v.len()));
    }
}

#[test]
fn a_draft_render_tells_the_fill_so_and_a_full_one_does_not_reuse_its_image() {
    let fill = Arc::new(MeanFill::default());
    let s = with(vec![spot(SpotMode::Fill, SpotFill::Lama)]);
    let draft = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(400, 300) };
    // (the draft's answer is "no fill yet": the Classic one is drawn)
    *fill.reply.lock().unwrap_or_else(PoisonError::into_inner) = Some(None);
    let src = Arc::new(scene());
    let cache = crate::StageCache::default();
    let (d, f) = install(&fill, || {
        let d = crate::render_cached(&src, &SourceInfo::default(), &s, &draft, &cache).image;
        *fill.reply.lock().unwrap_or_else(PoisonError::into_inner) = None;
        let f = crate::render_cached(&src, &SourceInfo::default(), &s, &RenderRequest::fit(400, 300), &cache).image;
        (d, f)
    });
    let asked = fill.asked.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(asked.iter().map(|a| a.2).collect::<Vec<_>>(), vec![true, false], "{asked:?}");
    assert_ne!(d.data, f.data, "the full render drew the learned fill, not the draft's Classic one");
}

#[test]
fn the_fill_does_not_depend_on_the_white_balance() {
    let fill = Arc::new(MeanFill::default());
    let req = RenderRequest::fit(400, 300);
    let info = SourceInfo { raw: true, ..Default::default() };
    for temp in [3500.0, 5500.0, 8000.0] {
        let mut s = with(vec![spot(SpotMode::Fill, SpotFill::Lama)]);
        s.wb.temp = temp;
        install(&fill, || render(&scene(), &info, &s, &req));
    }
    // the same pixels went in each time: the learned fill sees the picture before the white balance
    let asked = fill.asked.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(asked.len(), 3);
    assert!(asked.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn a_window_matches_the_whole_render() {
    let fill = Arc::new(MeanFill::default());
    let s = with(vec![Spot { points: vec![Point::new(0.5, 0.5), Point::new(0.56, 0.5)], ..spot(SpotMode::Fill, SpotFill::Lama) }]);
    let full_req = RenderRequest::fit(400, 300);
    let win = PixelWindow { x: 120, y: 90, w: 160, h: 120 };
    let (whole, part) = install(&fill, || (out(&s, &full_req), out(&s, &RenderRequest { window: Some(win), ..full_req })));
    let mut worst = 0u8;
    for y in 0..win.h {
        for x in 0..win.w {
            let (a, b) = (whole.get(win.x + x, win.y + y), part.get(x, y));
            worst = worst.max((0..3).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0));
        }
    }
    assert!(worst <= 1, "the window differs from the whole render by up to {worst}");
}

#[test]
fn feather_and_opacity_blend_the_learned_fill() {
    let fill = Arc::new(MeanFill::default());
    let req = RenderRequest::fit(400, 300);
    let none = install(&fill, || out(&with(vec![Spot { opacity: 0.0, ..spot(SpotMode::Fill, SpotFill::Lama) }]), &req));
    assert_eq!(none.data, out(&with(vec![]), &req).data, "no opacity draws nothing");
    let half = install(&fill, || out(&with(vec![Spot { opacity: 50.0, ..spot(SpotMode::Fill, SpotFill::Lama) }]), &req));
    let (plain, full) = (out(&with(vec![]), &req), install(&fill, || out(&with(vec![spot(SpotMode::Fill, SpotFill::Lama)]), &req)));
    let (a, b, c) = (plain.get(200, 150)[0], half.get(200, 150)[0], full.get(200, 150)[0]);
    assert!((a.min(c)..=a.max(c)).contains(&b) && b != a && b != c, "{a} {b} {c}");
}

#[test]
fn hostile_spots_do_not_panic() {
    let fill = Arc::new(MeanFill::default());
    let frame = Frame::new(40, 30, &DevelopSettings::default(), true);
    for (p, size) in [((0.0, 0.0), 2.0), ((-5.0, 9.0), 0.1), ((0.5, 0.5), 0.0), ((f64::NAN, 0.5), 0.1), ((0.5, 0.5), f64::INFINITY)] {
        let mut img = Rgb32f::from_fn(40, 30, |x, _| [x as f32 / 40.0; 3]);
        let s = Spot { mode: SpotMode::Fill, points: vec![Point::new(p.0, p.1)], size, fill: SpotFill::Lama, ..Default::default() };
        let done = spots_learned::apply(&mut img, std::slice::from_ref(&s), &frame, frame.px_per_long(40), 40.0, &*fill, false);
        spots::apply_except(&mut img, &[s], &done, &frame, frame.px_per_long(40));
        assert!(img.data.iter().flatten().all(|v| v.is_finite()));
    }
    // no points at all, and an empty picture
    let empty = Spot { points: vec![], fill: SpotFill::Lama, ..Default::default() };
    let mut img = Rgb32f::from_fn(40, 30, |_, _| [0.5; 3]);
    assert_eq!(spots_learned::apply(&mut img, &[empty], &frame, 40.0, 40.0, &*fill, false), vec![false]);
    let mut none = Rgb32f::from_fn(0, 0, |_, _| [0.0; 3]);
    assert!(spots_learned::apply(&mut none, &[spot(SpotMode::Fill, SpotFill::Lama)], &frame, 40.0, 40.0, &*fill, false).iter().all(|d| !d));
}

#[test]
fn a_spot_without_a_fill_in_its_file_is_classic() {
    let old = r#"{"mode":"fill","points":[{"x":0.5,"y":0.5}],"size":0.03,"feather":50.0,"opacity":100.0,"source_offset":null}"#;
    let s: Spot = serde_json::from_str(old).expect("an older file's spot");
    assert_eq!(s.fill, SpotFill::Classic);
    assert!(!serde_json::to_string(&s).expect("json").contains("\"fill\":"), "Classic is not written, so files stay as they were");
    let lama = Spot { fill: SpotFill::Lama, ..s };
    let json = serde_json::to_string(&lama).expect("json");
    assert!(json.contains(r#""fill":"lama""#), "{json}");
    assert_eq!(serde_json::from_str::<Spot>(&json).expect("round trip"), lama);
}

#[test]
fn stopping_the_fill_stops_the_whole_render_not_one_spot() {
    let fill = Arc::new(MeanFill::default());
    fill.stop_at_first.store(true, std::sync::atomic::Ordering::Relaxed);
    let spots: Vec<Spot> =
        [0.2, 0.4, 0.6, 0.8].iter().map(|x| Spot { points: vec![Point::new(*x, 0.5)], ..spot(SpotMode::Fill, SpotFill::Lama) }).collect();
    let req = RenderRequest::fit(400, 300);
    let stopped = install(&fill, || out(&with(spots.clone()), &req));
    let asked = fill.asked.lock().unwrap_or_else(PoisonError::into_inner).len();
    assert_eq!(asked, 1, "the first spot was stopped; the other three must not start");
    // the spots that did not get a learned fill show the Classic one
    let classic = out(&with(spots.iter().map(|s| Spot { fill: SpotFill::Classic, ..s.clone() }).collect()), &req);
    assert_eq!(stopped.get(320, 150), classic.get(320, 150));
    // a later render starts afresh
    fill.stop_at_first.store(false, std::sync::atomic::Ordering::Relaxed);
    install(&fill, || out(&with(spots.clone()), &req));
    assert_eq!(fill.asked.lock().unwrap_or_else(PoisonError::into_inner).len(), 1 + 4);
}

#[test]
fn a_preview_and_a_full_size_render_give_the_fill_the_same_picture() {
    let s = with(vec![spot(SpotMode::Fill, SpotFill::Lama)]);
    let mut seen = Vec::new();
    let mut centre = Vec::new();
    for (w, h) in [(100, 75), (200, 150), (800, 600)] {
        let fill = Arc::new(MeanFill::default());
        let img = install(&fill, || out(&s, &RenderRequest::fit(w, h)));
        let asked = fill.asked.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert_eq!(asked.len(), 1, "{w}×{h}: {asked:?}");
        seen.push((asked[0].0, asked[0].1));
        centre.push(img.get(w / 2, h / 2));
    }
    // the same crop at the same size, to the rounding of the spot's box to whole render pixels (a few percent in the
    // smallest render), whatever the render's size; without the scaling they differ by the render's ratio, 4 to 1
    for (a, b) in seen.iter().zip(&seen[1..]) {
        assert!(a.0.abs_diff(b.0) * 100 <= a.0.max(b.0) * 8 && a.1.abs_diff(b.1) * 100 <= a.1.max(b.1) * 8, "the fill saw {seen:?}");
    }
    // and so the hole comes out alike
    for c in &centre[1..] {
        assert!((0..3).all(|k| c[k].abs_diff(centre[0][k]) <= 8), "{centre:?}");
    }
}

#[test]
fn a_big_hole_is_handed_over_at_a_capped_size() {
    let fill = Arc::new(MeanFill::default());
    // an original reported ten times the source: the crop would be far over the cap
    let info = SourceInfo { native_long: Some(200 * 40), ..Default::default() };
    install(&fill, || render(&scene(), &info, &with(vec![spot(SpotMode::Fill, SpotFill::Lama)]), &RenderRequest::fit(400, 300)));
    let asked = fill.asked.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].0.max(asked[0].1) <= spots_learned::FILL_MAX_SIDE, "{asked:?}");
}

/// Issue #82: a LaMa spot asks its fill only when it is new or changed (or reads what a changed spot drew), so adding the
/// 13th spot asks once, not thirteen times; a drag's draft that got no fill is asked again by the full render.
#[test]
fn a_lama_spot_edit_asks_the_fill_only_for_the_spots_it_reaches() {
    let fill = Arc::new(MeanFill::default());
    let at = |x: f64, y: f64| Spot { points: vec![Point::new(x, y)], size: 0.02, ..spot(SpotMode::Remove, SpotFill::Lama) };
    let mut s = with((0..12).map(|i| at(0.1 + 0.2 * (i % 5) as f64, 0.15 + 0.3 * (i / 5) as f64)).collect());
    let src = Arc::new(scene());
    let (info, req) = (SourceInfo::default(), RenderRequest::fit(400, 300));
    let cache = crate::StageCache::default();
    let asked = || fill.asked.lock().unwrap_or_else(PoisonError::into_inner).len();
    let edit = |s: &DevelopSettings, req: &RenderRequest| {
        let before = asked();
        let kept = crate::render_cached(&src, &info, s, req, &cache).image;
        let n = asked() - before;
        (n, kept.data == render(&src, &info, s, req).image.data)
    };
    install(&fill, || {
        assert_eq!(edit(&s, &req), (12, true));
        s.spots.push(at(0.5, 0.92));
        assert_eq!(edit(&s, &req), (1, true), "the 13th spot alone");
        s.spots[3].points[0].x += 0.01;
        assert_eq!(edit(&s, &req), (1, true), "a moved spot alone");
        s.light.exposure = 1.0;
        s.wb.temp += 800.0;
        s.wb.mode = lightcraft_develop::WbMode::Custom;
        assert_eq!(edit(&s, &req), (0, true), "the white balance and tone ask nothing");
        // a drag's draft gets no fill for the changed spot; the full render after it asks for that one spot again
        *fill.reply.lock().unwrap_or_else(PoisonError::into_inner) = Some(None);
        s.spots[5].opacity = 60.0;
        let draft = RenderRequest { quality: Quality::Draft, ..req };
        assert_eq!(edit(&s, &draft).0, 1);
        *fill.reply.lock().unwrap_or_else(PoisonError::into_inner) = None;
        assert_eq!(edit(&s, &req), (1, true), "the full render asks again for the spot the draft could not fill");
    });
}

/// Fork feature (`fork/object-remove.md`): a LaMa spot in an object's shape is filled over its whole shape, by a
/// whole render (an export) and by the incremental path a view's renders take alike, not in a circle around its pin.
#[test]
fn a_shaped_lama_spot_is_filled_over_its_whole_shape_in_the_preview_and_the_export() {
    let fill = Arc::new(MeanFill::default());
    // a shape over the middle of the picture, [0.3, 0.3]–[0.7, 0.7], much bigger than the spot's radius
    let seg = lightcraft_develop::SegMask::from_logits_in(16, &[8.0; 256], [0.3, 0.3, 0.7, 0.7]);
    let shaped = Spot {
        points: vec![Point::new(0.5, 0.5)],
        size: 0.005,
        area: Some(lightcraft_develop::SpotArea::new("object", seg)),
        ..spot(SpotMode::Remove, SpotFill::Lama)
    };
    let s = with(vec![shaped]);
    let src = Arc::new(scene());
    let (info, req) = (SourceInfo::default(), RenderRequest::fit(400, 300));
    let cache = crate::StageCache::default();
    install(&fill, || {
        let plain = render(&src, &info, &with(vec![]), &req).image;
        let export = render(&src, &info, &s, &req).image;
        let preview = crate::render_cached(&src, &info, &s, &req, &cache).image;
        assert_eq!(preview.data, export.data, "the preview draws what the export draws");
        // near the shape's corners, far from the pin: filled with the fill's flat colour like its middle
        for (x, y) in [(130, 100), (270, 100), (130, 200), (270, 200)] {
            assert_ne!(export.get(x, y), plain.get(x, y), "({x}, {y}) is inside the shape");
            assert_eq!(export.get(x, y), export.get(200, 150), "({x}, {y}) has the fill");
        }
        // and outside the shape and its margin, nothing changed
        assert_eq!(export.get(20, 20), plain.get(20, 20));
        assert_eq!(export.get(380, 280), plain.get(380, 280));
    });
}
