//! The pixels under a Reduce grid as they were before the feature ran, kept so a full Reduce gives them back.
//!
//! A box that fits in [`SAVED_MAX_BYTES`] is copied into memory. A bigger one (a large export painted over) is
//! written to a temporary file in bands as it is taken and read back band by band as the pixels are mixed back, so
//! memory stays within the cap at any size and a render is never missing its Reduce. (If the file can't be made,
//! the copy stays in memory rather than skip the Reduce.)

use std::cell::Cell;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use lightcraft_raster::{Rgb32f, par_rows};

use crate::feature::View;
use crate::input::PartMask;

/// Memory the saved pixels of a Reduce may take (12 bytes a pixel: about 44 megapixels); a bigger box goes through
/// a file, a band at a time.
pub(super) const SAVED_MAX_BYTES: usize = 512 << 20;
const PIXEL_BYTES: usize = std::mem::size_of::<[f32; 3]>();

thread_local! {
    static CAP: Cell<Option<usize>> = const { Cell::new(None) };
    static SPILLS: Cell<usize> = const { Cell::new(0) };
}

/// Run `f` with the cap on saved pixels at `bytes` on this thread (tests make it small to go through the file).
#[doc(hidden)]
pub fn with_saved_cap<R>(bytes: usize, f: impl FnOnce() -> R) -> R {
    let before = CAP.with(|c| c.replace(Some(bytes)));
    let out = f();
    CAP.with(|c| c.set(before));
    out
}

/// How many Reduce copies went through a file on this thread (a test checks it did).
#[doc(hidden)]
pub fn spills_made() -> usize {
    SPILLS.with(Cell::get)
}

fn cap() -> usize {
    CAP.with(Cell::get).unwrap_or(SAVED_MAX_BYTES)
}

/// A temporary file, removed when dropped.
struct Spill {
    file: std::fs::File,
    path: PathBuf,
}

impl Spill {
    fn make() -> Option<Spill> {
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("lightcraft-refine-{}-{}.tmp", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).ok()?;
        Some(Spill { file, path })
    }
}

impl Drop for Spill {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

enum Store {
    Memory(Vec<[f32; 3]>),
    /// Written band by band (`band_rows` rows of the box each), row after row.
    File {
        spill: Spill,
        band_rows: usize,
    },
}

pub(super) struct Saved {
    /// The box, `[x0, y0, x1, y1)`.
    b: [usize; 4],
    store: Store,
}

impl Saved {
    /// The pixels of `b` of `img`.
    pub(super) fn take(img: &Rgb32f, b: [usize; 4]) -> Option<Saved> {
        let (bw, bh) = (b[2].checked_sub(b[0])?, b[3].checked_sub(b[1])?);
        let row = |y: usize| img.data.get(y * img.width + b[0]..y * img.width + b[2]);
        let cap = cap();
        if bw.saturating_mul(bh).saturating_mul(PIXEL_BYTES) > cap
            && let Some(saved) = Saved::spill(img, b, bw, cap)
        {
            return Some(saved);
        }
        let mut px = Vec::with_capacity(bw.saturating_mul(bh));
        for y in b[1]..b[3] {
            px.extend_from_slice(row(y)?);
        }
        Some(Saved { b, store: Store::Memory(px) })
    }

    /// The same, written to a file in bands that each take half the cap (the band read back and its bytes share it).
    fn spill(img: &Rgb32f, b: [usize; 4], bw: usize, cap: usize) -> Option<Saved> {
        let band_rows = ((cap / 2) / (bw.max(1) * PIXEL_BYTES)).max(1);
        let mut spill = Spill::make()?;
        let mut bytes = Vec::with_capacity(band_rows.min(b[3] - b[1]) * bw * PIXEL_BYTES);
        let mut y = b[1];
        while y < b[3] {
            let end = (y + band_rows).min(b[3]);
            bytes.clear();
            for yy in y..end {
                for p in img.data.get(yy * img.width + b[0]..yy * img.width + b[2])? {
                    bytes.extend(p.iter().flat_map(|v| v.to_le_bytes()));
                }
            }
            spill.file.write_all(&bytes).ok()?;
            y = end;
        }
        spill.file.flush().ok()?;
        SPILLS.with(|c| c.set(c.get() + 1));
        Some(Saved { b, store: Store::File { spill, band_rows } })
    }

    /// Mix each pixel back toward its saved value by the grid's cut there (the saved bits at a full cut).
    pub(super) fn mix_back(&mut self, img: &mut Rgb32f, view: &dyn View, g: &PartMask) {
        let [x0, y0, x1, y1] = self.b;
        let bw = x1 - x0;
        match &mut self.store {
            Store::Memory(px) => mix_rows(img, view, g, [x0, x1], y0, px),
            Store::File { spill, band_rows } => {
                if spill.file.seek(SeekFrom::Start(0)).is_err() {
                    return;
                }
                let mut bytes = Vec::new();
                let mut y = y0;
                while y < y1 {
                    let end = (y + *band_rows).min(y1);
                    bytes.resize((end - y) * bw * PIXEL_BYTES, 0);
                    // (a read that fails leaves the rest of the box as the feature made it)
                    if spill.file.read_exact(&mut bytes).is_err() {
                        return;
                    }
                    let (pixels, _) = bytes.as_chunks::<PIXEL_BYTES>();
                    let px: Vec<[f32; 3]> = pixels
                        .iter()
                        .map(|c| std::array::from_fn(|k| f32::from_le_bytes([c[4 * k], c[4 * k + 1], c[4 * k + 2], c[4 * k + 3]])))
                        .collect();
                    mix_rows(img, view, g, [x0, x1], y, &px);
                    y = end;
                }
            }
        }
    }
}

/// Mix the rows of `img` from `ya` (columns `[x0, x1)`) back toward `px` (those pixels as they were) by the cut.
fn mix_rows(img: &mut Rgb32f, view: &dyn View, g: &PartMask, [x0, x1]: [usize; 2], ya: usize, px: &[[f32; 3]]) {
    let (w, bw) = (img.width, x1 - x0);
    if bw == 0 {
        return;
    }
    let rows_n = px.len() / bw;
    let Some(rows) = img.data.get_mut(ya * w..(ya + rows_n) * w) else { return };
    par_rows(rows, w, |ry, row| {
        let y = ya + ry;
        let (Some(dst), Some(src)) = (row.get_mut(x0..x1), px.get(ry * bw..(ry + 1) * bw)) else { return };
        for (i, (d, o)) in dst.iter_mut().zip(src).enumerate() {
            let (ex, ey) = view.to_edit((x0 + i) as f64 + 0.5, y as f64 + 0.5);
            let c = g.sample(ex, ey);
            if c >= 1.0 {
                *d = *o;
            } else if c > 0.0 {
                for k in 0..3 {
                    d[k] += (o[k] - d[k]) * c;
                }
            }
        }
    });
}
