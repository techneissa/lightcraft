//! One feature run with its Refine strokes: Add widens the masks the feature reads for the face nearest each Add
//! stroke, and Reduce mixes the run's result back toward the pixels before it (all the way at a full Reduce, which
//! then leaves the original bits).

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_raster::{Plane, Rgb32f, par_rows};

use super::RefineStroke;
use super::raster::{Kind, Shape, grid};
use crate::feature::{Feature, Scene, View};
use crate::input::{Face, Fnv, NormRect, PartMask};

/// Largest side of a widened mask (cells).
const MASK_MAX: usize = 1024;
/// Widened masks kept (each per face, part and strokes).
const CACHE_ENTRIES: usize = 16;

/// Run `s` on `img` for `face` (`None`: the whole picture) with `strokes`, the feature's Refine strokes for it.
pub(crate) fn run_refined<T: Feature>(img: &mut Rgb32f, scene: &Scene<'_>, s: &T, face: Option<usize>, strokes: &[RefineStroke]) {
    let Some(shape) = edit_shape(scene.view()) else {
        s.apply(img, scene, face);
        return;
    };
    let cut = grid(strokes, &|_| true, Kind::Cut, shape).map(|(_, g)| g);
    let widened = widen::<T>(scene, face, strokes, shape);
    let saved = cut.as_ref().and_then(|g| Saved::take(img, scene.view(), &g.area));
    match &widened {
        Some(faces) => {
            let refs: Vec<&Face> = faces.iter().zip(&scene.faces).map(|(w, f)| w.as_deref().unwrap_or(f.face)).collect();
            let wide = scene.with_faces(&refs);
            s.apply(img, &wide, face);
        }
        None => s.apply(img, scene, face),
    }
    if let (Some(g), Some(saved)) = (cut, saved) {
        saved.mix_back(img, scene.view(), &g);
    }
}

/// The edit frame's shape (its width over its height, as it maps into the image).
fn edit_shape(view: &dyn View) -> Option<Shape> {
    let (o, x, y) = (view.edit_to_px(0.0, 0.0)?, view.edit_to_px(1.0, 0.0)?, view.edit_to_px(0.0, 1.0)?);
    let len = |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    Shape::from_aspect(len(o, x) / len(o, y))
}

/// The pixels a grid over `area` (edit frame) covers in the image: `[x0, y0, x1, y1)`, clamped; `None` outside it.
fn pixel_box(view: &dyn View, area: &NormRect, w: usize, h: usize) -> Option<[usize; 4]> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for i in 0..=8 {
        let t = i as f64 / 8.0;
        let (u, v) = (area.x0 + t * area.width(), area.y0 + t * area.height());
        for (nx, ny) in [(u, area.y0), (u, area.y1), (area.x0, v), (area.x1, v)] {
            if let Some((x, y)) = view.edit_to_px(nx, ny).filter(|(x, y)| x.is_finite() && y.is_finite()) {
                b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
            }
        }
    }
    let clamp = |v: f64, max: usize| v.clamp(0.0, max as f64) as usize;
    let r = [clamp(b[0].floor() - 1.0, w), clamp(b[1].floor() - 1.0, h), clamp(b[2].ceil() + 1.0, w), clamp(b[3].ceil() + 1.0, h)];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

/// The pixels under a Reduce grid as they were before the feature ran.
struct Saved {
    b: [usize; 4],
    px: Vec<[f32; 3]>,
}

impl Saved {
    fn take(img: &Rgb32f, view: &dyn View, area: &NormRect) -> Option<Saved> {
        let b = pixel_box(view, area, img.width, img.height)?;
        let mut px = Vec::with_capacity((b[2] - b[0]) * (b[3] - b[1]));
        for y in b[1]..b[3] {
            px.extend_from_slice(img.data.get(y * img.width + b[0]..y * img.width + b[2])?);
        }
        Some(Saved { b, px })
    }

