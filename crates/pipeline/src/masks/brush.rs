//! Brush parts: each stroke is stamped inside its own bounding box, and a part whose strokes
//! extend the strokes of an earlier render stamps only the new ones over that render's plane.

use std::cell::Cell;
use std::sync::Arc;

use lightcraft_develop::BrushStroke;
use lightcraft_raster::{Plane, Rgb32f};

use super::auto_edge::{AutoColor, AutoRef, AutoRefs, MedianBox, dab_sample, median_radius, search_margin, search_reach};
use super::{BrushDabs, accumulate, brush_dabs, dab_alpha, dab_pixel};
use crate::for_rows;
use crate::geometry::Frame;

thread_local! {
    static STROKES: Cell<usize> = const { Cell::new(0) };
}

/// How many strokes have been stamped into a brush plane on this thread: a test counts what adding
/// a stroke to a mask rasterizes.
pub fn strokes_rasterized() -> usize {
    STROKES.with(Cell::get)
}

/// Brush part planes kept (the last few: a part being painted into, and its earlier state for an
/// undo). Each is a full-frame plane, so this bounds the memory.
pub const BRUSH_ENTRIES: usize = 4;

/// Count one stroke stamped ([`strokes_rasterized`]): for a renderer that stamps strokes itself.
pub fn note_stroke() {
    STROKES.with(|c| c.set(c.get().wrapping_add(1)));
}

/// What a brush part's plane depends on besides its strokes: the render's frame and size and,
/// for Auto Mask, the image. Parts with the same base and the same leading strokes share a prefix.
pub fn brush_base(geo: u64, image: u64, auto: bool, w: usize, h: usize) -> u64 {
    crate::hash_of((geo, if auto { image } else { 0 }, w, h))
}

/// The planes of recently rasterized brush parts, keyed by the strokes they hold.
#[derive(Clone, Default)]
pub struct BrushCache {
    /// Oldest first.
    entries: Vec<Entry>,
}

#[derive(Clone)]
struct Entry {
    /// The frame, size and (Auto Mask) image the strokes were stamped on.
    base: u64,
    /// `chain[k]` identifies the first `k + 1` strokes, in order.
    chain: Vec<u64>,
    plane: Arc<Plane>,
}

impl BrushCache {
    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        self.entries.iter().map(|e| e.plane.data.len() * std::mem::size_of::<f32>()).sum()
    }
}

/// What a brush part is rasterized against, when its plane may be kept.
pub struct BrushCtx<'a> {
    pub cache: &'a mut BrushCache,
    /// The render's frame and size ([`super::AlphaKeys::geo`]) and, for Auto Mask, its image.
    pub geo: u64,
    pub image: u64,
}

/// Identifies strokes `0..=k` for every `k`: each stroke's fields hashed into the one before.
pub fn brush_chain(strokes: &[BrushStroke]) -> Vec<u64> {
    use std::hash::Hasher;
    let mut prev = 0u64;
    strokes
        .iter()
        .map(|s| {
            let mut h = super::key::Words::new();
            h.write_u64(prev);
            super::key::stroke(&mut h, s);
            prev = h.finish();
            prev
        })
        .collect()
}

/// The plane of brush strokes `strokes` (max-combine; erase strokes subtract), `w × h`. With a
/// `ctx`, strokes that extend a part it holds are stamped over that part's plane and nothing else.
///
/// Auto Mask strokes weight each dab's paint by how close each pixel's colour is to the dab's
/// reference ([`auto_refs`]).
pub fn rasterize(strokes: &[BrushStroke], frame: &Frame, w: usize, h: usize, img: &Rgb32f, log_l: &Plane, ctx: Option<&mut BrushCtx<'_>>) -> Plane {
    if w == 0 || h == 0 {
        return Plane::new(w, h);
    }
    let Some(ctx) = ctx else {
        let mut out = Plane::new(w, h);
        for s in strokes {
            stamp(s, frame, img, log_l, &mut out);
        }
        return out;
    };
    let auto = strokes.iter().any(|s| s.auto_mask);
    let base = brush_base(ctx.geo, ctx.image, auto, w, h);
    let chain = brush_chain(strokes);
    // the part this one extends (the longest held prefix of its strokes)
    let hit = ctx
        .cache
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.base == base && !e.chain.is_empty() && e.chain.len() <= chain.len() && chain.starts_with(&e.chain))
        .max_by_key(|(_, e)| e.chain.len())
        .map(|(i, _)| i);
    let (mut out, from) = match hit {
        Some(i) => {
            // (the most recently used goes last)
            let e = ctx.cache.entries.remove(i);
            let held = e.chain.len();
            ctx.cache.entries.push(e.clone());
            if held == chain.len() {
                return (*e.plane).clone();
            }
            ((*e.plane).clone(), held)
        }
        None => (Plane::new(w, h), 0),
    };
    for s in strokes.iter().skip(from) {
        stamp(s, frame, img, log_l, &mut out);
    }
    if !strokes.is_empty() {
        ctx.cache.entries.push(Entry { base, chain, plane: Arc::new(out.clone()) });
        while ctx.cache.entries.len() > BRUSH_ENTRIES {
            ctx.cache.entries.remove(0);
        }
    }
    out
}

/// The plane `ctx` holds for exactly `strokes` in a `w × h` render (made the most recently used),
/// as [`rasterize`] would return it: a renderer that has to fetch the image to stamp strokes asks
/// this first.
pub fn held_brush(strokes: &[BrushStroke], w: usize, h: usize, ctx: &mut BrushCtx<'_>) -> Option<Plane> {
    let auto = strokes.iter().any(|s| s.auto_mask);
    let base = brush_base(ctx.geo, ctx.image, auto, w, h);
    let chain = brush_chain(strokes);
    let i = ctx.cache.entries.iter().position(|e| e.base == base && !chain.is_empty() && e.chain == chain)?;
    let e = ctx.cache.entries.remove(i);
    let plane = (*e.plane).clone();
    ctx.cache.entries.push(e);
    Some(plane)
}

