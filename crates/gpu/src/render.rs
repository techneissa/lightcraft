//! The GPU renderer: the CPU pipeline's stages, evaluated on the device where a kernel exists and
//! on the CPU otherwise (per-stage hybrid). Stage results stay on the device and are cached per
//! view exactly like [`lightcraft_pipeline::StageCache`] (same keys, see [`lightcraft_pipeline::Plan`]).

use std::sync::{Arc, Mutex};

use lightcraft_develop::DevelopSettings;
use lightcraft_geom::Orientation;
use lightcraft_pipeline::finish::{FinishParams, FinishParamsInput};
use lightcraft_pipeline::geometry::SampleMode;
use lightcraft_pipeline::local::plane_at;
use lightcraft_pipeline::masks::PlanesOut;
use lightcraft_pipeline::nrc_joint::JointNoise;
use lightcraft_pipeline::spots_keep::{CpuKept, Kept, KeptStore};
use lightcraft_pipeline::{Plan, RenderRequest, Rendered, SourceInfo, local};
use lightcraft_raster::resample::Filter;
use lightcraft_raster::{Histogram, Plane, Rgb32f, Rgba8};

use crate::ctx::{Buf, FailKind, Gpu, fail, groups1, groups2};
use crate::params::{DEEP_WRITTEN, Present, finish_block};

/// Device-resident stages of one view, kept next to the CPU [`lightcraft_pipeline::StageCache`]
/// (as its extension). Holds the last two output sizes, like the CPU cache.
#[derive(Default)]
pub struct GpuStages {
    entries: Mutex<Vec<Entry>>,
    /// The uploaded source (a view renders one photo at several sizes).
    source: Mutex<Option<(Arc<Rgb32f>, Arc<Buf>)>>,
    /// The alphas of the last render's masks and the brush parts they were built from.
    alphas: crate::masks::AlphaStore,
    /// The spots of the last renders (`spots_keep`), on the device or (for the scene-linear stage on the CPU) on the
    /// host: a spot edit draws only what it reaches.
    spots: KeptStore<Kept<Buf, Buf>>,
    spots_cpu: KeptStore<CpuKept>,
    /// What the job that showed the last render made of it on the host, by that render's frame number (issue #111).
    pub(crate) host: Mutex<Option<crate::dirty::HostKept>>,
    /// Fork (`fork/perf-decontam-gpu.md`): the decontaminated masks' edges of the last render.
    decontam: crate::decontam::KeptEdges,
    /// The spots finished per scale and the LaMa fills, on the host, shared by every view (issue #176,
    /// `lightcraft_pipeline::spots_scale`).
    spot_finals: lightcraft_pipeline::spots_scale::Shared,
    /// The whole frame's noise model, for every window of it (issue #176).
    noise: lightcraft_pipeline::NoiseMemo,
}

const CAPACITY: usize = 2;

#[derive(Clone)]
struct Entry {
    src: Arc<Rgb32f>,
    geo: u64,
    sampled: Arc<Buf>,
    lin: Option<(u64, Arc<Buf>)>,
    /// The image after the spots (`spots_keep`) `lin` was made from (issue #111).
    post: std::sync::Weak<Buf>,
    /// The joint colour NR's noise model of the whole frame, keyed by `Plan::noise_key`.
    colour_noise: Option<(u64, JointNoise)>,
    planes: Planes,
    /// The finished render these stages made (issue #111): the next starts from it.
    last: Option<crate::dirty::Last>,
    /// Fork (`fork/perf-portrait-draft.md`): with portrait retouching on, the image before it (`post` then points at its
    /// device buffer), and the boxes the retouching of `lin` may have written (`None`: anywhere; empty without it).
    pre_retouch: Option<crate::pre_retouch::PreRetouch>,
    retouch_boxes: Option<Vec<lightcraft_pipeline::spots_keep::Rect>>,
}

/// Spatial planes of one linear image (`key` = its `lin_key`), each tagged with its radius.
#[derive(Clone, Default)]
pub(crate) struct Planes {
    pub(crate) key: u64,
    pub(crate) log_l: Option<Arc<Buf>>,
    pub(crate) base: Option<(local::BaseKey, Arc<Buf>)>,
    pub(crate) clarity: Option<(u32, Arc<Buf>)>,
    pub(crate) texture: Option<(u32, Arc<Buf>)>,
    /// Process V3: the sharpening band at the Radius slider's scale.
    pub(crate) sharpen: Option<(u32, Arc<Buf>)>,
    pub(crate) dark: Option<(u32, Arc<Buf>, f32)>,
    pub(crate) chroma: Option<(u32, Arc<Buf>)>,
}

impl GpuStages {
    fn get(&self, src: &Arc<Rgb32f>, geo: u64) -> Option<Entry> {
        self.lock().iter().find(|e| e.geo == geo && Arc::ptr_eq(&e.src, src)).cloned()
    }

    /// Keep `last` with the entry of `src` at `geo`.
    fn set_last(&self, src: &Arc<Rgb32f>, geo: u64, last: crate::dirty::Last) {
        if let Some(e) = self.lock().iter_mut().find(|e| e.geo == geo && Arc::ptr_eq(&e.src, src)) {
            e.last = Some(last);
        }
    }

    fn put(&self, e: Entry) {
        let mut v = self.lock();
        v.retain(|o| !(o.geo == e.geo && Arc::ptr_eq(&o.src, &e.src)));
        // (another photo's image before its retouching is of no use to this one's sliders)
        for o in v.iter_mut().filter(|o| !Arc::ptr_eq(&o.src, &e.src)) {
            o.pre_retouch = None;
        }
        v.push(e);
        while v.len() > CAPACITY {
            v.remove(0);
        }
    }

