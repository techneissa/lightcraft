//! The loupe's finished full-size picture as the disk cache keeps it ([`crate::DiskCache::put_full`]): lossless, so
//! a photo opened again shows at every zoom the very pixels it was rendered with (issue #178).
//!
//! A thumbnail is a JPEG ([`crate::disk`]); this is the photo at 100 %, where JPEG's chroma subsampling would show
//! on fine coloured detail. The file is the picture's 8-bit RGBA, each row stored as the difference from the pixel
//! to its left (photos compress far better that way), cut into bands of [`BAND_ROWS`] rows that are compressed
//! (zlib) one by one, so they are written and read on all cores:
//!
//! ```text
//! "LCFULL" 0 1 | width u32 | height u32 | bands u32 | bands × compressed length u32 | the bands
//! ```
//!
//! (little-endian). A file that is short, damaged or of another size than asked for reads as nothing: each band's
//! zlib checksum is checked, every length is checked against the file before anything is allocated, and the
//! picture is never bigger than the caller allows.

use lightcraft_raster::Rgba8;

/// What a full-size picture's file is named after its key (next to the thumbnails' [`crate::disk::EXT`]).
pub const EXT: &str = "lcfull";

const MAGIC: &[u8; 8] = b"LCFULL\x00\x01";

/// Rows per compressed band.
pub const BAND_ROWS: usize = 256;

/// The largest picture a file may hold (pixels): 268 MP.
pub const MAX_PIXELS: usize = 1 << 28;

/// zlib level: the fastest. The picture is written once per finished edit, in the background, and read back
/// whenever the photo is opened again; a higher level takes several times longer for a few per cent.
const LEVEL: u8 = 1;

const HEADER: usize = 8 + 4 + 4 + 4;

/// `img` as a full-size picture's file. `None`: empty, or larger than [`MAX_PIXELS`].
pub fn encode(img: &Rgba8) -> Option<Vec<u8>> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || w.checked_mul(h)? > MAX_PIXELS || img.data.len() != w * h {
        return None;
    }
    let bands = h.div_ceil(BAND_ROWS);
    let packed: Vec<Vec<u8>> = map_bands(bands, |b| {
        let rows = img.data.get(b * BAND_ROWS * w..((b + 1) * BAND_ROWS).min(h) * w).unwrap_or_default();
        let mut filtered = Vec::with_capacity(rows.len() * 4);
        for row in rows.chunks(w) {
            let mut left = [0u8; 4];
            for p in row {
                filtered.extend(p.iter().zip(left).map(|(v, l)| v.wrapping_sub(l)));
                left = *p;
            }
        }
        miniz_oxide::deflate::compress_to_vec_zlib(&filtered, LEVEL)
    });
    let data: usize = packed.iter().map(Vec::len).sum();
    let mut out = Vec::with_capacity(HEADER + bands * 4 + data);
    out.extend_from_slice(MAGIC);
    for v in [w, h, bands] {
        out.extend_from_slice(&u32::try_from(v).ok()?.to_le_bytes());
    }
    for band in &packed {
        out.extend_from_slice(&u32::try_from(band.len()).ok()?.to_le_bytes());
    }
    for band in packed {
        out.extend_from_slice(&band);
    }
    Some(out)
}

/// The width and height a file says it holds (from its header only), when it is one of these files.
pub fn dims(bytes: &[u8]) -> Option<(usize, usize)> {
    if bytes.get(..8)? != MAGIC {
        return None;
    }
    let (w, h) = (read_u32(bytes, 8)?, read_u32(bytes, 12)?);
    (w > 0 && h > 0 && w.checked_mul(h)? <= MAX_PIXELS).then_some((w, h))
}

