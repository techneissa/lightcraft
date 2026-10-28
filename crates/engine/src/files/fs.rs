use std::path::Path;

use super::preview::{fit_box, fs_preview_loader, preview_orientation};
use super::probe::probe_bytes;
use super::{Arc, DenoiseSpec, FileLoader, FileProbe, Filter, LoadOptions, PairLoader, ReducedLoader, fit};

/// The file's first [`lightcraft_raw::PREVIEW_PREFIX`] bytes (or the whole file when it is shorter),
/// which is what every format [`lightcraft_raw::bounded_preview`] covers keeps its pointer in.
pub(super) fn read_prefix(file: &mut std::fs::File, path: &str, total: u64) -> Result<Vec<u8>, String> {
    let len = lightcraft_raw::PREVIEW_PREFIX.min(usize::try_from(total).unwrap_or(usize::MAX));
    read_prefix_at(file, path, len as u64)
}

/// The file's first `len` bytes, read from the start.
pub(super) fn read_prefix_at(file: &mut std::fs::File, path: &str, len: u64) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek, SeekFrom};
    let len = usize::try_from(len).map_err(|_| format!("{path}: file too large"))?;
    let mut prefix = vec![0u8; len];
    file.seek(SeekFrom::Start(0)).map_err(|e| format!("{path}: {e}"))?;
    file.read_exact(&mut prefix).map_err(|e| format!("{path}: {e}"))?;
    Ok(prefix)
}

/// The end of a Fuji RAF's preview block, from the two big-endian u32s every RAF header carries at
/// 84 (offset) and 88 (length) — the same fields `lightcraft_raw`'s RAF reader uses, read here
/// without the rest of the file so the prefix can be made big enough first.
pub(super) fn raf_preview_extent(prefix: &[u8]) -> Option<u64> {
    if !prefix.starts_with(b"FUJIFILMCCD-RAW") {
        return None;
    }
    let u32_at = |at: usize| prefix.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]])).map(u64::from);
    let (offset, len) = (u32_at(84)?, u32_at(88)?);
    (offset > 0 && len > 0).then(|| offset.checked_add(len)).flatten()
}

/// The display-ready preview of a raw whose embedded JPEG was already located: the counterpart of
/// [`embedded_preview_srgb`] for [`preview_window`], which does not re-read the container.
/// `adobe_rgb`: the container declares the preview Adobe RGB ([`PreviewRead::adobe_rgb`]), applied
/// as [`decode_raw_preview`] applies it. `None` when the bytes are not a decodable image.
pub(super) fn quick_image_srgb(header: &[u8], window: &[u8], adobe_rgb: bool, max_edge: usize) -> Option<lightcraft_raster::Rgba8> {
    let decoded = if adobe_rgb && window.starts_with(&[0xff, 0xd8]) {
        lightcraft_codecs::decode_jpeg_with_fallback(window, fit_box(max_edge), lightcraft_codecs::NamedSpace::AdobeRgb)
    } else {
        lightcraft_codecs::decode(window, fit_box(max_edge))
    };
    let mut d = decoded.ok()?;
    if d.image.width.max(d.image.height) > max_edge {
        d.image = fit(&d.image, max_edge, max_edge, Filter::Box);
        d.alpha = None;
    }
    // The JPEG's own EXIF orientation, else the raw's (from its header), as the whole-file path does
    Some(d.to_srgb8().oriented(preview_orientation(header, d.orientation)))
}

/// The filesystem-backed [`PairLoader`]: the file is read and decoded once, and developed as usual and from the photo's
/// denoised picture. It holds two pictures at once, so it counts for more at the memory gate than a plain load.
pub fn fs_pair_loader() -> PairLoader {
    Arc::new(|path: &str, max_edge: usize, spec: &DenoiseSpec, options: LoadOptions| {
        let len = std::fs::metadata(path).map(|m| m.len() as usize).unwrap_or(0);
        let weight = len.saturating_mul(if max_edge <= crate::media::SourceLevel::Thumb.max_edge() { 4 } else { 10 });
        let gate = crate::memory::work_gate();
        let permit = if crate::memory::is_background() { gate.acquire(weight) } else { gate.acquire_urgent(weight) };
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        // (macOS's picture for the transfer is drawn before LightCraft develops both: the larger of the two weighs)
        let _permit = macos_permit(gate, permit, weight, &bytes, super::macos::TRANSFER_EDGE, options);
        let r = super::load::load_file(Path::new(path), bytes, max_edge, Some(spec), false, options).map(|(img, twin, info, _)| (img, twin, info));
        crate::memory::release();
        r
    })
}

