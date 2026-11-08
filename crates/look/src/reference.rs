//! Reading a reference picture from outside the library: an image file, pasted image bytes, or a web address. A
//! reference is never imported: it is decoded small, used, and dropped. A download is kept only in a cache folder the
//! caller names (a few files, the oldest removed), so matching to the same address twice does not download twice.
//!
//! Everything that can go wrong (no such file, a file too large, a corrupt image, an address that is not https, a
//! server that answers too much or nothing) is a [`LookError`], never a panic.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use lightcraft_raster::Rgba8;

use crate::LookError;

/// The largest reference file or download read, bytes.
pub const MAX_BYTES: u64 = 64 << 20;
/// The largest reference image decoded, pixels.
pub const MAX_PIXELS: u64 = 250_000_000;
/// The long edge a reference is decoded at (the fitter measures it smaller still).
pub const DECODE_EDGE: u32 = 1024;
/// [`DECODE_EDGE`] as a size.
pub const EDGE: usize = DECODE_EDGE as usize;

/// Where a reference picture comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    /// Image bytes (a pasted image, encoded).
    Bytes(Vec<u8>),
    /// An `https://` address.
    Url(String),
}

/// Decode `bytes` into a small display-ready picture (orientation applied).
pub fn decode(bytes: &[u8]) -> Result<Rgba8, LookError> {
    let opts = lightcraft_codecs::DecodeOptions { max_size: Some((DECODE_EDGE, DECODE_EDGE)), max_pixels: MAX_PIXELS };
    let d = lightcraft_codecs::decode(bytes, opts).map_err(|e| LookError::Decode(e.to_string()))?;
    if d.width == 0 || d.height == 0 {
        return Err(LookError::Decode("the image is empty".into()));
    }
    let img = d.to_srgb8();
    Ok(img.into_oriented(lightcraft_geom::Orientation::from_exif(d.orientation)))
}

/// Refuse a picture past [`MAX_PIXELS`] (before its pixels are touched).
pub fn check_pixels(width: usize, height: usize) -> Result<(), LookError> {
    match (width as u64).checked_mul(height as u64) {
        Some(n) if n <= MAX_PIXELS => Ok(()),
        _ => Err(LookError::TooLarge(format!("the picture is {width}×{height}; a reference may have at most {} megapixels", MAX_PIXELS / 1_000_000))),
    }
}

/// `img` with its long edge at most `edge` (an average of the pixels each one covers, in linear light), refused
/// past [`MAX_PIXELS`]: a pasted picture is made small before it is kept or measured.
pub fn fit_edge(img: &Rgba8, edge: usize) -> Result<Rgba8, LookError> {
    check_pixels(img.width, img.height)?;
    if img.width == 0 || img.height == 0 || img.data.len() != img.width.saturating_mul(img.height) {
        return Err(LookError::Invalid("the picture is empty".into()));
    }
    let long = img.width.max(img.height);
    if long <= edge.max(1) {
        return Ok(img.clone());
    }
    let k = edge.max(1) as f64 / long as f64;
    let (w, h) = (((img.width as f64 * k).round() as usize).max(1), ((img.height as f64 * k).round() as usize).max(1));
    use lightcraft_color::transfer::{decode_srgb8, encode_srgb8};
    let lin = img.map(|p| [decode_srgb8(p[0]), decode_srgb8(p[1]), decode_srgb8(p[2])]);
    let small = lightcraft_raster::resample::resize(&lin, w, h, lightcraft_raster::resample::Filter::Box);
    Ok(small.map(|p| [encode_srgb8(p[0]), encode_srgb8(p[1]), encode_srgb8(p[2]), 255]))
}

/// Read a file of at most [`MAX_BYTES`].
pub fn read_file(path: &Path) -> Result<Vec<u8>, LookError> {
    let meta = std::fs::metadata(path).map_err(|e| LookError::Io(format!("{}: {e}", path.display())))?;
    if !meta.is_file() {
        return Err(LookError::Io(format!("{} is not a file", path.display())));
    }
    if meta.len() > MAX_BYTES {
        return Err(LookError::TooLarge(format!("{} is {} MB; a reference may be at most {} MB", path.display(), meta.len() >> 20, MAX_BYTES >> 20)));
    }
    std::fs::read(path).map_err(|e| LookError::Io(format!("{}: {e}", path.display())))
}

/// Read the picture `src` names. `cache` is the folder downloads are kept in (none: not kept).
pub fn load(src: &Source, cache: Option<&Path>, cancel: &AtomicBool) -> Result<Rgba8, LookError> {
    match src {
        Source::File(p) => decode(&read_file(p)?),
        Source::Bytes(b) => {
            if b.len() as u64 > MAX_BYTES {
                return Err(LookError::TooLarge(format!("the pasted image is {} MB; at most {} MB", b.len() >> 20, MAX_BYTES >> 20)));
            }
            decode(b)
        }
        Source::Url(u) => decode(&web::fetch(u, cache, cancel)?),
    }
}

