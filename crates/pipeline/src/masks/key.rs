//! The hash of a mask's definition that keys its kept alpha ([`super::alpha_key`]) and its brush parts
//! ([`super::brush_chain`]): the raw bits of every number, not the text `{:?}` makes of them. A mask of hundreds of strokes of
//! hundreds of samples is some megabytes of numbers; formatting them every render cost more than rendering a cache hit.
//!
//! The keys live in memory only (no key built here is stored), so the hash may change with the code.

use std::hash::Hasher;

use lightcraft_develop::{BrushStroke, MaskShape, SegMask};
use lightcraft_geom::Point;

/// A 64-bit hasher that takes a word at a time (several GB/s over the numbers of a stroke); not for anything stored or hostile.
pub(super) struct Words(u64);

impl Words {
    pub fn new() -> Words {
        Words(0x9e37_79b9_7f4a_7c15)
    }

    #[inline]
    fn word(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    #[inline]
    fn f(&mut self, v: f64) {
        self.word(v.to_bits());
    }

    fn point(&mut self, p: &Point) {
        self.f(p.x);
        self.f(p.y);
    }

    fn points(&mut self, p: &[Point]) {
        self.word(p.len() as u64);
        p.iter().for_each(|p| self.point(p));
    }

    fn opt_f4(&mut self, r: Option<[f64; 4]>) {
        match r {
            Some(r) => {
                self.word(1);
                r.iter().for_each(|v| self.f(*v));
            }
            None => self.word(0),
        }
    }

    fn seg(&mut self, s: &SegMask) {
        self.word(u64::from(s.side));
        self.write(s.data.as_bytes());
        self.opt_f4(s.rect);
        match &s.model {
            Some(m) => {
                self.word(1);
                self.write(m.as_bytes());
            }
            None => self.word(0),
        }
    }

    fn segs(&mut self, seg: &Option<SegMask>, detail: &[SegMask], edge: f64) {
        match seg {
            Some(s) => {
                self.word(1);
                self.seg(s);
            }
            None => self.word(0),
        }
        self.word(detail.len() as u64);
        detail.iter().for_each(|d| self.seg(d));
        self.f(edge);
    }
}

impl Hasher for Words {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let (words, tail) = bytes.as_chunks::<8>();
        words.iter().for_each(|w| self.word(u64::from_le_bytes(*w)));
        let mut last = [0u8; 8];
        last[..tail.len()].copy_from_slice(tail);
        self.word(u64::from_le_bytes(last) ^ (tail.len() as u64) << 56);
    }

    fn write_u64(&mut self, v: u64) {
        self.word(v);
    }
}

/// Hash stroke `s` into `h`: every field, point and pressure sample.
pub(super) fn stroke(h: &mut Words, s: &BrushStroke) {
    h.word(s.points.len() as u64);
    for p in &s.points {
        h.point(p);
    }
    [s.size, s.feather, s.flow, s.density].iter().for_each(|v| h.f(*v));
    h.word(u64::from(s.erase) | u64::from(s.auto_mask) << 1);
    h.word(s.pressure.len() as u64);
    s.pressure.iter().for_each(|v| h.word(u64::from(v.to_bits())));
}

/// Hash shape `shape` into `h`. Every variant is named: a new one doesn't compile until it is hashed.
pub(super) fn shape(h: &mut Words, shape: &MaskShape) {
    match shape {
        MaskShape::Brush { strokes } => {
            h.word(1);
            h.word(strokes.len() as u64);
            strokes.iter().for_each(|s| stroke(h, s));
        }
        MaskShape::Linear { start, end } => {
            h.word(2);
            h.point(start);
            h.point(end);
        }
        MaskShape::Radial { center, rx, ry, angle, feather, invert } => {
            h.word(3);
            h.point(center);
            [rx, ry, angle, feather].iter().for_each(|v| h.f(**v));
            h.word(u64::from(*invert));
        }
        MaskShape::ColorRange { samples, refine } => {
            h.word(4);
            h.word(samples.len() as u64);
            samples.iter().flatten().for_each(|v| h.f(*v));
            h.f(*refine);
        }
        MaskShape::LuminanceRange { lo, hi, lo_feather, hi_feather } => {
            h.word(5);
            [lo, hi, lo_feather, hi_feather].iter().for_each(|v| h.f(**v));
        }
        MaskShape::DepthRange { lo, hi, feather } => {
            h.word(6);
            [lo, hi, feather].iter().for_each(|v| h.f(**v));
        }
        MaskShape::Subject { seg, detail, edge } => {
            h.word(7);
            h.segs(seg, detail, *edge);
        }
        MaskShape::Sky { seg, detail, edge } => {
            h.word(8);
            h.segs(seg, detail, *edge);
        }
        MaskShape::Background { seg, detail, edge } => {
            h.word(9);
            h.segs(seg, detail, *edge);
        }
        MaskShape::Object { hint, exclude, seg, detail, edge } => {
            h.word(10);
            h.points(hint);
            h.points(exclude);
            h.segs(seg, detail, *edge);
        }
        MaskShape::Prompt { text, seg, detail, edge, region } => {
            h.word(11);
            h.write(text.as_bytes());
            h.segs(seg, detail, *edge);
            // (issue #281: only when set, so a mask without one keeps its key)
            if region.is_some() {
                h.opt_f4(*region);
            }
        }
        MaskShape::People { person, parts, seg, detail, edge, .. } => {
            h.word(12);
            h.word(u64::from(*person));
            h.word(parts.len() as u64);
            parts.iter().for_each(|p| h.write(p.as_bytes()));
            // (fork, `fork/select-people.md`: the stored selection; the face it belongs to only names it)
            if seg.is_some() || !detail.is_empty() {
                h.segs(seg, detail, *edge);
            }
        }
        MaskShape::Landscape { class } => {
            h.word(13);
            h.write(class.as_bytes());
        }
    }
}