    /// Mix each pixel back toward its saved value by the grid's cut there (the saved bits at a full cut).
    fn mix_back(&self, img: &mut Rgb32f, view: &dyn View, g: &PartMask) {
        let [x0, y0, x1, y1] = self.b;
        let (w, bw) = (img.width, x1 - x0);
        let Some(rows) = img.data.get_mut(y0 * w..y1 * w) else { return };
        par_rows(rows, w, |ry, row| {
            let y = y0 + ry;
            let (Some(dst), Some(src)) = (row.get_mut(x0..x1), self.px.get(ry * bw..(ry + 1) * bw)) else { return };
            for (i, (d, o)) in dst.iter_mut().zip(src).enumerate() {
                let (ex, ey) = view.to_edit((x0 + i) as f64 + 0.5, y as f64 + 0.5);
                let c = g.sample(ex, ey);
                if c >= 1.0 {
                    *d = *o;
                } else if c > 0.0 {
                    for k in 0..3 {
                        d[k] += (o[k] - d[k]) * c;
                    }
                }
            }
        });
    }
}

/// The faces of `scene` with the masks `T` widens ([`Feature::ADD_PARTS`]) widened by the Add strokes nearest each
/// of them (`None` for a face they leave as it is); `None` when no Add stroke widens anything this run reads.
fn widen<T: Feature>(scene: &Scene<'_>, face: Option<usize>, strokes: &[RefineStroke], shape: Shape) -> Option<Vec<Option<Arc<Face>>>> {
    if T::ADD_PARTS.is_empty() || !strokes.iter().any(|s| s.mode == super::RefineMode::Add) {
        return None;
    }
    let owner: Vec<Option<usize>> = strokes.iter().map(|s| nearest_face(scene, s)).collect();
    let mut out: Vec<Option<Arc<Face>>> = vec![None; scene.faces.len()];
    for (k, slot) in out.iter_mut().enumerate() {
        if face.is_some_and(|f| f != k) {
            continue;
        }
        let Some((key, add)) = grid(strokes, &|i| owner.get(i).copied().flatten() == Some(k), Kind::Add, shape) else { continue };
        let Some(f) = scene.faces.get(k) else { continue };
        let mut wide = f.face.clone();
        for part in T::ADD_PARTS {
            let m = widened_mask(scene.view(), f.face.mask(*part), &add, key);
            match wide.masks.iter_mut().find(|(p, _)| p == part) {
                Some((_, slot)) => *slot = (*m).clone(),
                None => wide.masks.push((*part, (*m).clone())),
            }
        }
        *slot = Some(Arc::new(wide));
    }
    (!out.iter().all(Option::is_none)).then_some(out)
}

/// The face an Add stroke widens: the one whose box is nearest its points' centre, in units of the face's size.
fn nearest_face(scene: &Scene<'_>, s: &RefineStroke) -> Option<usize> {
    if s.mode != super::RefineMode::Add {
        return None;
    }
    let n = s.points.len().max(1) as f64;
    let (sx, sy) = s.points.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let (px, py) = scene.view().edit_to_px(sx / n, sy / n)?;
    let dist = |b: &[f64; 4], size: f64| {
        let dx = (b[0] - px).max(px - b[2]).max(0.0);
        let dy = (b[1] - py).max(py - b[3]).max(0.0);
        let d = (dx * dx + dy * dy).sqrt() / size.max(1e-9);
        if d.is_finite() { d } else { f64::INFINITY }
    };
    scene.faces.iter().enumerate().map(|(k, f)| (k, dist(&f.bounds, f.size))).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(k, _)| k)
}

static WIDE: Mutex<Vec<(u64, Arc<PartMask>)>> = Mutex::new(Vec::new());

