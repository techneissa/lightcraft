//! Fork feature (`fork/lama-fill.md`): spots filled by a learned fill. A fake fill stands in for LaMa (the real one
//! lives in the engine): what it gets and when, what happens when it says nothing, and that a window of a render matches
//! the whole render.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_develop::{DevelopSettings, Spot, SpotFill, SpotMode};
use lightcraft_geom::Point;
use lightcraft_raster::Rgb32f;

use crate::geometry::Frame;
use crate::learned_fill::{self, LearnedFill};
use crate::{PixelWindow, Quality, RenderRequest, SourceInfo, render, spots, spots_learned};

/// The installed fill is process-wide: tests that install one take turns.
static TURN: Mutex<()> = Mutex::new(());

/// Fills the hole with the mean of the crop's other pixels, and keeps what it was asked.
#[derive(Default)]
struct MeanFill {
    asked: Mutex<Vec<(usize, usize, bool)>>,
    /// What to answer instead of a fill.
    reply: Mutex<Option<Option<Vec<f32>>>>,
}

impl LearnedFill for MeanFill {
    fn fill(&self, rgb: &[f32], hole: &[f32], w: usize, h: usize, draft: bool) -> Option<Vec<f32>> {
        self.asked.lock().unwrap_or_else(PoisonError::into_inner).push((w, h, draft));
        if let Some(r) = self.reply.lock().unwrap_or_else(PoisonError::into_inner).clone() {
            return r;
        }
        let mut sum = [0.0f64; 3];
        let mut n = 0.0f64;
        for (px, m) in rgb.chunks_exact(3).zip(hole) {
            if *m <= 0.0 {
                for c in 0..3 {
                    sum[c] += px[c] as f64;
                }
                n += 1.0;
            }
        }
        let mean = sum.map(|s| (s / n.max(1.0)) as f32);
        let mut out = rgb.to_vec();
        for (px, m) in out.chunks_exact_mut(3).zip(hole) {
            if *m > 0.0 {
                px.copy_from_slice(&mean);
            }
        }
        Some(out)
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
        let done = spots_learned::apply(&mut img, std::slice::from_ref(&s), &frame, frame.px_per_long(40), &*fill, false);
        spots::apply_except(&mut img, &[s], &done, &frame, frame.px_per_long(40));
        assert!(img.data.iter().flatten().all(|v| v.is_finite()));
    }
    // no points at all, and an empty picture
    let empty = Spot { points: vec![], fill: SpotFill::Lama, ..Default::default() };
    let mut img = Rgb32f::from_fn(40, 30, |_, _| [0.5; 3]);
    assert_eq!(spots_learned::apply(&mut img, &[empty], &frame, 40.0, &*fill, false), vec![false]);
    let mut none = Rgb32f::from_fn(0, 0, |_, _| [0.0; 3]);
    assert!(spots_learned::apply(&mut none, &[spot(SpotMode::Fill, SpotFill::Lama)], &frame, 40.0, &*fill, false).iter().all(|d| !d));
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
