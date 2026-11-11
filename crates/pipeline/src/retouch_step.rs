//! Fork feature (`fork/retouch-foundation.md`): the portrait retouching step.
//!
//! It runs in the scene-linear stage ([`crate::lin_cpu`]), after white balance, spots and red eye and before noise
//! reduction; the global tone and colour, the local masks, detail and effects all come after it, in the per-pixel
//! finish. The retouch stage runs first, then the background stage. The geometry stage (Face Reshape) is not run
//! here: it is a warp in the frame's geometry ([`crate::reshape_step`]), so the faces and masks this step reads
//! already follow it.
//!
//! Skipped entirely, touching no pixel and hashing a 0 into the stage key, while the block is off or the Portrait
//! tab's eye is off ([`active`]).

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::background::{ANALYSIS_LONG, Analysis, Place};
use lightcraft_develop::retouch::{Face, Input, KeyMap, MAX_FACES, NormRect, Reads, Scene, Stage, View};
use lightcraft_geom::{Affine, Orientation, Point};
use lightcraft_raster::Rgb32f;

use crate::{PixelWindow, Plan, SourceInfo};

thread_local! {
    static PRE_RETOUCH_RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times this thread made the image before the retouch step (white balance, defringe, spots, red eye) on
/// the CPU, by either renderer: a test counts what a Portrait slider tick redoes with it.
pub fn pre_retouch_runs() -> usize {
    PRE_RETOUCH_RUNS.with(std::cell::Cell::get)
}

pub(crate) fn note_pre_retouch_run() {
    PRE_RETOUCH_RUNS.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Whether the step changes anything for `s`.
///
/// Face Reshape is not part of the step (it is a warp in the frame's geometry, [`crate::reshape_step`]): a block with
/// only Face Reshape on leaves the step off, so the scene-linear stage can stay on the GPU.
pub fn active(s: &DevelopSettings) -> bool {
    lightcraft_develop::retouch::active(s) && !only_reshape(&s.retouch)
}

/// Every feature but Face Reshape is off, for every face.
fn only_reshape(r: &lightcraft_develop::retouch::Retouch) -> bool {
    let rest_off = |f: &lightcraft_develop::retouch::Features| {
        let mut f = f.clone();
        f.reshape = Default::default();
        f.is_off()
    };
    rest_off(&r.global) && r.faces.iter().all(|o| rest_off(&o.settings))
}

/// What the step adds to the scene-linear stage's cache key: 0 while it is off; else the settings and the input
/// (faces, persons, masks), so a render made before the faces were found is never reused after.
pub fn key(s: &DevelopSettings, info: &SourceInfo) -> u64 {
    if !active(s) {
        return 0;
    }
    crate::hash_of((format!("{:?}", s.retouch), info.retouch.as_ref().map(|i| i.fingerprint()))) | 1
}

/// A photo's lens corrections and transform, for per-face settings kept in the other frame than the faces'
/// ([`lightcraft_develop::retouch::KeyFrame`]): a point after them → the point before them, both before the user's
/// rotation.
pub struct FrameKeyMap {
    frame: crate::geometry::Frame,
    orient: Orientation,
}

impl FrameKeyMap {
    /// The map of `frame` (any frame of the photo's settings: the warp does not depend on the crop or the size).
    pub fn new(frame: &crate::geometry::Frame, orient: Orientation) -> FrameKeyMap {
        let mut frame = frame.for_mask(true).into_owned();
        // (the lens corrections and transform only: Face Reshape is built from the faces this map matches)
        if let Some(w) = &mut frame.warp {
            w.reshape = None;
        }
        FrameKeyMap { frame, orient }
    }
}

impl KeyMap for FrameKeyMap {
    fn edit_to_photo(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (ox, oy) = self.orient.map(x, y, 1.0, 1.0);
        let p = self.frame.norm_to_mask(Point::new(ox, oy));
        let (ux, uy) = self.orient.inverse().map(p.x, p.y, 1.0, 1.0);
        (ux.is_finite() && uy.is_finite()).then_some((ux, uy))
    }
}

/// Background's analysis image (`fork/retouch-background.md`): the whole frame of the render before the step, at
/// [`ANALYSIS_LONG`] pixels on its long side (or the render's size, when smaller) whatever the window, so a zoomed
/// loupe window and the whole render read the same blurs, matte colours and cleaned backdrop. Made once per source and per setting
/// that changes the image before the step (white balance, spots, red eye, defringe, the local tone map), kept for
/// the newest [`ANALYSES_KEPT`]; the work Background makes from it is kept inside it. `None` while Background is off.
pub fn analysis(src: &Rgb32f, info: &SourceInfo, p: &Plan<'_>) -> Option<Arc<Analysis>> {
    let s = &*p.settings;
    if !active(s) || lightcraft_develop::retouch::Feature::is_off(&s.retouch.global.background) {
        return None;
    }
    let (full, fw, fh) = &p.full;
    // (a render smaller than that, a thumbnail or a fitted preview, is its own size)
    let k = (ANALYSIS_LONG as f64 / (*fw).max(*fh).max(1) as f64).min(1.0);
    let (aw, ah) = (((*fw as f64 * k).round() as usize).max(1), ((*fh as f64 * k).round() as usize).max(1));
    let mut before = s.clone();
    // (the image before the step: retouching off, so the step does not run in it)
    before.retouch = Default::default();
    let px_per_long = full.px_per_long(aw);
    let eyes = crate::redeye::resolve(src, &before.red_eye, before.orientation, full, aw, ah, px_per_long);
    let (wb_t, wb_tint) = crate::local::effective_wb(info, &before);
    let key = crate::hash_of((
        crate::optics::fingerprint(src),
        format!("{full:?}"),
        aw,
        ah,
        [wb_t, wb_tint, info.as_shot_temp, info.as_shot_tint].map(f64::to_bits),
        format!("{:?}", before.spots),
        format!("{eyes:?}"),
        format!("{:?}", before.optics),
        crate::spots_keep::pre_spot_sections(&before),
        crate::local_tone::enabled(info, &before),
        p.src_long,
        // (and the order of the Classic spots and the white balance, issue #184)
        (p.native_long, p.spots_after_wb && !before.spots.is_empty()),
    ));
    let found = |c: &[(u64, Arc<Analysis>)]| c.iter().find(|(k, _)| *k == key).map(|(_, a)| a.clone());
    if let Some(a) = found(&ANALYSES.lock().unwrap_or_else(PoisonError::into_inner)) {
        return Some(a);
    }
    let t = crate::profiling().then(std::time::Instant::now);
    let sub = Plan {
        settings: Cow::Owned(before),
        frame: full.clone(),
        w: aw,
        h: ah,
        px_per_long,
        detail_scale: crate::local::detail_scale(px_per_long, p.native_long),
        geo: 0,
        lin_key: 0,
        pre_nr_key: 0,
        noise_key: 0,
        eyes,
        fixed_air: None,
        keep: None,
        mattes: p.mattes.clone(),
        draft: true,
        recolor: Vec::new(),
        full: (full.clone(), aw, ah),
        retouch_analysis: None,
        spots_at: crate::spots_scale::At::default(),
        ..*p
    };
    let img = if sub.spots_after_wb || sub.settings.spots.is_empty() {
        let mut img = Rgb32f::clone(&crate::local::frame_proxy(src, full, aw, ah));
        crate::lin_cpu(&mut img, info, &sub);
        img
    } else {
        // (the spots are drawn before the white balance, issue #184: the proxy with its spots is kept apart, so
        // Temperature and Tint draw none of them again here either; review F1)
        let spot_key = crate::hash_of((
            crate::optics::fingerprint(src),
            format!("{full:?}"),
            (aw, ah),
            format!("{:?}", sub.settings.spots),
            format!("{:?}", sub.settings.optics),
            crate::spots_keep::pre_spot_sections(&sub.settings),
            (p.src_long, p.native_long),
        ));
        let spotted = spotted_proxy(spot_key, || {
            let mut img = Rgb32f::clone(&crate::local::frame_proxy(src, full, aw, ah));
            crate::spots_on_source(&mut img, info, &sub);
            img
        });
        let mut img = Rgb32f::clone(&spotted);
        note_pre_retouch_run();
        crate::spots_keep::before_classic(&mut img, info, &sub);
        crate::redeye::apply(&mut img, &sub.eyes);
        apply(&mut img, info, &sub);
        img
    };
    let a = Arc::new(Analysis::new(Arc::new(img), key));
    if let Some(t) = t {
        eprintln!("    retouch background analysis {aw}×{ah}: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    let mut c = ANALYSES.lock().unwrap_or_else(PoisonError::into_inner);
    c.retain(|(k, _)| *k != key);
    c.insert(0, (key, a.clone()));
    c.truncate(ANALYSES_KEPT);
    Some(a)
}

/// Analysis images kept (a draft's source and the full source's, or two photos).
const ANALYSES_KEPT: usize = 2;
static ANALYSES: Mutex<Vec<(u64, Arc<Analysis>)>> = Mutex::new(Vec::new());
/// The analysis proxies with their spots, before the white balance, by key (as many as [`ANALYSES`]).
static SPOTTED: Mutex<Vec<(u64, Arc<Rgb32f>)>> = Mutex::new(Vec::new());

/// The proxy with its spots kept under `key`, else `make`'s, kept.
fn spotted_proxy(key: u64, make: impl FnOnce() -> Rgb32f) -> Arc<Rgb32f> {
    if let Some((_, img)) = SPOTTED.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == key) {
        return img.clone();
    }
    let img = Arc::new(make());
    let mut c = SPOTTED.lock().unwrap_or_else(PoisonError::into_inner);
    c.retain(|(k, _)| *k != key);
    c.insert(0, (key, img.clone()));
    c.truncate(ANALYSES_KEPT);
    img
}

/// Where the plan's pixels lie in the whole render: its window's top-left pixel and the whole render's size.
fn place_of(p: &Plan<'_>) -> Place {
    let (full, fw, fh) = &p.full;
    let (fw, fh) = (*fw, *fh);
    if (p.w, p.h) == (fw, fh) {
        return Place { x: 0, y: 0, width: fw, height: fh };
    }
    let n = p.frame.out_to_norm(p.w, p.h).apply(Point::new(0.5, 0.5));
    let o = full.norm_to_out(fw, fh).apply(n);
    let at = |v: f64| if v.is_finite() { (v - 0.5).round() as i64 } else { 0 };
    Place { x: at(o.x), y: at(o.y), width: fw, height: fh }
}

/// The plan's output pixels ↔ normalised photo coordinates (those of spots and faces).
struct FrameView {
    to_norm: Affine,
    to_out: Affine,
    /// Where the image's top-left pixel lies in the frame's `w × h` pixels ([`Mapping`]).
    at: (f64, f64),
    keys: FrameKeyMap,
    place: Place,
    analysis: Option<Arc<Analysis>>,
}

impl FrameView {
    fn new(frame: &crate::geometry::Frame, w: usize, h: usize, orient: Orientation) -> FrameView {
        FrameView {
            to_norm: frame.out_to_norm(w, h),
            to_out: frame.norm_to_out(w, h),
            at: (0.0, 0.0),
            keys: FrameKeyMap::new(frame, orient),
            place: Place { x: 0, y: 0, width: w, height: h },
            analysis: None,
        }
    }

    fn of(m: &Mapping<'_>, orient: Orientation) -> FrameView {
        FrameView { at: m.at, ..FrameView::new(m.frame, m.w, m.h, orient) }
    }
}

impl View for FrameView {
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64) {
        let p = self.to_norm.apply(Point::new(shift(x, self.at.0), shift(y, self.at.1)));
        (p.x, p.y)
    }

    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        let p = self.to_out.apply(Point::new(nx, ny));
        let p = (shift(p.x, -self.at.0), shift(p.y, -self.at.1));
        (p.0.is_finite() && p.1.is_finite()).then_some(p)
    }

    fn key_map(&self) -> Option<&dyn KeyMap> {
        Some(&self.keys)
    }

    fn place(&self) -> Option<Place> {
        Some(self.place)
    }

    fn analysis(&self) -> Option<&Analysis> {
        self.analysis.as_deref()
    }
}

/// The plan's output pixels ↔ normalised coordinates on the photo before lens corrections and transform (faces
/// from a people parse, [`Input::on_photo`]): through the frame's warp, as local masks drawn on the photo are.
struct PhotoView {
    to_photo: crate::geometry::MaskMap,
    to_out: Box<dyn Fn(Point) -> Option<Point> + Sync>,
    /// As [`FrameView::at`].
    at: (f64, f64),
    /// The frame Refine strokes are in (after lens corrections and transform, as [`FrameView`]).
    edit: FrameView,
    place: Place,
    analysis: Option<Arc<Analysis>>,
}

impl PhotoView {
    fn new(frame: &crate::geometry::Frame, w: usize, h: usize, orient: Orientation) -> PhotoView {
        let edit = FrameView::new(frame, w, h, orient);
        let frame = frame.for_mask(true);
        PhotoView {
            to_photo: frame.out_to_mask(w, h),
            to_out: Box::new(frame.mask_to_out(w, h)),
            at: (0.0, 0.0),
            edit,
            place: Place { x: 0, y: 0, width: w, height: h },
            analysis: None,
        }
    }
}

impl PhotoView {
    fn of(m: &Mapping<'_>, orient: Orientation) -> PhotoView {
        let mut v = PhotoView::new(m.frame, m.w, m.h, orient);
        (v.at, v.edit.at) = (m.at, m.at);
        v
    }
}

impl View for PhotoView {
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64) {
        let p = self.to_photo.apply(Point::new(shift(x, self.at.0), shift(y, self.at.1)));
        (p.x, p.y)
    }

    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        (self.to_out)(Point::new(nx, ny)).map(|p| (shift(p.x, -self.at.0), shift(p.y, -self.at.1))).filter(|(x, y)| x.is_finite() && y.is_finite())
    }

    fn key_map(&self) -> Option<&dyn KeyMap> {
        Some(&self.edit.keys)
    }

    fn to_edit(&self, x: f64, y: f64) -> (f64, f64) {
        self.edit.to_norm(x, y)
    }

    fn edit_to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        self.edit.to_px(nx, ny)
    }

    fn place(&self) -> Option<Place> {
        Some(self.place)
    }

    fn analysis(&self) -> Option<&Analysis> {
        self.analysis.as_deref()
    }
}

