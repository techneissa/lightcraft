//! Fork (`fork/perf-decontam-gpu.md`): a decontaminated mask's soft edge on the GPU (`lightcraft_pipeline::decontam`).
//!
//! The edge's two colours are estimated once on the CPU, from the render's linear image and the mask's alpha read
//! back from the device (the estimate the CPU render makes, `decontam::estimate_edge`), and kept on the device by
//! what they are estimated from (`decontam::split_key`): a drag of any slider that leaves the image and the mask's
//! alpha alone (the mask's sliders, its Amount, the global sliders) neither estimates nor uploads them again. A
//! Portrait or spot tick changes the image, but only where the retouch step wrote (`Slot::boxes`): an edge whose
//! estimate reads nothing there, with the same alpha, is kept as well. Each
//! render appends them to its `masks` buffer (the finish kernel has no binding to spare) and describes them in `aux`;
//! `wgsl/decontam.wgsl` edits the two colours apart.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use lightcraft_pipeline::Plan;
use lightcraft_pipeline::decontam::{self, Edge};
use lightcraft_pipeline::finish::MASK_TERMS;
use lightcraft_pipeline::masks::{AlphaKeys, MaskAlpha, Mattes, alpha_key};
use lightcraft_pipeline::spots_keep::Rect;

use lightcraft_raster::Rgb32f;

use crate::ctx::{Buf, FailKind, fail};
use crate::render::{Cx, Host};

/// The finish kernel's entry point for a render with a decontaminated edge: `main` with the hooks of
/// `wgsl/decontam.wgsl` ([`with_main_dc`]). `main` itself stays as it was, so a render without one is drawn by the same
/// compiled code, byte for byte: with the hooks in it, the GPU's compiler rounded some pixels of a render without any
/// edge differently.
pub(crate) const MAIN_DC: &str = "main_dc";

/// Where the hooks go in `main`, and what follows each anchor.
const HOOKS: [(&str, &str); 3] = [
    (
        "    // --- local tone in log luminance\n    var l1 = l0;\n    if (dz != 0.0 || l_exp != 0.0) {\n        l1 = log_lum(c);\n    }\n",
        "    // (fork: at a decontaminated mask's soft edge, its edits made to each colour apart)\n    let ndc = pu(F_DC_N);\n    if (ndc > 0u) {\n        let dc = dc_scene(c, x, y, i, n, Local(lt, tint_on, tint_dir, tint_amt));\n        c = dc.xyz;\n        l1 = l1 + dc.w;\n    }\n",
    ),
    ("    // --- colour\n", "    let pre = d;\n"),
    (
        "    d = recolor_masks(d, i, n);\n",
        "    if (ndc > 0u) {\n        d = dc_colour(pre, d, select(1.0, exp2(delta), delta != 0.0), x, y, i, n);\n    }\n",
    ),
];

/// The finish module's source `src` (its `main` last) with `wgsl/decontam.wgsl` and [`MAIN_DC`] after it: a copy of
/// its `main` with [`HOOKS`]. Unchanged when `main` or an anchor isn't found (the kernel build then fails for the
/// missing entry point, and so does the test of this).
pub(crate) fn with_main_dc(src: &str) -> String {
    let made = (|| {
        let start = src.find("@compute @workgroup_size(16, 16)\nfn main(")?;
        let mut main = src.get(start..)?.replacen("fn main(", &format!("fn {MAIN_DC}("), 1);
        for (anchor, hook) in HOOKS {
            let at = main.find(anchor)? + anchor.len();
            main.insert_str(at, hook);
        }
        Some(format!("{src}{}\n{main}", include_str!("wgsl/decontam.wgsl")))
    })();
    made.unwrap_or_else(|| src.to_string())
}

/// Words of a decontaminated mask's descriptor in `aux`: its mask index, Amount, rows' offset, pixels' offset, layers.
pub(crate) const DESC_WORDS: usize = 5;

