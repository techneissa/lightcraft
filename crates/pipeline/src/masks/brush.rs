//! Brush parts: each stroke is stamped inside its own bounding box, and a part whose strokes
//! extend the strokes of an earlier render stamps only the new ones over that render's plane.

use std::cell::Cell;
use std::sync::Arc;

use lightcraft_develop::BrushStroke;
use lightcraft_raster::{Plane, Rgb32f};

use super::alpha::HashWrite;
use super::{accumulate, auto_refine, auto_similarity, brush_dabs, chromaticity, dab_alpha, dab_pixel, guided_cross};
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
    use std::fmt::Write;
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher, Hash, Hasher};
    let mut prev = 0u64;
    strokes
        .iter()
        .map(|s| {
            let mut h = BuildHasherDefault::<DefaultHasher>::default().build_hasher();
            prev.hash(&mut h);
            // (`Debug` of an f64 round-trips: distinct strokes write distinct text)
            let _ = write!(HashWrite(&mut h), "{s:?}");
            prev = h.finish();
            prev
        })
        .collect()
}

/// The plane of brush strokes `strokes` (max-combine; erase strokes subtract), `w × h`. With a
/// `ctx`, strokes that extend a part it holds are stamped over that part's plane and nothing else.
///
/// Auto Mask strokes weight each dab by [`auto_similarity`] to the pixel under its centre and are
/// refined with [`guided_cross`] on log luminance.
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

/// The rectangle `[x0, x1) × [y0, y1)` of `p`, as a plane.
fn crop(p: &Plane, x0: usize, y0: usize, x1: usize, y1: usize) -> Plane {
    let mut data = Vec::with_capacity((x1 - x0) * (y1 - y0));
    for y in y0..y1 {
        data.extend_from_slice(&p.row(y)[x0..x1]);
    }
    Plane { width: x1 - x0, height: y1 - y0, data }
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
    let refs: Vec<Option<(f32, [f32; 3])>> = bd
        .dabs
        .iter()
        .map(|d| {
            auto.then(|| {
                let j = dab_pixel(*d, w, h);
                (log_l.data[j], chromaticity(img.data[j]))
            })
        })
        .collect();
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
                if let Some(Some((rl, rch))) = refs.get(k) {
                    let i = y * w + x;
                    a *= auto_similarity(log_l.data[i], chromaticity(img.data[i]), *rl, *rch);
                }
                if let Some(v) = row.get_mut(x - x0) {
                    // flow accumulates within a stroke up to density
                    *v = accumulate(*v, a, flow, dens);
                }
            }
        }
    });
    // (the rectangle the stroke can change in `out`)
    let mut region = (x0, y0, x1, y1);
    if auto {
        // The refinement fills gaps along edges; it never takes paint away. Its filter reaches
        // `reach` px from the paint (two blurs), so it runs on the stroke's box plus twice that,
        // where the box's edge cannot change what is within `reach` of the paint, and nothing past
        // `reach` changes.
        let (sigma, eps) = auto_refine(bd.r);
        let reach = 2 * lightcraft_raster::blur::box_radii(sigma).iter().sum::<usize>() + 2;
        let (cx0, cy0) = (x0.saturating_sub(2 * reach), y0.saturating_sub(2 * reach));
        let (cx1, cy1) = ((x1 + 2 * reach).min(w), (y1 + 2 * reach).min(h));
        let guide = crop(log_l, cx0, cy0, cx1, cy1);
        let mut padded = Plane::new(cx1 - cx0, cy1 - cy0);
        for ry in 0..bh {
            let dst = (y0 - cy0 + ry) * padded.width + (x0 - cx0);
            if let (Some(d), Some(src)) = (padded.data.get_mut(dst..dst + bw), sp.data.get(ry * bw..(ry + 1) * bw)) {
                d.copy_from_slice(src);
            }
        }
        let q = guided_cross(&guide, &padded, sigma, eps);
        region = (x0.saturating_sub(reach).max(cx0), y0.saturating_sub(reach).max(cy0), (x1 + reach).min(cx1), (y1 + reach).min(cy1));
        let (rx0, ry0, rx1, ry1) = region;
        sp = Plane::new(rx1 - rx0, ry1 - ry0);
        for ry in 0..sp.height {
            let y = ry0 + ry;
            let row = &mut sp.data[ry * sp.width..(ry + 1) * sp.width];
            for (rx, v) in row.iter_mut().enumerate() {
                let (px, py) = (rx0 + rx - cx0, y - cy0);
                let own = if (x0..x1).contains(&(rx0 + rx)) && (y0..y1).contains(&y) { padded.data[py * padded.width + px] } else { 0.0 };
                *v = own.max(q.data[py * q.width + px]);
            }
        }
    }
    let (rx0, ry0, rx1, ry1) = region;
    let rw = rx1 - rx0;
    let Some(rows) = out.data.get_mut(ry0 * w..ry1 * w) else { return };
    for_rows(rows, w, |ry, row| {
        let Some(src) = sp.data.get(ry * rw..(ry + 1) * rw) else { return };
        let Some(dst) = row.get_mut(rx0..rx1) else { return };
        for (o, a) in dst.iter_mut().zip(src) {
            *o = if erase { *o * (1.0 - a) } else { o.max(*a) };
        }
    });
}