    fn source(&self, src: &Arc<Rgb32f>, upload: impl FnOnce() -> Buf) -> Arc<Buf> {
        let mut g = self.source.lock().unwrap_or_else(|e| e.into_inner());
        match &*g {
            Some((s, b)) if Arc::ptr_eq(s, src) => b.clone(),
            _ => {
                *g = None; // free the previous photo's buffer first
                let b = Arc::new(upload());
                *g = Some((src.clone(), b.clone()));
                b
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Number of cached output sizes.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Bytes held: device buffers (each counted once), and the images a loupe job kept on the host with them
    /// (`dirty::keep_host`, issue #111).
    pub fn bytes(&self) -> usize {
        let mut seen: Vec<*const Buf> = Vec::new();
        let mut total = 0;
        // (each spot's patches are its own)
        let mut patches = 0;
        let mut add = |b: &Arc<Buf>| {
            let p = Arc::as_ptr(b);
            if !seen.contains(&p) {
                seen.push(p);
                total += b.len * 4;
            }
        };
        for e in self.lock().iter() {
            add(&e.sampled);
            if let Some((_, b)) = &e.lin {
                add(b);
            }
            if let Some(l) = &e.last {
                add(&l.out);
            }
            if let Some(p) = e.pre_retouch.as_ref().and_then(|p| p.kept.peek()) {
                add(&p.dev);
                patches += p.host_bytes();
            }
            let p = &e.planes;
            p.log_l.iter().for_each(&mut add);
            p.base.iter().for_each(|(_, b)| add(b));
            p.clarity.iter().for_each(|(_, b)| add(b));
            p.texture.iter().for_each(|(_, b)| add(b));
            p.sharpen.iter().for_each(|(_, b)| add(b));
            p.dark.iter().for_each(|(_, b, _)| add(b));
            p.chroma.iter().for_each(|(_, b)| add(b));
        }
        if let Some((_, b)) = &*self.source.lock().unwrap_or_else(|e| e.into_inner()) {
            add(b);
        }
        self.alphas.each(&mut add);
        self.decontam.each(&mut add);
        patches += self.spots.bytes();
        let host = self.host.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, |k| k.bytes);
        total + patches + host
    }

    /// Drop everything (e.g. when memory is needed).
    pub fn clear(&self) {
        self.lock().clear();
        *self.source.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.alphas.clear();
        self.decontam.clear();
        self.spots.clear();
        self.spots_cpu.clear();
        self.noise.clear();
        *self.host.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A render in progress: the device and the commands recorded so far. Reading a result back
/// submits everything recorded before it.
pub(crate) struct Cx<'a> {
    pub gpu: &'a Gpu,
    enc: Option<wgpu::CommandEncoder>,
    /// Kernel invocations recorded since the last submit.
    pending: u64,
    /// Compute passes recorded since the last submit, and the most one submission held.
    passes: usize,
    most_passes: usize,
    /// Buffers shared with other renders that recorded commands read from (the resident grain
    /// tables): kept until those commands are submitted. Declared after `enc`, so an unsubmitted
    /// encoder is dropped before them.
    held: Vec<std::sync::Arc<Buf>>,
}

/// Kernel invocations recorded before a render submits them (under one full-size pass over a
/// 24 MP export, several passes over a preview): no single submission runs long.
const FLUSH_INVOCATIONS: u64 = 16 << 20;

/// Compute passes recorded before a render submits them, however small: wgpu records each pass in two
/// Metal command buffers of its own, and Metal loses the device past 4096 of them not yet done. Hundreds
/// of spots (about 14 small passes each) went past it in one submission and took the app down (issue #82).
pub(crate) const FLUSH_PASSES: usize = 256;

/// Pixels per dispatch of the per-pixel stage (the heaviest kernel).
const BAND_PIXELS: usize = 4 << 20;

impl<'a> Cx<'a> {
    pub fn new(gpu: &'a Gpu) -> Cx<'a> {
        Cx { gpu, enc: Some(gpu.encoder()), pending: 0, passes: 0, most_passes: 0, held: Vec::new() }
    }

    /// The most compute passes one submission of this render held.
    #[cfg(test)]
    pub fn most_passes(&self) -> usize {
        self.most_passes.max(self.passes)
    }

    /// Forget what was recorded since the last submit (it was just submitted).
    fn submitted(&mut self) {
        self.pending = 0;
        self.most_passes = self.most_passes.max(self.passes);
        self.passes = 0;
    }

    /// Record kernel `name` (see [`Gpu::run`]).
    pub fn run(&mut self, name: &str, p: &[u32], bufs: &[Option<&Buf>], groups: [u32; 3]) {
        let enc = self.enc.get_or_insert_with(|| self.gpu.encoder());
        self.gpu.run(enc, name, p, bufs, groups);
        // every kernel runs 256 invocations per workgroup
        self.pending += groups.iter().map(|g| *g as u64).product::<u64>() * 256;
        self.passes += 1;
        if self.pending >= FLUSH_INVOCATIONS || self.passes >= FLUSH_PASSES {
            self.flush();
        }
    }

    /// The finish kernel's `aux` buffer: `aux`, then the crystal grain's tables when it has them
    /// (copied on the GPU from the resident ones, see [`Gpu::grain_tables`]).
    pub fn finish_aux(&mut self, aux: &[f32], grain: Option<&lightcraft_pipeline::finish::GrainModel>) -> Buf {
        let Some(lightcraft_pipeline::finish::GrainModel::Crystal(g)) = grain else { return self.gpu.upload(aux) };
        let tables: usize = g.tile.sats.iter().map(Vec::len).sum();
        let out = self.gpu.buffer(aux.len().saturating_add(tables));
        if !aux.is_empty() && out.raw().size() >= (aux.len() as u64) * 4 {
            self.gpu.queue.write_buffer(out.raw(), 0, bytemuck::cast_slice(aux));
        }
        let enc = self.enc.get_or_insert_with(|| self.gpu.encoder());
        self.held.extend(self.gpu.grain_tables(enc, &g.tile, &out, aux.len()));
        out
    }

    /// Submit what is recorded and read back `len` values of `b`.
    pub fn read<T: bytemuck::Pod>(&mut self, b: &Buf, len: usize) -> Vec<T> {
        self.submitted();
        let enc = self.enc.take().unwrap_or_else(|| self.gpu.encoder());
        let out = self.gpu.finish_and_read(enc, b, len);
        self.held.clear();
        out
    }

    /// Submit what is recorded so far without waiting. Renders submit stage by stage and every
    /// [`FLUSH_INVOCATIONS`]: long submissions are what GPU watchdogs reset (Windows TDR, the
    /// i915 hang check; issue #78's iGPU spent seconds on an export recorded as one submission),
    /// and buffers dropped by earlier work become reusable by later work (lower peak memory).
    pub fn flush(&mut self) {
        self.submitted();
        if let Some(enc) = self.enc.take() {
            self.gpu.submit([enc.finish()]);
        }
        self.held.clear();
    }

    /// Submit what is recorded and wait for it (profiling only: stage timings).
    pub fn sync(&mut self) {
        self.submitted();
        if let Some(enc) = self.enc.take() {
            self.gpu.submit([enc.finish()]);
        }
        self.held.clear();
        let _ = self.gpu.poll(wgpu::PollType::wait_indefinitely());
    }

    /// A zero-filled buffer (the clear is recorded).
    pub fn zeroed(&mut self, len: usize) -> Buf {
        let out = self.gpu.buffer(len);
        let enc = self.enc.get_or_insert_with(|| self.gpu.encoder());
        enc.clear_buffer(out.raw(), 0, None);
        out
    }

    /// A copy of `b` (recorded).
    pub fn copy(&mut self, b: &Buf) -> Buf {
        let out = self.gpu.buffer(b.len);
        let enc = self.enc.get_or_insert_with(|| self.gpu.encoder());
        enc.copy_buffer_to_buffer(b.raw(), 0, out.raw(), 0, (b.len * 4) as u64);
        out
    }

    /// `len` values of `b` from `offset` (both in 32-bit words), as a new buffer (recorded).
    pub fn slice(&mut self, b: &Buf, offset: usize, len: usize) -> Buf {
        let out = self.gpu.buffer(len);
        let enc = self.enc.get_or_insert_with(|| self.gpu.encoder());
        enc.copy_buffer_to_buffer(b.raw(), (offset * 4) as u64, out.raw(), 0, (len * 4) as u64);
        out
    }

    /// Copy all of `src` into `dst` at `offset` (32-bit words; recorded).
    pub fn copy_into(&mut self, src: &Buf, dst: &Buf, offset: usize) {
        let enc = self.enc.get_or_insert_with(|| self.gpu.encoder());
        enc.copy_buffer_to_buffer(src.raw(), 0, dst.raw(), (offset * 4) as u64, (src.len * 4) as u64);
    }

    /// Copy `w × h` texels from `b` (rows of `stride` 32-bit texels, a multiple of 64) into
    /// `texture` (recorded).
    pub fn copy_to_texture(&mut self, b: &Buf, stride: usize, texture: &wgpu::Texture, w: usize, h: usize) {
        let enc = self.enc.get_or_insert_with(|| self.gpu.encoder());
        enc.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: b.raw(),
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some((stride * 4) as u32), rows_per_image: Some(h as u32) },
            },
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
        );
    }

    pub fn read_rgb(&mut self, b: &Buf, w: usize, h: usize) -> Rgb32f {
        Rgb32f { width: w, height: h, data: self.read(b, w * h * 3) }
    }

    pub fn read_plane(&mut self, b: &Buf, w: usize, h: usize) -> Plane {
        Plane { width: w, height: h, data: self.read(b, w * h) }
    }
}

pub(crate) fn rgb_words(img: &Rgb32f) -> &[f32] {
    bytemuck::cast_slice(&img.data)
}

// ---------------------------------------------------------------------------------------------
// Filters (twins of `lightcraft_raster::blur` / `resample` and `lightcraft_pipeline::local`)

/// Rows each `box_v` thread slides its running sum over (at least; longer for large radii, so the
/// priming sum stays a small share of the work).
const CHUNK: usize = 16;

/// Largest radius whose box pass sums each pixel's window directly: its result then depends on that window alone, so
/// a cut of the image with that margin gives the same bits (issue #111). Larger radii slide a running sum over
/// [`box_chunk`] rows (vertical) or pixels (horizontal: `box_h_run`, whose runs start on the row's first pixel, so a cut
/// of whole rows is unaffected), whose rounding a changed value carries down the run.
pub(crate) const BOX_DIRECT_MAX: usize = 7;

/// Rows each `box_v` thread covers for radius `r` (1: it sums each window directly).
pub(crate) fn box_chunk(r: usize) -> usize {
    if r <= BOX_DIRECT_MAX { 1 } else { (2 * r + 1).clamp(CHUNK, 128) }
}

/// One box pass of radius `r` (clamped edges) over a `w × h` image of `nc` interleaved channels,
/// `src → dst`: horizontal or vertical. `y0`: the image's first row in the whole image it was cut from (0 for a whole
/// image), where the vertical pass's runs start.
fn box_pass(cx: &mut Cx<'_>, (horizontal, r): (bool, usize), src: &Buf, dst: &Buf, (w, h): (usize, usize), nc: usize, y0: usize) {
    let chunk = box_chunk(r);
    let phase = (y0 % chunk) as u32;
    let p = [w as u32, h as u32, nc as u32, r as u32, chunk as u32, phase];
    let (k, groups) = match (horizontal, r > BOX_DIRECT_MAX) {
        // (a large radius slides a running sum along runs of `chunk` pixels, as `box_v` does down a column)
        (true, true) => ("box_h_run", groups2(w.div_ceil(chunk), h, [64, 4])),
        (true, false) => ("box_h", groups2(w, h, [64, 4])),
        (false, _) => ("box_v", groups2(w, (h + phase as usize).div_ceil(chunk), [64, 4])),
    };
    cx.run(k, &p, &[Some(src), Some(dst)], groups);
}

/// Gaussian blur of a `w × h` image of `nc` interleaved channels: three box passes each way
/// (`lightcraft_raster::blur::gaussian`).
pub(crate) fn gaussian(cx: &mut Cx<'_>, src: &Buf, w: usize, h: usize, nc: usize, sigma: f32) -> Buf {
    gaussian_at(cx, src, (w, h), nc, sigma, ((w, h), 0))
}

/// [`gaussian`] of a `w × h` cut of a whole image (`whole`: its size and the cut's first row): the radii are the whole
/// image's, and the vertical runs start on its rows.
pub(crate) fn gaussian_at(cx: &mut Cx<'_>, src: &Buf, (w, h): (usize, usize), nc: usize, sigma: f32, whole: ((usize, usize), usize)) -> Buf {
    let ((fw, fh), y0) = whole;
    let Some(radii) = lightcraft_raster::blur::radii_for(sigma, fw, fh) else {
        return cx.copy(src);
    };
    let passes: Vec<(bool, usize)> = radii.iter().map(|r| (true, *r)).chain(radii.iter().map(|r| (false, *r))).filter(|p| p.1 > 0).collect();
    if passes.is_empty() {
        return cx.copy(src);
    }
    // ping-pong between two buffers
    let mut bufs = [cx.gpu.buffer(w * h * nc), cx.gpu.buffer(w * h * nc)];
    for (i, (horizontal, r)) in passes.iter().enumerate() {
        let (out, prev) = (&bufs[i % 2], &bufs[(i + 1) % 2]);
        box_pass(cx, (*horizontal, *r), if i == 0 { src } else { prev }, out, (w, h), nc, y0);
    }
    let last = (passes.len() - 1) % 2;
    let [a, b] = std::mem::replace(&mut bufs, [cx.gpu.buffer(0), cx.gpu.buffer(0)]);
    if last == 0 { a } else { b }
}

/// Resample taps of `lightcraft_raster::resample::resize`, packed for the `resize_*` kernels.
fn taps(src: usize, dst: usize, filter: Filter) -> Vec<u32> {
    pack_taps(&lightcraft_raster::resample::weights(src, dst, filter), 0)
}

/// Taps (first source index and weights, per output index) packed for the `resize_*` kernels, the source indices less
/// `shift` (a cut of the source starting there).
pub(crate) fn pack_taps(w: &[(usize, Vec<f32>)], shift: usize) -> Vec<u32> {
    let mut t = Vec::with_capacity(w.len() * 3);
    let mut weights = Vec::new();
    let base = w.len() * 3;
    for (lo, ws) in w {
        t.extend_from_slice(&[lo.saturating_sub(shift) as u32, ws.len() as u32, (base + weights.len()) as u32]);
        weights.extend(ws.iter().map(|v| v.to_bits()));
    }
    t.extend(weights);
    t
}

/// `lightcraft_raster::resample::resize` of an `nc`-channel image.
pub(crate) fn resize(cx: &mut Cx<'_>, src: &Buf, (sw, sh): (usize, usize), (dw, dh): (usize, usize), nc: usize, filter: Filter) -> Buf {
    let (dw, dh) = (dw.max(1), dh.max(1));
    if (sw, sh) == (dw, dh) {
        return cx.copy(src);
    }
    resize_with(cx, src, (sw, sh), (dw, dh), nc, (&taps(sw, dw, filter), &taps(sh, dh, filter)))
}

/// Resample an `nc`-channel `sw × sh` image to `dw × dh` with packed taps ([`pack_taps`]) across and down.
pub(crate) fn resize_with(
    cx: &mut Cx<'_>,
    src: &Buf,
    (sw, sh): (usize, usize),
    (dw, dh): (usize, usize),
    nc: usize,
    (tx, ty): (&[u32], &[u32]),
) -> Buf {
    let tx = cx.gpu.upload(tx);
    let tmp = cx.gpu.buffer(dw * sh * nc);
    cx.run("resize_h", &[sw as u32, sh as u32, dw as u32, nc as u32], &[Some(src), Some(&tx), Some(&tmp)], groups2(dw, sh, [16, 16]));
    let ty = cx.gpu.upload(ty);
    let out = cx.gpu.buffer(dw * dh * nc);
    cx.run("resize_v", &[dw as u32, sh as u32, dh as u32, nc as u32], &[Some(&tmp), Some(&ty), Some(&out)], groups2(dw, dh, [16, 16]));
    out
}

/// An element-wise kernel of the `map` module over `n` items.
pub(crate) fn map(cx: &mut Cx<'_>, k: &str, n: usize, extra: &[u32], ins: [Option<&Buf>; 3], out: &Buf) {
    let mut p = vec![n as u32];
    p.extend_from_slice(extra);
    cx.run(k, &p, &[ins[0], ins[1], ins[2], Some(out)], groups1(n));
}

/// Self-guided filter of a plane (`local::guided`).
fn guided(cx: &mut Cx<'_>, p: &Buf, w: usize, h: usize, sigma: f32, eps: f32) -> Buf {
    guided_at(cx, p, (w, h), sigma, eps, ((w, h), 0))
}

/// [`guided`] of a `w × h` cut of a whole image (`whole`, as [`gaussian_at`]).
pub(crate) fn guided_at(cx: &mut Cx<'_>, p: &Buf, (w, h): (usize, usize), sigma: f32, eps: f32, whole: ((usize, usize), usize)) -> Buf {
    let ab = guided_coeffs_at(cx, p, (w, h), sigma, eps, whole);
    let q = cx.gpu.buffer(w * h);
    map(cx, "guided_apply", w * h, &[], [Some(p), Some(&ab), None], &q);
    q
}

/// Blurred (a, b) coefficients, interleaved.
fn guided_coeffs(cx: &mut Cx<'_>, p: &Buf, w: usize, h: usize, sigma: f32, eps: f32) -> Buf {
    guided_coeffs_at(cx, p, (w, h), sigma, eps, ((w, h), 0))
}

/// [`guided_coeffs`] of a `w × h` cut of a whole image (`whole`, as [`gaussian_at`]).
pub(crate) fn guided_coeffs_at(cx: &mut Cx<'_>, p: &Buf, (w, h): (usize, usize), sigma: f32, eps: f32, whole: ((usize, usize), usize)) -> Buf {
    let n = w * h;
    let pp = cx.gpu.buffer(2 * n);
    map(cx, "guided_pre", n, &[], [Some(p), None, None], &pp);
    let m = gaussian_at(cx, &pp, (w, h), 2, sigma, whole);
    let ab = cx.gpu.buffer(2 * n);
    map(cx, "guided_ab", n, &[eps.to_bits()], [Some(&m), None, None], &ab);
    gaussian_at(cx, &ab, (w, h), 2, sigma, whole)
}

/// Separable median of a plane (`local::median_hv`).
pub(crate) fn median_hv(cx: &mut Cx<'_>, p: &Buf, w: usize, h: usize, r: usize) -> Buf {
    if r == 0 || w * h == 0 {
        return cx.copy(p);
    }
    let pr = [w as u32, h as u32, 1, r.min(local::BASE_DETAIL_MAX) as u32];
    let tmp = cx.gpu.buffer(w * h);
    cx.run("median_h", &pr, &[Some(p), Some(&tmp)], groups2(w, h, [16, 16]));
    let out = cx.gpu.buffer(w * h);
    cx.run("median_v", &pr, &[Some(&tmp), Some(&out)], groups2(w, h, [16, 16]));
    out
}

/// The highlights/shadows base (`local::tone_base`; `detail` 0: V1, no median).
fn tone_base(cx: &mut Cx<'_>, l: &Buf, w: usize, h: usize, sigma: f32, detail: usize) -> Buf {
    if detail == 0 {
        return guided_fast(cx, l, w, h, sigma, local::BASE_EPS);
    }
    let m = median_hv(cx, l, w, h, detail);
    guided_fast(cx, &m, w, h, sigma, local::BASE_EPS)
}

/// Fast guided filter (`local::guided_fast`): coefficients on a subsampled grid.
fn guided_fast(cx: &mut Cx<'_>, p: &Buf, w: usize, h: usize, sigma: f32, eps: f32) -> Buf {
    let s = local::guided_fast_step(sigma);
    if s <= 1 {
        return guided(cx, p, w, h, sigma, eps);
    }
    let (lw, lh) = (w.div_ceil(s).max(1), h.div_ceil(s).max(1));
    let lo = resize(cx, p, (w, h), (lw, lh), 1, Filter::Box);
    let ab = guided_coeffs(cx, &lo, lw, lh, sigma / s as f32, eps);
    let up = resize(cx, &ab, (lw, lh), (w, h), 2, Filter::Bilinear);
    let q = cx.gpu.buffer(w * h);
    map(cx, "guided_apply", w * h, &[], [Some(p), Some(&up), None], &q);
    q
}

// ---------------------------------------------------------------------------------------------
// Geometry (`Frame::sample`)

/// Integer pixel map `(x, y) → (m0·x + m1·y + m2, m3·x + m4·y + m5)` from an oriented image back
/// to the source of `w × h` (`Image::oriented`: optional horizontal flip, then quarter turns).
pub(crate) fn orient_map(o: Orientation, w: usize, h: usize) -> [i32; 6] {
    let (flip, turns) = o.to_parts();
    let (w, h) = (w as i32, h as i32);
    let mut ops: Vec<fn(i32, i32) -> [i32; 6]> = Vec::new();
    if flip {
        ops.push(|w, _| [-1, 0, w - 1, 0, 1, 0]);
    }
    let cw: fn(i32, i32) -> [i32; 6] = |_, h| [0, 1, 0, -1, 0, h - 1];
    let half: fn(i32, i32) -> [i32; 6] = |w, h| [-1, 0, w - 1, 0, -1, h - 1];
    match turns {
        1 => ops.push(cw),
        2 => ops.push(half),
        3 => {
            ops.push(half);
            ops.push(cw);
        }
        _ => {}
    }
    // compose: T ← T ∘ op, tracking each op's input size
    let mut t = [1, 0, 0, 0, 1, 0];
    let (mut cw_, mut ch_) = (w, h);
    for op in ops {
        let o = op(cw_, ch_);
        t = [
            t[0] * o[0] + t[1] * o[3],
            t[0] * o[1] + t[1] * o[4],
            t[0] * o[2] + t[1] * o[5] + t[2],
            t[3] * o[0] + t[4] * o[3],
            t[3] * o[1] + t[4] * o[4],
            t[3] * o[2] + t[4] * o[5] + t[5],
        ];
        if o[0] == 0 {
            (cw_, ch_) = (ch_, cw_);
        }
    }
    t
}

fn affine_bits(a: &lightcraft_geom::Affine) -> [u32; 6] {
    a.0.map(|v| (v as f32).to_bits())
}

/// The `sample_warp` kernel's parameters (layout documented in `geom.wgsl`).
fn warp_params(
    wp: &lightcraft_pipeline::optics::Warp,
    base: (usize, usize),
    out: (usize, usize),
    o2t: &lightcraft_geom::Affine,
    sx: f64,
    sy: f64,
) -> Vec<u32> {
    let f = |v: f64| (v as f32).to_bits();
    let mut p = vec![base.0 as u32, base.1 as u32, out.0 as u32, out.1 as u32];
    p.extend(affine_bits(o2t));
    p.extend([f(sx), f(sy), f(wp.w), f(wp.h)]);
    let persp = wp.persp != lightcraft_geom::Homography::IDENTITY;
    p.push(persp as u32);
    p.extend(wp.persp_inv.0.map(f));
    p.push(f(wp.k1));
    p.extend(wp.ca.map(f));
    p.push(f(wp.lens_dist));
    let warp = wp.lens.and_then(|l| l.warp);
    p.push(warp.is_some() as u32);
    let w = warp.unwrap_or_default();
    p.extend(w.planes.iter().flatten().map(|v| f(*v)));
    p.extend([f(w.center.x), f(w.center.y), f(w.radius)]);
    p.extend([f(wp.vig_stops), f(wp.vig_power), f(wp.lens_vig)]);
    let vig = wp.lens.and_then(|l| l.vignette);
    p.push(vig.is_some() as u32);
    let v = vig.unwrap_or_default();
    p.extend(v.k.map(f));
    p.extend([f(v.center.x), f(v.center.y), f(v.radius)]);
    p.push(wp.per_channel() as u32);
    p.push(wp.has_gain() as u32);
    debug_assert_eq!(p.len(), 65);
    p
}

/// Output rows per block of [`coverage_mask`]; a block is one mask word (32 px) wide. 16 rows measured
/// ~20 % faster than 8 at 6000 × 4000 (fewer interval evaluations, few more edge pixels).
const COVER_ROWS: usize = 16;

/// The largest source (bytes on the device) a view's stages keep uploaded between renders.
const RETAIN_SOURCE_BYTES: usize = 96 << 20;

/// Big sources uploaded to the shared copy so far (diagnostics and tests).
static SOURCE_UPLOADS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The one big source (the original a zoom window is cut from) kept on the device for all the
/// views of the photo: the Before and After windows and every pan share it, instead of each
/// view's stages holding (or re-uploading) 288 MB of a 24 MP photo. It does not count as a view's
/// stages; it is released with the buffer pool when the app idles.
static SHARED_SOURCE: std::sync::Mutex<Option<(std::sync::Weak<Rgb32f>, Arc<Buf>)>> = std::sync::Mutex::new(None);

fn shared_source(src: &Arc<Rgb32f>, upload: impl FnOnce() -> Buf) -> Arc<Buf> {
    let mut g = SHARED_SOURCE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((weak, buf)) = &*g
        && weak.upgrade().is_some_and(|s| Arc::ptr_eq(&s, src))
    {
        return buf.clone();
    }
    // the previous photo's copy goes (on a render thread it is retired with the render's other
    // buffers, so for a moment both exist)
    *g = None;
    SOURCE_UPLOADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let buf = Arc::new(upload());
    // a failed upload's buffer holds zeros: this render falls back, and no other may reuse it
    if crate::ctx::failed() {
        return buf;
    }
    *g = Some((Arc::downgrade(src), buf.clone()));
    buf
}

/// Free the shared big source (see [`shared_source`]).
pub(crate) fn release_shared_source() {
    *SHARED_SOURCE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Device bytes of the shared big source, 0 when there is none.
pub(crate) fn shared_source_bytes() -> usize {
    SHARED_SOURCE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, |(_, b)| b.len * 4)
}

pub(crate) fn source_uploads() -> u64 {
    SOURCE_UPLOADS.load(std::sync::atomic::Ordering::Relaxed)
}

/// The reference framing decision ([`Warp::covers`](lightcraft_pipeline::optics::Warp::covers)) for every
/// output pixel, one bit per pixel, rows padded to 32-bit words. Blocks of 32 × [`COVER_ROWS`] pixels whose
/// interval bounds place them clearly inside or outside the image are filled at once
/// ([`Warp::block_coverage`](lightcraft_pipeline::optics::Warp::block_coverage), the same formulas evaluated
/// with outward-rounded intervals); only blocks near the image edge evaluate each pixel. Same bits as
/// evaluating every pixel.
fn coverage_mask(wp: &lightcraft_pipeline::optics::Warp, o2t: &lightcraft_geom::Affine, w: usize, h: usize) -> Vec<u32> {
    let words = w.div_ceil(32);
    let mut coverage = vec![0u32; words * h];
    if words == 0 {
        return coverage;
    }
    lightcraft_raster::par_rows(&mut coverage, words * COVER_ROWS, |band, rows| {
        let y0 = band * COVER_ROWS;
        let y1 = y0 + rows.len() / words;
        for word in 0..words {
            let (x0, x1) = (word * 32, (word * 32 + 32).min(w));
            let full = if x1 - x0 == 32 { u32::MAX } else { (1u32 << (x1 - x0)) - 1 };
            let block = wp.block_coverage(o2t, x0, x1, y0, y1);
            for (y, row) in (y0..y1).zip(rows.chunks_exact_mut(words)) {
                let Some(bits) = row.get_mut(word) else { continue };
                *bits = match block {
                    Some(true) => full,
                    Some(false) => 0,
                    None => (x0..x1).filter(|&x| wp.covers(o2t, x, y)).fold(0, |b, x| b | 1 << (x - x0)),
                };
            }
        }
    });
    coverage
}

/// Resample the source into the output frame (`Frame::sample`). May return the source buffer
/// itself when the frame is the identity.
fn sample(cx: &mut Cx<'_>, src: &Arc<Rgb32f>, src_buf: Arc<Buf>, plan: &Plan<'_>) -> Arc<Buf> {
    let (w, h) = (plan.w, plan.h);
    let fr = &plan.frame;
    let sp = fr.sample_plan(src.width, src.height, w, h);
    let oriented = if fr.orient == Orientation::Normal {
        src_buf
    } else {
        let out = cx.gpu.buffer(sp.ow * sp.oh * 3);
        let mut p = vec![src.width as u32, src.height as u32, sp.ow as u32, sp.oh as u32];
        p.extend(orient_map(fr.orient, src.width, src.height).map(|v| v as u32));
        cx.run("orient", &p, &[Some(&src_buf), Some(&out), None, None], groups2(sp.ow, sp.oh, [16, 16]));
        Arc::new(out)
    };
    let (base, (bw, bh)) = match sp.prefilter {
        Some(d) => (Arc::new(resize(cx, &oriented, (sp.ow, sp.oh), d, 3, Filter::Mitchell)), d),
        None => (oriented, (sp.ow, sp.oh)),
    };
    let mut affine = |xf: &lightcraft_geom::Affine| {
        let out = cx.gpu.buffer(w * h * 3);
        let mut p = vec![bw as u32, bh as u32, w as u32, h as u32];
        p.extend(affine_bits(xf));
        cx.run("sample_affine", &p, &[Some(&base), Some(&out), None, None], groups2(w, h, [16, 16]));
        Arc::new(out)
    };
    match (&sp.mode, fr.warp.as_ref()) {
        (SampleMode::Copy, _) => base,
        (SampleMode::Affine(xf), _) => affine(xf),
        // `sample_plan` plans a warp only when there is one; without it the same mapping is affine
        (SampleMode::Warp(o2t), None) => affine(&(lightcraft_geom::Affine::scale(sp.sx, sp.sy) * *o2t)),
        (SampleMode::Warp(o2t), Some(wp)) => {
            let out = cx.gpu.buffer(w * h * 3);
            let p = warp_params(wp, (bw, bh), (w, h), o2t, sp.sx, sp.sy);
            // f32 rounding can cross a source edge. Keep the reference f64
            // framing decision in a bit mask; color sampling stays on the GPU.
            let coverage = cx.gpu.upload(&coverage_mask(wp, o2t, w, h));
            // (fork, `fork/retouch-reshape.md`: Face Reshape's grid; without one the kernel reads the zeroed dummy)
            let field = wp.reshape.as_ref().map(|f| cx.gpu.upload(&f.gpu_table()));
            cx.run("sample_warp", &p, &[Some(&base), Some(&out), Some(&coverage), field.as_ref()], groups2(w, h, [16, 16]));
            Arc::new(out)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The render

/// CPU copies made along the way (for stages that still run on the CPU).
#[derive(Default)]
pub(crate) struct Host {
    sampled: Option<Rgb32f>,
    pub(crate) lin: Option<Arc<Rgb32f>>,
    pub(crate) log_l: Option<Arc<Plane>>,
}

pub(crate) fn profiling() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("LIGHTCRAFT_PROFILE").is_some())
}

/// A finished render on the window's device, ready to draw: an `Rgba8Unorm` texture (the 8-bit
/// output, sRGB-curve encoded, as egui's own textures) that never went through host memory.
#[derive(Clone, Debug)]
pub struct DisplayFrame {
    pub texture: wgpu::Texture,
    pub width: usize,
    pub height: usize,
}

/// What a render produced: pixels in host memory, or (asked for with `display` and possible) a
/// texture on the window's device plus the histogram.
pub enum Output {
    Image(Rendered),
    Display {
        frame: DisplayFrame,
        histogram: Histogram,
    },
    /// The alphas of the visible masks (see [`Want::MaskPlanes`]).
    Masks(lightcraft_pipeline::masks::PlanesOut),
    /// Pixels, and the same picture fitted to the size asked for ([`Want::ImageFit`]) when that was made on the device.
    ImageFit(Rendered, Option<Rgba8>),
}

/// What a render is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want<'a> {
    /// Pixels in host memory.
    Image,
    /// A texture on the window's device when [`can_display`], else pixels.
    Display,
    /// Pixels, and the picture shrunk to fit this size with a Lanczos filter on the device before it is read back (the CPU's
    /// `fit_for_display`, in the same colour space), when the picture is larger and nothing is drawn over it on the CPU.
    ImageFit((usize, usize)),
    /// Only the planes `ask` names, with each mask's Amount applied (`lightcraft_pipeline::mask_planes`): the photo is not
    /// finished, and the planes are not read back when the asker holds them.
    MaskPlanes(&'a lightcraft_pipeline::masks::PlanesAsk),
}

/// Whether a render of this plan can end in a texture instead of a readback: the device is the
/// window's, nothing is drawn over the image on the CPU (overlays) or cut from it (a window that
/// spots grew), and the texture fits the device.
fn can_display(gpu: &Gpu, plan: &Plan<'_>, req: &RenderRequest) -> bool {
    let max = gpu.device.limits().max_texture_dimension_2d as usize;
    gpu.window && req.overlay == lightcraft_pipeline::Overlay::None && plan.keep.is_none() && plan.w <= max && plan.h <= max && plan.w * plan.h > 0
}

/// Render on `gpu`, reusing `stages` (if given); with `display`, into a texture when
/// [`can_display`]. `None` (or a result the caller discards) when the render failed: the reason is
/// recorded with [`crate::ctx::fail`].
#[allow(clippy::too_many_arguments)]
pub fn render(
    gpu: &Gpu,
    src: &Arc<Rgb32f>,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&GpuStages>,
    fault: Option<crate::Fault>,
    want: Want<'_>,
) -> Option<Output> {
    let _in_flight = InFlight::new();
    let mut t = profiling().then(std::time::Instant::now);
    // Each stage is submitted on its own; under `LIGHTCRAFT_PROFILE` also waited for, so the
    // timings are real.
    let lap = |what: &str, t: &mut Option<std::time::Instant>, cx: &mut Cx<'_>| match t {
        Some(t) => {
            cx.sync();
            let (peak, now) = crate::ctx::take_peak();
            eprintln!("  gpu {what}: {:.1} ms (device buffers: peak {} MB, now {} MB)", t.elapsed().as_secs_f64() * 1e3, peak >> 20, now >> 20);
            *t = std::time::Instant::now();
        }
        None => cx.flush(),
    };
    let plan = lightcraft_pipeline::plan_kept(src, info, s, req, stages.map(|g| &g.spot_finals.0));
    let (w, h) = (plan.w, plan.h);
    let Some(n) = w.checked_mul(h).filter(|n| n.checked_mul(12).is_some()) else {
        fail(FailKind::Limit, format!("{w}×{h} output is too large"));
        return None;
    };
    let _limit = match fault {
        Some(crate::Fault::Limit(bytes)) => Some(crate::ctx::LimitOverride::new(bytes)),
        _ => None,
    };
    if !gpu.fits(n * 3) {
        fail(
            FailKind::Limit,
            format!("{w}×{h} needs {} MiB buffers, over the device's {} MiB storage-buffer limit", (n * 12).div_ceil(1 << 20), gpu.limit() >> 20),
        );
        return None;
    }
    let s = &*plan.settings;
    let cached = stages.and_then(|c| c.get(src, plan.geo));
    // the view's last render at this size, when this one differs from it only in its spots (issue #111)
    crate::dirty::note_frame(None);
    let rest = stages
        // (a dropped dispatch, or a device failing elsewhere, may: they fail an incremental render too, and the checks
        // must catch them)
        .filter(|_| fault.is_none_or(|f| matches!(f, crate::Fault::DropWork | crate::Fault::BrokenElsewhere)))
        .filter(|_| !matches!(want, Want::MaskPlanes(_)) && crate::dirty::eligible(info, &plan))
        .map(|_| Arc::new(crate::dirty::Rest::of(info, &plan, req)));
    let start = cached.as_ref().and_then(|e| e.last.clone()).filter(|l| rest.as_deref() == Some(&*l.rest));
    if profiling() {
        eprintln!("  gpu dirty: {}", crate::dirty::why(rest.as_deref(), cached.as_ref().and_then(|e| e.last.as_ref())));
    }
    let mut host = Host::default();
    let mut cx = Cx::new(gpu);
    lap("plan", &mut t, &mut cx);

    // 1. geometry (on the CPU when the source exceeds the device's buffer limit)
    let sampled = match &cached {
        Some(e) => e.sampled.clone(),
        // (a WarpRectilinear2 chain also goes to the CPU: its degree-14 polynomial needs f64)
        None if gpu.fits(src.data.len() * 3) && !plan.frame.warp.as_ref().is_some_and(|w| w.needs_f64()) => {
            let upload = || gpu.upload(rgb_words(src));
            // (a source too big for a view's stages to keep is the one shared copy: a window is
            // cut from the photo's own pixels, and what its stages keep is its own sampled pixels)
            let src_buf = match stages {
                Some(c) if src.data.len() * 12 <= RETAIN_SOURCE_BYTES => c.source(src, upload),
                Some(_) => shared_source(src, upload),
                None => Arc::new(upload()),
            };
            sample(&mut cx, src, src_buf, &plan)
        }
        None => {
            let img = plan.frame.sample(src, w, h);
            let b = Arc::new(gpu.upload(rgb_words(&img)));
            host.sampled = Some(img);
            b
        }
    };
    lap("sample", &mut t, &mut cx);

    // 2. white balance, defringe, spots, noise reduction
    let at = ViewAt { stages, src, cached: cached.as_ref(), start: start.as_ref() };
    let Linear { lin, tiles: t_lin, post, colour_noise, pre_retouch, retouch_boxes } = linear_stage(&mut cx, &at, &sampled, info, &plan, &mut host);
    lap("wb/nr", &mut t, &mut cx);

    // 3. spatial planes (`t_out`: the output tiles that change from `start`'s)
    let (planes, prep, t_out) = planes_stage(&mut cx, &at, (&lin, t_lin.as_ref()), &plan, req);
    lap("planes", &mut t, &mut cx);
    let edge_boxes = retouch_boxes.clone();
    if let Some(c) = stages {
        let lin = Some((plan.lin_key, lin.clone()));
        c.put(Entry {
            src: src.clone(),
            geo: plan.geo,
            sampled: sampled.clone(),
            lin,
            post,
            colour_noise,
            planes,
            last: None,
            pre_retouch,
            retouch_boxes,
        });
    }

    // 4. masks
    let (masks, terms) = crate::masks::masks(&mut cx, (&lin, &sampled), &prep, &plan, &mut host, stages.map(|c| (&c.alphas, src)));
    lap("masks", &mut t, &mut cx);
    if let Want::MaskPlanes(ask) = want {
        return Some(Output::Masks(mask_planes(&mut cx, masks.as_ref(), &plan, (&lin, &prep.log_l), Arc::as_ptr(src) as usize, ask)));
    }
    // fork (`fork/perf-decontam-gpu.md`): the decontaminated masks' edges, after the alphas
    let edge_lin = crate::decontam::Lin { buf: &lin, boxes: edge_boxes.as_deref() };
    let (masks, dc) = crate::decontam::with_edges(&mut cx, &plan, masks, &terms, &edge_lin, &mut host, stages.map(|c| (&c.decontam, src)))?;

    // 5. per-pixel stage
    let mut fp = FinishParams::new(FinishParamsInput {
        settings: s,
        frame: &plan.frame,
        info,
        image_size: (w, h),
        px_per_long: plan.px_per_long,
        air_pre: prep.air,
        space: req.space,
        draft: req.quality == lightcraft_pipeline::Quality::Draft,
    });
    // soft proofing as the CPU's `finish` sets it up (an output-device table never gets here: see `render`)
    fp.proof = req.proof.map(|pr| pr.params(req.space));
    // a float render is `finish_deep`: its samples are the output space's, whatever the display
    let deep = req.depth != lightcraft_pipeline::OutputDepth::U8;
    if let Some(d) = req.display.as_ref().filter(|_| !deep) {
        fp.for_display(d, req.proof);
    }
    let present = Present {
        clarity: prep.clarity.is_some(),
        texture: prep.texture.is_some(),
        sharpen: prep.sharpen.is_some(),
        sharpen_off: if prep.texture.is_some() && prep.sharpen.is_some() { n } else { 0 },
        dark: prep.dark.is_some(),
        chroma: masks.is_some() && prep.chroma.is_some(),
    };
    // the finish kernel's `tex` binding: the texture plane, then (process V3) the sharpening plane at
    // `SHARP_OFF` (one binding for both keeps the kernel within the storage-buffer count every
    // supported device has); one plane alone is bound as it is
    let tex_buf: Option<Arc<Buf>> = match (&prep.texture, &prep.sharpen) {
        (Some(t), Some(sh)) => {
            let both = cx.gpu.buffer(2 * n);
            cx.copy_into(t, &both, 0);
            cx.copy_into(sh, &both, n);
            Some(Arc::new(both))
        }
        (Some(b), None) | (None, Some(b)) => Some(b.clone()),
        (None, None) => None,
    };
    let (p, aux) = finish_block(&fp, &terms, &present, deep);
    let (p, aux) = crate::decontam::describe(p, aux, &dc.desc);
    if crate::ctx::failed() {
        return None; // a malformed parameter block failed the render: the CPU fallback redoes it
    }
    let aux = cx.finish_aux(&aux, fp.grain.as_ref());
    // the last output where only `t_out` changes (issue #111; never a float render: its output is three words a pixel,
    // and `Rest` holds the request, so a float render never starts from an 8-bit one); else cleared, so that work which
    // never ran cannot pass for an image (see the check below)
    // (nor when a decontaminated edge was estimated again: what it changes reaches past the tiles that changed)
    let from = start.as_ref().zip(t_out.as_ref()).filter(|_| !deep).filter(|_| !dc.fresh);
    let out = Arc::new(match from {
        Some((l, _)) => cx.copy(&l.out),
        None => cx.zeroed(if deep { 3 * n } else { n }),
    });
    if fault == Some(crate::Fault::Validation) {
        // a dispatch the device rejects (too many workgroups)
        cx.run("box_h", &[0; 5], &[Some(&aux), Some(&out)], [gpu.device.limits().max_compute_workgroups_per_dimension + 1, 1, 1]);
    }
    // in bands of rows: each dispatch (and submission) stays short on a slow GPU
    let y0_at = crate::params::index("Y0").unwrap_or(0);
    let x0_at = crate::params::index("X0").unwrap_or(0);
    let mut p = p;
    let whole = [[0, 0, w, h]];
    let rects = from.map_or_else(|| whole.to_vec(), |(_, tiles)| tiles.rects());
    if from.is_some() {
        // the copied output is written everywhere: cleared where this render writes, so that work which never runs
        // there still fails the checks below (and the display's)
        for r in &rects {
            crate::dirty::cut::clear_rect(&mut cx, 1, &out, w, r);
        }
    }
    if stages.is_some() {
        crate::dirty::note_tiles(from.map_or_else(|| crate::dirty::Tiles::all((w, h)).len(), |(_, tiles)| tiles.count()));
    }
    if profiling() && stages.is_some() {
        let all = crate::dirty::Tiles::all((w, h)).len();
        eprintln!(
            "  gpu tiles: {} of {all} (linear {:?}, start {})",
            from.map_or(all, |(_, t)| t.count()),
            t_lin.as_ref().map(|t| t.count()),
            start.is_some()
        );
    }
    for r in rects.iter().filter(|_| fault != Some(crate::Fault::DropWork)) {
        let rw = r[2].saturating_sub(r[0]);
        let rows = (BAND_PIXELS / rw.max(1) / 16 * 16).max(16); // whole workgroups: bands never overlap
        for y0 in (r[1]..r[3]).step_by(rows) {
            p[y0_at] = y0 as u32;
            p[x0_at] = r[0] as u32;
            cx.run(
                dc.entry(),
                &p,
                &[
                    Some(&lin),
                    Some(&prep.log_l),
                    Some(&prep.base),
                    prep.clarity.as_deref(),
                    tex_buf.as_deref(),
                    prep.dark.as_deref(),
                    masks.as_ref(),
                    Some(&aux),
                    Some(&out),
                ],
                groups2(rw, rows.min(r[3] - y0), [16, 16]),
            );
        }
    }
    if deep {
        let rendered = read_deep(&mut cx, &out, &plan, req);
        lap("finish + readback (deep)", &mut t, &mut cx);
        return rendered.map(Output::Image);
    }
    // what to keep for the next render, once this one is known to be good
    let keep_last = || {
        let (Some(c), Some(rest)) = (stages, rest.clone()) else { return };
        let frame = crate::dirty::next_frame();
        let air = prep.air.to_bits();
        c.set_last(src, plan.geo, crate::dirty::Last { rest, lin_key: plan.lin_key, out: out.clone(), frame, air });
        let changed =
            from.filter(|_| plan.keep.is_none() && req.overlay == lightcraft_pipeline::Overlay::None).map(|(l, _)| (l.frame, rects.clone()));
        crate::dirty::note_frame(Some(crate::dirty::FrameNote { frame, changed }));
    };
    // (recorded before the readback below, which submits it)
    let fitted = match want {
        Want::ImageFit(max) if plan.keep.is_none() && req.overlay == lightcraft_pipeline::Overlay::None => fit_on_device(&mut cx, &out, (w, h), max),
        _ => None,
    };
    if want == Want::Display && can_display(gpu, &plan, req) {
        let shown = to_texture(&mut cx, &out, w, h);
        if shown.is_some() {
            keep_last();
        }
        return shown.map(|(frame, histogram)| Output::Display { frame, histogram });
    }
    // the alpha a mask overlay shows: copied out before the readback below submits
    let overlay_mask = req.overlay.mask(s).map(|m| {
        let list = s.masks.iter().filter(|m| m.visible && !m.components.is_empty());
        match (list.map(|m| m.id).position(|id| id == m.id), masks.as_ref()) {
            (Some(mi), Some(all)) => Ok(cx.slice(all, mi * n, n)),
            _ => Err(m),
        }
    });
    // every visible mask's alpha, for `Overlay::AllMasks` (one tint over all masks at once).
    // The masks buffer holds each visible mask's alpha back-to-back in the same `list` order the
    // single-mask overlay uses, so read the whole buffer back in ONE GPU->CPU transfer and split it
    // on the CPU, instead of one synchronous readback per mask (which stalls the pipeline N times).
    let all_overlay_alphas: Option<Vec<Plane>> = if matches!(req.overlay, lightcraft_pipeline::Overlay::AllMasks { .. }) {
        masks.as_ref().map(|all| {
            let list: Vec<_> = s.masks.iter().filter(|m| m.visible && !m.components.is_empty()).collect();
            if list.is_empty() {
                return Vec::new();
            }
            let raw: Vec<f32> = cx.read(all, n * list.len());
            let mut alphas = Vec::with_capacity(list.len());
            for mi in 0..list.len() {
                let s = mi * n;
                alphas.push(Plane { width: w, height: h, data: raw[s..s + n].to_vec() });
            }
            alphas
        })
    } else {
        None
    };
    let data: Vec<[u8; 4]> = cx.read(&out, n);
    let image = Rgba8 { width: w, height: h, data };
    lap("finish + readback", &mut t, &mut cx);
    if crate::ctx::failed() {
        return None;
    }
    // The per-pixel kernel writes alpha 255 everywhere and `out` was cleared: a pixel with any
    // other alpha means work silently did not run (a driver that reset or dropped a submission
    // without reporting it — issue #78's all-black exports).
    let unwritten = unwritten(&image.data);
    if unwritten > 0 {
        fail(FailKind::Fatal, format!("the GPU returned an incomplete image ({unwritten} of {n} pixels unwritten)"));
        return None;
    }
    let region = plan.keep.map_or((0, 0, w, h), |k| (k.x, k.y, k.w, k.h));
    let histogram = histogram(&mut cx, &out, w, region);
    lap("histogram", &mut t, &mut cx);
    // An entirely black result (upstream work that did not run would give that too): redo it on
    // the CPU, which costs time only for the rare photo that really is black.
    if n >= 4096 && [&histogram.r, &histogram.g, &histogram.b].iter().all(|c| c.first() == Some(&histogram.total)) {
        fail(FailKind::Blank, "the GPU result is entirely black; rendering it on the CPU to be sure".into());
        return None;
    }
    keep_last();
    let mut image = image;
    let overlay_mask = overlay_mask.map(|r| match r {
        Ok(b) => cx.read_plane(&b, w, h),
        // a hidden mask: evaluate it on the CPU
        Err(m) => {
            let img = host.lin.get_or_insert_with(|| Arc::new(cx.read_rgb(&lin, w, h))).clone();
            let l = host.log_l.get_or_insert_with(|| Arc::new(cx.read_plane(&prep.log_l, w, h))).clone();
            lightcraft_pipeline::masks::evaluate_one(m, &plan.frame, w, h, &img, &l, s.light.exposure as f32, plan.mattes.as_deref())
        }
    });
    // a sharpening view: the detail planes this render sharpened with
    let overlay_mask = overlay_mask.or_else(|| {
        req.overlay.reads_detail().then(|| {
            let l = host.log_l.get_or_insert_with(|| Arc::new(cx.read_plane(&prep.log_l, w, h))).clone();
            let v3 = plan.settings.process >= lightcraft_develop::Process::V3.version();
            let band = if v3 { prep.sharpen.as_deref() } else { prep.texture.as_deref() };
            let band = band.map(|b| cx.read_plane(b, w, h));
            lightcraft_pipeline::visualize::sharpen_plane(req.overlay, &plan.settings, &l, band.as_ref(), plan.px_per_long, plan.detail_scale)
        })?
    });
    // a Portrait slider's area map: made from the faces, never the pixels
    let overlay_mask = overlay_mask.or_else(|| match req.overlay {
        lightcraft_pipeline::Overlay::RetouchArea { feature, face } => lightcraft_pipeline::retouch_step::area(info, &plan, feature, face),
        _ => None,
    });
    lightcraft_pipeline::visualize::apply(&mut image, req.overlay, &plan, overlay_mask.as_ref(), all_overlay_alphas.as_deref());
    // spots grew the window the render worked on: cut it back to the request
    if let Some(k) = plan.keep {
        image = image.crop(k.x, k.y, k.w, k.h);
    }
    let rendered = Rendered { image, histogram, deep: None };
    let Some((buf, (fw, fh))) = fitted else { return Some(Output::Image(rendered)) };
    let data: Vec<[u8; 4]> = cx.read(&buf, fw * fh);
    if crate::ctx::failed() {
        return None;
    }
    Some(Output::ImageFit(rendered, Some(Rgba8 { width: fw, height: fh, data })))
}

/// A view's stages and what they kept for this render's size: the entry, and the last render when this one differs
/// from it only in its spots (issue #111).
struct ViewAt<'a> {
    stages: Option<&'a GpuStages>,
    src: &'a Arc<Rgb32f>,
    cached: Option<&'a Entry>,
    start: Option<&'a crate::dirty::Last>,
}