/// One kept edge on the device, with what it was estimated from.
#[derive(Clone)]
struct Slot {
    /// What it is looked up by: [`edge_key`] without the portrait retouching (`Plan::edge_lin_key`).
    key: u64,
    /// The full [`edge_key`] of the last render it served: the same again needs no check.
    exact: u64,
    buf: Arc<Buf>,
    /// Where the retouch step wrote in the image it was estimated from (`None`: anywhere).
    boxes: Option<Vec<Rect>>,
    /// Which squares of the frame hold its pixels.
    foot: Arc<Footprint>,
    /// A hash of the mask's alpha near the edge it was estimated from ([`Footprint::alpha_hash`]).
    alpha: u64,
}

/// The squares ([`Footprint::CELL`] px) of a frame that hold edge pixels, and those within reach of one.
struct Footprint {
    cols: usize,
    rows: usize,
    hit: Vec<bool>,
    /// The squares within the edge's reach of a hit one: where its estimate reads the image and the alpha.
    near: Vec<bool>,
}

impl Footprint {
    const CELL: usize = 32;

    fn of(edge: &Edge, (width, height): (usize, usize), reach: usize) -> Self {
        let (cols, rows) = (width.div_ceil(Self::CELL), height.div_ceil(Self::CELL));
        let hit = edge.occupied(Self::CELL, width);
        let m = reach.div_ceil(Self::CELL);
        let mut near = vec![false; hit.len()];
        for (i, _) in hit.iter().enumerate().filter(|(_, h)| **h) {
            let (cx, cy) = (i % cols.max(1), i / cols.max(1));
            for y in cy.saturating_sub(m)..=(cy + m).min(rows.saturating_sub(1)) {
                for x in cx.saturating_sub(m)..=(cx + m).min(cols.saturating_sub(1)) {
                    if let Some(n) = near.get_mut(y * cols + x) {
                        *n = true;
                    }
                }
            }
        }
        Self { cols, rows, hit, near }
    }

    /// Whether any box, grown by `margin` px, reaches a square that holds an edge pixel.
    fn reached_by(&self, boxes: &[Rect], margin: usize) -> bool {
        boxes.iter().filter(|b| b[2] > b[0] && b[3] > b[1]).any(|b| {
            let c = |v: usize| v / Self::CELL;
            let (x0, x1) = (c(b[0].saturating_sub(margin)), c(b[2].saturating_add(margin)).min(self.cols.saturating_sub(1)));
            let (y0, y1) = (c(b[1].saturating_sub(margin)), c(b[3].saturating_add(margin)).min(self.rows.saturating_sub(1)));
            (y0..=y1).any(|y| (x0..=x1).any(|x| self.hit.get(y * self.cols + x).copied().unwrap_or(false)))
        })
    }

    /// A hash of the alpha plane's bits in the squares within reach of the edge.
    fn alpha_hash(&self, plane: &lightcraft_raster::Plane) -> u64 {
        use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher, Hash, Hasher};
        let mut h = BuildHasherDefault::<DefaultHasher>::default().build_hasher();
        for y in 0..plane.height {
            let Some(row) = plane.data.get(y * plane.width..(y + 1) * plane.width) else { continue };
            for cx in 0..self.cols {
                if self.near.get((y / Self::CELL) * self.cols + cx).copied().unwrap_or(false) {
                    let x0 = cx * Self::CELL;
                    row.get(x0..(x0 + Self::CELL).min(row.len())).unwrap_or(&[]).iter().for_each(|v| v.to_bits().hash(&mut h));
                }
            }
        }
        h.finish()
    }
}

/// The edges of the last render's decontaminated masks on the device: the GPU twin of the CPU stage cache's kept
/// edges, next to the view's other stages.
#[derive(Default)]
pub(crate) struct KeptEdges {
    inner: Mutex<Kept>,
}

#[derive(Default)]
struct Kept {
    /// The source the edges were estimated on: `split_key` holds no source pixels (nor mattes), so another photo of
    /// the same size and settings has the same key (as `AlphaStore` does). Weak: the store never keeps a photo alive,
    /// and a dropped source's address can't be taken again while this is held.
    source: Option<Weak<Rgb32f>>,
    mattes: Option<Arc<Mattes>>,
    slots: Vec<Slot>,
}

