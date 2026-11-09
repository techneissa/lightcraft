//! Blemishes: Amount, Keep Moles and Freckles. Removes spots and acne from the skin, keeps moles and freckles when
//! asked.
//!
//! Two parts:
//!
//! - **Finding** ([`find`], [`with_spots`]): once per photo, on the picture the face's masks come from, by the
//!   provider (the engine finds them with the people parse and keeps them with the masks, [`crate::Face::spots`]).
//!   Spots are small places darker or redder than the skin around them, 0.3–3 % of the face across, inside the
//!   Facial Skin mask; each gets a kind (blemish, mole, freckle), a strength and a donor (a clean patch of skin next
//!   to it). A face whose provider did not look ([`crate::Face::spots`] `None`: the examples, the tests) is looked at
//!   on the image being retouched.
//! - **Repair** (`apply`, every render): each spot is filled from the skin around it (a smooth fill from a ring of
//!   skin samples, [`RING`] radii out) times the texture of its donor patch, faded in over [`CORE`]…[`FEATHER`]
//!   radii, weighted by the skin mask. The Amount slider only changes each spot's weight ([`Blemish::weights`]): no
//!   detection runs while it moves.
//!
//! Sizes are fractions of the face's size, so the draft, the loupe and an export repair the same spots the same way.
//! The fill reads skin only (every ring sample and donor pixel is weighted by the mask, so those off it count for
//! nothing), so no colour from hair, brows or lips bleeds in: no halo. Pixels where the mask is 0 never change.

use lightcraft_raster::{Rgb32f, par_rows};
use serde::{Deserialize, Serialize};

use crate::input::{Face, Input, Spot, SpotKind, Spots};
use crate::{Control, Feature, Part, Scene, Scope, Spec, Stage, WholePhoto};

mod find;

pub use find::find;

/// The fill reaches this many spot radii (the spot itself, fully replaced).
pub const CORE: f64 = 1.7;
/// … and fades out by this many radii.
pub const FEATHER: f64 = 2.6;
/// The ring of skin samples the fill comes from, in spot radii.
pub const RING: f64 = 3.0;
/// Ring samples per spot.
const RING_SAMPLES: usize = 16;
/// The donor's texture is borrowed as a ratio to its own fill, held within this factor either way (a speck of dust
/// in the donor can't print a black or white dot).
const TEXTURE_RANGE: f32 = 1.6;
/// A spot under this radius in the image's pixels is too small to see at this size: left alone.
const MIN_PX: f64 = 0.35;
/// The largest spot radius repaired, a fraction of the face's size: what [`find`] gives at most (its largest size,
/// 3 % of the face across), with room to spare. A larger one from a provider is repaired at this radius.
pub const MAX_RADIUS: f64 = 0.016;
/// The farthest a donor patch may be from its spot, in spot radii ([`find`] puts it 2.6 or 3.4 radii away); a donor
/// farther away is not used.
pub const MAX_DONOR: f64 = 3.6;
/// What a spot's repair reads around its centre, in spot radii: its donor ([`MAX_DONOR`]) and the donor's ring of
/// skin samples ([`RING`], plus their small averages, half a radius).
const READS: f64 = MAX_DONOR + RING + 0.5;

/// The Blemishes settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Blemish {
    /// 0…100; 0 is off. Low values remove only the strongest spots; 100 removes every one found.
    pub amount: f64,
    /// Leave moles and freckles as they are (on by default).
    pub keep_moles: bool,
}

impl Default for Blemish {
    fn default() -> Blemish {
        Blemish { amount: 0.0, keep_moles: true }
    }
}

/// The Keep Moles and Freckles switch as a control: 1 on (the default), 0 off.
const KEEP: Spec = Spec {
    id: "retouch.blemish.keepMoles",
    label: "Keep Moles and Freckles",
    min: 0.0,
    max: 1.0,
    default: 1.0,
    step: 1.0,
    decimals: 0,
    centered: false,
};

