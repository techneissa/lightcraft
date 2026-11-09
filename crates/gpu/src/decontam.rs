//! Fork (`fork/perf-decontam-gpu.md`): a decontaminated mask's soft edge on the GPU (`lightcraft_pipeline::decontam`).
//!
//! The edge's two colours are estimated once on the CPU, from the render's linear image and the mask's alpha read
//! back from the device (the estimate the CPU render makes, `decontam::estimate_edge`), and kept on the device by
//! what they are estimated from (`decontam::split_key`): a drag of any slider that leaves the image and the mask's
//! alpha alone (the mask's sliders, its Amount, the global sliders) neither estimates nor uploads them again. Each
//! render appends them to its `masks` buffer (the finish kernel has no binding to spare) and describes them in `aux`;
//! `wgsl/decontam.wgsl` edits the two colours apart.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lightcraft_pipeline::Plan;
use lightcraft_pipeline::decontam::{self, Edge};
use lightcraft_pipeline::finish::MASK_TERMS;
use lightcraft_pipeline::masks::{AlphaKeys, MaskAlpha, alpha_key};

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

/// The edges of the last render's decontaminated masks on the device, by [`decontam::split_key`]: the GPU twin of
/// the CPU stage cache's kept edges, next to the view's other stages.
#[derive(Default)]
pub(crate) struct KeptEdges {
    inner: Mutex<Vec<(u64, Arc<Buf>)>>,
}

impl KeptEdges {
    fn lock(&self) -> MutexGuard<'_, Vec<(u64, Arc<Buf>)>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn clear(&self) {
        self.lock().clear();
    }

    /// Hand every device buffer held to `add` (the caller counts each once).
    pub(crate) fn each(&self, mut add: impl FnMut(&Arc<Buf>)) {
        self.lock().iter().for_each(|(_, b)| add(b));
    }
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

/// The render's masks buffer with its decontaminated edges appended ([`attach`]), or as it was when it has none.
/// `None` when that failed the render (the CPU renders it).
pub(crate) fn with_edges(
    cx: &mut Cx<'_>,
    plan: &Plan<'_>,
    masks: Option<Buf>,
    terms: &[[f32; MASK_TERMS]],
    lin: &Buf,
    host: &mut Host,
    kept: Option<&KeptEdges>,
) -> Option<(Option<Buf>, Edges)> {
    let attached = masks.as_ref().and_then(|m| attach(cx, plan, m, terms, lin, host, kept));
    if crate::ctx::failed() {
        return None;
    }
    Some(match attached {
        Some(a) => (Some(a.masks), Edges { desc: a.desc, fresh: a.fresh }),
        None => (masks, Edges::default()),
    })
}

/// The render's decontaminated masks (`decontam::applies`), their edges estimated or taken from `kept`, appended to
/// `masks` (the alphas of `list`'s masks, `masks_len` words). `None` when no mask is decontaminated, or a mask's edge
/// holds no pixel. A buffer over the device's limit fails the render ([`FailKind::Limit`]): the CPU renders it.
pub(crate) fn attach(
    cx: &mut Cx<'_>,
    plan: &Plan<'_>,
    masks: &Buf,
    terms: &[[f32; MASK_TERMS]],
    lin: &Buf,
    host: &mut Host,
    kept: Option<&KeptEdges>,
) -> Option<Attached> {
    let s = &*plan.settings;
    if !s.masks.iter().any(decontam::applies) {
        return None;
    }
    let (w, h) = (plan.w, plan.h);
    let n = w.checked_mul(h)?;
    let list: Vec<_> = s.masks.iter().filter(|m| m.visible && !m.components.is_empty()).collect();
    let keys = AlphaKeys { geo: plan.geo, image: plan.lin_key };
    let ev = s.light.exposure as f32;
    let mut guard = kept.map(KeptEdges::lock);
    let old = guard.as_deref_mut().map(std::mem::take).unwrap_or_default();
    let mut used: Vec<(u64, Arc<Buf>)> = Vec::new();
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
        let key = decontam::split_key(plan.lin_key, alpha_key(m, keys, ev), plan.px_per_long);
        let buf = match used.iter().chain(&old).find(|(o, _)| *o == key) {
            Some((_, b)) => b.clone(),
            None => {
                fresh = true;
                let img: Arc<Rgb32f> = host.lin.get_or_insert_with(|| Arc::new(cx.read_rgb(lin, w, h))).clone();
                let plane = cx.slice(masks, k.checked_mul(n)?, n);
                let alpha = MaskAlpha::from_plane(cx.read_plane(&plane, w, h));
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
                Arc::new(cx.gpu.upload(&words))
            }
        };
        if !used.iter().any(|(o, _)| *o == key) {
            used.push((key, buf.clone()));
        }
        // (an edge with no pixel: `height + 1` row starts and nothing else)
        if buf.len > h + 1 {
            found.push((k, (m.decontaminate / 100.0).clamp(0.0, 1.0) as f32, layers, buf));
        }
    }
    if let Some(g) = guard.as_deref_mut() {
        *g = used;
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
    let (Some(n_at), Some(off_at)) = (crate::params::index("DC_N"), crate::params::index("DC_OFF")) else { return (p, aux) };
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
