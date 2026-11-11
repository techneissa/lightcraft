//! Fork (`fork/perf-portrait-draft.md`, `fork/perf-portrait-gpu.md`): the scene-linear stage with portrait retouching
//! on. The image before the step (white balance, defringe, spots, red eye) is made on the device, as without
//! retouching (on the CPU only for spots with no GPU kernel), and kept there, keyed by [`Plan::pre_retouch_key`], so a
//! Portrait slider tick neither redoes it nor reads the frame back. The step runs on the CPU, on the band of rows that
//! holds everything it reads for each face ([`lightcraft_pipeline::retouch_step::write_boxes`]): only that band is
//! read back, and only the rows of the boxes go up again. Those boxes are the dirty rectangles the later stages redo
//! (issue #111's tiles). A step that may write anywhere (Background, Refine strokes) reads back and uploads the whole
//! frame. The kept images count against [`DEVICE`], which lets the oldest go.

use std::sync::{Arc, Weak};

use lightcraft_pipeline::kept_budget::{Budget, Kept};
use lightcraft_pipeline::spots_keep::{CpuKept, Rect};
use lightcraft_pipeline::{Plan, SourceInfo, retouch_step};
use lightcraft_raster::Rgb32f;

use crate::ctx::Buf;
use crate::render::{Cx, KeptSpots, rgb_words};