/// The references of an Auto Mask stroke's dabs ([`AutoRefs`]), and the colours of its box
/// `x0..x1 × y0..y1` with the margin the dabs' samples look into ([`MedianBox`], medians of radius
/// `rho`: [`median_radius`]).
pub(crate) fn auto_refs(
    bd: &BrushDabs,
    img: &Rgb32f,
    log_l: &Plane,
    (x0, y0, x1, y1): (usize, usize, usize, usize),
    rho: usize,
) -> (Vec<AutoRef>, MedianBox) {
    use rayon::prelude::*;
    let (w, h) = (log_l.width, log_l.height);
    let m = search_margin(bd.r.max(0.5));
    let (mx0, my0, mx1, my1) = (x0.saturating_sub(m), y0.saturating_sub(m), (x1 + m).min(w), (y1 + m).min(h));
    // the pixels each row's dabs cover (from, to)
    let mut spans = vec![(usize::MAX, 0usize); my1.saturating_sub(my0)];
    let clamp = |v: f64, max: usize| v.clamp(0.0, max as f64) as usize;
    for (k, d) in bd.dabs.iter().enumerate() {
        let r = bd.dab(k).0;
        let (a, z) = (clamp((d.x - r).floor(), w), clamp((d.x + r).ceil(), w));
        for y in clamp((d.y - r).floor(), h).max(my0)..clamp((d.y + r).ceil(), h).min(my1) {
            if let Some(s) = spans.get_mut(y - my0) {
                *s = (s.0.min(a), s.1.max(z));
            }
        }
    }
    let med = MedianBox::new(img, log_l, (mx0, my0, mx1, my1), rho, |y| spans.get(y.wrapping_sub(my0)).copied().filter(|s| s.0 < s.1));
    let samples: Vec<AutoColor> = (0..bd.dabs.len())
        .into_par_iter()
        .map(|k| {
            let j = bd.dabs.get(k).map_or(0, |d| dab_pixel(*d, w, h));
            dab_sample(|x, y| med.at(x, y), (j % w) as isize, (j / w) as isize, search_reach(bd.dab(k).0))
        })
        .collect();
    let mut chain = AutoRefs::default();
    let refs = samples.into_iter().map(|s| chain.next(s)).collect();
    (refs, med)
}

/// Stamp stroke `s` into `out`: its dabs inside their bounding box (flow accumulating up to density
/// within the stroke), then the stroke combined with `out` there, adding paint or erasing it.
fn stamp(s: &BrushStroke, frame: &Frame, img: &Rgb32f, log_l: &Plane, out: &mut Plane) {
    note_stroke();
    let (w, h) = (out.width, out.height);
    let bd = brush_dabs(s, frame, w, h);
    if bd.dabs.is_empty() {
        return;
    }
    let (dens, erase, auto) = (bd.density, bd.erase, bd.auto);
    // (no dab is larger than the brush: pen pressure only shrinks it)
    let rmax = bd.r.max(0.5);
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for d in &bd.dabs {
        lo = [lo[0].min(d.x), lo[1].min(d.y)];
        hi = [hi[0].max(d.x), hi[1].max(d.y)];
    }
    let clamp = |v: f64, max: usize| v.clamp(0.0, max as f64) as usize;
    let (x0, x1) = (clamp((lo[0] - rmax).floor(), w), clamp((hi[0] + rmax).ceil(), w));
    let (y0, y1) = (clamp((lo[1] - rmax).floor(), h), clamp((hi[1] + rmax).ceil(), h));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let (bw, bh) = (x1 - x0, y1 - y0);
    let (refs, med) =
        if auto { auto_refs(&bd, img, log_l, (x0, y0, x1, y1), median_radius(frame.px_per_long(w))) } else { (Vec::new(), MedianBox::empty()) };
    let mut sp = Plane::new(bw, bh);
    for_rows(&mut sp.data, bw, |ry, row| {
        let y = y0 + ry;
        let cy = y as f64 + 0.5;
        for (k, d) in bd.dabs.iter().enumerate() {
            let (r, hard, flow) = bd.dab(k);
            if (cy - d.y).abs() > r {
                continue;
            }
            let xa = clamp((d.x - r).floor(), w).max(x0);
            let xb = clamp((d.x + r).ceil(), w).min(x1);
            for x in xa..xb {
                let dd = lightcraft_geom::Point::new(x as f64 + 0.5, cy).dist(*d);
                if dd > r {
                    continue;
                }
                let mut a = dab_alpha(dd, r, hard);
                if let Some(rf) = refs.get(k) {
                    a *= rf.weight(med.at(x as isize, y as isize));
                }
                if let Some(v) = row.get_mut(x - x0) {
                    // flow accumulates within a stroke up to density
                    *v = accumulate(*v, a, flow, dens);
                }
            }
        }
    });
    let Some(rows) = out.data.get_mut(y0 * w..y1 * w) else { return };
    for_rows(rows, w, |ry, row| {
        let Some(src) = sp.data.get(ry * bw..(ry + 1) * bw) else { return };
        let Some(dst) = row.get_mut(x0..x1) else { return };
        for (o, a) in dst.iter_mut().zip(src) {
            *o = if erase { *o * (1.0 - a) } else { o.max(*a) };
        }
    });
}

#[cfg(test)]
#[path = "tests_auto_edge.rs"]
mod tests_auto_edge;