/// Filesystem-backed hooks (native). On the web the host installs bytes-based hooks instead.
///
/// Their working memory is bounded by [`crate::memory::work_gate`]: background loads (grid
/// thumbnails, neighbour prefetch: [`crate::memory::in_background`]) and import probes wait while
/// too much is in flight, each counting its file's size times the expansion of decoding it;
/// interactive loads (the loupe, exports) never wait — they are counted, so background work
/// yields to them.
pub fn fs_hooks() -> (FileLoader, FileProbe) {
    let loader: FileLoader = Arc::new(|path: &str, max_edge: usize, options: LoadOptions| {
        fs_load(path, max_edge, options, |bytes| {
            super::load::load_file(Path::new(path), bytes, max_edge, None, false, options).map(|(img, _, info, _)| (img, info))
        })
    });
    let probe: FileProbe = Arc::new(|path: &str| {
        let len = std::fs::metadata(path).map(|m| usize::try_from(m.len()).unwrap_or(usize::MAX)).unwrap_or(0);
        let _permit = crate::memory::work_gate().acquire(len);
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        let mut info = probe_bytes(path, &bytes)?;
        info.file_modified = file_modified(path);
        Ok(info)
    });
    (loader, probe)
}

/// The filesystem [`ReducedLoader`]: [`super::load::load_vec_reduced`] of the file, gated like [`fs_hooks`]' loader.
pub fn fs_reduced_loader() -> ReducedLoader {
    Arc::new(|path: &str, max_edge: usize, options: LoadOptions| {
        fs_load(path, max_edge, options, |bytes| {
            super::load::load_file(Path::new(path), bytes, max_edge, None, true, options).map(|(img, _, info, reduced)| (img, info, reduced))
        })
    })
}

/// Read `path` and `load` it, waiting on [`crate::memory::work_gate`] as [`fs_hooks`] explains.
fn fs_load<T>(path: &str, max_edge: usize, options: LoadOptions, load: impl FnOnce(Vec<u8>) -> Result<T, String>) -> Result<T, String> {
    let len = std::fs::metadata(path).map(|m| usize::try_from(m.len()).unwrap_or(usize::MAX)).unwrap_or(0);
    let weight = len.saturating_mul(if max_edge <= crate::media::SourceLevel::Thumb.max_edge() { 3 } else { 6 });
    let gate = crate::memory::work_gate();
    let permit = if crate::memory::is_background() { gate.acquire(weight) } else { gate.acquire_urgent(weight) };
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let _permit = macos_permit(gate, permit, weight, &bytes, max_edge, options);
    let r = load(bytes);
    // the file, the samples and the intermediate images are gone: give their pages back
    crate::memory::release();
    r
}

/// The memory gate weight of a load that would weigh `own` without macOS Colour, once its `bytes` are read: a photo on
/// macOS Colour (on a Mac that can render it) weighs Core Image's peak footprint for a picture no larger than
/// `macos_edge` (`files::macos::gate_weight`: 1 GB at any size up to 33 MP, more for larger sensors), far more than
/// its file size times the decode's expansion; `None` for any other load. If the decoder then fails, the Camera
/// Matching decode that replaces it fits in the same weight.
pub(crate) fn load_weight(own: usize, bytes: &[u8], macos_edge: usize, options: LoadOptions, macos_available: bool) -> Option<usize> {
    let macos = options.colour == lightcraft_develop::CameraColour::Macos && macos_available;
    macos.then(|| own.max(super::macos::gate_weight(bytes, macos_edge)))
}

/// The permit a load holds while it decodes: `permit` (its own weight, taken before the file was read), or for a
/// macOS Colour load ([`load_weight`]) Core Image's weight instead, waited for even by an interactive load, so the
/// loupe never stacks a second gigabyte on a thumbnail's render.
pub(super) fn macos_permit<'g>(
    gate: &'g crate::memory::WorkGate,
    permit: crate::memory::Permit<'g>,
    own: usize,
    bytes: &[u8],
    macos_edge: usize,
    options: LoadOptions,
) -> crate::memory::Permit<'g> {
    match load_weight(own, bytes, macos_edge, options, lightcraft_macraw::available()) {
        Some(weight) => {
            drop(permit);
            gate.acquire(weight)
        }
        None => permit,
    }
}

/// `path`'s modification time as ISO 8601 (see [`ProbeInfo::file_modified`]); `None` when the
/// filesystem doesn't say or the time is before 1970.
fn file_modified(path: &str) -> Option<String> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = i64::try_from(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs()).ok()?;
    Some(crate::import::civil(secs))
}

impl crate::Session {
    /// Install the filesystem file hooks (desktop, CLI, MCP headless), and on macOS the Apple Photos
    /// runner ([`crate::apple_photos::osascript`]; installed, not run).
    pub fn with_fs(mut self) -> Self {
        let (l, p) = fs_hooks();
        self.media.file_loader = Some(l);
        self.media.reduced_loader = Some(fs_reduced_loader());
        self.media.file_probe = Some(p);
        self.media.preview_loader = Some(fs_preview_loader());
        self.media.denoise.loader = Some(fs_pair_loader());
        #[cfg(target_os = "macos")]
        {
            self.apple_photos = Some(crate::apple_photos::osascript());
        }
        self
    }
}
