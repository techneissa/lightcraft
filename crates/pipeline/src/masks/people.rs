//! Fork (`fork/select-people.md`): the edge filter of a Select People part's grids.
//!
//! A Select People part stores one grid over the selection and up to four finer ones over small faces. The guided
//! filter that lands a grid's edge on the photo's ([`super::sample_seg`]) is sized from the grid that gave each pixel
//! its logit: the coarse grid's wide window would otherwise smear a small face's lips or irises, and an edge an edge
//! model drew (`+matting`, `+vision`) is not filtered at all, so preview and export draw the same outline.

use lightcraft_develop::SegMask;
use lightcraft_geom::Point;
use lightcraft_raster::Plane;

use super::{SEG_EPS, SEG_WINDOW, SegGrid, guided_cross_fast, smooth};
use crate::for_rows;
use crate::geometry::Frame;

/// Whether these grids are a Select People part's (their model is `people:<set>`).
pub(super) fn is_people(seg: Option<&SegMask>, detail: &[SegMask]) -> bool {
    model(seg, detail).is_some_and(|m| m.starts_with("people:"))
}

fn model<'a>(seg: Option<&'a SegMask>, detail: &'a [SegMask]) -> Option<&'a str> {
    seg.or(detail.first()).and_then(|s| s.model.as_deref())
}

/// Filter `out` (the logits [`super::sample_seg`] sampled, `coarse` outside every finer grid, the highest of `patches`
/// inside them) with each pixel's own grid's window.
#[allow(clippy::too_many_arguments)]
pub(super) fn filter(
    coarse: Option<&SegGrid>,
    seg: Option<&SegMask>,
    patches: &[SegGrid],
    detail: &[SegMask],
    gain: f32,
    frame: &Frame,
    w: usize,
    log_l: &Plane,
    out: &mut Plane,
) {
    if super::edge_drawn(seg, detail, true) {
        return;
    }
    let h = out.height;
    // which grid gave each pixel its logit: 0 the coarse grid, k the k-th finer one
    let mut owner = vec![0u8; w * h];
    if !patches.is_empty() {
        let m = frame.out_to_mask(w, h);
        for_rows(&mut owner, w, |y, row| {
            for (x, o) in row.iter_mut().enumerate() {
                let n = m.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
                let best = patches.iter().enumerate().filter(|(_, p)| p.contains(n)).map(|(k, p)| (k, p.logit(n))).max_by(|a, b| a.1.total_cmp(&b.1));
                *o = best.map_or(0, |(k, _)| u8::try_from(k + 1).unwrap_or(u8::MAX));
            }
        });
    }
    // each grid's window (none where its cells are no wider than a pixel), in output pixels
    let windows: Vec<Option<f32>> = std::iter::once(coarse)
        .chain(patches.iter().map(Some))
        .map(|g| g.map(|g| g.cell_px(frame, w)).filter(|c| *c >= 1.0).map(|c| (SEG_WINDOW * c) as f32))
        .collect();
    let mut done: Vec<f32> = Vec::new();
    let sampled = out.clone();
    for window in windows.iter().flatten() {
        if done.iter().any(|d| (d - window).abs() < 1e-3) {
            continue;
        }
        done.push(*window);
        let mut filtered = sampled.clone();
        guided_cross_fast(log_l, &mut filtered, *window, SEG_EPS, |mean| 1.0 - smooth(3.0 * gain, 7.0 * gain, mean.abs()));
        for ((v, f), o) in out.data.iter_mut().zip(&filtered.data).zip(&owner) {
            if windows.get(usize::from(*o)).copied().flatten().is_some_and(|ow| (ow - window).abs() < 1e-3) {
                *v = *f;
            }
        }
    }
}