/// What the linear stage made (`linear_stage`).
struct Linear {
    /// The white-balanced, retouched, denoised image.
    lin: Arc<Buf>,
    /// The tiles of it that differ from the one `start` was made from (when it was).
    tiles: Option<crate::dirty::Tiles>,
    /// The image after the spots it was made from.
    post: std::sync::Weak<Buf>,
    /// The joint colour NR's noise model of the whole frame (`Entry::colour_noise`).
    colour_noise: Option<(u64, JointNoise)>,
    /// `Entry::pre_retouch` and `Entry::retouch_boxes`.
    pre_retouch: Option<crate::pre_retouch::PreRetouch>,
    retouch_boxes: Option<Vec<lightcraft_pipeline::spots_keep::Rect>>,
}

/// The white-balanced, retouched, denoised image and what goes with it ([`Linear`]).
fn linear_stage(cx: &mut Cx<'_>, at: &ViewAt<'_>, sampled: &Buf, info: &SourceInfo, plan: &Plan<'_>, host: &mut Host) -> Linear {
    let (src, size) = (at.src, (plan.w, plan.h));
    let old_lin = at.cached.and_then(|e| e.lin.clone());
    let old_post = at.cached.map_or_else(std::sync::Weak::new, |e| e.post.clone());
    let mut colour_noise =
        at.cached.and_then(|e| e.colour_noise).filter(|(k, _)| *k == plan.noise_key).or_else(|| at.stages.and_then(|g| g.noise.get(plan.noise_key)));
    let old_retouch = at.cached.map_or(Some(Vec::new()), |e| e.retouch_boxes.clone());
    if let Some((_, b)) = old_lin.clone().filter(|(k, _)| *k == plan.lin_key) {
        let same = at.start.is_some_and(|l| l.lin_key == plan.lin_key).then(|| crate::dirty::Tiles::none(size));
        let pre_retouch = at.cached.and_then(|e| e.pre_retouch.clone());
        return Linear { lin: b, tiles: same, post: old_post, colour_noise, pre_retouch, retouch_boxes: old_retouch };
    }
    if colour_noise.is_none() {
        colour_noise = lightcraft_pipeline::colour_noise(src, info, plan).map(|n| (plan.noise_key, n));
        if let (Some(g), Some(n)) = (at.stages, colour_noise) {
            g.noise.put(n);
        }
    }
    let kept = at.stages.map(|c| (c.spots.get(src, plan.geo), c.spots_cpu.get(src, plan.geo))).unwrap_or_default();
    let s = &*plan.settings;
    let retouch_alone = lightcraft_pipeline::retouch_step::active(s)
        && !lightcraft_pipeline::local_tone::enabled(info, s)
        && !s.spots.iter().any(lightcraft_pipeline::spots_learned::is_learned);
    let (pre, post, redone, pre_retouch, retouch_boxes) = if retouch_alone {
        // portrait retouching (fork `fork/perf-portrait-draft.md`): from the image before it, redone where it writes
        let kept_pre = at.cached.and_then(|e| e.pre_retouch.as_ref());
        let r = crate::pre_retouch::linear(cx, sampled, host.sampled.take(), info, plan, kept_pre, kept.1.as_deref());
        if let (Some(c), Some(drawn)) = (at.stages, r.spots) {
            c.spots.put(src, plan.geo, None, 0);
            let host = drawn.as_ref().map_or(0, |k| k.bytes());
            c.spots_cpu.put(src, plan.geo, drawn, host);
        }
        let from = r.from;
        let redone =
            crate::pre_retouch::changed(old_retouch.as_deref(), r.boxes.as_deref()).map(|rects| crate::dirty::Redone { from: from.clone(), rects });
        (r.img, from, redone, Some(r.pre), r.boxes)
    } else {
        let (pre, k, redone) = linear(cx, sampled, info, plan, host, (kept.0.as_deref(), kept.1.as_deref()));
        let post = k.0.as_ref().map_or_else(std::sync::Weak::new, |k| Arc::downgrade(&k.post));
        if let Some(c) = at.stages {
            let device = k.0.as_ref().map_or(0, |k| crate::spots_keep::bytes(k));
            c.spots.put(src, plan.geo, k.0, device);
            let host = k.1.as_ref().map_or(0, |k| k.bytes());
            c.spots_cpu.put(src, plan.geo, k.1, host);
        }
        // (no retouching drew on it, or the CPU stage did without saying where)
        let boxes = (!lightcraft_pipeline::retouch_step::active(s)).then(Vec::new);
        (pre, post, redone, None, boxes)
    };
    let noise = colour_noise.as_ref().map(|c| &c.1);
    // only the boxes the spots rewrote, when the kept linear image was made from the image they started from
    let dirty = match (at.start, &redone, &old_lin) {
        (Some(l), Some(r), Some((k, old))) if *k == l.lin_key && std::sync::Weak::ptr_eq(&r.from, &old_post) => {
            crate::dirty::linear(cx, &pre, old, plan, &crate::dirty::Tiles::of(size, &r.rects), noise)
        }
        _ => None,
    };
    let (lin, tiles) = match dirty {
        Some((b, tiles)) => (Arc::new(b), Some(tiles)),
        None => {
            let t = profiling().then(std::time::Instant::now);
            let b = denoise(cx, pre, plan, noise);
            if let Some(t) = t {
                cx.sync();
                eprintln!("    gpu denoise: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
            }
            (Arc::new(b), None)
        }
    };
    Linear { lin, tiles, post, colour_noise, pre_retouch, retouch_boxes }
}

/// The planes of `lin` (with its tiles that changed from `start`'s, only there) and the output tiles that change from
/// `start`'s output (`None`: all of them, or no `start`).
fn planes_stage(
    cx: &mut Cx<'_>,
    at: &ViewAt<'_>,
    (lin, t_lin): (&Arc<Buf>, Option<&crate::dirty::Tiles>),
    plan: &Plan<'_>,
    req: &RenderRequest,
) -> (Planes, Prep, Option<crate::dirty::Tiles>) {
    let old = at.cached.map(|e| e.planes.clone());
    let dirty = match (t_lin, &old, at.start) {
        (Some(tiles), Some(p), Some(l)) if p.key == l.lin_key && p.key != plan.lin_key => crate::dirty::planes(cx, lin, plan, req, p, tiles),
        _ => None,
    };
    let (planes, mut prep, mut t_out) = match dirty {
        Some((planes, prep, tiles)) => (planes, prep, Some(tiles)),
        None => {
            let mut planes = match old {
                Some(p) if p.key == plan.lin_key => p,
                _ => Planes { key: plan.lin_key, ..Default::default() },
            };
            let prep = prepare(cx, lin, plan, req, &mut planes);
            // (the same linear image gives the same planes)
            (planes, prep, t_lin.filter(|tiles| tiles.count() == 0).cloned())
        }
    };
    // a window dehazes with the whole frame's airlight
    if let Some(a) = plan.fixed_air {
        prep.air = a;
    }
    // an airlight that moved changes every pixel
    if let (Some(l), Some(tiles)) = (at.start, &mut t_out)
        && l.air != prep.air.to_bits()
    {
        *tiles = crate::dirty::Tiles::all((plan.w, plan.h));
    }
    (planes, prep, t_out)
}

static DEVICE_FITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many display fits the device has made in this process (a test says a job's fit was made there, not on the CPU).
pub fn device_fits() -> u64 {
    DEVICE_FITS.load(std::sync::atomic::Ordering::Relaxed)
}

/// The finished picture `out` (`w × h` packed RGBA8) shrunk to fit `max` with a Lanczos3 filter, as `fit_for_display` does on the
/// CPU: the 8-bit values filtered as floats with the same taps (`resample::weights`), rounded and clamped. `None` when the
/// picture already fits, or the device cannot hold the floats (the buffers are three floats a pixel, the first pass as wide as
/// the result and as tall as the picture).
fn fit_on_device(cx: &mut Cx<'_>, out: &Buf, (w, h): (usize, usize), (max_w, max_h): (usize, usize)) -> Option<(Buf, (usize, usize))> {
    let (fw, fh) = lightcraft_raster::resample::fit_size(w, h, max_w.max(1), max_h.max(1));
    if (fw, fh) == (w, h) || fw > w || fh > h {
        return None;
    }
    let floats = |px: usize| px.checked_mul(3).filter(|words| cx.gpu.fits(*words));
    floats(w * h)?;
    floats(fw * h)?;
    floats(fw * fh)?;
    DEVICE_FITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let f = cx.gpu.buffer(w * h * 3);
    cx.run("unpack_rgb8", &[(w * h) as u32], &[Some(out), Some(&f), None], groups1(w * h));
    let small = resize(cx, &f, (w, h), (fw, fh), 3, Filter::Lanczos3);
    let packed = cx.gpu.buffer(fw * fh);
    cx.run("pack_rgb8", &[(fw * fh) as u32], &[None, Some(&small), Some(&packed)], groups1(fw * fh));
    Some((packed, (fw, fh)))
}

/// The planes `ask` names, from the masks buffer (`masks()`: one plane after another in the masks' order), each with its Amount
/// applied as `Evaluated::plane` does and cut to the request's window. Only the wanted planes are read back, and none when the
/// asker holds them (`PlanesAsk::known` is their signature).
fn mask_planes(
    cx: &mut Cx<'_>,
    all: Option<&Buf>,
    plan: &Plan<'_>,
    (lin, log_l): (&Buf, &Buf),
    source: usize,
    ask: &lightcraft_pipeline::masks::PlanesAsk,
) -> PlanesOut {
    use lightcraft_pipeline::masks::{planes_signature, wanted};
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let signature = planes_signature(plan, source, ask.want);
    if ask.known == Some(signature) {
        return PlanesOut { signature, planes: None };
    }
    if n == 0 {
        return PlanesOut { signature, planes: Some(Vec::new()) };
    }
    // the device's stack holds the visible masks, in order
    let visible: Vec<_> = plan.settings.masks.iter().filter(|m| m.visible && !m.components.is_empty()).collect();
    let mut out = Vec::new();
    let mut host: Option<(Rgb32f, Plane)> = None;
    for m in wanted(&plan.settings.masks, ask.want) {
        let mut plane = match (visible.iter().position(|v| v.id == m.id), all) {
            // (a plane at a time: one mask's alpha is what a tint shows, and the whole stack is many times that)
            (Some(mi), Some(all)) => {
                let one = cx.slice(all, mi * n, n);
                cx.read_plane(&one, w, h)
            }
            // a hidden mask is not on the device: evaluated on the CPU from the image, as the overlay always did
            _ => {
                let (img, l) = host.get_or_insert_with(|| (cx.read_rgb(lin, w, h), cx.read_plane(log_l, w, h)));
                lightcraft_pipeline::masks::evaluate_one(m, &plan.frame, w, h, img, l, plan.settings.light.exposure as f32, plan.mattes.as_deref())
            }
        };
        let amount = (m.adjust.amount / 100.0) as f32;
        if (amount - 1.0).abs() > 1e-6 {
            plane.data.iter_mut().for_each(|v| *v *= amount);
        }
        out.push((m.id, plan.keep.map_or(plane.clone(), |k| plane.crop(k.x, k.y, k.w, k.h))));
    }
    PlanesOut { signature, planes: Some(out) }
}

/// The finished float output `out` (three words per pixel) read back as the CPU's float render
/// returns it, with the same checks as an 8-bit readback: every pixel written, not entirely black.
fn read_deep(cx: &mut Cx<'_>, out: &Buf, plan: &Plan<'_>, req: &RenderRequest) -> Option<Rendered> {
    use lightcraft_pipeline::{DeepImage, DeepSamples};
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let words: Vec<u32> = cx.read(out, 3 * n);
    if crate::ctx::failed() {
        return None;
    }
    let unwritten = words.as_chunks::<3>().0.iter().filter(|p| p.iter().any(|v| v & DEEP_WRITTEN == 0)).count();
    if unwritten > 0 {
        fail(FailKind::Fatal, format!("the GPU returned an incomplete image ({unwritten} of {n} pixels unwritten)"));
        return None;
    }
    let samples = words.into_iter().map(|v| f32::from_bits(v & !DEEP_WRITTEN)).collect();
    let deep = DeepImage { width: w, height: h, space: req.space, samples: DeepSamples::F32(samples) };
    let deep = match plan.keep {
        Some(k) => deep.crop(k.x, k.y, k.w, k.h),
        None => deep,
    };
    let image = deep.to_rgba8();
    let histogram = Histogram::of_srgb8(&image);
    if n >= 4096 && [&histogram.r, &histogram.g, &histogram.b].iter().all(|c| c.first() == Some(&histogram.total)) {
        fail(FailKind::Blank, "the GPU result is entirely black; rendering it on the CPU to be sure".into());
        return None;
    }
    Some(Rendered { image, histogram, deep: Some(deep) })
}

/// The finished output `out` (`w × h` packed RGBA8) as a texture on the device, with its
/// histogram; the same checks as a readback (every pixel written, not entirely black), done on
/// the device: only the histogram and a pixel count come back.
fn to_texture(cx: &mut Cx<'_>, out: &Buf, w: usize, h: usize) -> Option<(DisplayFrame, Histogram)> {
    let n = w * h;
    let unwritten = cx.zeroed(1);
    cx.run("unwritten", &[n as u32], &[Some(out), None, Some(&unwritten)], groups1(n));
    let histogram = histogram(cx, out, w, (0, 0, w, h));
    let left = cx.read::<u32>(&unwritten, 1).first().copied().unwrap_or(0);
    if crate::ctx::failed() {
        return None;
    }
    if left > 0 {
        fail(FailKind::Fatal, format!("the GPU returned an incomplete image ({left} of {n} pixels unwritten)"));
        return None;
    }
    if n >= 4096 && [&histogram.r, &histogram.g, &histogram.b].iter().all(|c| c.first() == Some(&histogram.total)) {
        fail(FailKind::Blank, "the GPU result is entirely black; rendering it on the CPU to be sure".into());
        return None;
    }
    // texture copies take rows of a multiple of 256 bytes: pad them unless they already are
    let stride = w.next_multiple_of(64);
    let padded = (stride != w).then(|| {
        let b = cx.gpu.buffer(stride * h);
        cx.run("pad_rows", &[w as u32, h as u32, stride as u32], &[Some(out), Some(&b), None], groups2(w, h, [16, 16]));
        b
    });
    let texture = cx.gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lightcraft view"),
        size: wgpu::Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        // (COPY_SRC: a screenshot or a test can read it back)
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    cx.copy_to_texture(padded.as_ref().unwrap_or(out), stride, &texture, w, h);
    cx.flush();
    if crate::ctx::failed() {
        return None;
    }
    Some((DisplayFrame { texture, width: w, height: h }, histogram))
}

/// Pixels whose alpha is not 255 (cheap: no early exit, so it vectorizes).
fn unwritten(data: &[[u8; 4]]) -> usize {
    data.chunks(4096).filter(|c| c.iter().fold(255u8, |a, p| a & p[3]) != 255).map(|c| c.iter().filter(|p| p[3] != 255).count()).sum()
}

/// What a view keeps of its last render's spots, on the device and on the host.
type KeptSpots = (Option<Arc<Kept<Buf, Buf>>>, Option<Arc<CpuKept>>);

/// The white-balanced, retouched image before noise reduction, drawing only the spots an edit reaches when `kept` (the
/// view's last spots) allows; with what to keep for the next render, and what the spots redrew (issue #111).
fn linear(
    cx: &mut Cx<'_>,
    sampled: &Buf,
    info: &SourceInfo,
    plan: &Plan<'_>,
    host: &mut Host,
    kept: (Option<&Kept<Buf, Buf>>, Option<&CpuKept>),
) -> (Buf, KeptSpots, Option<crate::dirty::Redone>) {
    let mut keep: KeptSpots = (None, None);
    let mut redone = None;
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let s = &*plan.settings;
    // (a window that pastes its finished spots draws none, so it stays on the device: issue #176)
    let cpu_spots = plan.spots_at.paste.is_none() && s.spots.iter().any(lightcraft_pipeline::spots::drawn_on_cpu);
    let img = if lightcraft_pipeline::local_tone::enabled(info, s) || cpu_spots {
        // the camera's local tone mapping (ProRAW gain table, off by default), or a spot with no GPU kernel (LaMa,
        // Content-Aware, a shaped spot: `spots::drawn_on_cpu`): the whole scene-linear stage on the CPU
        // (portrait retouching included, fork `fork/retouch-foundation.md`)
        let mut img = match host.sampled.take() {
            Some(i) => i,
            None => cx.read_rgb(sampled, w, h),
        };
        keep.1 = lightcraft_pipeline::spots_keep::lin_cpu(&mut img, info, plan, kept.1);
        cx.gpu.upload(rgb_words(&img))
    } else {
        // `lin_cpu`'s order: spots, white balance, defringe, red eye (issue #184); or in the old order, white balance,
        // defringe, spots, red eye
        let balanced = |cx: &mut Cx<'_>, img: &Buf| {
            let m = local::wb_matrix_for(info, s);
            let mut p = vec![m.is_some() as u32];
            p.extend(m.unwrap_or_default().iter().flatten().map(|v| v.to_bits()));
            let out = cx.gpu.buffer(n * 3);
            map(cx, "wb_k", n, &p, [Some(img), None, None], &out);
            defringe(cx, out, plan)
        };
        let out = if plan.spots_after_wb {
            let (out, k, r) = crate::spots_keep::spots(cx, info, plan, kept.0, |cx| balanced(cx, sampled));
            keep.0 = k;
            redone = r;
            out
        } else {
            let (spotted, k, r) = crate::spots_keep::spots(cx, info, plan, kept.0, |cx| cx.copy(sampled));
            keep.0 = k;
            // (what the spots rewrote changes after the white balance where they wrote, and defringe's radius around it)
            let reach = lightcraft_pipeline::optics::defringe_params(s, plan.px_per_long / lightcraft_pipeline::optics::DEFRINGE_REF_LONG)
                .map_or(0, |d| d.radius.saturating_add(1));
            redone = r.map(|mut r| {
                for b in &mut r.rects {
                    *b = [
                        b[0].saturating_sub(reach),
                        b[1].saturating_sub(reach),
                        b[2].saturating_add(reach).min(w),
                        b[3].saturating_add(reach).min(h),
                    ];
                }
                r
            });
            balanced(cx, &spotted)
        };
        if plan.eyes.is_empty() {
            out
        } else {
            let mut p = vec![w as u32, plan.eyes.len() as u32];
            p.extend(plan.eyes.iter().flat_map(|e| e.words()).map(f32::to_bits));
            let eyed = cx.gpu.buffer(n * 3);
            map(cx, "redeye_k", n, &p, [Some(&out), None, None], &eyed);
            eyed
        }
    };
    (img, keep, redone)
}

/// Defringe (`optics::defringe`) of the white-balanced image.
fn defringe(cx: &mut Cx<'_>, img: Buf, plan: &Plan<'_>) -> Buf {
    use lightcraft_pipeline::optics;
    let Some(dp) = optics::defringe_params(&plan.settings, plan.px_per_long / optics::DEFRINGE_REF_LONG) else { return img };
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let lab = cx.gpu.buffer(n * 3);
    map(cx, "fringe_lab", n, &[], [Some(&img), None, None], &lab);
    // the lightness channel, then its min and max around each pixel
    let l = cx.gpu.buffer(n);
    map(cx, "subsample", n, &[3], [Some(&lab), None, None], &l);
    // (a radius past the image reads the same pixels as one at its size)
    let r = dp.radius.min(w.max(h)) as u32;
    let filtered = |cx: &mut Cx<'_>, max: u32| {
        let tmp = cx.gpu.buffer(n);
        cx.run("minmax", &[w as u32, h as u32, r, 0, max], &[Some(&l), Some(&tmp)], groups2(w, h, [16, 16]));
        let out = cx.gpu.buffer(n);
        cx.run("minmax", &[w as u32, h as u32, r, 1, max], &[Some(&tmp), Some(&out)], groups2(w, h, [16, 16]));
        out
    };
    let (lo, hi) = (filtered(cx, 0), filtered(cx, 1));
    // the kernel writes only the pixels it changes
    let out = cx.copy(&img);
    let p = [dp.purple.0, dp.purple.1, dp.green.0, dp.green.1, dp.purple_strength, dp.green_strength].map(f32::to_bits);
    map(cx, "fringe_apply", n, &p, [Some(&lab), Some(&lo), Some(&hi)], &out);
    out
}