/// `v` moved by `d` pixels; `v` itself, to the bit, when `d` is 0.
fn shift(v: f64, d: f64) -> f64 {
    if d == 0.0 { v } else { v + d }
}

/// What a plan's pixels map through: the frame of `w × h` output pixels and where the image's top-left pixel lies
/// in it. A window maps through the whole frame and is moved by its whole-pixel offset, so its pixel positions are
/// exactly the whole frame's less a whole number, and a feature that rounds a position to a pixel (a mask's box, a
/// grid's first cell) rounds it as the whole frame does. The window's own frame maps the same points a few bits
/// differently: a mask's top edge at 71.99999999999999 in the whole frame came to 7.0 in a window 65 pixels down, so
/// Even Tone's grid started a row lower there and its cells fell elsewhere (#238).
struct Mapping<'a> {
    frame: &'a crate::geometry::Frame,
    w: usize,
    h: usize,
    at: (f64, f64),
}

impl<'a> Mapping<'a> {
    fn of(p: &'a Plan<'_>) -> Mapping<'a> {
        let (full, fw, fh) = &p.full;
        let whole = |v: f64| v.is_finite() && v >= 0.0 && v.fract() == 0.0;
        match p.frame.view {
            // (only for a window of that whole frame, at a whole pixel)
            Some(v) if full.view.is_none() && (v.full_w, v.full_h) == (*fw as f64, *fh as f64) && whole(v.x) && whole(v.y) => {
                Mapping { frame: full, w: *fw, h: *fh, at: (v.x, v.y) }
            }
            _ => Mapping { frame: &p.frame, w: p.w, h: p.h, at: (0.0, 0.0) },
        }
    }

    fn view(&self, on_photo: bool, orient: Orientation) -> Box<dyn View> {
        if on_photo { Box::new(PhotoView::of(self, orient)) } else { Box::new(FrameView::of(self, orient)) }
    }
}

