//! Spots on the device drawn again only where an edit reaches ([`lightcraft_pipeline::spots_keep`], issue #82): the
//! view keeps the white-balanced image before its spots and the image after them on the device.

use std::sync::Arc;

use lightcraft_pipeline::spots_keep::{self as keep, Kept, Rect};
use lightcraft_pipeline::{Plan, SourceInfo};

use crate::ctx::Buf;
use crate::render::Cx;

/// The image before the spots (`before_spots` makes it, unless `kept` holds it) with the spots drawn: only those an
/// edit reaches when `kept` holds this image's last spots. Returns what to keep for the next render (`None` without
/// spots).
pub(crate) fn spots(
    cx: &mut Cx<'_>,
    info: &SourceInfo,
    plan: &Plan<'_>,
    kept: Option<&Kept<Buf>>,
    before_spots: impl FnOnce(&mut Cx<'_>) -> Buf,
) -> (Buf, Option<Arc<Kept<Buf>>>) {
    let s = &*plan.settings;
    if s.spots.is_empty() {
        return (before_spots(cx), None);
    }
    let (w, h) = (plan.w, plan.h);
    let key = keep::key(info, plan);
    let hit = kept.filter(|k| k.key == key && k.pre.len == w * h * 3 && k.post.len == w * h * 3);
    let pre = match hit {
        Some(k) => k.pre.clone(),
        None => Arc::new(before_spots(cx)),
    };
    let new = keep::entries(&s.spots, &[], &plan.frame, w, h, plan.px_per_long);
    let (work, draw) = match hit.and_then(|k| keep::redo(&k.drawn, &new).map(|r| (k, r))) {
        Some((k, r)) => {
            let work = cx.copy(&k.post);
            restore(cx, &pre, &work, &r.restore, w);
            (work, r.draw)
        }
        None => (cx.copy(&pre), vec![true; s.spots.len()]),
    };
    crate::render::draw_spots(cx, &work, plan, &draw);
    let post = Arc::new(work);
    // (what follows writes its own buffers, but the kept one must never be handed on)
    let out = cx.copy(&post);
    (out, Some(Arc::new(Kept { key, pre, post, drawn: new, done: Vec::new() })))
}

/// The pixels of `rects` copied from `from` into `to` (RGB images `w` pixels wide), row by row.
fn restore(cx: &mut Cx<'_>, from: &Buf, to: &Buf, rects: &[Rect], w: usize) {
    for r in merged(rects) {
        let (x0, x1) = (r[0].min(w), r[2].min(w));
        if x1 <= x0 {
            continue;
        }
        if x0 == 0 && x1 == w {
            // whole rows: one copy
            cx.copy_words(from, r[1] * w * 3, to, r[1] * w * 3, (r[3].saturating_sub(r[1])) * w * 3);
            continue;
        }
        for y in r[1]..r[3] {
            cx.copy_words(from, (y * w + x0) * 3, to, (y * w + x0) * 3, (x1 - x0) * 3);
        }
    }
}

/// `rects` without the ones another one holds (each pixel is copied once per rectangle holding it).
fn merged(rects: &[Rect]) -> Vec<Rect> {
    let inside = |a: &Rect, b: &Rect| a[0] >= b[0] && a[1] >= b[1] && a[2] <= b[2] && a[3] <= b[3];
    let mut out: Vec<Rect> = Vec::new();
    for (i, r) in rects.iter().enumerate() {
        // (of two equal rectangles the first stays)
        let held = rects.iter().enumerate().any(|(j, o)| j != i && inside(r, o) && (r != o || j < i));
        if !held && r[2] > r[0] && r[3] > r[1] {
            out.push(*r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn rectangles_held_by_another_are_copied_once() {
        let m = super::merged(&[[0, 0, 10, 10], [2, 2, 5, 5], [0, 0, 10, 10], [8, 8, 20, 20], [3, 3, 3, 9]]);
        assert_eq!(m, vec![[0, 0, 10, 10], [8, 8, 20, 20]]);
    }
}
