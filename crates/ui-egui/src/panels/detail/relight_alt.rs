//! Fork (issue #294, `fork/relight.md`): what ⌥ shows while a Relight control is dragged (a slider, the Near–Far
//! track, the light's handle on the photo or in the depth box): where the light reaches and how it hits each surface,
//! drawn over the loupe with the photo dimmed a little under it. Two looks, picked by the switch under the Near–Far
//! track ([`crate::state::RelightLook`]):
//!
//! - **Crosses**: a grid of + marks lying on their surfaces (foreshortened by the field's normal), sized and brightened
//!   by the light's gain there, tinted by the light's colour; hollow rings, also on their surfaces, where Darken
//!   Surroundings darkens.
//! - **Contours**: lines of equal gain at fixed stop levels, labelled in stops, and a dashed line where darkening
//!   starts.
//!
//! Drawing never slows the drag. The geometry ([`lightcraft_relight::overlay`], in normalized photo coordinates)
//! is built from the shading the render already made (`lightcraft_pipeline::relight::shading_for`: once per light,
//! shared with the render) by one worker thread that takes the newest request only; the UI draws the newest ready
//! geometry as egui shapes, the one for the light on screen when it is ready, else the last one of the same photo
//! (a frame or two behind the drag), never one of another photo.

use std::sync::{Condvar, Mutex, MutexGuard, Once, OnceLock, PoisonError};

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_geom::Point;
use lightcraft_pipeline::relight::{Input, Scene, scene_of, shading_for, shading_key};
use lightcraft_relight::overlay::{self, Contours, Marks};

use super::geometry::CanvasMap;
use super::{DevelopSettings, LightcraftApp};
use crate::state::{RelightLook, RightPanel};
use crate::theme::Tokens;

/// How far apart the crosses look on screen (points).
const SPACING: f32 = 30.0;
/// The photo's dimming under the overlay (black, of 255).
const DIM: u8 = 72;
/// The grid sizes (spots along the photo's long side) a zoom snaps to, so a pinch doesn't rebuild the grid at every step.
const LADDER: [usize; 11] = [12, 18, 24, 36, 48, 72, 96, 144, 192, 288, 400];

/// Whether the preview shows now: ⌥ is held while a Relight control is dragged in the Edit panel.
pub(super) fn active(app: &LightcraftApp) -> bool {
    app.ui.alt_held
        && !app.ui.fullscreen
        && app.ui.right == RightPanel::Edit
        && app.ui.dragging_control.as_deref().is_some_and(|c| c.starts_with("relight."))
}

/// What a geometry is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum What {
    Crosses { grid: usize },
    Contours,
}

/// What names a geometry: the photo's field, the lights' shading of it (every light's numbers, Darken Surroundings and
/// the exposure) and what is made of it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    field: u64,
    lights: u64,
    shape: What,
}

impl Key {
    fn new(input: &Input, scene: &Scene, ev: f64, shape: What) -> Key {
        Key { field: input.key, lights: shading_key(input, scene, ev), shape }
    }
}

enum Geometry {
    Marks(Marks),
    Contours(Contours),
}

struct Request {
    key: Key,
    input: Input,
    scene: Scene,
    ev: f64,
    ctx: egui::Context,
}

struct Ready {
    key: Key,
    geometry: std::sync::Arc<Geometry>,
}

#[derive(Default)]
struct Slot {
    /// The newest request, not taken yet.
    pending: Option<Request>,
    /// The key being built.
    busy: Option<Key>,
    ready: Option<Ready>,
    /// The key of the last build that panicked: it is not asked for again (every frame would panic again).
    failed: Option<Key>,
}

#[derive(Default)]
struct Store {
    slot: Mutex<Slot>,
    wake: Condvar,
}

fn store() -> &'static Store {
    static STORE: OnceLock<Store> = OnceLock::new();
    STORE.get_or_init(Store::default)
}

fn lock(s: &Store) -> MutexGuard<'_, Slot> {
    s.slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The geometry of `req`: the lights' shading (the render's, when it made it already) and what is drawn of it; with
/// the milliseconds each took.
fn build(req: &Request) -> (Geometry, [f64; 2]) {
    let t = std::time::Instant::now();
    let (_, shading) = shading_for(&req.input, &req.scene, req.ev);
    let shaded = t.elapsed().as_secs_f64() * 1e3;
    let geometry = match req.key.shape {
        What::Crosses { grid } => Geometry::Marks(overlay::marks(&req.input.field, &shading, grid)),
        What::Contours => Geometry::Contours(overlay::contours(&shading)),
    };
    (geometry, [shaded, t.elapsed().as_secs_f64() * 1e3 - shaded])
}