/// `old` (a face's mask, in the faces' frame) widened by `add` (a grid in the edit frame): the larger of the two at
/// each point. Kept per mask, grid and the map between the two frames.
fn widened_mask(view: &dyn View, old: Option<&PartMask>, add: &PartMask, add_key: u64) -> Arc<PartMask> {
    let to_edit = |n: (f64, f64)| view.to_px(n.0, n.1).map(|(x, y)| view.to_edit(x, y));
    // the add grid's area in the faces' frame
    let a = add.area;
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for i in 0..=16 {
        let t = i as f64 / 16.0;
        let (u, v) = (a.x0 + t * a.width(), a.y0 + t * a.height());
        for (ex, ey) in [(u, a.y0), (u, a.y1), (a.x0, v), (a.x1, v)] {
            if let Some((nx, ny)) = view.edit_to_px(ex, ey).map(|(x, y)| view.to_norm(x, y)).filter(|(x, y)| x.is_finite() && y.is_finite()) {
                b = [b[0].min(nx), b[1].min(ny), b[2].max(nx), b[3].max(ny)];
            }
        }
    }
    let mut area = NormRect::new(b[0], b[1], b[2], b[3]);
    if let Some(o) = old.filter(|o| o.area.is_valid()) {
        area = if area.is_valid() {
            NormRect::new(area.x0.min(o.area.x0), area.y0.min(o.area.y0), area.x1.max(o.area.x1), area.y1.max(o.area.y1))
        } else {
            o.area
        };
    }
    let mut h = Fnv::new();
    h.u64(add_key);
    for v in [area.x0, area.y0, area.x1, area.y1] {
        h.u64(v.to_bits());
    }
    // the map between the frames, at a few points (rounded: renders of one photo differ by rounding)
    for (u, v) in [(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (0.0, 0.5), (0.5, 0.5), (1.0, 0.5), (0.0, 1.0), (0.5, 1.0), (1.0, 1.0)] {
        let (ex, ey) = to_edit((area.x0 + u * area.width(), area.y0 + v * area.height())).unwrap_or((f64::NAN, f64::NAN));
        h.u64(((ex * 1e7).round()).to_bits());
        h.u64(((ey * 1e7).round()).to_bits());
    }
    match old {
        None => h.u64(u64::MAX),
        Some(o) => {
            for v in [o.area.x0, o.area.y0, o.area.x1, o.area.y1] {
                h.u64(v.to_bits());
            }
            h.u64(o.plane.width as u64);
            h.u64(o.plane.height as u64);
            for v in &o.plane.data {
                h.u64(u64::from(v.to_bits()));
            }
        }
    }
    let key = h.0;
    {
        let c = WIDE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, m)) = c.iter().find(|(k, _)| *k == key) {
            return m.clone();
        }
    }
    let m = Arc::new(make_widened(old, add, area, &to_edit));
    let mut c = WIDE.lock().unwrap_or_else(PoisonError::into_inner);
    c.retain(|(k, _)| *k != key);
    c.push((key, m.clone()));
    while c.len() > CACHE_ENTRIES {
        c.remove(0);
    }
    m
}

fn make_widened(old: Option<&PartMask>, add: &PartMask, area: NormRect, to_edit: &(dyn Fn((f64, f64)) -> Option<(f64, f64)> + Sync)) -> PartMask {
    if !area.is_valid() {
        return old.cloned().unwrap_or(PartMask { plane: Plane::new(0, 0), area });
    }
    // cells as fine as the finer of the two
    let cell = |m: &PartMask| (m.area.width() / m.plane.width.max(1) as f64, m.area.height() / m.plane.height.max(1) as f64);
    let (mut cw, mut ch) = cell(add);
    if let Some(o) = old.filter(|o| o.plane.width > 0 && o.plane.height > 0) {
        let (ow, oh) = cell(o);
        (cw, ch) = (cw.min(ow), ch.min(oh));
    }
    let side = |e: f64, c: f64| if c.is_finite() && c > 0.0 { ((e / c).ceil() as usize).clamp(1, MASK_MAX) } else { 1 };
    let (w, h) = (side(area.width(), cw), side(area.height(), ch));
    let mut plane = Plane::new(w, h);
    par_rows(&mut plane.data, w, |j, row| {
        let ny = area.y0 + (j as f64 + 0.5) * area.height() / h as f64;
        for (i, v) in row.iter_mut().enumerate() {
            let nx = area.x0 + (i as f64 + 0.5) * area.width() / w as f64;
            let base = old.map_or(0.0, |o| o.sample(nx, ny));
            let wide = to_edit((nx, ny)).map_or(0.0, |(ex, ey)| add.sample(ex, ey));
            *v = base.max(wide);
        }
    });
    PartMask { plane, area }
}

impl<'a> Scene<'a> {
    /// This scene with its faces' masks taken from `faces` (one per face of [`Scene::faces`], same order).
    pub(crate) fn with_faces<'b>(&'b self, faces: &[&'b Face]) -> Scene<'b> {
        let list = self
            .faces
            .iter()
            .zip(faces)
            .map(|(f, w)| crate::FaceCtx { face: *w, index: f.index, bounds: f.bounds, landmarks: f.landmarks.clone(), size: f.size })
            .collect();
        Scene::from_parts(list, self.width, self.height, self.view())
    }
}