impl Feature for Blemish {
    const KEY: &'static str = "blemish";
    const LABEL: &'static str = "Blemishes";
    const STAGE: Stage = Stage::Retouch;
    const SCOPE: Scope = Scope::Face;
    const CONTROLS: &'static [Control<Self>] = &[
        Control { spec: Spec::amount("retouch.blemish.amount", "Amount"), get: |s| s.amount, set: |s, v| s.amount = v },
        Control { spec: KEEP, get: |s| if s.keep_moles { 1.0 } else { 0.0 }, set: |s, v| s.keep_moles = v >= 0.5 },
    ];
    /// Spots are found and repaired on the Facial Skin mask only.
    const PARTS: &'static [Part] = &[Part::FacialSkin];
    /// A spot's centre lies in the mask's box, and its repair reads [`READS`] of the largest spot's radius around it
    /// (0.11 of the face), with a few pixels to spare for small faces. (Reads off the mask count for nothing, so in
    /// practice it reads the mask's area only.)
    const REACH: f64 = READS * MAX_RADIUS + 0.03;

    /// Off while Amount is 0 (the switch alone changes nothing).
    fn is_off(&self) -> bool {
        !(self.amount.is_finite() && self.amount > 0.0)
    }

    fn area(scene: &Scene<'_>, faces: &[usize]) -> Option<lightcraft_raster::Plane> {
        crate::area::spots(scene, faces)
    }

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>) {
        let Some(face) = face else { return };
        if self.is_off() || img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let Some(f) = scene.faces.get(face) else { return };
        if f.face.mask(Part::FacialSkin).is_none() {
            return;
        }
        let looked;
        let spots = match &f.face.spots {
            Some(s) => s,
            None => {
                looked = find(img, scene, face);
                &looked
            }
        };
        let weights = self.weights(&spots.list);
        repair(img, scene, face, &spots.list, &weights);
    }
}

/// What Blemishes does to one face's spots at some settings ([`Blemish::report`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Report {
    /// Spots removed (weight above 0).
    pub removed: usize,
    /// Moles and freckles left alone by Keep Moles and Freckles.
    pub moles: usize,
    pub freckles: usize,
    /// The share of the face's skin repaired (0…1): the removed spots' cores, by their weight.
    pub area: f64,
}

impl Blemish {
    /// Each spot's weight (0…1) at these settings: 0 for moles and freckles while they are kept; else Amount, which
    /// takes the strongest spots first (at 50 a spot of strength 1 is fully removed and a faint one not at all; at
    /// 100 every spot is). The Refine brush (#99) will scale these per spot.
    pub fn weights(&self, spots: &[Spot]) -> Vec<f32> {
        let a = if self.amount.is_finite() { (self.amount.clamp(0.0, 100.0) / 100.0) as f32 } else { 0.0 };
        spots
            .iter()
            .map(|s| {
                if self.keep_moles && s.kind != SpotKind::Blemish {
                    return 0.0;
                }
                let p = if s.strength.is_finite() { s.strength.clamp(0.0, 1.0) } else { 0.0 };
                ((a - (1.0 - p) * 0.5) / 0.5).clamp(0.0, 1.0)
            })
            .collect()
    }

    /// How many spots of `spots` these settings remove and keep, and the share of the skin they repair.
    pub fn report(&self, spots: &Spots) -> Report {
        let w = if self.is_off() { vec![0.0; spots.list.len()] } else { self.weights(&spots.list) };
        let mut r = Report::default();
        for (s, w) in spots.list.iter().zip(w) {
            if w > 0.0 {
                r.removed += 1;
                r.area += std::f64::consts::PI * (s.radius * CORE).powi(2) * f64::from(w);
            } else if self.keep_moles {
                match s.kind {
                    SpotKind::Mole => r.moles += 1,
                    SpotKind::Freckle => r.freckles += 1,
                    SpotKind::Blemish => {}
                }
            }
        }
        r.area = if spots.skin > 0.0 { (r.area / spots.skin).clamp(0.0, 1.0) } else { 0.0 };
        r
    }
}

/// `faces` (normalised to `img`, the whole picture their masks were made from) with their spots found
/// ([`crate::Face::spots`]); a face without a Facial Skin mask gets an empty list.
pub fn with_spots(img: &Rgb32f, faces: Vec<Face>) -> Vec<Face> {
    let view = WholePhoto { width: img.width, height: img.height };
    let input = Input::new(faces);
    let found: Vec<(usize, Spots)> = {
        let scene = Scene::new(&input, &view, img.width, img.height);
        scene.faces.iter().enumerate().map(|(i, f)| (f.index, find(img, &scene, i))).collect()
    };
    let mut faces = input.faces;
    for f in &mut faces {
        f.spots = Some(Spots::default());
    }
    for (i, s) in found {
        if let Some(f) = faces.get_mut(i) {
            f.spots = Some(s);
        }
    }
    faces
}

/// A spot mapped into the image: centre, radius and donor offset in pixels.
struct Placed {
    c: (f64, f64),
    r: f64,
    offset: Option<(f64, f64)>,
}

