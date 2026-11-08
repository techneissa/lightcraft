//! Spots on the device drawn again only where an edit reaches ([`lightcraft_pipeline::spots_keep`], issue #82): the
//! view keeps the image after its spots and each spot's box before and after it drew, on the device.

use std::sync::Arc;

use lightcraft_pipeline::spots_keep::{self as keep, Kept, Patch, Rect, Step};
use lightcraft_pipeline::{Plan, SourceInfo};

use crate::ctx::Buf;
use crate::render::{Cx, draw_spot};

type DeviceKept = Kept<Buf, Buf>;
type DevicePatch = Option<Arc<Patch<Buf>>>;

/// The image before the spots (`before_spots` makes it, unless `kept` holds this image's last spots) with the spots
/// drawn: with `kept`, only those an edit reaches. Returns what to keep for the next render (`None` without spots).
pub(crate) fn spots(
    cx: &mut Cx<'_>,
    info: &SourceInfo,
    plan: &Plan<'_>,
    kept: Option<&DeviceKept>,
    before_spots: impl FnOnce(&mut Cx<'_>) -> Buf,
) -> (Buf, Option<Arc<DeviceKept>>) {
    let s = &*plan.settings;
    if s.spots.is_empty() {
        return (before_spots(cx), None);
    }
    let (w, h) = (plan.w, plan.h);
    let key = keep::key(info, plan, &[]);
    let new = keep::entries(&s.spots, &[], &plan.frame, w, h, plan.px_per_long);
    let mut patches: Vec<DevicePatch> = vec![None; new.len()];
    let plan_from =
        kept.filter(|k| k.key == key && k.post.len == w * h * 3).map(|k| (k, keep::redo(&k.drawn, &new))).filter(|(k, r)| k.has_patches(r));
    if crate::render::profiling() {
        let why = match (kept, &plan_from) {
            (_, Some((_, r))) => format!("drew {} (took off {})", r.drawn(), r.undo.len()),
            (None, _) => "nothing kept: drew all".to_string(),
            (Some(k), None) if k.key != key => "image before the spots changed: drew all".to_string(),
            (Some(_), None) => "patches missing: drew all".to_string(),
        };
        eprintln!("  gpu spots: {} of {} in {w}×{h}: {why}", new.iter().flatten().count(), new.len());
    }
    let img = match plan_from {
        Some((k, r)) => {
            let img = cx.copy(&k.post);
            // (a spot not drawn again keeps its patches: it is the old one, reading the same pixels)
            for (slot, old) in patches.iter_mut().zip(&r.same) {
                *slot = old.and_then(|o| k.patches.get(o).cloned().flatten());
            }
            for u in &r.undo {
                if let (Some(Some(d)), Some(Some(p))) = (k.drawn.get(*u), k.patches.get(*u)) {
                    patch(cx, &img, &p.before, &d.at.write, w, h, true);
                }
            }
            for step in &r.steps {
                match *step {
                    Step::Draw(i) => draw(cx, &img, plan, &new, i, &mut patches),
                    Step::Paste { i, old } => {
                        if let (Some(Some(d)), Some(Some(p))) = (new.get(i), k.patches.get(old)) {
                            patch(cx, &img, &p.after, &d.at.write, w, h, true);
                        }
                    }
                }
            }
            img
        }
        None => {
            let img = before_spots(cx);
            for i in 0..new.len() {
                draw(cx, &img, plan, &new, i, &mut patches);
            }
            img
        }
    };
    let post = Arc::new(img);
    // (what follows writes its own buffers, but the kept one must never be handed on)
    let out = cx.copy(&post);
    (out, Some(Arc::new(Kept { key, post, drawn: new, patches, done: Vec::new() })))
}

/// Device bytes `k` holds: its image after the spots and each spot's boxes.
pub(crate) fn bytes(k: &DeviceKept) -> usize {
    (k.post.len + k.patches.iter().flatten().map(|p| p.before.len + p.after.len).sum::<usize>()) * 4
}

/// Draw spot `i` on `img`, keeping its box before and after.
fn draw(cx: &mut Cx<'_>, img: &Buf, plan: &Plan<'_>, new: &[Option<keep::Drawn>], i: usize, patches: &mut [DevicePatch]) {
    let (Some(Some(d)), Some(spot)) = (new.get(i), plan.settings.spots.get(i)) else { return };
    let r = &d.at.write;
    let n = (r[2] - r[0]) * (r[3] - r[1]) * 3;
    let (before, after) = (cx.gpu.buffer(n), cx.gpu.buffer(n));
    patch(cx, img, &before, r, plan.w, plan.h, false);
    draw_spot(cx, img, plan, spot);
    patch(cx, img, &after, r, plan.w, plan.h, false);
    if let Some(slot) = patches.get_mut(i) {
        *slot = Some(Arc::new(Patch { before, after }));
    }
}

/// `img`'s box `r` copied into `p` (`back`: `p` copied into it).
fn patch(cx: &mut Cx<'_>, img: &Buf, p: &Buf, r: &Rect, w: usize, h: usize, back: bool) {
    let (bw, bh) = (r[2].saturating_sub(r[0]), r[3].saturating_sub(r[1]));
    if bw == 0 || bh == 0 || p.len < bw * bh * 3 || r[2] > w || r[3] > h {
        return;
    }
    let params = [w, h, r[0], r[1], bw, bh, back as usize].map(|v| v as u32);
    // (the kernel's other writable bindings each need a buffer of their own)
    let unused = [cx.gpu.buffer(1), cx.gpu.buffer(1), cx.gpu.buffer(1)];
    let [al, df, wt] = &unused;
    cx.run("spot_patch", &params, &[Some(img), None, Some(al), Some(p), Some(df), Some(wt)], crate::ctx::groups2(bw, bh, [16, 16]));
}