/// One spot of the spot removal (`spots::apply`) on `img` in place, reading the image as the earlier spots left it
/// (`spots_keep` draws them in order).
pub(crate) fn draw_spot(cx: &mut Cx<'_>, img: &Buf, plan: &Plan<'_>, spot: &lightcraft_develop::Spot) {
    use lightcraft_pipeline::spots;
    let (w, h) = (plan.w, plan.h);
    // (a hidden spot draws nothing: `DevelopSettings::effective` already leaves it out of a render)
    if !spot.hidden {
        let Some(l) = spots::layout(spot, &plan.frame, w, h, plan.px_per_long) else { return };
        let Some(&first) = l.pts.first() else { return };
        lightcraft_pipeline::spots_keep::note_spot();
        let offset = match l.offset {
            Some(o) => o,
            None => auto_source(cx, img, w, h, first, l.r),
        };
        let pts = cx.gpu.upload(&l.pts.iter().flat_map(|p| [p.0, p.1]).collect::<Vec<f32>>());
        let nb = l.bw * l.bh;
        let (al, sp, df, wt) = (cx.gpu.buffer(nb), cx.gpu.buffer(3 * nb), cx.gpu.buffer(3 * nb), cx.gpu.buffer(nb));
        let mut p = vec![w as u32, h as u32, l.bx as u32, l.by as u32, l.bw as u32, l.bh as u32, l.pts.len() as u32];
        p.extend([l.r, l.inner, l.opacity, offset.0, offset.1].map(f32::to_bits));
        p.push(l.heal as u32);
        let groups = groups2(l.bw, l.bh, [16, 16]);
        cx.run("spot_prep", &p, &[Some(img), Some(&pts), Some(&al), Some(&sp), Some(&df), Some(&wt)], groups);
        // Heal: the tone correction is the masked difference's normalized blur
        let blurred = l.heal.then(|| {
            let sigma = (l.r * 0.6).max(1.0);
            (gaussian(cx, &df, l.bw, l.bh, 3, sigma), gaussian(cx, &wt, l.bw, l.bh, 1, sigma))
        });
        let (bd, bm) = blurred.as_ref().map_or((&df, &wt), |(d, m)| (d, m));
        cx.run("spot_apply", &p, &[Some(img), Some(&pts), Some(&al), Some(&sp), Some(bd), Some(bm)], groups);
    }
}

