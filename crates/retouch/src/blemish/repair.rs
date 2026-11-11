//! The repair of one face's spots in two parts (issue #234): every spot Amount can remove is healed once, at full
//! strength, and the patches are kept between renders ([`Patches`]); a render blends each spot's patch in by its
//! weight ([`Patches::blend`]). So a drag of Amount heals nothing again: it only blends.
//!
//! Spots whose repairs read apart from each other ([`READS`] radii) heal together in parallel; a spot near an
//! earlier one waits for it and heals from the skin as that one left it (healed fully), so overlapping repairs never
//! undo each other. The batches depend only on the spots, so a draft and an export heal the same way.

use std::sync::Arc;

use lightcraft_raster::{Plane, Rgb32f, par_rows};

use super::heal::{self, Healed, Healer};
use super::{CORE, Placed, READS, place};
use crate::input::Spot;
use crate::memo::{self, Memo};
use crate::{Part, Scene};

/// The memory the kept patches may hold (24 bytes a repaired pixel: 800 spots on a draft take about 6 MB), for every
/// face of the photo, the draft's and the loupe's.
const KEPT_BYTES: usize = 96 << 20;

static MEMO: Memo<Patches> = Memo::new(KEPT_BYTES);

/// Drop the kept patches (another photo is open).
pub(super) fn forget() {
    MEMO.clear();
}

/// Every removable spot of one face healed at full strength: per batch (in the order they heal), each spot's index
/// and its repaired pixels, with how much of each to blend in at full strength (the mask there).
pub(super) struct Patches {
    batches: Vec<Vec<(usize, Vec<Healed>)>>,
}

impl Patches {
    /// Heal the spots of `spots` that `removable` marks, at full strength, on `img` (face `face` of `scene`, its
    /// Facial Skin mask's box `b`), without the cache. `None` when the face has no size.
    pub(super) fn new(img: &Rgb32f, scene: &Scene<'_>, face: usize, b: [usize; 4], spots: &[Spot], removable: &[bool]) -> Option<Patches> {
        let size = scene.faces.get(face)?.size;
        let placed: Vec<Option<Placed>> = spots.iter().map(|s| place(scene, size, b, s)).collect();
        let near = Near::new(&placed);
        let mask = mask_over(scene, face, b);
        let h = Healer { scene, face, b, all: &placed, near: &near, mask: &mask, pores: heal::TEXTURE * size };
        let mut work = img.clone();
        let mut batches: Vec<Vec<(usize, Vec<Healed>)>> = Vec::new();
        for batch in batches_of(&placed, removable) {
            let mut patches: Vec<(usize, Vec<Healed>)> = batch.iter().map(|k| (*k, Vec::new())).collect();
            {
                let src: &Rgb32f = &work;
                par_rows(&mut patches, 1, |_, slot| {
                    if let Some((k, out)) = slot.first_mut()
                        && let Some(Some(p)) = placed.get(*k)
                    {
                        *out = h.heal(src, p, 1.0, *k);
                    }
                });
            }
            for (_, patch) in &patches {
                blend(&mut work, patch, 1.0);
            }
            batches.push(patches);
        }
        Some(Patches { batches })
    }

    fn bytes(&self) -> usize {
        self.batches.iter().flatten().map(|(_, p)| p.capacity() * std::mem::size_of::<Healed>()).sum()
    }

    /// Blend each spot's patch into `img` by its weight in `weights` (none for a weight of 0), in the order they
    /// healed.
    pub(super) fn blend(&self, img: &mut Rgb32f, weights: &[f32]) {
        for (k, patch) in self.batches.iter().flatten() {
            if let Some(w) = weights.get(*k).filter(|w| **w > 0.0) {
                blend(img, patch, *w);
            }
        }
    }
}

/// The patches for face `face`'s spots on `img` (its Facial Skin mask's box `b`), kept for these pixels, mask, place
/// and spots, or healed now.
pub(super) fn patches(img: &Rgb32f, scene: &Scene<'_>, face: usize, b: [usize; 4], spots: &[Spot], removable: &[bool]) -> Option<Arc<Patches>> {
    let size = scene.faces.get(face)?.size;
    let mut h = memo::Hash::new(0x626c_656d);
    h.u64(memo::pixels(img, b));
    h.u64(memo::mask(scene, face, Part::FacialSkin, b)?);
    h.u64(spots.len() as u64);
    for (s, r) in spots.iter().zip(removable) {
        h.u64(u64::from(*r));
        match place(scene, size, b, s) {
            Some(p) => {
                for v in [p.c.0, p.c.1, p.r] {
                    h.f64(v);
                }
                let (dx, dy) = p.offset.unwrap_or((f64::NAN, f64::NAN));
                h.f64(dx);
                h.f64(dy);
            }
            None => h.u64(u64::MAX),
        }
    }
    MEMO.get_or(h.finish(), || Patches::new(img, scene, face, b, spots, removable), Patches::bytes)
}