/// The worker: builds the newest request, over and over. A panic in a build leaves that geometry out and the worker alive.
fn work() {
    let s = store();
    loop {
        let req = {
            let mut g = lock(s);
            loop {
                if let Some(r) = g.pending.take() {
                    g.busy = Some(r.key.clone());
                    break r;
                }
                g = s.wake.wait(g).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&req))).ok();
        if let (true, Some((g, [shading, geometry]))) = (lightcraft_pipeline::profiling(), &made) {
            let (what, n) = match g {
                Geometry::Marks(m) => ("crosses", m.marks.len()),
                Geometry::Contours(c) => ("contours", c.lines.iter().map(|l| l.points.len()).sum()),
            };
            eprintln!("    relight overlay {what} ({n} points): shading {shading:.1} ms (0 when the render made it), geometry {geometry:.1} ms");
        }
        {
            let mut g = lock(s);
            g.busy = None;
            match made {
                Some((m, _)) => g.ready = Some(Ready { key: req.key.clone(), geometry: std::sync::Arc::new(m) }),
                None => g.failed = Some(req.key.clone()),
            }
        }
        req.ctx.request_repaint();
    }
}

/// The geometry to draw for `key`, asking for it when it isn't ready: its own when ready, else the last ready one of
/// the same photo and the same look (`None` when there is none).
fn geometry_for(key: &Key, ask: impl FnOnce() -> Request) -> Option<std::sync::Arc<Geometry>> {
    static WORKER: Once = Once::new();
    let s = store();
    let mut g = lock(s);
    if let Some(r) = g.ready.as_ref().filter(|r| r.key == *key) {
        return Some(r.geometry.clone());
    }
    let waiting = g.pending.as_ref().is_some_and(|p| p.key == *key) || g.busy.as_ref() == Some(key) || g.failed.as_ref() == Some(key);
    if !waiting {
        g.pending = Some(ask());
        s.wake.notify_one();
    }
    let same_look = |r: &&Ready| r.key.field == key.field && std::mem::discriminant(&r.key.shape) == std::mem::discriminant(&key.shape);
    let stale = g.ready.as_ref().filter(same_look).map(|r| r.geometry.clone());
    drop(g);
    WORKER.call_once(|| {
        // (a thread that can't start leaves the overlay out)
        let _ = std::thread::Builder::new().name("relight-overlay".into()).spawn(work);
    });
    stale
}

/// The grid size (spots along the long side) that looks `SPACING` apart with the photo `long` points long on screen.
fn grid_for(long: f32) -> usize {
    let want = if long.is_finite() { long / SPACING } else { 0.0 };
    LADDER.iter().copied().min_by(|a, b| (*a as f32 - want).abs().total_cmp(&(*b as f32 - want).abs())).unwrap_or(LADDER[4])
}

