//! The kept searches: a face's fills by a hash of everything the search reads (the pixels around the face, its
//! masks, its place and the view's mapping), so a slider drag blends kept fills instead of searching again.
//!
//! The hash covers every pixel the search reads, so a kept result is exactly what a new search would give: a render
//! never depends on what was rendered before.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_raster::{Rgb32f, par_rows};

use super::grid::Grid;
use super::region::{PAD, PARTS};
use super::repair::Fill;
use crate::Scene;

/// Most searches kept (a face in the draft, the loupe and a zoomed window, for a few photos).
const KEEP: usize = 12;
/// Most fills kept in all (24 bytes each: about 200 MB).
const MAX_FILLS: usize = 8 << 20;
/// Mask samples hashed per mask, at most (spread over it).
const MASK_SAMPLES: usize = 4096;
/// Bump when the search changes, so a result of the old one is never used.
const VERSION: u64 = 1;

static KEPT: Mutex<VecDeque<(u64, Arc<Vec<Fill>>)>> = Mutex::new(VecDeque::new());

/// The kept fills under `key`, or `make`'s, kept.
pub(super) fn get_or(key: u64, make: impl FnOnce() -> Vec<Fill>) -> Arc<Vec<Fill>> {
    {
        let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(pos) = kept.iter().position(|(k, _)| *k == key)
            && let Some(hit) = kept.remove(pos)
        {
            let fills = Arc::clone(&hit.1);
            kept.push_front(hit);
            return fills;
        }
    }
    let fills = Arc::new(make());
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    kept.retain(|(k, _)| *k != key);
    kept.push_front((key, Arc::clone(&fills)));
    let mut total = 0usize;
    let mut keep = 0usize;
    for (_, f) in kept.iter() {
        if keep >= KEEP || (keep > 0 && total.saturating_add(f.len()) > MAX_FILLS) {
            break;
        }
        total = total.saturating_add(f.len());
        keep += 1;
    }
    kept.truncate(keep);
    fills
}

/// A 64-bit hash, a word at a time.
struct Hash(u64);

impl Hash {
    fn word(&mut self, v: u64) {
        self.0 = (self.0 ^ v).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.0 ^= self.0 >> 31;
    }

    fn f64(&mut self, v: f64) {
        self.word(v.to_bits());
    }
}

/// The key of face `face`'s search on `img`.
pub(super) fn key(img: &Rgb32f, scene: &Scene<'_>, face: usize, grid: &Grid) -> u64 {
    let mut h = Hash(0x5354_5241_5948_4149 ^ VERSION);
    h.word(img.width as u64);
    h.word(img.height as u64);
    for v in [grid.ox, grid.oy, grid.cell, grid.face] {
        h.f64(v);
    }
    for v in grid.px {
        h.word(v as u64);
    }
    // the view's mapping, at the searched box's corners and centre
    let [x0, y0, x1, y1] = grid.px.map(|v| v as f64);
    for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1), ((x0 + x1) / 2.0, (y0 + y1) / 2.0)] {
        let (nx, ny) = scene.view().to_norm(x, y);
        h.f64(nx);
        h.f64(ny);
    }
    if let Some(ctx) = scene.faces.get(face) {
        for p in PARTS {
            let Some(m) = ctx.face.mask(*p) else {
                h.word(u64::MAX);
                continue;
            };
            h.word(*p as u64);
            for v in [m.area.x0, m.area.y0, m.area.x1, m.area.y1] {
                h.f64(v);
            }
            h.word(m.plane.width as u64);
            h.word(m.plane.height as u64);
            let step = (m.plane.data.len() / MASK_SAMPLES).max(1);
            for v in m.plane.data.iter().step_by(step) {
                h.word(u64::from(v.to_bits()));
            }
        }
    }
    // every pixel the search reads
    let pad = (PAD * grid.face).ceil().max(0.0) as usize + 2;
    let [x0, y0, x1, y1] = grid.px;
    let (x0, y0) = (x0.saturating_sub(pad), y0.saturating_sub(pad));
    let (x1, y1) = (x1.saturating_add(pad).min(img.width), y1.saturating_add(pad).min(img.height));
    // (a hash per row, rows in parallel, then the rows' hashes in order)
    let mut rows = vec![0u64; y1.saturating_sub(y0)];
    par_rows(&mut rows, 1, |r, slot| {
        let y = y0 + r;
        let mut rh = Hash(y as u64);
        if let Some(row) = img.data.get(y * img.width + x0..y * img.width + x1.max(x0)) {
            for p in row {
                rh.word(u64::from(p[0].to_bits()) | (u64::from(p[1].to_bits()) << 32));
                rh.word(u64::from(p[2].to_bits()));
            }
        }
        if let Some(s) = slot.first_mut() {
            *s = rh.0;
        }
    });
    for r in rows {
        h.word(r);
    }
    h.0
}
