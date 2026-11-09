//! Fork feature (`fork/glare-highlights.md`): a correction made of tiles, each covering a part of the photo.
//!
//! The first format (`LCREFL01`) holds one grid for the whole photo. A model run on an area, and a glare fill, change
//! only part of the photo, at that part's own resolution, so their correction is a list of tiles (`LCREFL02`): each
//! tile is a box (fractions of the original photo, before crop or rotation) and a grid of the linear Rec. 2020 change
//! to subtract inside it, already weighted by the soft edge. A render scales each tile's grid to the box's size in its
//! own pixels, so a preview and an export apply the same correction.

use lightcraft_raster::Rgb32f;

pub const MAGIC_V1: &[u8; 8] = b"LCREFL01";
pub const MAGIC_V2: &[u8; 8] = b"LCREFL02";
/// Longest side of a whole-photo grid (`LCREFL01`).
pub const MAX_EDGE_V1: usize = 1024;
/// Longest side of one tile's grid.
pub const MAX_TILE_EDGE: usize = 2048;
/// Most tiles in one correction.
pub const MAX_TILES: usize = 64;
/// Most grid pixels of all tiles together (12 bytes each).
pub const MAX_PIXELS: usize = 12 << 20;
/// Largest change a value may hold (linear light; glare on a raw can sit well above white).
const MAX_VALUE: f32 = 64.0;

/// One part of a correction.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    /// Left, top, right, bottom: fractions of the original photo.
    pub rect: [f32; 4],
    pub grid: Rgb32f,
}

impl Tile {
    /// The whole photo.
    pub fn whole(grid: Rgb32f) -> Self {
        Tile { rect: [0.0, 0.0, 1.0, 1.0], grid }
    }

    /// The tile's box in a `w × h` picture's pixels, `[x0, y0, x1, y1)`, kept inside it; `None` when empty.
    pub fn pixel_box(&self, w: usize, h: usize) -> Option<[usize; 4]> {
        let [l, t, r, b] = self.rect;
        let px = |v: f32, n: usize| ((v as f64 * n as f64).round().max(0.0) as usize).min(n);
        let (x0, y0, x1, y1) = (px(l, w), px(t, h), px(r, w), px(b, h));
        (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
    }
}

fn valid_rect(r: [f32; 4]) -> bool {
    r.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) && r[0] < r[2] && r[1] < r[3]
}

/// The bytes of a tiled correction.
pub fn encode(tiles: &[Tile]) -> Result<Vec<u8>, String> {
    if tiles.is_empty() || tiles.len() > MAX_TILES {
        return Err("Invalid number of correction tiles".into());
    }
    let total: usize = tiles.iter().map(|t| t.grid.data.len()).sum();
    if total > MAX_PIXELS {
        return Err("Reflection correction too large".into());
    }
    let mut b = Vec::with_capacity(12 + tiles.len() * 24 + total * 12);
    b.extend_from_slice(MAGIC_V2);
    b.extend((tiles.len() as u32).to_le_bytes());
    for t in tiles {
        let g = &t.grid;
        if !valid_rect(t.rect) || g.width == 0 || g.height == 0 || g.width.max(g.height) > MAX_TILE_EDGE || g.data.len() != g.width * g.height {
            return Err("Invalid correction tile".into());
        }
        b.extend((g.width as u32).to_le_bytes());
        b.extend((g.height as u32).to_le_bytes());
        for v in t.rect {
            b.extend(v.to_le_bytes());
        }
        for p in &g.data {
            for v in p {
                let v = if v.is_finite() { v.clamp(-MAX_VALUE, MAX_VALUE) } else { 0.0 };
                b.extend(v.to_le_bytes());
            }
        }
    }
    Ok(b)
}

/// A little-endian reader that never reads past the end.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let end = self.at.checked_add(N).ok_or("Truncated correction")?;
        let b: [u8; N] = self.bytes.get(self.at..end).ok_or("Truncated correction")?.try_into().map_err(|_| "Truncated correction")?;
        self.at = end;
        Ok(b)
    }
    fn u32(&mut self) -> Result<usize, String> {
        Ok(u32::from_le_bytes(self.take()?) as usize)
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take()?))
    }
    fn grid(&mut self, w: usize, h: usize, max: f32) -> Result<Rgb32f, String> {
        let n = w.checked_mul(h).ok_or("Invalid reflection dimensions")?;
        let bytes = n.checked_mul(12).ok_or("Invalid reflection dimensions")?;
        if self.bytes.len().saturating_sub(self.at) < bytes {
            return Err("Truncated correction".into());
        }
        let mut data = Vec::with_capacity(n);
        for _ in 0..n {
            let mut p = [0.0; 3];
            for v in &mut p {
                *v = self.f32()?;
                if !v.is_finite() || v.abs() > max {
                    return Err("Invalid reflection values".into());
                }
            }
            data.push(p);
        }
        Ok(Rgb32f { width: w, height: h, data })
    }
}

