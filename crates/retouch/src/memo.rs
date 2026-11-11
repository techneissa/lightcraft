//! Work a feature keeps between renders, so a slider drag redoes only what the slider changes (issue #234).
//!
//! A feature splits its run into the part no slider of it changes (made from the pixels it is given, the face's
//! mask and where the image lies) and the part a slider sets. The first part is kept here by a key of everything it
//! was made from ([`pixels`], [`mask`]): a render with the same key reuses it, and any other render makes its own.
//! The step gets the same pixels for every tick of one of its own sliders (the picture before the step is kept by
//! the pipeline, and the features before it run with the same settings), so a drag makes it once.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_raster::{Rgb32f, par_rows};

use crate::{Part, Scene};

/// Kept work of one kind, the most recent last, within a number of entries and of bytes.
pub(crate) struct Memo<T> {
    entries: Mutex<Vec<(u64, Arc<T>, usize)>>,
    most: usize,
    bytes: usize,
}

impl<T> Memo<T> {
    pub(crate) const fn new(most: usize, bytes: usize) -> Memo<T> {
        Memo { entries: Mutex::new(Vec::new()), most, bytes }
    }

    /// The work kept for `key`, or `make`'s (kept when it takes at most half the byte limit, which `size` measures:
    /// a large export's work is made and dropped, and leaves the draft's and the loupe's kept).
    /// `make` runs outside the lock, so renders on other threads never wait for it.
    pub(crate) fn get_or(&self, key: u64, make: impl FnOnce() -> Option<T>, size: impl Fn(&T) -> usize) -> Option<Arc<T>> {
        {
            let mut c = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(i) = c.iter().position(|(k, _, _)| *k == key) {
                let e = c.remove(i);
                let got = e.1.clone();
                c.push(e);
                return Some(got);
            }
        }
        let made = Arc::new(make()?);
        let bytes = size(&made);
        if bytes <= self.bytes / 2 {
            let mut c = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            c.retain(|(k, _, _)| *k != key);
            c.push((key, made.clone(), bytes));
            while c.len() > self.most || c.iter().map(|e| e.2).sum::<usize>() > self.bytes {
                c.remove(0);
            }
        }
        Some(made)
    }
}

/// A 64-bit hash, fed a word at a time (multiply and fold: fast, and every bit of a word reaches every bit).
#[derive(Clone, Copy)]
pub(crate) struct Hash(u64);

const MUL: u64 = 0x9e37_79b9_7f4a_7c15;

impl Hash {
    pub(crate) fn new(seed: u64) -> Hash {
        Hash(seed ^ 0x243f_6a88_85a3_08d3)
    }

    pub(crate) fn u64(&mut self, v: u64) {
        let h = (self.0 ^ v).wrapping_mul(MUL);
        self.0 = h ^ (h >> 29);
    }

    pub(crate) fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }

    pub(crate) fn finish(self) -> u64 {
        let h = self.0.wrapping_mul(MUL);
        h ^ (h >> 32)
    }
}

/// A hash of `img`'s pixels over the box `b` (and of the box and the image's size): rows hashed in parallel, three
/// independent chains a row (one per channel), then joined in order.
pub(crate) fn pixels(img: &Rgb32f, b: [usize; 4]) -> u64 {
    let [x0, y0, x1, y1] = b;
    let (x1, y1) = (x1.min(img.width), y1.min(img.height));
    let mut rows = vec![0u64; y1.saturating_sub(y0)];
    par_rows(&mut rows, 1, |y, out| {
        let start = (y0 + y) * img.width;
        let Some(row) = img.data.get(start + x0..start + x1.max(x0)) else { return };
        let mut lanes = [Hash::new(1), Hash::new(2), Hash::new(3)];
        for p in row {
            for (l, v) in lanes.iter_mut().zip(p) {
                l.u64(u64::from(v.to_bits()));
            }
        }
        if let Some(o) = out.first_mut() {
            let mut h = Hash::new(y as u64);
            for l in lanes {
                h.u64(l.finish());
            }
            *o = h.finish();
        }
    });
    let mut h = Hash::new(0x7069_7865_6c73);
    for v in [img.width, img.height, x0, y0, x1, y1] {
        h.u64(v as u64);
    }
    for r in rows {
        h.u64(r);
    }
    h.finish()
}

/// A hash of face `face`'s `part` mask as the box `b` of the image reads it: the mask's own values and area, and
/// where 25 points over the box fall in the photo (a crop, a turn or a lens correction moves them). `None`
/// without that mask.
pub(crate) fn mask(scene: &Scene<'_>, face: usize, part: Part, b: [usize; 4]) -> Option<u64> {
    let f = scene.faces.get(face)?;
    let m = f.face.mask(part)?;
    let mut h = Hash::new(0x6d61_736b);
    h.f64(f.size);
    for v in [m.area.x0, m.area.y0, m.area.x1, m.area.y1] {
        h.f64(v);
    }
    h.u64(m.plane.width as u64);
    h.u64(m.plane.height as u64);
    let mut rows = vec![0u64; m.plane.height];
    let w = m.plane.width.max(1);
    par_rows(&mut rows, 1, |y, out| {
        let mut l = Hash::new(y as u64);
        for v in m.plane.data.get(y * w..(y + 1) * w).unwrap_or(&[]) {
            l.u64(u64::from(v.to_bits()));
        }
        if let Some(o) = out.first_mut() {
            *o = l.finish();
        }
    });
    for r in rows {
        h.u64(r);
    }
    let view = scene.view();
    for i in 0..5 {
        for j in 0..5 {
            let x = b[0] as f64 + (b[2] - b[0]) as f64 * i as f64 / 4.0;
            let y = b[1] as f64 + (b[3] - b[1]) as f64 * j as f64 / 4.0;
            let (nx, ny) = view.to_norm(x, y);
            h.f64(nx);
            h.f64(ny);
        }
    }
    Some(h.finish())
}