/// Read a body through `read` (bytes into the buffer, 0 at the end), refusing more than `cap` bytes: at once when the
/// server announces a larger `declared` length, else as soon as more arrives.
pub fn read_capped(declared: Option<u64>, cap: u64, mut read: impl FnMut(&mut [u8]) -> Result<usize, LookError>) -> Result<Vec<u8>, LookError> {
    let too_large = || LookError::TooLarge(format!("the download is larger than {} MB", cap >> 20));
    if declared.is_some_and(|n| n > cap) {
        return Err(too_large());
    }
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = read(&mut buf)?;
        if n == 0 {
            return Ok(out);
        }
        if (out.len() as u64).saturating_add(n as u64) > cap {
            return Err(too_large());
        }
        out.extend_from_slice(buf.get(..n).unwrap_or_default());
    }
}

/// The cache file name for `url` (FNV-1a of the address: stable across runs).
pub fn cache_name(url: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.trim().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}.img")
}

/// Check that `url` is an `https://` address the client can use.
#[cfg(not(target_arch = "wasm32"))]
pub fn check_url(url: &str) -> Result<lightcraft_fetch::http::Url, LookError> {
    let u = lightcraft_fetch::http::Url::parse(url).map_err(|e| LookError::Download(e.to_string()))?;
    if !u.tls {
        return Err(LookError::Download("only https:// addresses are used".into()));
    }
    if u.host.is_empty() {
        return Err(LookError::Download("the address has no host".into()));
    }
    Ok(u)
}

#[cfg(not(target_arch = "wasm32"))]
mod web {
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use lightcraft_fetch::http::{self, Limits};

    use super::{LookError, MAX_BYTES, cache_name, check_url, read_capped};

    /// Downloads kept in the cache folder (the oldest go first).
    const CACHE_FILES: usize = 16;
    const MAX_REDIRECTS: usize = 5;

    pub fn fetch(url: &str, cache: Option<&Path>, cancel: &AtomicBool) -> Result<Vec<u8>, LookError> {
        let mut u = check_url(url)?;
        let cached = cache.map(|c| c.join(cache_name(url)));
        if let Some(p) = &cached
            && let Ok(meta) = std::fs::metadata(p)
            && meta.len() <= MAX_BYTES
            && let Ok(b) = std::fs::read(p)
        {
            return Ok(b);
        }
        let limits = Limits { connect: Duration::from_secs(10), stall: Duration::from_secs(20), cancel };
        let net = |e: http::HttpError| match e {
            http::HttpError::Cancelled => LookError::Cancelled,
            e => LookError::Download(e.to_string()),
        };
        for _ in 0..=MAX_REDIRECTS {
            let mut r = http::get(&u, &[("Accept", "image/*".to_string())], &limits).map_err(net)?;
            if (300..400).contains(&r.status) {
                let loc = r.header("location").ok_or_else(|| LookError::Download(format!("redirect {} without an address", r.status)))?;
                let next = u.join(loc).map_err(net)?;
                if !next.tls {
                    return Err(LookError::Download("the address redirects to an insecure (http) address".into()));
                }
                u = next;
                continue;
            }
            if r.status != 200 {
                return Err(LookError::Download(format!("the server answered {}", r.status)));
            }
            let declared = r.content_length();
            let bytes = read_capped(declared, MAX_BYTES, |buf| r.read(buf, &limits).map_err(net))?;
            if let (Some(p), Some(dir)) = (&cached, cache) {
                keep(dir, p, &bytes);
            }
            return Ok(bytes);
        }
        Err(LookError::Download("too many redirects".into()))
    }

    /// Keep a download in the cache (failing quietly: the cache is only a saving), removing the oldest past
    /// [`CACHE_FILES`].
    fn keep(dir: &Path, path: &Path, bytes: &[u8]) {
        if std::fs::create_dir_all(dir).is_err() || std::fs::write(path, bytes).is_err() {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "img"))
            .filter_map(|p| std::fs::metadata(&p).and_then(|m| m.modified()).ok().map(|t| (t, p)))
            .collect();
        if files.len() <= CACHE_FILES {
            return;
        }
        files.sort();
        for (_, p) in files.iter().take(files.len() - CACHE_FILES) {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::LookError;
    pub fn fetch(_: &str, _: Option<&std::path::Path>, _: &std::sync::atomic::AtomicBool) -> Result<Vec<u8>, LookError> {
        Err(LookError::Download("the web build cannot download a reference".into()))
    }
}