/// Retouch `img` (the plan's output pixels) in place: the retouch stage, then the background stage.
pub fn apply(img: &mut Rgb32f, info: &SourceInfo, p: &Plan<'_>) {
    apply_with(img, info, p, &p.settings);
}

/// [`apply`] with settings `s` in place of the plan's (the same photo, frame and size): the ⌥-drag map retouches
/// with a slider at its default too (`crate::retouch_effect`).
pub(crate) fn apply_with(img: &mut Rgb32f, info: &SourceInfo, p: &Plan<'_>, s: &DevelopSettings) {
    if !active(s) {
        return;
    }
    let none = Input::default();
    let input: &Input = info.retouch.as_deref().unwrap_or(&none);
    let o = s.orientation;
    let place = place_of(p);
    let analysis = p.retouch_analysis.clone();
    let m = Mapping::of(p);
    let view: Box<dyn View> = if input.on_photo {
        Box::new(PhotoView { place, analysis, ..PhotoView::of(&m, o) })
    } else {
        Box::new(FrameView { place, analysis, ..FrameView::of(&m, o) })
    };
    crate::timed("retouch", || {
        for stage in [Stage::Retouch, Stage::Background] {
            lightcraft_develop::retouch::run::apply(img, &s.retouch, input, view.as_ref(), stage);
        }
    });
}

/// The window a zoomed render of `win` must render so retouching gives the same pixels there as the whole frame
/// does: `win` grown to hold everything retouching reads for each face whose reads reach into it, then for each
/// face whose reads reach into that, and so on (the face's box and the areas of the masks the features on read,
/// grown by their [`lightcraft_develop::retouch::Feature::REACH`] times the face's size). The result is cut back to `win` ([`crate::Plan::keep`]), so zoom never changes a
/// retouch. `frame` is the whole frame, `full_w × full_h` its output size.
pub fn window_for_reads(
    s: &DevelopSettings,
    info: &SourceInfo,
    frame: &crate::geometry::Frame,
    full_w: usize,
    full_h: usize,
    win: PixelWindow,
) -> PixelWindow {
    if !active(s) {
        return win;
    }
    let (Some(input), Some(reads)) = (info.retouch.as_deref(), s.retouch.reads()) else { return win };
    let o = s.orientation;
    let view: Box<dyn View> =
        if input.on_photo { Box::new(PhotoView::new(frame, full_w, full_h, o)) } else { Box::new(FrameView::new(frame, full_w, full_h, o)) };
    // where each face's own Refine Add strokes widen its masks: their points' box grown by their radius, in pixels
    let scene = Scene::new(input, view.as_ref(), full_w, full_h);
    let adds = lightcraft_develop::retouch::refine::add_reach(&s.retouch, input, &scene);
    let (fw, fh) = (full_w as f64, full_h as f64);
    let mut boxes: Vec<[f64; 4]> = scene
        .faces
        .iter()
        .zip(&adds)
        .filter_map(|(f, add)| {
            let add = match add.map(|(b, r)| add_box(b, r, view.as_ref())) {
                None => None,
                Some(Some(a)) => Some(a),
                // (where the strokes widen maps nowhere: all of the frame, to be sure)
                Some(None) => return Some([0.0, 0.0, fw, fh]),
            };
            match read_box(f.face, &reads, add, view.as_ref()) {
                ReadBox::Skipped => None,
                ReadBox::At(b) => Some(b),
                // (part of its reads maps nowhere: all of the frame, to be sure)
                ReadBox::Unsure => Some([0.0, 0.0, fw, fh]),
            }
        })
        .map(|[rx0, ry0, rx1, ry1]| [rx0.max(0.0), ry0.max(0.0), rx1.min(fw), ry1.min(fh)])
        .collect();
    let [mut x0, mut y0, mut x1, mut y1] = [win.x as f64, win.y as f64, (win.x + win.w) as f64, (win.y + win.h) as f64];
    // Until no face's reads touch the window without lying inside it: the features run feature by feature over
    // every face, so a face's later feature reads what an earlier one changed for a neighbouring face, and that
    // change is right only when the window holds all the neighbour reads too. (Within one face the box already
    // holds every pixel any of its features reads, so an earlier feature's changes inside it are the whole
    // frame's.) At most one pass per face.
    while let Some(i) = boxes.iter().position(|r| r[0] < x1 && r[2] > x0 && r[1] < y1 && r[3] > y0) {
        let r = boxes.swap_remove(i);
        (x0, y0, x1, y1) = (x0.min(r[0]), y0.min(r[1]), x1.max(r[2]), y1.max(r[3]));
    }
    let (x, y) = (x0.floor().max(0.0) as usize, y0.floor().max(0.0) as usize);
    let (r, b) = ((x1.ceil().min(fw) as usize).max(x + 1), (y1.ceil().min(fh) as usize).max(y + 1));
    PixelWindow { x, y, w: r - x, h: b - y }.clamped(full_w, full_h)
}

/// Whether the step may change any pixel of a photo with `info`'s faces: it is on, and there is a face to retouch or a
/// feature on that retouches more than faces. Without faces, Portrait settings leave everything as it was (the noise
/// measurement included, [`crate::colour_noise`]).
pub fn touches(s: &DevelopSettings, info: &SourceInfo) -> bool {
    active(s) && (info.retouch.as_ref().is_some_and(|i| !i.faces.is_empty()) || s.retouch.global.writes_outside_faces())
}

/// Where the step may change pixels of the plan's `p.w × p.h` render: one `[x0, y0, x1, y1)` box per face, what
/// retouching reads for it ([`window_for_reads`]'s boxes: every face feature writes inside them,
/// [`lightcraft_develop::retouch::Feature::writes_near_face`]). Empty while the step is off; `None` when it may change
/// any pixel: a feature on writes outside its face (Background, strokes), or part of a face's reads maps nowhere.
pub fn write_boxes(info: &SourceInfo, p: &Plan<'_>) -> Option<Vec<crate::spots_keep::Rect>> {
    let s = &*p.settings;
    if !active(s) {
        return Some(Vec::new());
    }
    // Guard: every field of the block, of a face's own settings and of what they read is named here (no `..`), so a
    // new one stops the build until this function says whether its writes stay in the boxes. Refine strokes (#151)
    // add `refine` (for every face and per face): a block with any stroke returns `None`, the whole frame
    // (`fork/perf-portrait-draft.md`).
    let lightcraft_develop::retouch::Retouch { global, faces, refine } = &s.retouch;
    if global.writes_outside_faces()
        || faces
            .iter()
            .any(|lightcraft_develop::retouch::FaceOverride { face: _, settings, refine }| settings.writes_outside_faces() || !refine.is_empty())
        || !refine.is_empty()
    {
        return None;
    }
    let (Some(input), Some(reads)) = (info.retouch.as_deref(), s.retouch.reads()) else { return Some(Vec::new()) };
    let Reads { reach: _, parts: _ } = &reads;
    let view = Mapping::of(p).view(input.on_photo, s.orientation);
    let (w, h) = (p.w as f64, p.h as f64);
    let mut out = Vec::new();
    for face in input.faces.iter().take(MAX_FACES) {
        let [x0, y0, x1, y1] = match read_box(face, &reads, None, view.as_ref()) {
            ReadBox::Skipped => continue,
            ReadBox::At(b) => b,
            ReadBox::Unsure => return None,
        };
        let (x0, y0, x1, y1) = (x0.floor().clamp(0.0, w), y0.floor().clamp(0.0, h), x1.ceil().clamp(0.0, w), y1.ceil().clamp(0.0, h));
        if x1 > x0 && y1 > y0 {
            out.push([x0 as usize, y0 as usize, x1 as usize, y1 as usize]);
        }
    }
    Some(out)
}

/// Pixels of a face's box and masks' areas are followed along each edge at this many points (a lens or transform
/// warp bends them).
const EDGE_STEPS: usize = 16;