/// `spots::auto_source` on the device: every candidate scored at once, the 48 scores read back.
fn auto_source(cx: &mut Cx<'_>, img: &Buf, w: usize, h: usize, target: (f32, f32), r: f32) -> (f32, f32) {
    use lightcraft_pipeline::spots;
    let cands = spots::candidates(r);
    let mut data: Vec<f32> = cands.iter().flat_map(|d| [d.0, d.1]).collect();
    data.extend(spots::ring_dirs().iter().flat_map(|d| [d.0, d.1]));
    let pts = cx.gpu.upload(&data);
    let [around, within] = spots::score_radii(r);
    let mut p = vec![w as u32, h as u32];
    p.extend([target.0, target.1].iter().chain(&around).chain(&within).map(|v| v.to_bits()));
    let scores = cx.gpu.buffer(cands.len());
    cx.run("spot_score", &p, &[Some(img), Some(&pts), Some(&scores), None, None, None], [cands.len().div_ceil(64) as u32, 1, 1]);
    let scores: Vec<f32> = cx.read(&scores, cands.len());
    spots::rank(&cands, &scores).first().copied().unwrap_or_else(|| spots::fallback_source(r))
}

/// The output's histogram (`Histogram::of_srgb8` of the `region` x, y, w, h of the `w`-wide
/// packed RGBA8 `out`, on the same sampling grid), counted on the device.
fn histogram(cx: &mut Cx<'_>, out: &Buf, w: usize, region: (usize, usize, usize, usize)) -> Histogram {
    let (x0, y0, rw, rh) = region;
    let step = (((rw * rh) as f64 / 1_000_000.0).sqrt().ceil() as usize).max(1);
    let (nx, ny) = (rw.div_ceil(step), rh.div_ceil(step));
    let bins = cx.zeroed(4 * Histogram::BINS + 1);
    let p = [w, x0, y0, nx, ny, step].map(|v| v as u32);
    cx.run("histogram", &p, &[Some(out), Some(&bins)], groups2(nx, ny, [16, 16]));
    let v: Vec<u32> = cx.read(&bins, 4 * Histogram::BINS + 1);
    let part = |k: usize| v.get(k * Histogram::BINS..(k + 1) * Histogram::BINS).map_or_else(|| vec![0; Histogram::BINS], <[u32]>::to_vec);
    // (SDR bins: an HDR edit's histogram is made after the render, `media::develop`)
    Histogram { r: part(0), g: part(1), b: part(2), luma: part(3), total: v.get(4 * Histogram::BINS).copied().unwrap_or(0), hdr: None }
}