impl KeptEdges {
    fn lock(&self) -> MutexGuard<'_, Kept> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn clear(&self) {
        *self.lock() = Kept::default();
    }

    /// The edges held for a render of `src` with `mattes`, taken out (the lock is not held while the render
    /// estimates): nothing for another photo or other mattes.
    fn take(&self, src: &Arc<Rgb32f>, mattes: Option<&Arc<Mattes>>) -> Vec<Slot> {
        let mut g = self.lock();
        let same_src = g.source.as_ref().is_some_and(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(src)));
        let same_mattes = match (&g.mattes, mattes) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        let slots = std::mem::take(&mut g.slots);
        if same_src && same_mattes { slots } else { Vec::new() }
    }

    /// Hold `slots`, the edges a render of `src` with `mattes` used, and drop everything else.
    fn keep(&self, src: &Arc<Rgb32f>, mattes: Option<&Arc<Mattes>>, slots: Vec<Slot>) {
        *self.lock() = Kept { source: Some(Arc::downgrade(src)), mattes: mattes.cloned(), slots };
    }

    /// Hand every device buffer held to `add` (the caller counts each once).
    pub(crate) fn each(&self, mut add: impl FnMut(&Arc<Buf>)) {
        self.lock().slots.iter().for_each(|s| add(&s.buf));
    }
}

/// What a kept edge is looked up by: the CPU's [`decontam::split_key`] (the image `lin`, the mask's alpha and the
/// frame's scale), and here also the render's quality and process (the linear image doesn't read them today beyond
/// what `lin_key` holds, but a stale edge would show; a draft tick and a full render of the same size never share one).
fn edge_key(plan: &Plan<'_>, lin: u64, alpha: u64) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    let base = decontam::split_key(lin, alpha, plan.px_per_long);
    BuildHasherDefault::<DefaultHasher>::default().hash_one((base, plan.draft, plan.settings.process))
}

/// How far (px) a retouched region reaches into an edge's estimate: the windows its colours are read from, and the
/// denoise radius between the retouch step and the finished image (a bound, generous for any setting).
fn reach(px_per_long: f64) -> usize {
    decontam::reach(px_per_long).saturating_add(16).saturating_add((px_per_long * 0.01).max(0.0) as usize)
}

thread_local! {
    static UPLOADS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many decontaminated edges renders on this thread have uploaded to a device (tests: a kept edge is not uploaded
/// again).
pub fn edge_uploads() -> u64 {
    UPLOADS.with(std::cell::Cell::get)
}

/// What a render's decontaminated masks add to the finish kernel's inputs.
pub(crate) struct Attached {
    /// The masks buffer with the edges after its planes (and chroma).
    pub masks: Buf,
    /// The descriptors, in mask order ([`DESC_WORDS`] each), for `aux`.
    pub desc: Vec<f32>,
    /// An edge was estimated in this render: the pixels it changes reach past any tile a spot edit redrew.
    pub fresh: bool,
}

/// The finish kernel's decontamination inputs of a render ([`with_edges`]): none, or its edges' descriptors.
#[derive(Default)]
pub(crate) struct Edges {
    pub desc: Vec<f32>,
    pub fresh: bool,
}

impl Edges {
    /// The finish kernel's entry point for the render.
    pub(crate) fn entry(&self) -> &'static str {
        if self.desc.is_empty() { "main" } else { MAIN_DC }
    }
}

/// The linear image a render's edges are estimated from.
pub(crate) struct Lin<'a> {
    pub buf: &'a Buf,
    /// Where the retouch step wrote in it (`None`: anywhere).
    pub boxes: Option<&'a [Rect]>,
}

/// The render's masks buffer with its decontaminated edges appended ([`attach`]), or as it was when it has none.
/// `None` when that failed the render (the CPU renders it).
pub(crate) fn with_edges(
    cx: &mut Cx<'_>,
    plan: &Plan<'_>,
    masks: Option<Buf>,
    terms: &[[f32; MASK_TERMS]],
    lin: &Lin<'_>,
    host: &mut Host,
    kept: Option<(&KeptEdges, &Arc<Rgb32f>)>,
) -> Option<(Option<Buf>, Edges)> {
    let attached = masks.as_ref().and_then(|m| attach(cx, plan, m, terms, lin, host, kept));
    if masks.is_none()
        && let Some((k, _)) = kept
    {
        k.clear();
    }
    if crate::ctx::failed() {
        return None;
    }
    Some(match attached {
        Some(a) => (Some(a.masks), Edges { desc: a.desc, fresh: a.fresh }),
        None => (masks, Edges::default()),
    })
}