/// Spot `s` in the image (whose mask's box is `b`); `None` when it is too small to see or its centre is outside the
/// box. Its radius is held to [`MAX_RADIUS`] and its donor to [`MAX_DONOR`] radii, so what the repair reads stays
/// within [`Blemish::REACH`] of the mask's area (zoom never changes a repair).
fn place(scene: &Scene<'_>, size: f64, b: [usize; 4], s: &Spot) -> Option<Placed> {
    let view = scene.view();
    let c = view.to_px(s.at.0, s.at.1)?;
    let r = s.radius.min(MAX_RADIUS) * size;
    let inside = c.0 >= b[0] as f64 && c.1 >= b[1] as f64 && c.0 <= b[2] as f64 && c.1 <= b[3] as f64;
    if !(r.is_finite() && r >= MIN_PX && inside) {
        return None;
    }
    // whole pixels: a fractional offset would blend neighbouring pixels and soften the borrowed pores
    let offset = s
        .donor
        .and_then(|d| view.to_px(d.0, d.1))
        .map(|d| ((d.0 - c.0).round(), (d.1 - c.1).round()))
        .filter(|o| o.0.is_finite() && o.1.is_finite() && (o.0 * o.0 + o.1 * o.1).sqrt() <= MAX_DONOR * r);
    Some(Placed { c, r, offset })
}

/// Repair each spot of `spots` with weight above 0, in order, in place.
fn repair(img: &mut Rgb32f, scene: &Scene<'_>, face: usize, spots: &[Spot], weights: &[f32]) {
    let Some(size) = scene.faces.get(face).map(|f| f.size) else { return };
    let Some(b) = scene.mask_box(face, Part::FacialSkin) else { return };
    let placed: Vec<Option<Placed>> = spots.iter().map(|s| place(scene, size, b, s)).collect();
    let h = Healer { scene, face, b, all: &placed };
    // each spot's repair from the image as it is, in parallel; then blended in, in order
    let mut patches: Vec<Vec<Healed>> = vec![Vec::new(); placed.len()];
    {
        let src: &Rgb32f = img;
        par_rows(&mut patches, 1, |k, slot| {
            if let (Some(Some(p)), Some(w), Some(out)) = (placed.get(k), weights.get(k), slot.first_mut())
                && *w > 0.0
            {
                *out = h.heal(src, p, *w, k);
            }
        });
    }
    for (i, new, k) in patches.into_iter().flatten() {
        if let Some(d) = img.data.get_mut(i) {
            let v: [f32; 3] = std::array::from_fn(|c| d[c] + k * (new[c] - d[c]));
            if v.iter().all(|v| v.is_finite()) {
                *d = v;
            }
        }
    }
}

/// One repaired pixel: its index, its repaired colour and how much of it to blend in.
type Healed = (usize, [f32; 3], f32);

/// What healing one face's spots reads besides the image.
struct Healer<'a> {
    scene: &'a Scene<'a>,
    face: usize,
    /// The mask's box: nothing outside it is written.
    b: [usize; 4],
    /// Every spot, placed (a ring sample inside another spot counts for nothing).
    all: &'a [Option<Placed>],
}

/// A ring sample: the small average, its weight and its place.
type Sample = ([f32; 3], f32, (f64, f64));

/// Bilinear sample of `img` at pixel position (`x`, `y`) (centres at +0.5); `None` outside it.
fn sample(img: &Rgb32f, x: f64, y: f64) -> Option<[f32; 3]> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || !x.is_finite() || !y.is_finite() {
        return None;
    }
    let (fx, fy) = (x - 0.5, y - 0.5);
    if fx < -0.5 || fy < -0.5 || fx > w as f64 - 0.5 || fy > h as f64 - 0.5 {
        return None;
    }
    let (fx, fy) = (fx.clamp(0.0, (w - 1) as f64), fy.clamp(0.0, (h - 1) as f64));
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let px = |x: usize, y: usize| img.data.get(y * w + x).copied().unwrap_or_default();
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    Some(std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * tx;
        let bottom = c[i] + (d[i] - c[i]) * tx;
        top + (bottom - top) * ty
    }))
}