/// The pixels Refine Add strokes can widen a mask to: the box `b` of their points (the frame [`View::to_edit`] speaks)
/// grown by `r` long edges of that frame. `None` when it maps nowhere.
fn add_box(b: [f64; 4], r: f64, view: &dyn View) -> Option<[f64; 4]> {
    let corners = [(b[0], b[1]), (b[2], b[1]), (b[0], b[3]), (b[2], b[3])];
    let mut out = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for (x, y) in corners {
        let (px, py) = view.edit_to_px(x, y).filter(|(x, y)| x.is_finite() && y.is_finite())?;
        out = [out[0].min(px), out[1].min(py), out[2].max(px), out[3].max(py)];
    }
    let (o, ex, ey) = (view.edit_to_px(0.0, 0.0)?, view.edit_to_px(1.0, 0.0)?, view.edit_to_px(0.0, 1.0)?);
    let len = |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let grow = r * len(o, ex).max(len(o, ey)) + 2.0;
    grow.is_finite().then_some([out[0] - grow, out[1] - grow, out[2] + grow, out[3] + grow])
}

/// What retouching reads for one face, in a view's pixels.
enum ReadBox {
    /// Nothing: the step skips the face (its box is not a box or a corner maps nowhere, as `Scene` decides).
    Skipped,
    /// `[x0, y0, x1, y1]`, unclamped.
    At([f64; 4]),
    /// Part of what it reads maps nowhere (a face partly off a warped frame): the box is not known.
    Unsure,
}

/// What retouching reads for `face` in `view`'s pixels: its box, the areas of the masks `reads` names and of its
/// strands (Stray Hair repairs them), and `add` (where Refine Add strokes widen its masks, [`add_box`]), grown by
/// `reads.reach` times the face's size.
fn read_box(face: &Face, reads: &Reads, add: Option<[f64; 4]>, view: &dyn View) -> ReadBox {
    let fb = &face.bounds;
    let corners = [(fb.x0, fb.y0), (fb.x1, fb.y0), (fb.x1, fb.y1), (fb.x0, fb.y1)];
    if !fb.is_valid() || corners.iter().any(|(x, y)| view.to_px(*x, *y).is_none()) {
        return ReadBox::Skipped;
    }
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    if !add_rect(&mut b, fb, view) {
        return ReadBox::Unsure;
    }
    // the face's size, as the step sees it: the longer side of its box's bounding box
    let size = (b[2] - b[0]).max(b[3] - b[1]);
    if !(size.is_finite() && size > 0.0) {
        return ReadBox::Skipped;
    }
    let masks = reads.parts.iter().filter_map(|p| face.mask(*p)).map(|m| &m.area);
    let strands = face.strands.as_ref().map(|s| &s.area);
    // (a kept Glasses Glare repair writes over its own area, which can reach past the face's box and its reach)
    let repair = face.patch.as_deref().map(|p| &p.area);
    for area in masks.chain(strands).chain(repair) {
        if !add_rect(&mut b, area, view) {
            return ReadBox::Unsure;
        }
    }
    if let Some(a) = add {
        b = [b[0].min(a[0]), b[1].min(a[1]), b[2].max(a[2]), b[3].max(a[3])];
    }
    // (and a pixel for rounding)
    let grow = reads.reach * size + 2.0;
    if !grow.is_finite() {
        return ReadBox::Unsure;
    }
    ReadBox::At([b[0] - grow, b[1] - grow, b[2] + grow, b[3] + grow])
}

/// Grow the pixel box `b` to hold normalised rectangle `r` as `view` maps it; `false` when a point of its edge maps
/// nowhere (an empty or malformed rectangle adds nothing).
fn add_rect(b: &mut [f64; 4], r: &NormRect, view: &dyn View) -> bool {
    if !r.is_valid() {
        return true;
    }
    for i in 0..=EDGE_STEPS {
        let t = i as f64 / EDGE_STEPS as f64;
        let (u, v) = (r.x0 + t * r.width(), r.y0 + t * r.height());
        for (nx, ny) in [(u, r.y0), (u, r.y1), (r.x0, v), (r.x1, v)] {
            match view.to_px(nx, ny).filter(|(x, y)| x.is_finite() && y.is_finite()) {
                Some((px, py)) => *b = [b[0].min(px), b[1].min(py), b[2].max(px), b[3].max(py)],
                None => return false,
            }
        }
    }
    true
}