/// Noise reduction (`local::denoise`). `colour_noise`: the whole frame's noise model for the joint colour
/// NR (`lightcraft_pipeline::colour_noise`); `None`: measured on `img`.
pub(crate) fn denoise(cx: &mut Cx<'_>, img: Buf, plan: &Plan<'_>, colour_noise: Option<&JointNoise>) -> Buf {
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let (lum, col) = local::nr_params(&plan.settings, plan.detail_scale);
    let mut img = img;
    if let Some(nr) = lum {
        let l = cx.gpu.buffer(n);
        map(cx, "log_lum_k", n, &[], [Some(&img), None, None], &l);
        let f = guided(cx, &l, w, h, nr.sigma, nr.eps);
        let out = cx.gpu.buffer(n * 3);
        map(cx, "nr_lum", n, &[nr.k.to_bits(), nr.contrast.to_bits()], [Some(&img), Some(&l), Some(&f)], &out);
        img = out;
    }
    if col.is_some() && local::joint_colour_nr(&plan.settings) {
        let Some(p) = local::nrc_params(&plan.settings, plan.detail_scale) else { return img };
        let s = &plan.settings.detail;
        let (detail, amount) = ((s.nr_color_detail / 100.0) as f32, (s.nr_color / 100.0) as f32);
        if let Some(out) = color_nr_joint(cx, &img, w, h, &p, detail, amount, colour_noise) {
            return out;
        }
        // too small for the noise samples, or not even one band fits the device: this step on the CPU
        let mut host = cx.read_rgb(&img, w, h);
        let mut d = (*plan.settings).clone();
        d.detail.nr_luminance = 0.0;
        local::denoise(&mut host, &d, plan.detail_scale, colour_noise);
        return cx.gpu.upload(rgb_words(&host));
    }
    if let Some(nr) = col {
        let chroma = cx.gpu.buffer(n * 3);
        map(cx, "chroma_k", n, &[], [Some(&img), None, None], &chroma);
        let b = gaussian(cx, &chroma, w, h, 3, nr.sigma);
        let out = cx.gpu.buffer(n * 3);
        map(cx, "nr_col", n, &[nr.t.to_bits()], [Some(&img), Some(&chroma), Some(&b)], &out);
        img = out;
    }
    img
}