thread_local! {
    static FULL_UPLOAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test hook: renders on this thread retouch the whole frame (read back and uploaded whole) instead of the band and
/// the rows of the step's boxes, so a test can compare a tick with a render that does not depend on the boxes.
pub fn full_upload(on: bool) {
    FULL_UPLOAD.with(|c| c.set(on));
}

/// The device's budget for kept images before the retouch step, across every view.
pub(crate) static DEVICE: Budget = Budget::new(512 << 20);

/// The image before the retouch step, on the device.
pub(crate) struct Images {
    pub(crate) dev: Arc<Buf>,
}

/// [`Images`] by [`Plan::pre_retouch_key`], under the budget.
#[derive(Clone)]
pub(crate) struct PreRetouch {
    pub(crate) key: u64,
    pub(crate) kept: Kept<Images>,
}

/// What [`linear`] made.
pub(crate) struct Retouched {
    /// The retouched image before noise reduction, on the device.
    pub(crate) img: Buf,
    /// The image it was made from, to keep, and its device buffer (what the next tick's image is made from).
    pub(crate) pre: PreRetouch,
    pub(crate) from: Weak<Buf>,
    /// The boxes the step may have written; `None`: anywhere.
    pub(crate) boxes: Option<Vec<Rect>>,
    /// The spots of the image before the step, to keep (`None` when the spots were not drawn again).
    pub(crate) spots: Option<KeptSpots>,
}

/// The retouched image before noise reduction: from `kept` when it is the image before the step this plan needs,
/// else made from the resampled source `sampled` (on the device, drawing its spots from `spots.0`, the view's last
/// ones; or, for spots with no GPU kernel, on the CPU from `host_sampled` when the source was resampled there, else
/// read back, drawing them from `spots.1`).
pub(crate) fn linear(
    cx: &mut Cx<'_>,
    sampled: &Buf,
    host_sampled: Option<Rgb32f>,
    info: &SourceInfo,
    plan: &Plan<'_>,
    kept: Option<&PreRetouch>,
    spots: (Option<&lightcraft_pipeline::spots_keep::Kept<Buf, Buf>>, Option<&CpuKept>),
) -> Retouched {
    let (w, h) = (plan.w, plan.h);
    let t = crate::render::profiling().then(std::time::Instant::now);
    let reused = kept.filter(|k| k.key == plan.pre_retouch_key).and_then(|k| k.kept.get().map(|i| (k.clone(), i)));
    let (pre, images, drawn) = match reused {
        Some((k, i)) => (k, i, None),
        None => {
            let (dev, drawn): (Buf, KeptSpots) = if crate::render::cpu_spots(plan) {
                let mut img = host_sampled.unwrap_or_else(|| cx.read_rgb(sampled, w, h));
                let k = lightcraft_pipeline::spots_keep::lin_cpu_pre_retouch(&mut img, info, plan, spots.1);
                (cx.gpu.upload(rgb_words(&img)), (None, k))
            } else {
                retouch_step::note_pre_retouch_run();
                let (b, k, _) = crate::render::linear_on_device(cx, sampled, info, plan, spots.0);
                (b, (k, None))
            };
            let bytes = dev.len * std::mem::size_of::<f32>();
            let images = Arc::new(Images { dev: Arc::new(dev) });
            let kept = Kept::new(images.clone(), &[(&DEVICE, bytes)]);
            (PreRetouch { key: plan.pre_retouch_key, kept }, images, Some(drawn))
        }
    };
    if t.is_some() {
        cx.sync();
    }
    let t1 = crate::render::profiling().then(std::time::Instant::now);
    let boxes = retouch_step::write_boxes(info, plan);
    let full = FULL_UPLOAD.with(std::cell::Cell::get);
    let (out, rows) = match boxes.as_deref().filter(|_| !full).map(row_spans) {
        // nothing the step may write
        Some(spans) if spans.is_empty() => (cx.copy(&images.dev), 0),
        Some(spans) => band(cx, &images.dev, info, plan, &spans).unwrap_or_else(|| whole(cx, &images.dev, info, plan)),
        None => whole(cx, &images.dev, info, plan),
    };
    if let (Some(t), Some(t1)) = (t, t1) {
        cx.sync();
        let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f64() * 1e3;
        eprintln!(
            "    gpu retouch: before it {:.1} ms ({}), read back {rows} of {h} rows, step, upload {:.1} ms",
            ms(t, t1),
            if drawn.is_some() { "made" } else { "kept" },
            ms(t1, std::time::Instant::now())
        );
    }
    Retouched { img: out, from: Arc::downgrade(&images.dev), pre, boxes, spots: drawn }
}

/// The step on the whole frame: read back, retouched and uploaded whole. With the rows read back.
fn whole(cx: &mut Cx<'_>, dev: &Buf, info: &SourceInfo, plan: &Plan<'_>) -> (Buf, usize) {
    let mut img = cx.read_rgb(dev, plan.w, plan.h);
    retouch_step::apply(&mut img, info, plan);
    (cx.gpu.upload(rgb_words(&img)), plan.h)
}

/// The step on the band of rows that holds every span (`spans`, the rows of the step's boxes, merged and sorted): only
/// the band is read back, and a copy of `dev` gets the spans' rows of the result. With the rows read back. `None` when
/// a span's rows are not in the image or `dev` is not its size (the caller retouches the whole frame).
fn band(cx: &mut Cx<'_>, dev: &Buf, info: &SourceInfo, plan: &Plan<'_>, spans: &[(usize, usize)]) -> Option<(Buf, usize)> {
    let row = plan.w.checked_mul(3)?;
    let (y0, y1) = (spans.first()?.0, spans.last()?.1);
    if y1 > plan.h || dev.len != row.checked_mul(plan.h)? || y1 <= y0 {
        return None;
    }
    let part = cx.slice(dev, y0 * row, (y1 - y0) * row);
    let mut img = cx.read_rgb(&part, plan.w, y1 - y0);
    retouch_step::apply_rows(&mut img, info, plan, y0);
    let words = rgb_words(&img);
    let out = cx.copy(dev);
    for &(a, b) in spans {
        let part = words.get((a - y0) * row..(b - y0) * row)?;
        if !part.is_empty() {
            let up = cx.gpu.upload(part);
            cx.copy_into(&up, &out, a * row);
        }
    }
    Some((out, y1 - y0))
}

/// The rows `[y0, y1)` the boxes cover, overlapping spans merged.
fn row_spans(boxes: &[Rect]) -> Vec<(usize, usize)> {
    let mut spans: Vec<(usize, usize)> = boxes.iter().filter(|b| b[3] > b[1]).map(|b| (b[1], b[3])).collect();
    spans.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (a, b) in spans {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// The boxes where two retouched images made from the same image before the step may differ: where either step
/// wrote (`None`: anywhere).
pub(crate) fn changed(old: Option<&[Rect]>, new: Option<&[Rect]>) -> Option<Vec<Rect>> {
    Some([old?, new?].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_spans_merge_overlapping_boxes() {
        assert_eq!(row_spans(&[[0, 10, 5, 20], [3, 15, 9, 30], [0, 40, 1, 41], [0, 50, 9, 50]]), vec![(10, 30), (40, 41)]);
        assert!(row_spans(&[]).is_empty());
    }

    #[test]
    fn a_change_anywhere_is_anywhere() {
        let a = [[0, 0, 4, 4]];
        assert_eq!(changed(Some(&a), Some(&[])), Some(a.to_vec()));
        assert_eq!(changed(None, Some(&a)), None);
        assert_eq!(changed(Some(&a), None), None);
    }
}