/// What one decontaminated mask asks of [`edge_of`]: its place `k` in the render's list, its full and its loose
/// ([`Slot::key`]) key, the alphas' buffer and the linear image.
struct Want<'a> {
    k: usize,
    exact: u64,
    key: u64,
    masks: &'a Buf,
    lin: &'a Lin<'a>,
}

/// The edge of a mask ([`Want`]): one the view held that still serves it, or estimated from the linear image and the
/// mask's alpha read back, and uploaded. With whether it was estimated; `None` when that failed the render.
fn edge_of(cx: &mut Cx<'_>, plan: &Plan<'_>, want: &Want<'_>, host: &mut Host, held: &mut [Slot]) -> Option<(Slot, bool)> {
    let (w, h) = (plan.w, plan.h);
    let n = w.checked_mul(h)?;
    if let Some(s) = held.iter().find(|s| s.exact == want.exact) {
        return Some((s.clone(), false));
    }
    let plane = cx.slice(want.masks, want.k.checked_mul(n)?, n);
    let alpha_plane = cx.read_plane(&plane, w, h);
    // the same edge but for the retouch step: kept when nothing it wrote reaches the edge's estimate, and the alpha is
    // the very one it was estimated from
    let boxes = want.lin.boxes;
    let margin = reach(plan.px_per_long);
    let still = held.iter_mut().find(|s| {
        s.key == want.key
            && s.alpha == s.foot.alpha_hash(&alpha_plane)
            && crate::pre_retouch::changed(s.boxes.as_deref(), boxes).is_some_and(|r| !s.foot.reached_by(&r, margin))
    });
    if let Some(s) = still {
        s.exact = want.exact;
        return Some((s.clone(), false));
    }
    let img: Arc<Rgb32f> = host.lin.get_or_insert_with(|| Arc::new(cx.read_rgb(want.lin.buf, w, h))).clone();
    let alpha = MaskAlpha::from_plane(alpha_plane.clone());
    let edge: Edge = decontam::estimate_counted(&img, &alpha, plan.px_per_long);
    let Some(words) = edge.device_words(h) else {
        fail(FailKind::Limit, format!("a decontaminated edge of {} pixels doesn't fit a GPU buffer", edge.len()));
        return None;
    };
    if !cx.gpu.fits(words.len()) {
        fail(FailKind::Limit, format!("a decontaminated edge of {} pixels is over the device's buffer limit", edge.len()));
        return None;
    }
    UPLOADS.with(|c| c.set(c.get() + 1));
    let foot = Footprint::of(&edge, (w, h), margin);
    let slot = Slot {
        key: want.key,
        exact: want.exact,
        buf: Arc::new(cx.gpu.upload(&words)),
        boxes: boxes.map(<[Rect]>::to_vec),
        alpha: foot.alpha_hash(&alpha_plane),
        foot: Arc::new(foot),
    };
    Some((slot, true))
}