/// Run a `nrcj.wgsl` kernel on `n` items with inputs `a`, `b` and output `out`.
pub(crate) fn nrcj(cx: &mut Cx<'_>, k: &str, n: usize, p: &[u32], a: &Buf, b: Option<&Buf>, out: &Buf) {
    cx.run(k, p, &[Some(a), b, Some(out)], groups1(n));
}

/// Device memory left aside when the joint NR plans its bands from the free memory (the rest of the
/// render, the display, other programs).
const VRAM_RESERVE: u64 = 384 << 20;

/// Renders on the device right now ([`render`]). An export batch renders photos side by side: while one
/// plans its noise reduction bands, each of the others may still allocate the buffers of its other stages.
static RENDERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Held while a noise reduction plans its bands and runs them: two plans made at once from the same free
/// memory would together ask for more than there is.
static BANDS: Mutex<()> = Mutex::new(());

/// Counts a render in [`RENDERS`] while it lives.
struct InFlight;

impl InFlight {
    fn new() -> Self {
        RENDERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        InFlight
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        RENDERS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The band budget from the device memory free right now and our own pooled buffers (which we reuse), less
/// [`VRAM_RESERVE`] for each render on the device (this one's other stages and every other render's).
fn band_budget_from(free: u64, pooled: u64, renders: usize) -> usize {
    let reserve = VRAM_RESERVE.saturating_mul(renders.max(1) as u64);
    free.saturating_add(pooled).saturating_sub(reserve) as usize
}

/// The joint colour noise reduction (`lightcraft_pipeline::nrc_joint`) on the GPU, the same steps as the
/// CPU: only the noise samples are read back (the noise model is fitted on the CPU, by the same code).
/// Large images are filtered in horizontal bands whose halo covers every step's reach, so each kept pixel
/// gets the whole-image result; the noise model is the whole image's, or `frame_noise` (the whole frame's,
/// for a window). `None` when the image is too small for the noise samples (the caller falls back to the CPU).
#[allow(clippy::too_many_arguments)]
fn color_nr_joint(
    cx: &mut Cx<'_>,
    img: &Buf,
    w: usize,
    h: usize,
    p: &local::NrcParams,
    detail: f32,
    amount: f32,
    frame_noise: Option<&JointNoise>,
) -> Option<Buf> {
    use lightcraft_pipeline::nrc_joint;
    let strength = nrc_joint::strength(amount);
    let brake = nrc_joint::speck_brake(amount);
    if w < 3 || h < 3 {
        return None;
    }
    if strength <= 0.0 {
        return Some(cx.copy(img));
    }
    let n = w * h;
    let started = profiling().then(std::time::Instant::now);
    let mut noise = match frame_noise {
        Some(noise) => *noise,
        None => {
            // noise samples of the whole image: same pixels as the CPU, fitted by the same code
            let stride = nrc_joint::noise_sample_stride(n);
            let count = n.div_ceil(stride);
            let smp = cx.gpu.buffer(11 * count);
            nrcj(cx, "nrcj_samples", count, &[count as u32, w as u32, h as u32, stride as u32], img, None, &smp);
            let raw: Vec<f32> = cx.read(&smp, 11 * count);
            drop(smp);
            if let Some(t) = started {
                eprintln!("  gpu joint samples + read: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
            }
            let samples: Vec<nrc_joint::NoiseSample> = raw
                .as_chunks::<11>()
                .0
                .iter()
                .filter(|s| s[0] == 1.0 && s.iter().all(|v| v.is_finite()))
                .map(|s| (s[1], [[s[2], s[3], s[4]], [s[5], s[6], s[7]], [s[8], s[9], s[10]]]))
                .collect();
            nrc_joint::noise_from_samples(samples)
        }
    };
    noise.tolerance = nrc_joint::tolerance(detail);
    if let Some(t) = started {
        eprintln!("  gpu joint noise model: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    let sigma = p.passes.last().map_or(0.0, |q| q.sigma);
    let plan = nrc_joint::scale_plan(sigma, w, h);
    let halo = joint_halo(&plan);
    // Memory: the whole output (3 values per pixel) is held besides the input; a band then holds about
    // 12 values per pixel (guide 3, two chroma buffers 2 + 2, paths 2, weights 2; up to 12 while the
    // speck brake and the guide run). The budget for both together is `band_budget_from` the device memory
    // free right now (where the driver can be asked, `crate::vram`); else a fixed 2 GB. One band plan at a
    // time (`BANDS`), held until the bands are done. PROTOTYPE: `NRV2_GPU_BAND_MB` overrides it.
    // (free memory asked before the lock: the first question for a device waits for the driver)
    let free = crate::vram::free_bytes(&cx.gpu.info);
    let _bands = BANDS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixed = std::env::var("NRV2_GPU_BAND_MB").ok().and_then(|v| v.parse::<usize>().ok()).map(|mb| mb << 20);
    let renders = RENDERS.load(std::sync::atomic::Ordering::Relaxed);
    let measured = free.map(|free| band_budget_from(free, crate::ctx::memory().pooled, renders));
    let budget = fixed.or(measured).unwrap_or(2048 << 20);
    if profiling() {
        eprintln!(
            "  gpu joint budget: {} MB ({}, {renders} render(s) on the device)",
            budget >> 20,
            if fixed.is_some() {
                "NRV2_GPU_BAND_MB"
            } else if measured.is_some() {
                "free device memory"
            } else {
                "fixed default"
            }
        );
    }
    let out_bytes = 12 * n;
    let per_row = 48 * w;
    // no band may exceed the device's largest buffer either (3 values per pixel are the largest)
    let limit_rows = (cx.gpu.limit() as usize / (12 * w)).max(1);
    let rows = (budget.saturating_sub(out_bytes) / per_row).min(limit_rows);
    let core = if rows >= h {
        h
    } else if rows > 2 * halo {
        rows - 2 * halo
    } else {
        // not even one band with its halo fits: this stage runs on the CPU (the rest of the render stays here)
        return None;
    };
    let out = cx.gpu.buffer(3 * n);
    let mut y0 = 0;
    while y0 < h {
        let y1 = (y0 + core).min(h);
        let (b0, b1) = if core >= h { (0, h) } else { (y0.saturating_sub(halo), (y1 + halo).min(h)) };
        let filtered = joint_band(cx, img, w, b0, b1 - b0, &noise, brake, &plan);
        let kept = (y1 - y0) * w;
        nrcj(cx, "nrcj_recon", kept, &[kept as u32, strength.to_bits(), (y0 * w) as u32, (b0 * w) as u32], img, Some(&filtered), &out);
        drop(filtered);
        // wait: the driver frees a dropped buffer only once the GPU is done with it, and a band's
        // buffers are large (without waiting, freed memory piled up to the whole 4 GB of a Quadro M2200)
        cx.sync();
        y0 = y1;
    }
    Some(out)
}

/// Rows (and columns) a pixel of the joint colour NR with the scales `plan` depends on: a wrong value at a band edge
/// (treated as an image border) moves inwards by delta per sweep, two sweeps per scale; at the farthest row it reaches,
/// a scale's weight and path still read up to delta rows further (at most the largest delta); the guide's blur and
/// pilot reach a few rows, the speck brake two more. Hence 2·Σdelta + max delta + 10. The sweeps go across as they go
/// down, so a cut of the image with this margin on every side gives the whole image's result inside it (issue #111).
pub(crate) fn joint_halo(plan: &[(usize, f32)]) -> usize {
    plan.iter().map(|d| 2 * d.0).sum::<usize>() + plan.last().map_or(0, |d| d.0) + 10
}

/// The filtered chroma pairs of rows `b0 .. b0 + bh` of the image, treated as an image of their own.
#[allow(clippy::too_many_arguments)]
pub(crate) fn joint_band(
    cx: &mut Cx<'_>,
    img: &Buf,
    w: usize,
    b0: usize,
    bh: usize,
    noise: &lightcraft_pipeline::nrc_joint::JointNoise,
    brake: lightcraft_pipeline::nrc_joint::SpeckBrake,
    plan: &[(usize, f32)],
) -> Buf {
    let n = w * bh;
    let mut np = vec![n as u32, w as u32, bh as u32];
    np.extend(noise.model.alpha.iter().chain(&noise.model.beta).chain(&noise.ymax).map(|v| v.to_bits()));
    np.push(noise.tolerance.to_bits());
    let mut yuv = cx.gpu.buffer(3 * n);
    nrcj(cx, "nrcj_yuv", n, &[n as u32, (b0 * w) as u32], img, None, &yuv);
    if brake.share > 0.0 {
        let mut sp = np.clone();
        sp.extend([brake.threshold.to_bits(), brake.share.to_bits()]);
        let despecked = cx.gpu.buffer(3 * n);
        nrcj(cx, "nrcj_speck", n, &sp, &yuv, None, &despecked);
        yuv = despecked;
    }
    let blurred = gaussian(cx, &yuv, w, bh, 3, 0.8);
    let guide = cx.gpu.buffer(3 * n);
    nrcj(cx, "nrcj_pilot", n, &np, &yuv, Some(&blurred), &guide);
    drop(blurred);
    let mut filtered = cx.gpu.buffer(2 * n);
    nrcj(cx, "nrcj_uv", n, &[n as u32], &yuv, None, &filtered);
    drop(yuv);
    // dropped buffers only become reusable (and their memory free) once the GPU is done with them
    cx.sync();
    // paths (exp(−path), as on the CPU) and pair weights; the next scale's paths go to the scratch buffer,
    // free after the diffusion steps, as on the CPU
    let mut paths = cx.gpu.buffer(2 * n);
    nrcj(cx, "nrcj_links1", n, &np, &guide, None, &paths);
    let links = cx.gpu.buffer(2 * n);
    let mut lp = np.clone();
    lp.push(1);
    nrcj(cx, "nrcj_links", n, &lp, &guide, Some(&paths), &links);
    let mut scratch = cx.gpu.buffer(2 * n);
    for (k, &(delta, rate)) in plan.iter().enumerate() {
        let dp = [n as u32, w as u32, bh as u32, delta as u32, rate.to_bits()];
        for _ in 0..2 {
            nrcj(cx, "nrcj_dh", n, &dp, &filtered, Some(&links), &scratch);
            std::mem::swap(&mut filtered, &mut scratch);
            nrcj(cx, "nrcj_dv", n, &dp, &filtered, Some(&links), &scratch);
            std::mem::swap(&mut filtered, &mut scratch);
        }
        if let Some(&(next, _)) = plan.get(k + 1) {
            nrcj(cx, "nrcj_paths", n, &[n as u32, w as u32, bh as u32, delta as u32], &paths, None, &scratch);
            std::mem::swap(&mut paths, &mut scratch);
            let mut lp = np.clone();
            lp.push(next as u32);
            nrcj(cx, "nrcj_links", n, &lp, &guide, Some(&paths), &links);
        }
    }
    drop((scratch, paths, links, guide));
    cx.sync();
    filtered
}

/// The planes the per-pixel stage reads.
pub(crate) struct Prep {
    pub(crate) log_l: Arc<Buf>,
    pub(crate) base: Arc<Buf>,
    pub(crate) clarity: Option<Arc<Buf>>,
    pub(crate) texture: Option<Arc<Buf>>,
    /// Process V3: the sharpening band.
    pub(crate) sharpen: Option<Arc<Buf>>,
    pub(crate) dark: Option<Arc<Buf>>,
    /// Blurred chromaticity (local Moiré / Noise).
    pub(crate) chroma: Option<Arc<Buf>>,
    pub(crate) air: f32,
}

/// The spatial planes (`local::prepare`), reusing those in `planes`.
fn prepare(cx: &mut Cx<'_>, lin: &Buf, plan: &Plan<'_>, req: &RenderRequest, planes: &mut Planes) -> Prep {
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let sig = local::plane_sigmas(&plan.settings, plan.px_per_long, plan.detail_scale, req.quality);
    let log_l = match &planes.log_l {
        Some(b) => b.clone(),
        None => {
            let l = cx.gpu.buffer(n);
            map(cx, "log_lum_k", n, &[], [Some(lin), None, None], &l);
            let b = Arc::new(l);
            planes.log_l = Some(b.clone());
            b
        }
    };
    let base = match sig.base {
        Some(sg) => {
            let key = local::base_key(&plan.settings, sg, sig.base_detail);
            plane_at(&mut planes.base, key, || tone_base(cx, &log_l, w, h, sg, sig.base_detail))
        }
        None => log_l.clone(),
    };
    let clarity = sig.clarity.map(|sg| plane_at(&mut planes.clarity, sg.to_bits(), || guided_fast(cx, &log_l, w, h, sg, local::CLARITY_EPS)));
    let texture = sig.texture.map(|sg| plane_at(&mut planes.texture, sg.to_bits(), || gaussian(cx, &log_l, w, h, 1, sg)));
    let sharpen = sig.sharpen.map(|sg| plane_at(&mut planes.sharpen, sg.to_bits(), || gaussian(cx, &log_l, w, h, 1, sg)));
    let (dark, air) = match sig.dark {
        None => (None, 1.0),
        Some(sg) => match &planes.dark {
            Some((k, d, air)) if *k == sg.to_bits() => (Some(d.clone()), *air),
            _ => {
                let d0 = cx.gpu.buffer(n);
                map(cx, "dark_k", n, &[], [Some(lin), None, None], &d0);
                let d = gaussian(cx, &d0, w, h, 1, sg);
                // airlight: a percentile of every 7th value, as on the CPU
                let m = n.div_ceil(local::AIRLIGHT_STEP);
                let sub = cx.gpu.buffer(m);
                map(cx, "subsample", m, &[local::AIRLIGHT_STEP as u32], [Some(&d), None, None], &sub);
                let air = local::airlight_of(cx.read(&sub, m));
                cx.flush();
                let b = Arc::new(d);
                planes.dark = Some((sg.to_bits(), b.clone(), air));
                (Some(b), air)
            }
        },
    };
    let chroma = sig.chroma.map(|sg| {
        plane_at(&mut planes.chroma, sg.to_bits(), || {
            let ch = cx.gpu.buffer(n * 3);
            map(cx, "chroma_k", n, &[], [Some(lin), None, None], &ch);
            gaussian(cx, &ch, w, h, 3, sg)
        })
    });
    Prep { log_l, base, clarity, texture, sharpen, dark, chroma, air }
}

// (fork, `fork/retouch-reshape.md`)
#[cfg(test)]
#[path = "render/tests_reshape.rs"]
mod tests_reshape;

#[cfg(test)]
mod tests {
    use super::*;

    /// However small its passes, a render submits them before there are [`FLUSH_PASSES`] (issue #82).
    #[test]
    fn a_render_submits_its_passes_in_bounded_batches() {
        let _serial = crate::DEVICE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(gpu) = crate::device() else {
            lightcraft_testkit::skip!(Gpu, "no GPU device");
            return;
        };
        let _scope = crate::ctx::RenderScope::new(gpu);
        let mut cx = Cx::new(gpu);
        let (a, b) = (gpu.upload(&[1.0f32; 64]), gpu.buffer(16));
        for _ in 0..3 * FLUSH_PASSES + 7 {
            map(&mut cx, "subsample", 16, &[4], [Some(&a), None, None], &b);
        }
        let most = cx.most_passes();
        cx.flush();
        assert!(most <= FLUSH_PASSES, "{most} passes in one submission");
    }

    /// Two exports side by side: each band plan leaves room for the other render, so the two together
    /// stay within the free memory they both measured.
    #[test]
    fn band_budget_leaves_room_for_every_render_on_the_device() {
        let (free, pooled) = (3000u64 << 20, 200u64 << 20);
        let alone = band_budget_from(free, pooled, 1);
        assert_eq!(alone, (3200usize << 20) - VRAM_RESERVE as usize);
        assert_eq!(band_budget_from(free, pooled, 0), alone, "the render itself is always counted");
        let two = band_budget_from(free, pooled, 2);
        assert_eq!(two, alone - VRAM_RESERVE as usize);
        assert_eq!(band_budget_from(100 << 20, 0, 3), 0, "nothing left: no band fits, the CPU runs the step");
        let before = RENDERS.load(std::sync::atomic::Ordering::Relaxed);
        {
            let _a = InFlight::new();
            let _b = InFlight::new();
            assert!(RENDERS.load(std::sync::atomic::Ordering::Relaxed) >= before + 2);
        }
        assert!(RENDERS.load(std::sync::atomic::Ordering::Relaxed) < before + 2, "a finished render no longer counts");
    }

    #[test]
    fn coverage_mask_matches_per_pixel_decision() {
        use lightcraft_develop::{EmbeddedLens, EmbeddedWarp};
        let lens = EmbeddedLens {
            warp: Some(EmbeddedWarp {
                planes: [[1.0, -0.03, 0.01, 0.0, 0.001, -0.002]; 3],
                center: lightcraft_geom::Point::new(0.52, 0.48),
                radius: 0.6,
            }),
            vignette: None,
            stages: Default::default(),
            source: Default::default(),
        };
        let mut s = DevelopSettings::default();
        s.optics.lens_profile = true;
        s.optics.distortion = -40.0;
        s.geometry.horizontal = -15.0;
        s.geometry.rotate = 3.0;
        for orientation in [Orientation::Normal, Orientation::Rotate270] {
            s.orientation = orientation;
            let f = lightcraft_pipeline::geometry::Frame::with_lens(900, 600, &s, true, Some(&lens));
            let wp = f.warp.as_ref().expect("warp");
            // widths that do and don't fill the last word; heights that do and don't fill the last band
            for (w, h) in [f.fit(700, 700), (333, 221), (64, 16), (1, 1)] {
                let o2t = f.out_to_oriented(w, h);
                let mask = coverage_mask(wp, &o2t, w, h);
                let words = w.div_ceil(32);
                assert_eq!(mask.len(), words * h);
                for y in 0..h {
                    for x in 0..w {
                        let bit = mask[y * words + x / 32] >> (x % 32) & 1 == 1;
                        assert_eq!(bit, wp.covers(&o2t, x, y), "{orientation:?} {w}x{h}: pixel {x},{y}");
                    }
                    let pad = mask[y * words + words - 1] >> (w % 32);
                    assert!(w.is_multiple_of(32) || pad == 0, "padding bits set");
                }
            }
        }
    }

    /// The perspective horizon in view: its denominator is exactly 0 on output row 75 (clamped to 1e-300) and
    /// changes sign there, so the blocks around it are undecided (clamp, and division by a range that may hold
    /// zero) and are decided per pixel, while blocks clear of it are filled at once.
    #[test]
    fn coverage_mask_with_the_horizon_in_view() {
        use lightcraft_geom::{Affine, Homography};
        let (w, h) = (900, 600);
        let mut wp = lightcraft_pipeline::optics::Warp::identity(w as f64, h as f64);
        // centred coordinates: v = (y − 300) / 450, denominator 2·v + m8, exactly 0 at y = 75.5
        let m8 = -((75.5 - 300.0) / 450.0 * 2.0);
        wp.persp_inv = Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, m8]);
        wp.persp = wp.persp_inv.inverse().expect("invertible");
        let o2t = Affine::IDENTITY;
        assert_eq!(wp.block_coverage(&o2t, 0, 32, 64, 80), None, "the block holding the horizon");
        assert_eq!(wp.block_coverage(&o2t, 0, 32, 0, 16), Some(false), "beyond the horizon");
        assert_eq!(wp.block_coverage(&o2t, 448, 480, 288, 304), Some(true), "the centre");
        let mask = coverage_mask(&wp, &o2t, w, h);
        let words = w.div_ceil(32);
        let mut covered = 0;
        for y in 0..h {
            for x in 0..w {
                let bit = mask[y * words + x / 32] >> (x % 32) & 1 == 1;
                assert_eq!(bit, wp.covers(&o2t, x, y), "pixel {x},{y}");
                covered += bit as usize;
            }
        }
        assert!(covered > 0 && covered < w * h, "{covered} px covered");
    }

    /// The blur against `lightcraft_raster::blur::gaussian`: widths that do and don't fill the last
    /// workgroup, windows wider than the image, and 1, 2 (as two planes) and 3 channels.
    #[test]
    fn gaussian_matches_cpu() {
        // (creating the device writes the crash sentinel another test swaps out)
        let _serial = crate::DEVICE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use lightcraft_raster::blur;
        let Some(gpu) = crate::device() else { return };
        let _scope = crate::ctx::RenderScope::new(gpu);
        let v = |x: usize, y: usize, c: usize| ((x * 7 + y * 13 + c * 5) % 17) as f32 / 16.0 + if x % 61 < 3 { 2.0 } else { 0.0 };
        for (w, h) in [(1000, 5), (257, 3), (5, 4), (1, 9)] {
            for sigma in [0.8f32, 2.0, 4.5, 30.0, 250.0, 1000.0] {
                let planes: Vec<Plane> = (0..3).map(|c| Plane::from_fn(w, h, |x, y| v(x, y, c))).collect();
                let want: Vec<Plane> = planes.iter().map(|p| blur::gaussian(p, sigma)).collect();
                for nc in [1, 2, 3] {
                    let words: Vec<f32> = (0..w * h).flat_map(|i| (0..nc).map(move |c| (i, c))).map(|(i, c)| planes[c].data[i]).collect();
                    let mut cx = Cx::new(gpu);
                    let src = gpu.upload(&words);
                    let out = gaussian(&mut cx, &src, w, h, nc, sigma);
                    let got: Vec<f32> = cx.read(&out, w * h * nc);
                    for (i, g) in got.iter().enumerate() {
                        let want = want[i % nc].data[i / nc];
                        assert!((g - want).abs() < 1e-5 * (1.0 + sigma / 10.0), "{w}×{h} nc={nc} sigma={sigma}: word {i} {g} vs {want}");
                    }
                }
            }
        }
    }

    /// Kernel timings on a 24 MP plane: `cargo test --release -p lightcraft-gpu -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_kernels() {
        // (creating the device writes the crash sentinel another test swaps out)
        let _serial = crate::DEVICE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(gpu) = crate::device() else { return };
        let _scope = crate::ctx::RenderScope::new(gpu);
        let (w, h) = (6000, 4000);
        eprintln!("{} ({:?})", gpu.info.name, gpu.info.backend);
        let src = gpu.upload(&vec![0.5f32; w * h * 3]);
        let time = |name: &str, f: &mut dyn FnMut(&mut Cx<'_>)| {
            let mut best = f64::MAX;
            for _ in 0..5 {
                let mut cx = Cx::new(gpu);
                cx.sync();
                let t = std::time::Instant::now();
                f(&mut cx);
                cx.sync();
                best = best.min(t.elapsed().as_secs_f64() * 1e3);
            }
            eprintln!("{name:<40} {best:.1} ms");
        };
        for nc in [1, 2, 3] {
            let out = gpu.buffer(w * h * nc);
            // noise reduction's radii, Texture's and Dehaze's at 6000 px
            for r in [2, 11, 120] {
                for (k, horizontal) in [("box_h", true), ("box_v", false)] {
                    time(&format!("{k} nc={nc} r={r}"), &mut |cx| box_pass(cx, (horizontal, r), &src, &out, (w, h), nc, 0));
                }
            }
        }
        for nc in [1, 2, 3] {
            time(&format!("gaussian nc={nc} sigma=2"), &mut |cx| drop(gaussian(cx, &src, w, h, nc, 2.0)));
        }
        time("gaussian nc=3 sigma=4.5 (colour NR)", &mut |cx| drop(gaussian(cx, &src, w, h, 3, 4.5)));
        time("map log_lum", &mut |cx| {
            let o = cx.gpu.buffer(w * h);
            map(cx, "log_lum_k", w * h, &[], [Some(&src), None, None], &o);
        });
        time("guided sigma=1.75", &mut |cx| drop(guided(cx, &src, w, h, 1.75, 0.01)));
    }

    /// The device scores Remove's candidate sources like the CPU and picks the same one.
    #[test]
    fn auto_source_picks_the_cpu_candidate() {
        // (creating the device writes the crash sentinel another test swaps out)
        let _serial = crate::DEVICE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(gpu) = crate::device() else {
            let why = crate::unavailable_reason().unwrap_or_default();
            assert!(!why.contains("kernels failed to build"), "{why}");
            return;
        };
        let _scope = crate::ctx::RenderScope::new(gpu);
        let img = lightcraft_scenes::demo_library()[3].render(640, 427);
        let buf = gpu.upload(rgb_words(&img));
        let mut differ = Vec::new();
        for (tx, ty, r) in [(25.6, 213.0, 19.2), (320.0, 192.0, 19.2), (100.0, 50.0, 12.0), (600.0, 400.0, 25.0), (320.0, 213.0, 6.0)] {
            let mut cx = Cx::new(gpu);
            let g = auto_source(&mut cx, &buf, img.width, img.height, (tx, ty), r);
            let c = lightcraft_pipeline::spots::auto_source(&img, (tx, ty), r);
            if g != c {
                differ.push(((tx, ty, r), g, c));
            }
        }
        assert!(differ.is_empty(), "(target, gpu, cpu): {differ:?}");
    }

    #[test]
    fn orient_map_matches_image_oriented() {
        use Orientation::*;
        let img = Rgb32f::from_fn(5, 3, |x, y| [x as f32, y as f32, 0.0]);
        for o in [Normal, Rotate90, Rotate180, Rotate270, FlipH, Transverse, FlipV, Transpose] {
            let r = img.oriented(o);
            let m = orient_map(o, 5, 3);
            for y in 0..r.height {
                for x in 0..r.width {
                    let (x, y) = (x as i32, y as i32);
                    let (sx, sy) = (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]);
                    assert_eq!(r.get(x as usize, y as usize), img.get(sx as usize, sy as usize), "{o:?} at {x},{y}");
                }
            }
        }
    }
}