/// The picture in `bytes`, when it is a whole, undamaged file of a picture of at most `max` (width, height).
pub fn decode(bytes: &[u8], max: (usize, usize)) -> Option<Rgba8> {
    let (w, h) = dims(bytes)?;
    let bands = read_u32(bytes, 16)?;
    if w > max.0 || h > max.1 || bands != h.div_ceil(BAND_ROWS) {
        return None;
    }
    // every band's place in the file, checked against its length before anything is unpacked
    let mut at = HEADER.checked_add(bands.checked_mul(4)?)?;
    let mut spans = Vec::with_capacity(bands);
    for b in 0..bands {
        let len = read_u32(bytes, HEADER + b * 4)?;
        let end = at.checked_add(len)?;
        spans.push(bytes.get(at..end)?);
        at = end;
    }
    if at != bytes.len() {
        return None;
    }
    let rows: Vec<Option<Vec<[u8; 4]>>> = map_bands(bands, |b| {
        let n = (((b + 1) * BAND_ROWS).min(h) - b * BAND_ROWS) * w;
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(spans.get(b)?, n * 4).ok()?;
        if raw.len() != n * 4 {
            return None;
        }
        let mut px = Vec::with_capacity(n);
        for row in raw.as_chunks::<4>().0.chunks(w) {
            let mut left = [0u8; 4];
            for d in row {
                let p = [d[0].wrapping_add(left[0]), d[1].wrapping_add(left[1]), d[2].wrapping_add(left[2]), d[3].wrapping_add(left[3])];
                px.push(p);
                left = p;
            }
        }
        Some(px)
    });
    let mut data = Vec::with_capacity(w * h);
    for band in rows {
        data.extend(band?);
    }
    Some(Rgba8 { width: w, height: h, data })
}

fn read_u32(bytes: &[u8], at: usize) -> Option<usize> {
    let b: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
    usize::try_from(u32::from_le_bytes(b)).ok()
}

/// `f` for each band, on all cores where there are threads.
fn map_bands<T: Send>(bands: usize, f: impl Fn(usize) -> T + Sync + Send) -> Vec<T> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        (0..bands).into_par_iter().map(f).collect()
    }
    #[cfg(target_arch = "wasm32")]
    {
        (0..bands).map(f).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: usize, h: usize) -> Rgba8 {
        let data = (0..w * h).map(|i| [(i * 7 % 251) as u8, (i / w * 3 % 256) as u8, (i % w) as u8, 255]).collect();
        Rgba8 { width: w, height: h, data }
    }

    #[test]
    fn a_picture_reads_back_exactly() {
        let img = picture(300, BAND_ROWS * 2 + 17);
        let bytes = encode(&img).expect("encoded");
        assert_eq!(dims(&bytes), Some((300, BAND_ROWS * 2 + 17)));
        assert_eq!(decode(&bytes, (300, BAND_ROWS * 2 + 17)), Some(img));
    }

    #[test]
    fn a_short_damaged_or_other_sized_file_reads_as_nothing() {
        let img = picture(64, 300);
        let bytes = encode(&img).expect("encoded");
        assert_eq!(decode(&bytes, (64, 299)), None, "bigger than allowed");
        for cut in [0, 7, HEADER, HEADER + 5, bytes.len() / 2, bytes.len() - 1] {
            assert_eq!(decode(bytes.get(..cut).unwrap_or_default(), (64, 300)), None, "cut at {cut}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(decode(&longer, (64, 300)), None, "trailing bytes");
        // a flipped byte in the compressed data: the band's checksum (or its stream) no longer matches
        let mut flipped = bytes.clone();
        let at = bytes.len() - 3;
        flipped[at] ^= 0x55;
        assert_eq!(decode(&flipped, (64, 300)), None, "damaged data");
        // a header that claims a huge picture is refused before anything is allocated
        let mut huge = bytes;
        huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&huge, (64, 300)), None);
        assert_eq!(decode(b"LCFULL\x00\x01garbage", (64, 300)), None);
    }

    // Given a full-size picture kept in the disk cache, when the same key asks for it it reads back exactly; another
    // key (other settings) finds nothing; and once its file is cut short it reads as nothing and is removed
    #[test]
    fn the_disk_cache_reads_back_its_full_size_picture_and_drops_a_damaged_one() {
        let dir = std::env::temp_dir().join(format!("lc-preview-full-{}-{}", std::process::id(), crate::next_tick()));
        let cache = crate::DiskCache::new(&dir, 64 << 20);
        let img = picture(120, 90);
        let key = crate::Hash128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
        assert!(cache.put_full(key, &img));
        assert_eq!(cache.get_full(key, (120, 90)), Some(img.clone()));
        assert_eq!(cache.get_full(crate::Hash128(key.0 ^ 1), (120, 90)), None, "other settings");
        assert_eq!(cache.get_full(key, (119, 90)), None, "bigger than allowed");
        assert!(cache.put_full(key, &img));
        assert!(cache.put_full(key, &img));
        let hex = key.to_string();
        let file = dir.join(&hex[..2]).join(format!("{hex}.{EXT}"));
        let bytes = std::fs::read(&file).expect("the file");
        std::fs::write(&file, &bytes[..bytes.len() / 3]).expect("cut it short");
        assert_eq!(cache.get_full(key, (120, 90)), None, "a short file");
        assert!(!file.exists(), "the damaged file is removed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