impl Healer<'_> {
    /// The skin mask at pixel position (`x`, `y`).
    fn mask(&self, x: f64, y: f64) -> f32 {
        if !(x >= 0.0 && y >= 0.0) {
            return 0.0;
        }
        self.scene.mask_at(self.face, Part::FacialSkin, x as usize, y as usize)
    }

    /// One ring of skin samples, radius `r`, around `c`: each a small average (radius `small`) weighted by the skin
    /// mask at each of its points, so hair or background next to the skin never comes in; its weight the mask's mean
    /// over those points (0 inside another spot than the `me`-th, or outside the image). `me` `None`: no spot is
    /// skipped.
    fn ring(&self, img: &Rgb32f, c: (f64, f64), r: f64, small: f64, me: Option<usize>) -> [Sample; RING_SAMPLES] {
        std::array::from_fn(|i| {
            let a = i as f64 * std::f64::consts::TAU / RING_SAMPLES as f64;
            let q = (c.0 + r * a.cos(), c.1 + r * a.sin());
            let inside_other = me.is_some_and(|me| {
                self.all
                    .iter()
                    .enumerate()
                    .any(|(k, o)| k != me && o.as_ref().is_some_and(|o| ((o.c.0 - q.0).powi(2) + (o.c.1 - q.1).powi(2)).sqrt() < o.r * CORE))
            });
            if inside_other {
                return ([0.0; 3], 0.0, q);
            }
            let mut acc = [0.0f32; 3];
            let mut n = 0.0f32;
            for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0), (0.7, 0.7), (-0.7, 0.7), (0.7, -0.7), (-0.7, -0.7)] {
                let (x, y) = (q.0 + dx * small, q.1 + dy * small);
                let m = self.mask(x, y);
                if m <= 0.0 {
                    continue;
                }
                if let Some(p) = sample(img, x, y) {
                    acc = std::array::from_fn(|c| acc[c] + p[c] * m);
                    n += m;
                }
            }
            if n <= 0.0 {
                return ([0.0; 3], 0.0, q);
            }
            (acc.map(|v| v / n), n / 9.0, q)
        })
    }
}

/// The smooth fill at `p` from ring samples (inverse squared distance); `None` without weight.
fn fill(samples: &[Sample], p: (f64, f64), shift: (f64, f64)) -> Option<[f32; 3]> {
    let mut acc = [0.0f32; 3];
    let mut wsum = 0.0f32;
    for (v, m, q) in samples {
        if *m <= 0.0 {
            continue;
        }
        let d2 = ((q.0 - shift.0 - p.0).powi(2) + (q.1 - shift.1 - p.1).powi(2)) as f32;
        let w = m / (d2 + 0.25);
        acc = std::array::from_fn(|c| acc[c] + v[c] * w);
        wsum += w;
    }
    (wsum > 0.0).then(|| acc.map(|v| v / wsum))
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Healer<'_> {
    /// Heal spot `p` (the `me`-th) with weight `w`, reading and writing `img` inside the mask's box.
    fn heal(&self, img: &Rgb32f, p: &Placed, w: f32, me: usize) -> Vec<Healed> {
        let small = (p.r * 0.5).max(0.7);
        let here = self.ring(img, p.c, p.r * RING, small, Some(me));
        // enough skin around it to fill from: at least a quarter of the ring
        if here.iter().map(|s| s.1).sum::<f32>() < RING_SAMPLES as f32 * 0.25 {
            return Vec::new();
        }
        let donor = p.offset.map(|o| (o, self.ring(img, (p.c.0 + o.0, p.c.1 + o.1), p.r * RING, small, None)));
        let reach = p.r * FEATHER + 0.75;
        let [bx0, by0, bx1, by1] = self.b;
        let x0 = ((p.c.0 - reach).floor().max(bx0 as f64)) as usize;
        let y0 = ((p.c.1 - reach).floor().max(by0 as f64)) as usize;
        let x1 = ((p.c.0 + reach).ceil().max(0.0) as usize).min(bx1);
        let y1 = ((p.c.1 + reach).ceil().max(0.0) as usize).min(by1);
        let (inner, outer) = (p.r * CORE, reach);
        let mut out: Vec<Healed> = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                let q = (x as f64 + 0.5, y as f64 + 0.5);
                let d = ((q.0 - p.c.0).powi(2) + (q.1 - p.c.1).powi(2)).sqrt();
                let fade = 1.0 - smoothstep(inner, outer, d);
                if fade <= 0.0 {
                    continue;
                }
                let m = self.mask(q.0, q.1);
                let k = (fade as f32) * w * m;
                if k <= 0.0 {
                    continue;
                }
                let i = y * img.width + x;
                if i >= img.data.len() {
                    continue;
                }
                let Some(base) = fill(&here, q, (0.0, 0.0)) else { continue };
                let new: [f32; 3] = match &donor {
                    // the donor's texture counts as far as the donor pixel is skin: hair or background next to it
                    // lends nothing
                    Some((o, ring_d)) => match (sample(img, q.0 + o.0, q.1 + o.1), fill(ring_d, q, (o.0, o.1)), self.mask(q.0 + o.0, q.1 + o.1)) {
                        (Some(tex), Some(dbase), md) if md > 0.0 => std::array::from_fn(|c| {
                            let t = if dbase[c] > 1e-5 { (tex[c] / dbase[c]).clamp(1.0 / TEXTURE_RANGE, TEXTURE_RANGE) } else { 1.0 };
                            base[c] * (1.0 + md * (t - 1.0))
                        }),
                        _ => base,
                    },
                    None => base,
                };
                if new.iter().all(|v| v.is_finite()) {
                    out.push((i, new, k));
                }
            }
        }
        out
    }
}