/// Blend `patch` into `img` by `w` (each pixel by `w` times its own share).
fn blend(img: &mut Rgb32f, patch: &[Healed], w: f32) {
    for (i, new, m) in patch {
        let k = w * m;
        if let Some(d) = img.data.get_mut(*i) {
            let v: [f32; 3] = std::array::from_fn(|c| d[c] + k * (new[c] - d[c]));
            if v.iter().all(|v| v.is_finite()) {
                *d = v;
            }
        }
    }
}

/// The removable spots in batches, in order: each spot joins the first batch whose spots all read apart from it
/// ([`READS`] radii), or starts a new one.
fn batches_of(placed: &[Option<Placed>], removable: &[bool]) -> Vec<Vec<usize>> {
    let near = Near::new(placed);
    let reach = near.most * 2.0 * READS;
    let mut of: Vec<Option<usize>> = vec![None; placed.len()];
    let mut batches: Vec<Vec<usize>> = Vec::new();
    for (k, p) in placed.iter().enumerate() {
        let Some(p) = p else { continue };
        if !removable.get(k).copied().unwrap_or(false) {
            continue;
        }
        // the batches of the earlier spots whose repairs read into this one's
        let mut taken: Vec<usize> = Vec::new();
        near.any(p.c, reach, |j| {
            if let (Some(Some(q)), Some(Some(bj))) = (placed.get(j), of.get(j))
                && ((p.c.0 - q.c.0).powi(2) + (p.c.1 - q.c.1).powi(2)).sqrt() <= (p.r + q.r) * READS
            {
                taken.push(*bj);
            }
            false
        });
        let at = (0..=batches.len()).find(|i| !taken.contains(i)).unwrap_or(batches.len());
        match batches.get_mut(at) {
            Some(b) => b.push(k),
            None => batches.push(vec![k]),
        }
        if let Some(o) = of.get_mut(k) {
            *o = Some(at);
        }
    }
    batches
}

/// The placed spots by where they are: a grid of cells as wide as the largest spot's [`CORE`] disc, so the spots
/// near a point are found without looking at every spot.
pub(super) struct Near {
    /// The largest radius.
    most: f64,
    cell: f64,
    origin: (f64, f64),
    w: usize,
    h: usize,
    /// Spot indices per cell, row by row.
    cells: Vec<Vec<usize>>,
}

/// Most cells a side (spots spread over a huge box share cells; the answers stay the same).
const MOST_CELLS: usize = 512;

impl Near {
    pub(super) fn new(placed: &[Option<Placed>]) -> Near {
        let (mut lo, mut hi, mut most) = ((f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY), 0.0f64);
        for p in placed.iter().flatten() {
            lo = (lo.0.min(p.c.0), lo.1.min(p.c.1));
            hi = (hi.0.max(p.c.0), hi.1.max(p.c.1));
            most = most.max(p.r);
        }
        if !(lo.0 <= hi.0 && lo.1 <= hi.1 && most.is_finite()) {
            return Near { most: 0.0, cell: 1.0, origin: (0.0, 0.0), w: 0, h: 0, cells: Vec::new() };
        }
        let span = (hi.0 - lo.0).max(hi.1 - lo.1);
        let cell = (most * CORE).max(1.0).max(span / MOST_CELLS as f64);
        let (w, h) = (((hi.0 - lo.0) / cell) as usize + 1, ((hi.1 - lo.1) / cell) as usize + 1);
        let mut cells = vec![Vec::new(); w * h];
        let mut near = Near { most, cell, origin: lo, w, h, cells: Vec::new() };
        for (k, p) in placed.iter().enumerate() {
            let Some(p) = p else { continue };
            let (x, y) = near.cell_of(p.c);
            if let Some(c) = cells.get_mut(y * w + x) {
                c.push(k);
            }
        }
        near.cells = cells;
        near
    }

