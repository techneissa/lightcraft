//! Fork (`fork/perf-portrait-draft.md`): the scene-linear stage with portrait retouching on. The retouch step runs on
//! the CPU; the image before it (white balance, defringe, spots, red eye) is kept on the host and on the device, keyed
//! by [`Plan::pre_retouch_key`], so a Portrait slider tick neither reads the frame back nor redoes those. The step's
//! result goes up only in the rows of the boxes it may write ([`lightcraft_pipeline::retouch_step::write_boxes`]),
//! and those boxes are the dirty rectangles the later stages redo (issue #111's tiles). The kept images count against
//! [`lightcraft_pipeline::kept_budget::HOST`] and [`DEVICE`], which let the oldest go.

use std::sync::{Arc, Weak};

use lightcraft_pipeline::kept_budget::{Budget, HOST, Kept};
use lightcraft_pipeline::spots_keep::{CpuKept, Rect};
use lightcraft_pipeline::{Plan, SourceInfo, retouch_step};
use lightcraft_raster::Rgb32f;

use crate::ctx::Buf;
use crate::render::{Cx, rgb_words};

thread_local! {
    static FULL_UPLOAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test hook: renders on this thread upload the retouched image whole instead of the rows of the step's boxes, so a
/// test can compare a tick with a render that does not depend on the boxes.
pub fn full_upload(on: bool) {
    FULL_UPLOAD.with(|c| c.set(on));
}

/// The device's budget for kept images before the retouch step, across every view.
pub(crate) static DEVICE: Budget = Budget::new(512 << 20);

/// The image before the retouch step, on the host and on the device.
pub(crate) struct Images {
    pub(crate) host: Rgb32f,
    pub(crate) dev: Arc<Buf>,
}

impl Images {
    /// Bytes held on the host (the device buffer is counted with the stages' buffers).
    pub(crate) fn host_bytes(&self) -> usize {
        self.host.data.len() * std::mem::size_of::<[f32; 3]>()
    }
}

/// [`Images`] by [`Plan::pre_retouch_key`], under the budgets.
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
    pub(crate) spots: Option<Option<Arc<CpuKept>>>,
}

/// The retouched image before noise reduction: from `kept` when it is the image before the step this plan needs,
/// else made from the resampled source (`host_sampled` when the source was resampled on the CPU, else read back from
/// `sampled`), drawing its spots from `spots` (the view's last ones).
pub(crate) fn linear(
    cx: &mut Cx<'_>,
    sampled: &Buf,
    host_sampled: Option<Rgb32f>,
    info: &SourceInfo,
    plan: &Plan<'_>,
    kept: Option<&PreRetouch>,
    spots: Option<&CpuKept>,
) -> Retouched {
    let (w, h) = (plan.w, plan.h);
    let t = crate::render::profiling().then(std::time::Instant::now);
    let reused = kept.filter(|k| k.key == plan.pre_retouch_key).and_then(|k| k.kept.get().map(|i| (k.clone(), i)));
    let (pre, images, drawn) = match reused {
        Some((k, i)) => (k, i, None),
        None => {
            let mut img = host_sampled.unwrap_or_else(|| cx.read_rgb(sampled, w, h));
            let kept_spots = lightcraft_pipeline::spots_keep::lin_cpu_pre_retouch(&mut img, info, plan, spots);
            let dev = Arc::new(cx.gpu.upload(rgb_words(&img)));
            let images = Arc::new(Images { host: img, dev });
            let bytes = images.host_bytes();
            let kept = Kept::new(images.clone(), &[(&HOST, bytes), (&DEVICE, bytes)]);
            (PreRetouch { key: plan.pre_retouch_key, kept }, images, Some(kept_spots))
        }
    };
    let t1 = crate::render::profiling().then(std::time::Instant::now);
    let mut img = images.host.clone();
    retouch_step::apply(&mut img, info, plan);
    let t2 = crate::render::profiling().then(std::time::Instant::now);
    let boxes = retouch_step::write_boxes(info, plan);
    // (a box whose rows are not in the image uploads it whole)
    let rows = boxes.as_deref().filter(|_| !FULL_UPLOAD.with(std::cell::Cell::get));
    let out = match rows.and_then(|b| upload_rows(cx, &images.dev, &img, b)) {
        Some(b) => b,
        None => cx.gpu.upload(rgb_words(&img)),
    };
    if let (Some(t), Some(t1), Some(t2)) = (t, t1, t2) {
        let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f64() * 1e3;
        let rows: usize = boxes.as_deref().map_or(h, |b| row_spans(b).iter().map(|(a, b)| b - a).sum());
        eprintln!(
            "    gpu retouch on the CPU: before it {:.1} ms ({}), copy + step {:.1} ms, upload {rows} of {h} rows {:.1} ms",
            ms(t, t1),
            if drawn.is_some() { "made" } else { "kept" },
            ms(t1, t2),
            ms(t2, std::time::Instant::now())
        );
    }
    Retouched { img: out, from: Arc::downgrade(&images.dev), pre, boxes, spots: drawn }
}

/// A copy of `base` (the image before the step, on the device) with the rows of `boxes` taken from `img`; `None` when
/// a box's rows are not all in the image or `base` is not its size (the caller uploads it whole).
fn upload_rows(cx: &mut Cx<'_>, base: &Buf, img: &Rgb32f, boxes: &[Rect]) -> Option<Buf> {
    let words = rgb_words(img);
    let row = img.width * 3;
    if base.len != words.len() {
        return None;
    }
    let parts: Vec<(usize, &[f32])> = row_spans(boxes)
        .into_iter()
        .map(|(y0, y1)| Some((y0 * row, words.get(y0.checked_mul(row)?..y1.checked_mul(row)?)?)))
        .collect::<Option<_>>()?;
    let out = cx.copy(base);
    for (at, part) in parts.into_iter().filter(|(_, p)| !p.is_empty()) {
        let up = cx.gpu.upload(part);
        cx.copy_into(&up, &out, at);
    }
    Some(out)
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