/// The marks' colour: amber, a little deeper for a warm light and paler for a cool one.
fn tint(warmth: f64, alpha: f32) -> Color32 {
    let w = (warmth / 100.0).clamp(-1.0, 1.0) as f32;
    let (g, b) = if w >= 0.0 { (196.0 - 46.0 * w, 90.0 - 50.0 * w) } else { (196.0 - 24.0 * w, 90.0 - 110.0 * w) };
    let r = if w >= 0.0 { 255.0 } else { 255.0 + 40.0 * w };
    Color32::from_rgba_unmultiplied(r as u8, g as u8, b as u8, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

/// The photo's axes on screen: the unit directions of normalized x and y at the photo's middle (the field's own x
/// and y, as a crop's rotation and a flip turn them).
fn screen_axes(map: &CanvasMap) -> [egui::Vec2; 2] {
    let e = 1e-3;
    let c = map.screen(Point::new(0.5, 0.5));
    let dir = |p: Pos2, fallback: egui::Vec2| {
        let d = p - c;
        if d.length() > 1e-6 { d.normalized() } else { fallback }
    };
    [dir(map.screen(Point::new(0.5 + e, 0.5)), vec2(1.0, 0.0)), dir(map.screen(Point::new(0.5, 0.5 + e)), vec2(0.0, 1.0))]
}

/// Draw the preview over the photo: the photo dimmed, then the chosen look.
pub(super) fn draw(app: &LightcraftApp, ui: &egui::Ui, p: &Painter, map: &CanvasMap, d: &DevelopSettings, id: PhotoId, clip: Rect) {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    let (Some(made), Some(scene)) = (app.session.relight_made(id), scene_of(d)) else { return };
    let map = map.for_mask(true);
    let p = p.with_clip_rect(clip);
    p.rect_filled(clip, 0.0, Color32::from_black_alpha(DIM));
    let (a, b) = (map.screen(Point::new(0.0, 0.0)), map.screen(Point::new(1.0, 1.0)));
    let long = (b.x - a.x).abs().max((b.y - a.y).abs());
    let shape = match app.ui.relight_look {
        RelightLook::Crosses => What::Crosses { grid: grid_for(long) },
        RelightLook::Contours => What::Contours,
    };
    let ev = d.light.exposure;
    let key = Key::new(&made.input, &scene, ev, shape);
    let ctx = ui.ctx().clone();
    let input = made.input.clone();
    let Some(g) = geometry_for(&key, || Request { key: key.clone(), input, scene, ev, ctx }) else { return };
    let warmth = mean_warmth(d);
    let t = lightcraft_pipeline::profiling().then(std::time::Instant::now);
    match &*g {
        Geometry::Marks(m) => draw_marks(&p, &map, m, warmth, clip),
        Geometry::Contours(c) => draw_contours(ui.ctx(), &p, &map, c, warmth, clip),
    }
    mark_selected(app, ui, &p, &map, d, id);
    if let Some(t) = t {
        eprintln!("    relight overlay drawn {:?} (UI thread, shapes made): {:.2} ms", key.shape, t.elapsed().as_secs_f64() * 1e3);
    }
}

/// The selected light's name beside its handle, so the combined light's picture says whose control is being dragged
/// (the handle itself, with its ring, is drawn above the dimming by the handles' own overlay).
fn mark_selected(app: &LightcraftApp, ui: &egui::Ui, p: &Painter, map: &CanvasMap, d: &DevelopSettings, id: PhotoId) {
    let sel = app.session.relight_selected(id);
    let Some(l) = d.relight.lights.get(sel) else { return };
    let t = Tokens::get(ui.ctx());
    // (a rim and a fill have no place: their name goes in the photo's corner)
    let (at, text) = if l.kind.has_place() {
        let Some(at) = map.try_screen(Point::new(l.x, l.y)) else { return };
        (at, crate::i18n::tr_format!("Light {n}", n = sel + 1))
    } else {
        let name = crate::i18n::tr(["Point", "Spot", "Sun / Window", "Softbox", "Rim", "Fill"].get(l.kind.index()).copied().unwrap_or("Point"));
        (p.clip_rect().left_top() + vec2(70.0, 20.0), crate::i18n::tr_format!("Light {n}: {kind}", n = sel + 1, kind = name))
    };
    let galley = p.layout_no_wrap(text, t.font(11.0), Color32::WHITE);
    // (just under the handle, inside its ring)
    let r = Rect::from_min_size(at + vec2(-(galley.size().x + 10.0) / 2.0, 14.0), galley.size() + vec2(10.0, 4.0));
    p.rect_filled(r, 3.0, Color32::from_black_alpha(160));
    p.galley(r.min + vec2(5.0, 2.0), galley, Color32::WHITE);
}

/// The lights' Warmth, averaged by Amount: what tints the marks and lines of the combined light.
fn mean_warmth(d: &DevelopSettings) -> f64 {
    let (sum, total) = d.relight.lights.iter().filter(|l| l.amount > 0.0).fold((0.0, 0.0), |(s, t), l| (s + l.warmth * l.amount, t + l.amount));
    if total > 0.0 { sum / total } else { 0.0 }
}

/// One mark's arm length as a fraction of half the spacing: the gain `stops` (absolute: [`overlay::peak_stops`] is the
/// most the Amount gives) from a small spot to the whole cell.
fn reach(stops: f32) -> f32 {
    0.08 + 0.92 * share_of_peak(stops)
}

/// `stops` (either way) as a share of the light's peak, 0..1.
fn share_of_peak(stops: f32) -> f32 {
    (stops.abs() / overlay::peak_stops()).clamp(0.0, 1.0)
}

fn draw_marks(p: &Painter, map: &CanvasMap, m: &Marks, warmth: f64, clip: Rect) {
    if m.cols == 0 || m.rows == 0 {
        return;
    }
    let (a, b) = (map.screen(Point::new(0.0, 0.0)), map.screen(Point::new(1.0, 1.0)));
    let cell = (b - a).length() / ((m.cols * m.cols + m.rows * m.rows) as f32).sqrt();
    let half = cell * 0.5;
    let [ex, ey] = screen_axes(map);
    let mut shapes: Vec<Shape> = Vec::with_capacity(m.marks.len() * 2);
    for k in &m.marks {
        let Some(at) = map.try_screen(Point::new(f64::from(k.u), f64::from(k.v))) else { continue };
        if !clip.expand(half).contains(at) {
            continue;
        }
        let r = half * reach(k.stops);
        let dirs = k.axes.map(|a| ex * a[0] + ey * a[1]);
        let s = share_of_peak(k.stops);
        let colour = tint(warmth, 0.2 + 0.8 * s.powf(0.7));
        if k.stops > 0.0 {
            let stroke = Stroke::new(if s > 0.4 { 1.6 } else { 1.2 }, colour);
            for dir in dirs {
                shapes.push(Shape::line_segment([at - dir * r, at + dir * r], stroke));
            }
        } else {
            // a hollow ring on the surface
            let ring: Vec<Pos2> = (0..14)
                .map(|i| {
                    let (sin, cos) = (i as f32 * std::f32::consts::TAU / 14.0).sin_cos();
                    at + dirs[0] * (cos * r * 0.8) + dirs[1] * (sin * r * 0.8)
                })
                .collect();
            shapes.push(Shape::closed_line(ring, Stroke::new(1.1, colour)));
        }
    }
    p.extend(shapes);
}

/// A stop value as a label: `+0.35`, `−0.5` (with the minus sign), no trailing zeros.
fn stops_label(level: f32) -> String {
    let text = format!("{:.2}", level.abs());
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{}{}", if level < 0.0 { '−' } else { '+' }, text)
}

fn draw_contours(ctx: &egui::Context, p: &Painter, map: &CanvasMap, c: &Contours, warmth: f64, clip: Rect) {
    let t = Tokens::get(ctx);
    // each line on screen, with its length in the clip (the longest line of a level carries its label)
    struct Drawn {
        level: f32,
        pts: Vec<Pos2>,
        seen: f32,
    }
    let mut drawn: Vec<Drawn> = Vec::new();
    for l in &c.lines {
        // (a line breaks where the warp can't be undone, as the marks skip such a spot)
        let mut pieces: Vec<Vec<Pos2>> = vec![Vec::new()];
        for q in &l.points {
            match map.try_screen(Point::new(f64::from(q[0]), f64::from(q[1]))) {
                Some(at) => pieces.last_mut().map(|v| v.push(at)).unwrap_or_default(),
                None if pieces.last().is_some_and(|v| !v.is_empty()) => pieces.push(Vec::new()),
                None => {}
            }
        }
        for pts in pieces {
            let (mut seen, mut bounds) = (0.0f32, Rect::NOTHING);
            for w in pts.windows(2) {
                bounds.extend_with(w[0]);
                if clip.contains(w[0]) && clip.contains(w[1]) {
                    seen += (w[1] - w[0]).length();
                }
            }
            if pts.len() >= 2 && bounds.intersects(clip) {
                drawn.push(Drawn { level: l.level, pts, seen });
            }
        }
    }
    for l in &drawn {
        let strength = (l.level.abs() / 1.0).clamp(0.0, 1.0);
        let colour = tint(warmth, 0.65 + 0.35 * strength);
        let (halo, line) = (Stroke::new(3.2, Color32::from_black_alpha(80)), Stroke::new(1.5, colour));
        if l.level == 0.0 {
            // where darkening starts
            p.extend(Shape::dashed_line(&l.pts, halo, 7.0, 6.0));
            p.extend(Shape::dashed_line(&l.pts, Stroke::new(1.5, Color32::from_rgb(255, 232, 190)), 7.0, 6.0));
        } else {
            p.add(Shape::line(l.pts.clone(), halo));
            p.add(Shape::line(l.pts.clone(), line));
        }
    }
    // a label for each level (not the dashed line), on its longest line in view, at its point nearest the middle
    let mut levels: Vec<f32> = drawn.iter().map(|d| d.level).filter(|v| *v != 0.0).collect();
    levels.sort_by(f32::total_cmp);
    levels.dedup();
    let mut placed: Vec<Rect> = Vec::new();
    for level in levels {
        let Some(best) = drawn.iter().filter(|d| d.level == level).max_by(|a, b| a.seen.total_cmp(&b.seen)) else { continue };
        let inner = clip.shrink(60.0);
        let mid = best.pts.len() / 2;
        let spot = (0..best.pts.len())
            .map(|i| (i as isize - mid as isize).unsigned_abs())
            .filter_map(|off| {
                [mid.checked_sub(off), Some(mid + off)].into_iter().flatten().filter_map(|i| best.pts.get(i)).copied().find(|q| inner.contains(*q))
            })
            .next();
        let Some(at) = spot else { continue };
        let galley = p.layout_no_wrap(stops_label(level), t.font(10.5), tint(warmth, 1.0));
        let r = Rect::from_center_size(at, galley.size() + vec2(8.0, 4.0));
        if placed.iter().any(|q| q.expand(2.0).intersects(r)) {
            continue;
        }
        p.rect_filled(r, 3.0, Color32::from_black_alpha(150));
        p.galley(r.center() - galley.size() / 2.0, galley, tint(warmth, 1.0));
        placed.push(r);
    }
}