    /// The cell holding point `q` (held to the grid).
    fn cell_of(&self, q: (f64, f64)) -> (usize, usize) {
        let at = |v: f64, o: f64, n: usize| {
            let c = ((v - o) / self.cell).floor();
            if c.is_nan() { 0 } else { c.clamp(0.0, n.saturating_sub(1) as f64) as usize }
        };
        (at(q.0, self.origin.0, self.w), at(q.1, self.origin.1, self.h))
    }

    /// Whether `f` holds for any spot whose centre lies within `reach` of `q` (`f` is asked of those and of some
    /// farther ones, so it measures the distance itself).
    pub(super) fn any(&self, q: (f64, f64), reach: f64, mut f: impl FnMut(usize) -> bool) -> bool {
        if self.w == 0 || !reach.is_finite() {
            return false;
        }
        let lo = self.cell_of((q.0 - reach, q.1 - reach));
        let hi = self.cell_of((q.0 + reach, q.1 + reach));
        (lo.1..=hi.1).any(|y| (lo.0..=hi.0).any(|x| self.cells.get(y * self.w + x).is_some_and(|c| c.iter().any(|k| f(*k)))))
    }

    /// The largest spot radius.
    pub(super) fn most(&self) -> f64 {
        self.most
    }
}

/// Face `face`'s Facial Skin mask over the box `b`, as [`Scene::mask_at`] gives it (read once for every spot).
fn mask_over(scene: &Scene<'_>, face: usize, [x0, y0, x1, y1]: [usize; 4]) -> Plane {
    let w = x1.saturating_sub(x0);
    let mut m = Plane::new(w, y1.saturating_sub(y0));
    par_rows(&mut m.data, w.max(1), |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            *v = scene.mask_at(face, Part::FacialSkin, x0 + x, y0 + y);
        }
    });
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blemish::Blemish;
    use crate::input::{Face, Input, NormRect, PartMask, SpotKind};
    use crate::{Feature, WholePhoto};

    /// A repeatable number in 0…1.
    fn hash(i: usize, salt: u64) -> f64 {
        let mut h = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        h ^= h >> 29;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 32;
        (h % 10_000) as f64 / 10_000.0
    }

    /// 120 spots, many close enough to heal one after another, some with donors, and a face of skin with pores and
    /// those spots darker, 300 px square.
    fn photo() -> (Rgb32f, Input) {
        let n = 300;
        let spots: Vec<Spot> = (0..120)
            .map(|i| {
                let at = (0.2 + 0.6 * hash(i, 1), 0.2 + 0.6 * hash(i, 2));
                let radius = 0.004 + 0.008 * hash(i, 3);
                let donor = (i % 3 != 0).then_some((at.0 + 3.0 * radius, at.1));
                let kind = [SpotKind::Blemish, SpotKind::Freckle, SpotKind::Mole][i % 3];
                Spot { at, radius, donor, kind, strength: hash(i, 4) as f32 }
            })
            .collect();
        let img = Rgb32f::from_fn(n, n, |x, y| {
            let pore = 1.0 + 0.04 * (hash(x * 1000 + y, 5) as f32 - 0.5);
            let (u, v) = ((x as f64 + 0.5) / n as f64, (y as f64 + 0.5) / n as f64);
            let dark: f64 = spots.iter().map(|s| (-((u - s.at.0).powi(2) + (v - s.at.1).powi(2)) / (s.radius * s.radius * 0.8)).exp()).sum();
            let k = pore * (1.0 - 0.3 * dark.min(1.0) as f32);
            [0.42 * k, 0.28 * k, 0.2 * k]
        });
        let disc = lightcraft_raster::Plane::from_fn(128, 128, |x, y| {
            (57.6 - ((x as f32 - 63.5).powi(2) + (y as f32 - 63.5).powi(2)).sqrt()).clamp(0.0, 1.0)
        });
        let area = NormRect::new(0.05, 0.05, 0.95, 0.95);
        let list = spots.into_iter().map(|s| Spot { at: (0.05 + 0.9 * s.at.0, 0.05 + 0.9 * s.at.1), ..s }).collect();
        let face = Face {
            masks: vec![(Part::FacialSkin, PartMask { plane: disc, area })],
            spots: Some(crate::input::Spots { list, skin: 0.5 }),
            ..Face::boxed(area)
        };
        (img, Input::new(vec![face]))
    }

    /// A drag of Amount on the kept patches gives the bits a fresh heal of the spots gives at each step, for both
    /// settings of Keep Moles and Freckles, and a changed pixel is not answered from the patches kept for the old ones.
    #[test]
    fn a_drag_on_kept_patches_equals_a_fresh_heal() {
        let (mut img, input) = photo();
        let view = WholePhoto { width: img.width, height: img.height };
        let Some(spots) = input.faces.first().and_then(|f| f.spots.as_ref()) else { panic!("the face has spots") };
        let mut before: Vec<Arc<Patches>> = Vec::new();
        for change in [false, true] {
            if change {
                // a pixel where the first spot's repair fades into the skin: its texture is in the patch
                let Some(s) = spots.list.first() else { panic!("spots") };
                let (n, size) = (img.width as f64, img.width as f64 * 0.9);
                let (x, y) = ((s.at.0 * n + 2.2 * s.radius * size) as usize, (s.at.1 * n) as usize);
                if let Some(p) = img.data.get_mut(y * img.width + x) {
                    *p = p.map(|v| v * 1.4);
                }
            }
            let scene = Scene::new(&input, &view, img.width, img.height);
            let Some(b) = scene.mask_box(0, Part::FacialSkin) else { panic!("the face maps into the photo") };
            for keep_moles in [false, true] {
                let removable: Vec<bool> = Blemish { amount: 100.0, keep_moles }.weights(&spots.list).iter().map(|w| *w > 0.0).collect();
                let Some(p) = Patches::new(&img, &scene, 0, b, &spots.list, &removable) else { panic!("healed") };
                assert!(p.batches.len() > 1, "some spots heal after others");
                for amount in [30.0, 80.0, 100.0, 55.0] {
                    let s = Blemish { amount, keep_moles };
                    let mut kept = img.clone();
                    s.apply(&mut kept, &scene, Some(0));
                    let mut fresh = img.clone();
                    p.blend(&mut fresh, &s.weights(&spots.list));
                    assert!(kept.data.iter().zip(&fresh.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)), "{s:?}, changed {change}");
                    assert!(kept.data != img.data, "{s:?} removes spots");
                }
                let (Some(a), Some(c)) = (patches(&img, &scene, 0, b, &spots.list, &removable), patches(&img, &scene, 0, b, &spots.list, &removable))
                else {
                    panic!("kept");
                };
                assert!(Arc::ptr_eq(&a, &c), "a tick reuses the patches");
                assert!(before.iter().all(|old| !Arc::ptr_eq(old, &a)), "a changed pixel makes new patches");
                before.push(a);
            }
        }
    }

    /// The spot index answers as a look at every spot does: the batches, and whether a point lies in another spot.
    #[test]
    fn the_spot_index_answers_as_a_search_of_every_spot() {
        let placed: Vec<Option<Placed>> = (0..400)
            .map(|i| (i % 17 != 0).then(|| Placed { c: (500.0 * hash(i, 6), 300.0 * hash(i, 7)), r: 1.0 + 9.0 * hash(i, 8).powi(3), offset: None }))
            .collect();
        let removable: Vec<bool> = (0..400).map(|i| i % 5 != 0).collect();
        // the greedy batches, by looking at every spot of every batch
        let mut every: Vec<Vec<usize>> = Vec::new();
        for (k, p) in placed.iter().enumerate() {
            let Some(p) = p else { continue };
            if !removable[k] {
                continue;
            }
            let apart =
                |j: &usize| placed[*j].as_ref().is_none_or(|q| ((p.c.0 - q.c.0).powi(2) + (p.c.1 - q.c.1).powi(2)).sqrt() > (p.r + q.r) * READS);
            match every.iter_mut().find(|b| b.iter().all(apart)) {
                Some(b) => b.push(k),
                None => every.push(vec![k]),
            }
        }
        assert_eq!(batches_of(&placed, &removable), every);
        let near = Near::new(&placed);
        for i in 0..2000 {
            let q = (520.0 * hash(i, 9) - 10.0, 320.0 * hash(i, 10) - 10.0);
            let each = placed.iter().flatten().any(|o| ((o.c.0 - q.0).powi(2) + (o.c.1 - q.1).powi(2)).sqrt() < o.r * CORE);
            let indexed = near.any(q, near.most() * CORE, |k| {
                placed[k].as_ref().is_some_and(|o| ((o.c.0 - q.0).powi(2) + (o.c.1 - q.1).powi(2)).sqrt() < o.r * CORE)
            });
            assert_eq!(indexed, each, "{q:?}");
        }
    }
}
