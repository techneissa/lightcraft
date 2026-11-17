//! What in the photo blocks light, and how deep each such thing is taken to be (issue #315, `fork/relight.md`).
//!
//! A depth map shows only the front of things. Two questions decide a shadow: is a pixel the front of a thing that
//! stands in front of something else (a person, a box, a pole, a car), or part of a surface that goes on (a floor, a
//! wall, a staircase, the depth model's ripples)? And how far behind its front does the thing reach?
//!
//! - **A thing is what has farther stuff on both sides of it** along some line through it (across, up and down, or
//!   either diagonal), within [`REACH`] of the picture's long side: its silhouette is closed on that line. A floor or a
//!   staircase recedes one way and comes nearer the other, a wall seen at an angle too, so none of them is a thing;
//!   a person in front of the steps beside them, a box on a wall, a car over the ground under it are. How sure that is
//!   grows smoothly with the depth step at the silhouette ([`GAP`]): a ripple or a step a few per cent deep is no
//!   thing, a step of a quarter of the depth is one in full, and nothing switches on at a single depth.
//! - **A thing is about as deep as it is wide:** it reaches behind its front by its narrowest width through the
//!   pixel (the shortest of those closed lines, in the scene's units at its depth), but never into what lies behind
//!   it (at most [`ROOM`] of the way to the background). A pole is as deep as it is thick, a person as deep as their
//!   body is wide, a box as deep as its side. Behind that the thing fades out over half its depth again
//!   ([`BACK_FADE`]), so a light moved out of its back changes the shadow smoothly.
//!
//! The answer depends on the photo's depth only, not on any light, and is made once per field with the reduced copy
//! the march runs on ([`Reduced`]).

use rayon::prelude::*;

use crate::field::{Field, LENS};

/// How far (a share of the picture's long side) a line is walked from a pixel to find farther stuff on each side.
const REACH: f32 = 0.5;
/// A pixel on the line is past the thing's outline where its log depth is this much farther than the pixel's own and
/// rises by [`JUMP`] over the last two steps: a step in depth, not the thing's own curve (the depth model makes a face
/// near the camera a quarter of its depth deep, rising slowly to its sides).
const OUTLINE: f32 = 0.1;
const JUMP: f32 = 0.1;
/// Past the outline, the background's depth is the farthest of this many pixels (the depth model blurs an outline
/// over a few pixels).
const BLUR: usize = 3;
/// The log-depth step from a thing to the nearer of its two backgrounds over which it becomes a thing in full: none
/// at the first number (13 %), all at the second (28 %), smoothly between.
const GAP: (f32, f32) = (0.12, 0.25);
/// How deep a thing is for its width (1: as deep as it is wide).
const DEEP_AS_WIDE: f32 = 1.0;
/// A thing reaches at most this share of the way from its front to the background behind its outline (in full), and
/// fades out by [`ROOM_END`] of it: the background itself is never inside it.
const ROOM: f32 = 0.7;
const ROOM_END: f32 = 0.9;
/// Behind its depth in full, a thing fades out over this share of that depth.
const BACK_FADE: f32 = 0.5;
/// The candidates whose sureness is within this of the surest one give the thing's depth (the narrowest of them).
const NEAR_SURE: f32 = 0.25;

/// A pixel as a blocker: `s` how sure it is the front of a thing (0 not at all .. 1), and how far behind its front the
/// thing reaches, as ratios of the front's depth: in full up to `full`, fading to nothing at `end`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Blocker {
    pub s: f32,
    pub full: f32,
    pub end: f32,
}

/// The field, reduced for the march: at the centre pixel of each `f × f` block its depth (linear and log), the photo's
/// brightness, and the pixel as a blocker. Made once per field.
#[derive(Clone, Debug)]
pub(crate) struct Reduced {
    pub f: usize,
    pub w: usize,
    pub h: usize,
    pub z: Vec<f32>,
    pub lz: Vec<f32>,
    pub gi: Vec<f32>,
    pub blk: Vec<Blocker>,
    /// The depths between which anything blocks: the nearest blocker's front and the farthest end of one (`f32::INFINITY` and
    /// 0 when nothing does).
    pub span: (f32, f32),
    /// The same for each [`TILE`] × [`TILE`] block of pixels (`tw` across), so a ray skips a block where nothing can hold it.
    pub tiles: Vec<(f32, f32)>,
    pub tw: usize,
}

/// The side of a block of [`Reduced::tiles`] (reduced pixels).
pub(crate) const TILE: usize = 4;