/// Read a correction of either format (at most `limit` bytes are read from the file).
pub fn read(path: &str) -> Result<Vec<Tile>, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| format!("Reflection correction missing: {e}. Recalculate or reset reflection removal."))?;
    let limit = 12 + MAX_TILES * 24 + MAX_PIXELS * 12;
    let mut bytes = Vec::new();
    file.by_ref().take(limit as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("Reflection correction too large".into());
    }
    decode(&bytes)
}

/// [`read`] from bytes.
pub fn decode(bytes: &[u8]) -> Result<Vec<Tile>, String> {
    let mut r = Reader { bytes, at: 8 };
    match bytes.get(..8) {
        Some(m) if m == MAGIC_V1.as_slice() => {
            let (w, h) = (r.u32()?, r.u32()?);
            if w == 0 || h == 0 || w > MAX_EDGE_V1 || h > MAX_EDGE_V1 || bytes.len() != 16 + w * h * 12 {
                return Err("Invalid reflection dimensions".into());
            }
            Ok(vec![Tile::whole(r.grid(w, h, 4.0)?)])
        }
        Some(m) if m == MAGIC_V2.as_slice() => {
            let n = r.u32()?;
            if n == 0 || n > MAX_TILES {
                return Err("Invalid number of correction tiles".into());
            }
            let mut tiles = Vec::with_capacity(n);
            let mut total = 0usize;
            for _ in 0..n {
                let (w, h) = (r.u32()?, r.u32()?);
                let rect = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
                total = total.saturating_add(w.saturating_mul(h));
                if w == 0 || h == 0 || w.max(h) > MAX_TILE_EDGE || total > MAX_PIXELS || !valid_rect(rect) {
                    return Err("Invalid correction tile".into());
                }
                tiles.push(Tile { rect, grid: r.grid(w, h, MAX_VALUE)? });
            }
            if r.at != bytes.len() {
                return Err("Invalid reflection correction".into());
            }
            Ok(tiles)
        }
        _ => Err("Invalid reflection correction".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(rect: [f32; 4], w: usize, h: usize, v: f32) -> Tile {
        Tile { rect, grid: Rgb32f::filled(w, h, [v, -v, 0.5 * v]) }
    }

    #[test]
    fn tiles_round_trip_and_the_first_format_still_reads() {
        let tiles = vec![tile([0.1, 0.2, 0.3, 0.4], 3, 2, 0.25), tile([0.5, 0.5, 1.0, 1.0], 1, 1, 9.0)];
        assert_eq!(decode(&encode(&tiles).unwrap()).unwrap(), tiles);
        let mut v1 = MAGIC_V1.to_vec();
        v1.extend(1u32.to_le_bytes());
        v1.extend(1u32.to_le_bytes());
        for v in [0.1f32, 0.2, 0.3] {
            v1.extend(v.to_le_bytes());
        }
        let read = decode(&v1).unwrap();
        assert_eq!(read, vec![Tile::whole(Rgb32f::filled(1, 1, [0.1, 0.2, 0.3]))]);
    }

    #[test]
    fn damaged_files_are_errors_not_panics() {
        let good = encode(&[tile([0.1, 0.2, 0.3, 0.4], 3, 2, 0.25)]).unwrap();
        for cut in 0..good.len() {
            assert!(decode(&good[..cut]).is_err(), "cut at {cut}");
        }
        let mut long = good.clone();
        long.push(0);
        assert!(decode(&long).is_err());
        let mut huge = MAGIC_V2.to_vec();
        huge.extend(1u32.to_le_bytes());
        huge.extend(u32::MAX.to_le_bytes());
        huge.extend(u32::MAX.to_le_bytes());
        assert!(decode(&huge).is_err());
        let mut bad_rect = good.clone();
        bad_rect[20..24].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode(&bad_rect).is_err());
        assert!(encode(&[]).is_err());
        assert!(encode(&[tile([0.5, 0.0, 0.4, 1.0], 1, 1, 0.0)]).is_err());
    }

    #[test]
    fn pixel_boxes_stay_inside_the_picture() {
        let t = tile([0.25, 0.5, 1.0, 1.0], 1, 1, 0.0);
        assert_eq!(t.pixel_box(100, 10), Some([25, 5, 100, 10]));
        assert_eq!(tile([0.0, 0.0, 0.001, 0.001], 1, 1, 0.0).pixel_box(100, 100), None);
    }
}