/// The faces of a photo for its [`SourceInfo`] (shared: one detection serves every render of the photo).
pub type Faces = Arc<Input>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OutputDepth, RenderRequest, render};
    use lightcraft_develop::controls;
    use lightcraft_develop::retouch::{Face, NormRect, Scene};

    fn scene() -> Rgb32f {
        lightcraft_scenes::demo_library()[0].render(240, 160)
    }

    fn faces(n: usize) -> SourceInfo {
        let faces = (0..n)
            .map(|i| {
                let (x, y) = ((i % 10) as f64 / 10.0, (i / 10) as f64 / 6.0);
                Face { landmarks: vec![(x + 0.03, y + 0.05); 5], ..Face::boxed(NormRect::new(x, y, x + 0.08, y + 0.12)) }
            })
            .collect();
        SourceInfo { retouch: Some(Arc::new(Input::new(faces))), ..SourceInfo::default() }
    }

    /// A block that is there but off (a face with its own settings, all at 0), with faces found: the render is the
    /// render without retouching, byte for byte, and the stage key is the same.
    #[test]
    fn an_all_off_block_renders_byte_identical() {
        let src = scene();
        let plain = DevelopSettings::default();
        let mut off = plain.clone();
        let face = Face::boxed(NormRect::new(0.4, 0.2, 0.6, 0.5));
        off.retouch.override_mut(std::slice::from_ref(&face), 0);
        assert!(!off.retouch.is_default() && off.retouch.is_off());
        // a feature on, but the Portrait tab's eye off
        let mut eye_off = plain.clone();
        controls::set(&mut eye_off, "retouch.skin.smoothness", 60.0);
        eye_off.set_section_enabled("portrait", false);
        for depth in [OutputDepth::U8, OutputDepth::F32Linear] {
            let req = RenderRequest { depth, ..RenderRequest::fit(120, 120) };
            let base = render(&src, &SourceInfo::default(), &plain, &req);
            for (s, info) in [(&off, faces(1)), (&off, SourceInfo::default()), (&eye_off, faces(3))] {
                let r = render(&src, &info, s, &req);
                assert_eq!(r.image, base.image, "{depth:?}");
                assert_eq!(r.deep.map(|d| format!("{:?}", d.samples)), base.deep.as_ref().map(|d| format!("{:?}", d.samples)));
                assert_eq!(key(s, &info), 0);
                let (a, b) = (crate::plan(&src, &info, s, &req), crate::plan(&src, &SourceInfo::default(), &plain, &req));
                assert_eq!((a.pre_nr_key, a.lin_key), (b.pre_nr_key, b.lin_key), "the cache keys of every other photo stay");
            }
        }
        assert!(!crate::lin_needs_cpu(&off, &SourceInfo::default()));
    }

    #[test]
    fn an_active_block_keys_its_settings_and_its_faces() {
        let mut s = DevelopSettings::default();
        controls::set(&mut s, "retouch.eyes.whites", 40.0);
        assert!(active(&s) && crate::lin_needs_cpu(&s, &SourceInfo::default()));
        let none = key(&s, &SourceInfo::default());
        assert_ne!(none, 0);
        assert_ne!(key(&s, &faces(1)), none, "faces found later redo the stage");
        assert_ne!(key(&s, &faces(1)), key(&s, &faces(2)));
        let mut more = s.clone();
        controls::set(&mut more, "retouch.eyes.whites", 41.0);
        assert_ne!(key(&more, &faces(1)), key(&s, &faces(1)));
    }

    #[test]
    fn no_panic_with_no_faces_fifty_faces_or_a_one_pixel_photo() {
        let mut s = DevelopSettings::default();
        for spec in lightcraft_develop::retouch::specs() {
            controls::set(&mut s, spec.id, spec.max);
        }
        for (src, size) in [(scene(), 96), (Rgb32f::filled(1, 1, [0.3, 0.2, 0.1]), 1), (Rgb32f::filled(1, 1, [0.3; 3]), 8)] {
            for info in [SourceInfo::default(), faces(0), faces(50)] {
                let r = render(&src, &info, &s, &RenderRequest::fit(size, size));
                assert!(r.image.width >= 1 && r.image.height >= 1);
            }
        }
    }

    /// Faces are in the photo's coordinates before the crop; the step sees them in the cropped output's pixels.
    #[test]
    fn faces_map_into_the_rendered_pixels() {
        let src = Rgb32f::filled(200, 100, [0.2; 3]);
        let mut s = DevelopSettings::default();
        s.crop.geometry.rect = lightcraft_geom::Rect { x0: 0.25, y0: 0.25, x1: 0.75, y1: 0.75 };
        let info = SourceInfo { retouch: Some(Arc::new(Input::new(vec![Face::boxed(NormRect::new(0.4, 0.4, 0.6, 0.6))]))), ..SourceInfo::default() };
        let p = crate::plan(&src, &info, &s, &RenderRequest::fit(100, 100));
        assert_eq!((p.w, p.h), (100, 50));
        let view = FrameView::new(&p.frame, p.w, p.h, Orientation::Normal);
        let input = info.retouch.as_deref().unwrap();
        let scene = Scene::new(input, &view, p.w, p.h);
        let b = scene.faces[0].bounds;
        for (got, want) in b.iter().zip([30.0, 15.0, 70.0, 35.0]) {
            assert!((got - want).abs() < 1e-6, "{b:?}");
        }
        assert!((scene.faces[0].size - 40.0).abs() < 1e-6);
    }

    /// A face with a Facial Skin mask over `area` (all 1), on the photo before lens corrections and transform.
    fn skin_on_photo(area: NormRect) -> SourceInfo {
        use lightcraft_develop::retouch::{Part, PartMask};
        let mask = PartMask { plane: lightcraft_raster::Plane::filled(4, 4, 1.0), area };
        let face = Face { masks: vec![(Part::FacialSkin, mask)], ..Face::boxed(area) };
        SourceInfo { retouch: Some(Arc::new(Input::new_on_photo(vec![face]))), ..SourceInfo::default() }
    }

    fn lit() -> DevelopSettings {
        let mut s = DevelopSettings::default();
        controls::set(&mut s, "retouch.skinTone.faceLight", 100.0);
        s
    }

    /// Without a lens or transform warp, faces on the photo land where faces after the warp do.
    #[test]
    fn faces_on_the_photo_render_as_faces_after_the_warp_without_a_warp() {
        let src = scene();
        let area = NormRect::new(0.3, 0.2, 0.6, 0.7);
        let on_photo = skin_on_photo(area);
        let mut after = on_photo.clone();
        let faces = on_photo.retouch.as_ref().unwrap().faces.clone();
        after.retouch = Some(Arc::new(Input::new(faces)));
        let req = RenderRequest::fit(160, 160);
        let (a, b) = (render(&src, &on_photo, &lit(), &req), render(&src, &after, &lit(), &req));
        assert_eq!(a.image, b.image);
        assert_ne!(a.image, render(&src, &SourceInfo::default(), &lit(), &req).image, "the mask arrived");
    }

    /// With a lens distortion, a mask made on the photo still covers the photo's content it was made on: the step
    /// maps it through the warp. Pixels away from the mask are unchanged.
    #[test]
    fn faces_on_the_photo_follow_the_lens_warp() {
        let (w, h) = (300, 200);
        let area = NormRect::new(0.1, 0.1, 0.3, 0.35);
        let inside = |x: usize, y: usize| {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            nx > area.x0 && nx < area.x1 && ny > area.y0 && ny < area.y1
        };
        // a red patch exactly where the mask is, on the photo
        let src = Rgb32f::from_fn(w, h, |x, y| if inside(x, y) { [0.5, 0.05, 0.05] } else { [0.1, 0.1, 0.1] });
        let mut s = lit();
        s.optics.distortion = 80.0;
        let req = RenderRequest::fit(w, h);
        let p = crate::plan(&src, &SourceInfo::default(), &s, &req);
        assert!(p.frame.for_mask(true).mask_warp().is_some(), "the distortion warps the picture");
        let mut off = s.clone();
        controls::set(&mut off, "retouch.skinTone.faceLight", 0.0);
        let plain = render(&src, &SourceInfo::default(), &off, &req).image;
        let done = render(&src, &skin_on_photo(area), &s, &req).image;
        let is_red = |a: [u8; 4]| a[0] > a[1].saturating_add(60);
        // Face Light's soft edge falls to nothing at the mask's border: its inner part is the red pixels at least an
        // edge's width (LIGHT_EDGE of the face's size, the patch's longer side) from anything not red
        let edge = (lightcraft_develop::retouch::skin_tone::LIGHT_EDGE * (area.width() * w as f64).max(area.height() * h as f64)).ceil() as usize;
        let (pw, ph) = (plain.width, plain.height);
        let deep = |x: usize, y: usize| {
            (y.saturating_sub(edge)..=(y + edge).min(ph - 1))
                .all(|yy| (x.saturating_sub(edge)..=(x + edge).min(pw - 1)).all(|xx| is_red(plain.get(xx, yy))))
        };
        let (mut red, mut inner, mut inner_lit, mut grey_lit) = (0, 0, 0, 0);
        for y in 0..ph {
            for x in 0..pw {
                let (a, b) = (plain.get(x, y), done.get(x, y));
                let is_grey = a[0].abs_diff(a[1]) < 4;
                red += usize::from(is_red(a));
                if is_red(a) && deep(x, y) {
                    inner += 1;
                    inner_lit += usize::from(b[0] > a[0]);
                }
                grey_lit += usize::from(is_grey && b != a);
            }
        }
        assert!(inner > 500, "{inner} red pixels an edge's width inside the patch");
        assert!(inner_lit * 100 >= inner * 97, "the warped patch is lit: {inner_lit} of {inner}");
        assert!(grey_lit * 100 <= red * 3, "and little else (its antialiased edge): {grey_lit}");
    }

    /// One person (a soft ellipse over the middle of the photo), from a people parse.
    fn a_person() -> SourceInfo {
        use lightcraft_develop::retouch::{Part, PartMask};
        let plane = lightcraft_raster::Plane::from_fn(128, 128, |x, y| {
            let (nx, ny) = ((x as f64 + 0.5) / 128.0, (y as f64 + 0.5) / 128.0);
            let d = (((nx - 0.5) / 0.15).powi(2) + ((ny - 0.6) / 0.35).powi(2)).sqrt();
            ((1.0 - d) * 12.0 + 0.5).clamp(0.0, 1.0) as f32
        });
        let mask = PartMask { plane, area: NormRect::new(0.0, 0.0, 1.0, 1.0) };
        let face = Face { masks: vec![(Part::EntirePerson, mask)], ..Face::boxed(NormRect::new(0.42, 0.25, 0.58, 0.45)) };
        SourceInfo { retouch: Some(Arc::new(Input::new_on_photo(vec![face]))), ..SourceInfo::default() }
    }

    /// Background reads the analysis image of the whole frame: a zoomed window, not grown at all, draws what the
    /// whole render draws inside it, and a pan reuses the work the whole render made.
    #[test]
    fn a_window_with_the_background_on_matches_the_whole_render() {
        let src = lightcraft_scenes::demo_library()[0].render(480, 320);
        let info = a_person();
        let mut s = DevelopSettings::default();
        controls::set(&mut s, "retouch.background.blur", 80.0);
        controls::set(&mut s, "retouch.background.clean", 60.0);
        controls::set(&mut s, "retouch.background.swap", 30.0);
        let fit = RenderRequest::fit(480, 320);
        let full = render(&src, &info, &s, &fit).image;
        let plain = render(&src, &info, &DevelopSettings::default(), &fit).image;
        assert_ne!(full, plain, "the background changed");
        for win in [crate::PixelWindow { x: 180, y: 100, w: 120, h: 100 }, crate::PixelWindow { x: 0, y: 0, w: 90, h: 70 }] {
            let req = RenderRequest { window: Some(win), ..fit };
            let w = render(&src, &info, &s, &req).image;
            assert_eq!((w.width, w.height), (win.w, win.h));
            let mut max = 0u8;
            for y in 0..win.h {
                for x in 0..win.w {
                    let (a, b) = (full.get(win.x + x, win.y + y), w.get(x, y));
                    max = (0..3).fold(max, |m, k| m.max(a[k].abs_diff(b[k])));
                }
            }
            assert!(max <= 1, "the window at {win:?} differs by {max}");
            let p = crate::plan(&src, &info, &s, &req);
            assert!(p.keep.is_none(), "the window is not grown");
            let a = p.retouch_analysis.as_deref().map(|a| (a.image.width, a.kept()));
            assert_eq!(a, Some((480, 1)), "one analysis image (the render's size: under 1600), its work made once for every window");
        }
        let plain = DevelopSettings::default();
        assert!(crate::plan(&src, &info, &plain, &fit).retouch_analysis.is_none(), "none while Background is off");
    }

    /// An on-photo face under a crop and a lens distortion: a draft and a render four times its size light the same
    /// share of the picture, all of the patch the mask was made on, and little else.
    #[test]
    fn a_draft_and_a_bigger_render_agree_with_a_crop_and_a_lens_distortion() {
        let (w, h) = (600, 400);
        let area = NormRect::new(0.15, 0.2, 0.35, 0.45);
        let inside = |x: usize, y: usize| {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            nx > area.x0 && nx < area.x1 && ny > area.y0 && ny < area.y1
        };
        let src = Rgb32f::from_fn(w, h, |x, y| if inside(x, y) { [0.5, 0.05, 0.05] } else { [0.1, 0.1, 0.1] });
        let mut s = lit();
        s.optics.distortion = 80.0;
        s.crop.geometry.rect = lightcraft_geom::Rect { x0: 0.1, y0: 0.1, x1: 0.8, y1: 0.85 };
        let mut off = s.clone();
        controls::set(&mut off, "retouch.skinTone.faceLight", 0.0);
        let mut shares = Vec::new();
        for (rw, rh) in [(150, 100), (600, 400)] {
            let req = RenderRequest { apply_crop: true, ..RenderRequest::fit(rw, rh) };
            let plain = render(&src, &SourceInfo::default(), &off, &req).image;
            let done = render(&src, &skin_on_photo(area), &s, &req).image;
            let (mut red, mut red_lit, mut grey_lit, mut changed) = (0, 0, 0, 0);
            let ow = plain.width;
            let reddish = |i: Option<usize>| i.and_then(|i| plain.data.get(i)).is_some_and(|a| a[0] > a[1].saturating_add(60));
            for (i, (a, b)) in plain.data.iter().zip(&done.data).enumerate() {
                // (the patch's inside: a pixel of its edge, part grey in a small render, may have its centre past
                // the mask)
                let around = [i.checked_sub(1), Some(i + 1), i.checked_sub(ow), Some(i + ow)];
                let is_red = reddish(Some(i)) && around.iter().all(|n| reddish(*n));
                red += usize::from(is_red);
                red_lit += usize::from(is_red && b[0] > a[0]);
                grey_lit += usize::from(a[0].abs_diff(a[1]) < 4 && b != a);
                changed += usize::from(a != b);
            }
            assert!(red > 100, "{red} red pixels at {rw}×{rh}");
            assert!(red_lit * 100 >= red * 99, "the warped, cropped patch is lit at {rw}×{rh}: {red_lit} of {red}");
            assert!(grey_lit * 100 <= red * 8, "and little else at {rw}×{rh}: {grey_lit}");
            shares.push(changed as f64 / plain.data.len() as f64);
        }
        assert!((shares[0] - shares[1]).abs() < 0.02, "the lit share: {shares:?}");
    }

    /// A face with the 478-point mesh (a frontal face's), Facial Skin over the box and Body Skin under it.
    fn meshed(r: NormRect) -> Face {
        use lightcraft_develop::retouch::{Part, PartMask};
        let five = vec![(r.x0 + 0.3 * r.width(), r.y0 + 0.45 * r.height()); 5];
        let at = |(fx, fy): (f64, f64)| (r.x0 + fx * r.width(), r.y0 + fy * r.height());
        let mesh = (0..478).map(|i| at(lightcraft_develop::retouch::wrinkles::areas::frontal(i)));
        let skin = |area: NormRect| PartMask { plane: lightcraft_raster::Plane::filled(8, 8, 1.0), area };
        Face {
            landmarks: five.into_iter().chain(mesh).collect(),
            masks: vec![(Part::FacialSkin, skin(r)), (Part::BodySkin, skin(NormRect::new(r.x0, r.y1, r.x1, (r.y1 + r.height()).min(1.0))))],
            ..Face::boxed(r)
        }
    }

    /// Wrinkles read about a fifth of a face around each pixel (`fork/retouch-wrinkles.md`): a zoomed window that
    /// holds part of a face is grown by that reach, and comes out as the whole render does inside it.
    #[test]
    fn a_zoomed_window_of_a_face_equals_the_whole_render_inside_it() {
        // skin with fine dark lines every 5 px, and pores
        let src = Rgb32f::from_fn(480, 320, |x, y| {
            let n = ((x * 7 + y * 13) % 11) as f32 / 11.0 - 0.5;
            let line = if y % 5 == 0 { 0.7 } else { 1.0 };
            [0.45, 0.3, 0.24].map(|c| c * line * (1.0 + 0.06 * n))
        });
        let info = SourceInfo { retouch: Some(Arc::new(Input::new(vec![meshed(NormRect::new(0.3, 0.15, 0.6, 0.6))]))), ..SourceInfo::default() };
        let mut s = DevelopSettings::default();
        for id in ["forehead", "crowsFeet", "smileLines", "neck"] {
            controls::set(&mut s, &format!("retouch.wrinkles.{id}"), 100.0);
        }
        let (w, h) = (960, 640);
        let whole = render(&src, &info, &s, &RenderRequest::fit(w, h)).image;
        let plain = render(&src, &info, &DevelopSettings::default(), &RenderRequest::fit(w, h)).image;
        for win in [crate::PixelWindow { x: 420, y: 150, w: 160, h: 120 }, crate::PixelWindow { x: 300, y: 330, w: 200, h: 150 }] {
            let grown = window_for_reads(&s, &info, &crate::plan(&src, &info, &s, &RenderRequest::fit(w, h)).frame, w, h, win);
            assert!(grown.w > win.w + 100 && grown.h > win.h + 80, "the window grows by the reach: {grown:?}");
            let part = render(&src, &info, &s, &RenderRequest { window: Some(win), ..RenderRequest::fit(w, h) }).image;
            let (mut most, mut changed) = (0u8, 0);
            for y in 0..win.h {
                for x in 0..win.w {
                    let (a, b) = (whole.get(win.x + x, win.y + y), part.get(x, y));
                    most = most.max((0..3).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0));
                    changed += usize::from(a != plain.get(win.x + x, win.y + y));
                }
            }
            assert!(changed > 1000, "the window holds retouched pixels: {changed}");
            assert!(most <= 1, "the window equals the whole render inside it ({win:?}): {most}");
        }
        // a window far from the face is not grown
        let far = crate::PixelWindow { x: 0, y: 0, w: 40, h: 40 };
        let frame = crate::plan(&src, &info, &s, &RenderRequest::fit(w, h)).frame;
        let near = NormRect::new(0.8, 0.8, 0.85, 0.85);
        let small = SourceInfo { retouch: Some(Arc::new(Input::new(vec![meshed(near)]))), ..SourceInfo::default() };
        assert_eq!(window_for_reads(&s, &small, &frame, w, h, far), far);
        assert_eq!(window_for_reads(&DevelopSettings::default(), &info, &frame, w, h, win_at(400, 200)), win_at(400, 200), "off: no growth");
    }

    fn win_at(x: usize, y: usize) -> crate::PixelWindow {
        crate::PixelWindow { x, y, w: 50, h: 50 }
    }

    /// A synthetic face (skin, a bluish dark circle under each eye, whites, irises) with YuNet's five points and the
    /// Facial Skin, Eye Whites and Iris masks, as a people parse gives them; the photo is `w × h`.
    fn face_photo(w: usize, h: usize) -> (Rgb32f, SourceInfo) {
        use lightcraft_develop::retouch::{Part, PartMask};
        const EYES: [(f64, f64); 2] = [(0.4, 0.4), (0.6, 0.4)];
        let ell = |nx: f64, ny: f64, c: (f64, f64), rx: f64, ry: f64| {
            let d = (((nx - c.0) / rx).powi(2) + ((ny - c.1) / ry).powi(2)).sqrt();
            ((1.1 - d) / 0.2).clamp(0.0, 1.0) as f32
        };
        let whites = move |nx: f64, ny: f64| EYES.iter().map(|c| ell(nx, ny, *c, 0.045, 0.03)).fold(0.0, f32::max);
        let iris = move |nx: f64, ny: f64| EYES.iter().map(|c| ell(nx, ny, *c, 0.018, 0.024)).fold(0.0, f32::max);
        let circle = move |nx: f64, ny: f64| EYES.iter().map(|c| ell(nx, ny, (c.0, c.1 + 0.05), 0.05, 0.03)).fold(0.0, f32::max);
        let src = Rgb32f::from_fn(w, h, |x, y| {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            // a little texture, so smoothing has something to do
            let tex = 1.0 + 0.06 * (((x * 7 + y * 13) % 5) as f32 - 2.0) / 2.0 + 0.1 * ((x as f32 / 3.2).sin() * (y as f32 / 3.2).sin());
            let dc = circle(nx, ny);
            let skin = [0.5 * (1.0 - 0.35 * dc) * tex, 0.36 * (1.0 - 0.4 * dc) * tex, 0.28 * (1.0 - 0.2 * dc) * tex];
            let mix = |a: [f32; 3], b: [f32; 3], t: f32| std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
            mix(mix(skin, [0.62, 0.5, 0.4], whites(nx, ny)), [0.12, 0.08, 0.05], iris(nx, ny))
        });
        let bounds = NormRect::new(0.28, 0.2, 0.72, 0.8);
        let mask = |f: &dyn Fn(f64, f64) -> f32| {
            let n = 96;
            let plane = lightcraft_raster::Plane::from_fn(n, n, |x, y| {
                f(bounds.x0 + (x as f64 + 0.5) / n as f64 * bounds.width(), bounds.y0 + (y as f64 + 0.5) / n as f64 * bounds.height())
            });
            PartMask { plane, area: bounds }
        };
        let face = Face {
            landmarks: vec![EYES[0], EYES[1], (0.5, 0.52), (0.44, 0.65), (0.56, 0.65)],
            masks: vec![
                (Part::FacialSkin, mask(&|x, y| 1.0 - whites(x, y).max(iris(x, y)))),
                (Part::EyeWhites, mask(&|x, y| whites(x, y) * (1.0 - iris(x, y)))),
                (Part::Iris, mask(&iris)),
            ],
            ..Face::boxed(bounds)
        };
        (src, SourceInfo { retouch: Some(Arc::new(Input::new(vec![face]))), ..SourceInfo::default() })
    }

    /// `info`'s faces with a face mesh (as the face mesh model gives it) after their five points.
    fn with_mesh(info: &SourceInfo) -> SourceInfo {
        let faces = info.retouch.iter().flat_map(|i| i.faces.iter()).map(|f| {
            let r = f.bounds;
            let at = |(fx, fy): (f64, f64)| (r.x0 + fx * r.width(), r.y0 + fy * r.height());
            let mesh = (0..478).map(|i| at(lightcraft_develop::retouch::wrinkles::areas::frontal(i)));
            Face { landmarks: f.landmarks.iter().copied().chain(mesh).collect(), ..f.clone() }
        });
        SourceInfo { retouch: Some(Arc::new(Input::new(faces.collect()))), ..SourceInfo::default() }
    }

    /// Zoom never changes a retouch: for each feature on its own, on a face with five points and on one with a face
    /// mesh, a zoomed render of a window holding part of what it changes gives the whole frame's pixels there. Every
    /// feature is checked and every failure listed (#238: Even Tone's grid started a row lower in a window, 4 to 5
    /// levels off). Blemishes, Teeth and Lips, Glasses Glare, Stray Hair, Background and Refine strokes have their
    /// own zoom tests, on photos that have what they repair.
    #[test]
    fn a_zoomed_window_gives_the_whole_frames_retouch() {
        let (w, h) = (480, 360);
        let (src, five) = face_photo(w, h);
        let mesh = with_mesh(&five);
        let mut base = DevelopSettings::default();
        base.detail.nr_color = 0.0;
        let req = RenderRequest::fit(w, h);
        let win = |x, y, w, h| crate::PixelWindow { x, y, w, h };
        // (the left eye's outer half and the skin under it; a patch of cheek; part of the cheek under the dark
        // circle Even Tone evens)
        let (eye, cheek, under) = (win(170, 136, 30, 40), win(200, 220, 40, 30), win(180, 150, 30, 30));
        let cases: [(&str, &[(&str, f64)], crate::PixelWindow); 13] = [
            ("eye whites", &[("eyes.whites", 100.0)], eye),
            ("iris", &[("eyes.iris", 100.0)], eye),
            ("dark circles", &[("eyes.darkCircles", 100.0)], under),
            ("eye bags", &[("eyes.eyeBags", 100.0)], under),
            ("smoothness", &[("skin.smoothness", 100.0)], cheek),
            ("shine", &[("skin.shine", 100.0)], cheek),
            ("texture", &[("skin.texture", 100.0)], cheek),
            ("even tone", &[("skinTone.evenTone", 100.0)], under),
            ("redness", &[("skinTone.redness", 100.0)], under),
            ("face light", &[("skinTone.faceLight", 50.0)], under),
            ("wrinkles", &[("wrinkles.forehead", 100.0), ("wrinkles.crowsFeet", 100.0), ("wrinkles.smileLines", 100.0)], cheek),
            ("slim face", &[("reshape.slimFace", 100.0)], cheek),
            ("eye size", &[("reshape.eyeSize", 100.0)], eye),
        ];
        let plain = render(&src, &five, &base, &req).image;
        let mut failures = Vec::new();
        for (name, set, win) in &cases {
            // (wrinkles and reshape need the mesh; the eyes' features read the eyes the picture has, which the
            // made-up mesh misses)
            let mut reached = false;
            for (with, info) in [("five points", &five), ("a mesh", &mesh)] {
                let mut s = base.clone();
                for (id, v) in *set {
                    controls::set(&mut s, &format!("retouch.{id}"), *v);
                }
                let full = render(&src, info, &s, &req).image;
                let zoomed = render(&src, info, &s, &RenderRequest { window: Some(*win), ..req }).image;
                assert_eq!((zoomed.width, zoomed.height), (win.w, win.h));
                let (mut worst, mut changed) = (0u8, 0);
                for y in 0..win.h {
                    for x in 0..win.w {
                        let (a, b, c) = (full.get(win.x + x, win.y + y), zoomed.get(x, y), plain.get(win.x + x, win.y + y));
                        worst = worst.max((0..3).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0));
                        changed += usize::from(a != c);
                    }
                }
                reached |= changed > 0;
                if worst > 1 {
                    failures.push(format!("{name} ({with}): the zoomed window differs from the whole frame by {worst}"));
                }
            }
            if !reached {
                failures.push(format!("{name}: the retouch does not reach the window"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// Stray Hair reads furthest (a band half a face past the hair): a zoomed window on a flyaway against the sky
    /// gives the whole frame's repair.
    #[test]
    fn a_zoomed_window_on_a_flyaway_gives_the_whole_frames_repair() {
        use lightcraft_develop::retouch::{Part, PartMask};
        let (w, h) = (640, 480);
        let ell = |nx: f64, ny: f64, c: (f64, f64), rx: f64, ry: f64| ((nx - c.0) / rx).powi(2) + ((ny - c.1) / ry).powi(2) <= 1.0;
        let face_at = move |nx: f64, ny: f64| ell(nx, ny, (0.5, 0.55), 0.08, 0.13);
        let hair_at = move |nx: f64, ny: f64| ell(nx, ny, (0.5, 0.47), 0.12, 0.14) && ny < 0.52 && !face_at(nx, ny);
        // a dark strand a pixel wide in the sky beside the hair
        let strand = |nx: f64, ny: f64| {
            let (ax, ay, bx, by) = (0.345 * 640.0, 0.5 * 480.0, 0.335 * 640.0, 0.36 * 480.0);
            let (px, py) = (nx * 640.0, ny * 480.0);
            let t = (((px - ax) * (bx - ax) + (py - ay) * (by - ay)) / ((bx - ax).powi(2) + (by - ay).powi(2))).clamp(0.0, 1.0);
            ((px - ax - t * (bx - ax)).powi(2) + (py - ay - t * (by - ay)).powi(2)).sqrt() < 0.6
        };
        let src = Rgb32f::from_fn(w, h, |x, y| {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            if strand(nx, ny) || hair_at(nx, ny) {
                [0.03, 0.02, 0.015]
            } else if face_at(nx, ny) {
                [0.5, 0.36, 0.28]
            } else {
                [0.55, 0.65, 0.78]
            }
        });
        // the masks cover the head only: the strand lies outside them, in the band Stray Hair searches past the hair
        let area = NormRect::new(0.375, 0.32, 0.625, 0.69);
        let mask = |f: &dyn Fn(f64, f64) -> bool| {
            let n = 96;
            let plane = lightcraft_raster::Plane::from_fn(n, n, |x, y| {
                f64::from(u8::from(f(area.x0 + (x as f64 + 0.5) / n as f64 * area.width(), area.y0 + (y as f64 + 0.5) / n as f64 * area.height())))
                    as f32
            });
            PartMask { plane, area }
        };
        let face = Face {
            masks: vec![(Part::Hair, mask(&hair_at)), (Part::FacialSkin, mask(&face_at))],
            ..Face::boxed(NormRect::new(0.42, 0.42, 0.58, 0.68))
        };
        // the strands are searched once, on the photo, as the face provider does with the people parse
        let faces = lightcraft_develop::retouch::hair::with_strands(&src, vec![face]);
        let info = SourceInfo { retouch: Some(Arc::new(Input::new_on_photo(faces))), ..SourceInfo::default() };
        let mut s = DevelopSettings::default();
        s.detail.nr_color = 0.0;
        let plain = s.clone();
        controls::set(&mut s, "retouch.hair.amount", 100.0);
        let req = RenderRequest::fit(w, h);
        // (its left edge on the strand, clear of the masks' area: only the reach makes the window see what is
        // beside the strand there)
        let win = crate::PixelWindow { x: 217, y: 180, w: 20, h: 50 };
        let full = render(&src, &info, &s, &req).image;
        let before = render(&src, &info, &plain, &req).image;
        let zoomed = render(&src, &info, &s, &RenderRequest { window: Some(win), ..req }).image;
        assert_eq!((zoomed.width, zoomed.height), (win.w, win.h));
        let (mut worst, mut changed) = (0u8, 0);
        for y in 0..win.h {
            for x in 0..win.w {
                let (a, b, c) = (full.get(win.x + x, win.y + y), zoomed.get(x, y), before.get(win.x + x, win.y + y));
                worst = worst.max((0..3).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0));
                changed += usize::from(a != c);
            }
        }
        assert!(changed > 20, "the strand is repaired in the window ({changed} changed)");
        assert!(worst <= 1, "the zoomed window differs from the whole frame by {worst}");
    }

    /// The window grows only for a face whose reads reach it, and not at all with retouching off.
    #[test]
    fn the_window_grows_for_a_retouched_face_it_touches() {
        let (w, h) = (480, 360);
        let (src, info) = face_photo(w, h);
        let mut s = DevelopSettings::default();
        let win = crate::PixelWindow { x: 170, y: 136, w: 30, h: 40 };
        let far = crate::PixelWindow { x: 0, y: 0, w: 20, h: 20 };
        let frame = crate::plan(&src, &info, &s, &RenderRequest::fit(w, h)).frame;
        assert_eq!(window_for_reads(&s, &info, &frame, w, h, win), win, "off: no growth");
        controls::set(&mut s, "retouch.eyes.whites", 50.0);
        let grown = window_for_reads(&s, &info, &frame, w, h, win);
        assert!(grown.x < win.x && grown.w > win.w + 100, "{grown:?}");
        assert_eq!(window_for_reads(&s, &SourceInfo::default(), &frame, w, h, win), win, "no faces");
        // a small face in a corner: a window across the frame from it keeps its size, one on it grows
        let corner = Face::boxed(NormRect::new(0.8, 0.8, 0.9, 0.9));
        let small = SourceInfo { retouch: Some(Arc::new(Input::new(vec![corner]))), ..SourceInfo::default() };
        assert_eq!(window_for_reads(&s, &small, &frame, w, h, far), far, "a window away from the face");
        let on = crate::PixelWindow { x: 400, y: 300, w: 10, h: 10 };
        let g = window_for_reads(&s, &small, &frame, w, h, on);
        assert!(g.x < 384 && g.y < 288 && g.x + g.w >= 432 && g.y + g.h >= 324, "the corner face's reads: {g:?}");
    }

    /// A face's later feature reads what an earlier feature changed for a neighbouring face, so a window that
    /// touches one face holds the reads of every face whose reads touch it, in a chain: three faces in a row, each
    /// one's reads overlapping the next's, and a window at the far end of the last.
    #[test]
    fn the_window_holds_the_reads_of_faces_chained_to_it() {
        let (w, h) = (480, 360);
        let (src, _) = face_photo(w, h);
        let faces: Vec<Face> = [0.1, 0.3, 0.5].iter().map(|x| Face::boxed(NormRect::new(*x, 0.4, x + 0.1, 0.5))).collect();
        let info = SourceInfo { retouch: Some(Arc::new(Input::new(faces.clone()))), ..SourceInfo::default() };
        let mut s = DevelopSettings::default();
        controls::set(&mut s, "retouch.eyes.whites", 50.0);
        let frame = crate::plan(&src, &info, &s, &RenderRequest::fit(w, h)).frame;
        let view = FrameView::new(&frame, w, h, s.orientation);
        let Some(reads) = s.retouch.reads() else { panic!("eyes on") };
        let boxes: Vec<[f64; 4]> = faces
            .iter()
            .filter_map(|f| match read_box(f, &reads, None, &view) {
                ReadBox::At(b) => Some(b),
                _ => None,
            })
            .collect();
        assert_eq!(boxes.len(), 3);
        // (each face's reads overlap the next one's, not the one after)
        assert!(boxes[0][2] > boxes[1][0] && boxes[1][2] > boxes[2][0] && boxes[0][2] < boxes[2][0], "{boxes:?}");
        let win = crate::PixelWindow { x: 310, y: 190, w: 8, h: 8 };
        assert!(boxes[2][2] > 318.0 && boxes[1][2] < 310.0, "the window touches only the last face's reads: {boxes:?}");
        let g = window_for_reads(&s, &info, &frame, w, h, win);
        let (gx0, gx1) = (g.x as f64, (g.x + g.w) as f64);
        for b in &boxes {
            assert!(gx0 <= b[0].max(0.0) && gx1 >= b[2].min(w as f64), "the window {g:?} holds the reads {b:?}");
        }
    }

    /// Several features on one face, run one after another, read only inside the face's read box, so the largest
    /// reach is enough: a zoomed window still equals the whole frame.
    #[test]
    fn chained_features_on_one_face_match_the_whole_frame() {
        let (w, h) = (480, 360);
        let (src, info) = face_photo(w, h);
        let mut s = DevelopSettings::default();
        s.detail.nr_color = 0.0;
        for id in ["skin.smoothness", "skin.shine", "skinTone.evenTone", "skinTone.redness", "eyes.whites", "eyes.darkCircles", "eyes.eyeBags"] {
            controls::set(&mut s, &format!("retouch.{id}"), 100.0);
        }
        controls::set(&mut s, "retouch.skinTone.faceLight", 50.0);
        let req = RenderRequest::fit(w, h);
        let full = render(&src, &info, &s, &req).image;
        for win in [crate::PixelWindow { x: 170, y: 136, w: 30, h: 40 }, crate::PixelWindow { x: 200, y: 220, w: 40, h: 30 }] {
            let zoomed = render(&src, &info, &s, &RenderRequest { window: Some(win), ..req }).image;
            let mut worst = 0u8;
            for y in 0..win.h {
                for x in 0..win.w {
                    let (a, b) = (full.get(win.x + x, win.y + y), zoomed.get(x, y));
                    worst = worst.max((0..3).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0));
                }
            }
            assert!(worst <= 1, "{win:?}: the zoomed window differs from the whole frame by {worst}");
        }
    }
}