/// The render's decontaminated masks (`decontam::applies`), their edges estimated or taken from `kept` (the view's
/// edges and the source they were estimated on), appended to `masks` (the alphas of `list`'s masks). `None` when no
/// mask is decontaminated (the edges kept are freed), or a mask's edge holds no pixel. A buffer over the device's
/// limit fails the render ([`FailKind::Limit`]): the CPU renders it.
pub(crate) fn attach(
    cx: &mut Cx<'_>,
    plan: &Plan<'_>,
    masks: &Buf,
    terms: &[[f32; MASK_TERMS]],
    lin: &Lin<'_>,
    host: &mut Host,
    kept: Option<(&KeptEdges, &Arc<Rgb32f>)>,
) -> Option<Attached> {
    let s = &*plan.settings;
    if !s.masks.iter().any(decontam::applies) {
        // (no mask is decontaminated: what was kept for an earlier one is of no use)
        if let Some((k, _)) = kept {
            k.clear();
        }
        return None;
    }
    let h = plan.h;
    let list: Vec<_> = s.masks.iter().filter(|m| m.renders()).collect();
    let (full, loose) = (AlphaKeys { geo: plan.geo, image: plan.lin_key }, AlphaKeys { geo: plan.geo, image: plan.edge_lin_key });
    let ev = s.light.exposure as f32;
    let mut held = kept.map(|(k, src)| k.take(src, plan.mattes.as_ref())).unwrap_or_default();
    let mut used: Vec<Slot> = Vec::new();
    // (mask index, Amount, layers, the edge's words)
    let mut found: Vec<(usize, f32, u32, Arc<Buf>)> = Vec::new();
    let mut fresh = false;
    for (k, m) in list.iter().enumerate() {
        if !decontam::applies(m) {
            continue;
        }
        let Some(t) = terms.get(k) else { continue };
        let (scene, colour) = decontam::layers_of(t);
        let layers = u32::from(scene) | (u32::from(colour) << 1);
        let want = Want {
            k,
            exact: edge_key(plan, plan.lin_key, alpha_key(m, full, ev)),
            key: edge_key(plan, plan.edge_lin_key, alpha_key(m, loose, ev)),
            masks,
            lin,
        };
        // (a render's own edges first: two masks of one alpha share one)
        let slot = match used.iter().find(|u| u.exact == want.exact) {
            Some(u) => u.clone(),
            None => {
                let (slot, estimated) = edge_of(cx, plan, &want, host, &mut held)?;
                fresh |= estimated;
                used.push(slot.clone());
                slot
            }
        };
        // (an edge with no pixel: `height + 1` row starts and nothing else)
        if slot.buf.len > h + 1 {
            found.push((k, (m.decontaminate / 100.0).clamp(0.0, 1.0) as f32, layers, slot.buf));
        }
    }
    if let Some((k, src)) = kept {
        k.keep(src, plan.mattes.as_ref(), used);
    }
    if found.is_empty() {
        return None;
    }
    let total = found.iter().try_fold(masks.len, |a, (.., b)| a.checked_add(b.len))?;
    if !cx.gpu.fits(total) || u32::try_from(total).is_err() {
        fail(FailKind::Limit, format!("the masks and their decontaminated edges ({total} words) are over the device's buffer limit"));
        return None;
    }
    let out = cx.gpu.buffer(total);
    cx.copy_into(masks, &out, 0);
    let mut at = masks.len;
    let mut desc = Vec::with_capacity(found.len() * DESC_WORDS);
    for (k, t, layers, b) in &found {
        cx.copy_into(b, &out, at);
        let rows = at as u32;
        let px = (at + h + 1) as u32;
        desc.extend([f32::from_bits(*k as u32), *t, f32::from_bits(rows), f32::from_bits(px), f32::from_bits(*layers)]);
        at += b.len;
    }
    Some(Attached { masks: out, desc, fresh })
}

/// The finish kernel's parameter block and `aux` with the descriptors `desc` ([`Attached::desc`]) after `aux`'s tables.
pub(crate) fn describe(mut p: Vec<u32>, mut aux: Vec<f32>, desc: &[f32]) -> (Vec<u32>, Vec<f32>) {
    let (Some(n_at), Some(off_at)) = (crate::params::index("DC_N"), crate::params::index("DC_OFF")) else {
        // (the render then draws no decontamination: say so once, not on every tick)
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| log::warn!("gpu: the finish kernel's parameters have no DC_N or DC_OFF; decontamination is off"));
        return (p, aux);
    };
    let count = desc.len() / DESC_WORDS;
    if let (Some(n), Some(off)) = (p.get_mut(n_at), u32::try_from(aux.len()).ok()) {
        *n = count as u32;
        if let Some(o) = p.get_mut(off_at) {
            *o = off;
        }
        aux.extend_from_slice(desc);
    }
    (p, aux)
}

#[cfg(test)]
mod tests {
    /// The finish module gets its `main_dc`, every hook in place (an anchor lost in an edit of `main` fails here, not
    /// only on a device).
    #[test]
    fn main_dc_is_main_with_every_hook() {
        let src = include_str!("wgsl/finish.wgsl");
        let made = super::with_main_dc(src);
        assert!(made.starts_with(src), "main is left as it was");
        let dc = made.get(src.len()..).unwrap_or_default();
        assert!(dc.contains(&format!("fn {}(", super::MAIN_DC)));
        for (anchor, hook) in super::HOOKS {
            assert!(dc.contains(&format!("{anchor}{hook}")), "hook after {anchor:?} missing");
        }
    }
}