impl Reduced {
    pub(crate) fn of(field: &Field, low_side: usize) -> Reduced {
        let (fw, fh) = (field.width.max(1), field.height.max(1));
        let f = fw.max(fh).div_ceil(low_side.max(1)).max(1);
        let (w, h) = (fw.div_ceil(f), fh.div_ceil(f));
        let (mut z, mut lz, mut gi) = (Vec::with_capacity(w * h), Vec::with_capacity(w * h), Vec::with_capacity(w * h));
        for y in 0..h {
            let cy = (y * f + f / 2).min(fh - 1);
            for x in 0..w {
                let i = cy * fw + (x * f + f / 2).min(fw - 1);
                let l = field.ln_z.get(i).copied().unwrap_or(0.0);
                let l = if l.is_finite() { l } else { 0.0 };
                lz.push(l);
                z.push(l.exp());
                gi.push(field.gi.get(i).copied().unwrap_or(0.0));
            }
        }
        let blk = blockers(&lz, w, h);
        let span = z.iter().zip(&blk).filter(|(_, b)| b.s > 0.0).fold((f32::INFINITY, 0.0f32), |(lo, hi), (z, b)| (lo.min(*z), hi.max(z * b.end)));
        let (tw, th) = (w.div_ceil(TILE), h.div_ceil(TILE));
        let mut tiles = vec![(f32::INFINITY, 0.0f32); tw * th];
        for (i, (zf, b)) in z.iter().zip(&blk).enumerate() {
            if b.s > 0.0
                && let Some(t) = tiles.get_mut((i / w / TILE) * tw + (i % w) / TILE)
            {
                *t = (t.0.min(*zf), t.1.max(zf * b.end));
            }
        }
        Reduced { f, w, h, z, lz, gi, blk, span, tiles, tw }
    }

    /// Memory held (bytes).
    pub(crate) fn bytes(&self) -> usize {
        (self.z.len() + self.lz.len() + self.gi.len() + 2 * self.tiles.len()) * 4 + self.blk.len() * std::mem::size_of::<Blocker>()
    }
}

/// `t` (clamped to 0..1) eased at both ends.
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Every pixel of the `w × h` log depth `lz` as a blocker (see the module docs).
pub(crate) fn blockers(lz: &[f32], w: usize, h: usize) -> Vec<Blocker> {
    let long = w.max(h).max(1) as f32;
    let reach = ((REACH * long).round() as usize).max(2);
    // (a thing's depth per pixel of its width, as a ratio of its own depth: the camera's extent per unit depth over the pixels)
    let per_px = LENS / long * DEEP_AS_WIDE;
    (0..w * h).into_par_iter().map(|i| blocker(lz, w, h, i % w, i / w, reach, per_px)).collect()
}

/// Walking from `(x, y)` by `(dx, dy)`, at most `steps` steps: the first step past an outline ([`OUTLINE`], [`JUMP`]),
/// and the background's log depth there; `None` when the walk leaves the picture first.
fn outline(lz: &[f32], w: usize, h: usize, (x, y): (usize, usize), (dx, dy): (isize, isize), steps: usize, own: f32) -> Option<(usize, f32)> {
    let at = |k: usize| -> Option<f32> {
        let qx = x.checked_add_signed(dx.checked_mul(k as isize)?)?;
        let qy = y.checked_add_signed(dy.checked_mul(k as isize)?)?;
        if qx >= w || qy >= h {
            return None;
        }
        lz.get(qy * w + qx).copied()
    };
    let mut before = [own, own];
    for k in 1..=steps {
        let v = at(k)?;
        let steep = v - before[0] > JUMP;
        before = [before[1], v];
        if steep && v - own > OUTLINE {
            let bg = (k..k + BLUR).filter_map(at).fold(v, f32::max);
            return Some((k, bg));
        }
    }
    None
}

fn blocker(lz: &[f32], w: usize, h: usize, x: usize, y: usize, reach: usize, per_px: f32) -> Blocker {
    let Some(own) = lz.get(y * w + x).copied() else { return Blocker::default() };
    // (sureness, depth ratio, room ratio) for each line through the pixel that is closed at both ends
    let mut found: [(f32, f32, f32); 4] = [(0.0, 0.0, 0.0); 4];
    let mut n = 0;
    for (d, len) in [((1, 0), 1.0f32), ((0, 1), 1.0), ((1, 1), std::f32::consts::SQRT_2), ((1, -1), std::f32::consts::SQRT_2)] {
        let steps = (reach as f32 / len) as usize;
        let (Some((ka, ba)), Some((kb, bb))) = (outline(lz, w, h, (x, y), d, steps, own), outline(lz, w, h, (x, y), (-d.0, -d.1), steps, own)) else {
            continue;
        };
        let gap = ba.min(bb) - own;
        let s = smooth((gap - GAP.0) / (GAP.1 - GAP.0));
        if s <= 0.0 {
            continue;
        }
        // (the pixels between the two outlines, this one included)
        let width = (ka + kb).saturating_sub(1) as f32 * len;
        if let Some(slot) = found.get_mut(n) {
            *slot = (s, width * per_px, gap.exp() - 1.0);
            n += 1;
        }
    }
    let found = found.get(..n).unwrap_or(&[]);
    let sure = found.iter().map(|c| c.0).fold(0.0f32, f32::max);
    let Some(&(_, deep, room)) = found.iter().filter(|c| c.0 >= sure - NEAR_SURE).min_by(|a, b| a.1.total_cmp(&b.1)) else {
        return Blocker::default();
    };
    let full = deep.min(ROOM * room);
    let end = (deep * (1.0 + BACK_FADE)).min(ROOM_END * room).max(full);
    if !(full.is_finite() && end.is_finite()) {
        return Blocker::default();
    }
    Blocker { s: sure, full: 1.0 + full, end: 1.0 + end }
}
