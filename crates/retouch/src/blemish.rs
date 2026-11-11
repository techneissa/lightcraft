//! Blemishes: Amount, Keep Moles and Freckles. Removes spots and acne from the skin, keeps moles and freckles when
//! asked.
//!
//! Two parts:
//!
//! - **Finding** ([`find`], [`with_spots`]): once per photo, on the picture the face's masks come from, by the
//!   provider (the engine finds them with the people parse and keeps them with the masks, [`crate::Face::spots`]).
//!   Spots are small places darker or redder than the skin around them, 0.3–3 % of the face across, inside the
//!   Facial Skin mask, not on a crease, a hair or stubble; each gets a kind (blemish, mole, freckle: by its colour,
//!   shape and darkness against the spots around it, and whether it is one of a freckle field), how much it shows
//!   and a donor (a clean patch of skin next to it, of the same texture and tone). A face whose provider did not
//!   look ([`crate::Face::spots`] `None`: the examples, the tests) is looked at on the image being retouched.
//! - **Repair** (`apply`, `heal`): each spot's tone is rebuilt from the ring of skin [`FEATHER`] radii out,
//!   following a crease or gradient through it, and its texture (pores and grain) comes from its donor patch, or
//!   without one its own finest grain; it replaces the spot out to [`CORE`] radii and fades into the skin by
//!   [`FEATHER`], weighted by the skin mask. Every spot the settings can remove is healed once, at full strength,
//!   and kept (`repair`); the Amount slider only changes the weight each spot's repair is blended in by
//!   ([`Blemish::weights`]: the most visible spots first, each fully): no detection or healing runs while it moves.
//!
//! Sizes are fractions of the face's size, so the draft, the loupe and an export repair the same spots the same way.
//! The repair reads skin only (the tone is the skin's colour weighted by the mask, and donor pixels count as far as
//! they are skin), so no colour from hair, brows or lips bleeds in: no halo. Pixels where the mask is 0 never change.

use lightcraft_raster::Rgb32f;
use serde::{Deserialize, Serialize};

use crate::input::{Face, Input, Spot, SpotKind, Spots};
use crate::{Control, Feature, Part, Scene, Scope, Spec, Stage, WholePhoto};

mod find;
mod heal;
mod repair;

pub use find::find;

/// Amount reaches the least visible spot at this share of the slider ([`Blemish::weights`]) …
pub const ORDER: f32 = 0.85;
/// … and each spot fades in over this share of it.
pub const FADE: f32 = 0.15;

/// The healed skin replaces the spot out to this many spot radii …
pub const CORE: f64 = 1.7;
/// … and fades into the skin around by this many: its tone is rebuilt from the skin on this ring ([`heal`]).
pub const FEATHER: f64 = 2.6;
/// A spot under this radius in the image's pixels is too small to see at this size: left alone.
const MIN_PX: f64 = 0.35;
/// The largest spot radius repaired, a fraction of the face's size: what [`find`] gives at most (its largest size,
/// 3 % of the face across), with room to spare. A larger one from a provider is repaired at this radius.
pub const MAX_RADIUS: f64 = 0.016;
/// The farthest a donor patch may be from its spot, in spot radii ([`find`] puts it 2.6 or 3.4 radii away); a donor
/// farther away is not used.
pub const MAX_DONOR: f64 = 3.6;
/// A donor's offset is rounded to whole pixels, so it may be this much (pixels) past [`MAX_DONOR`].
const ROUNDING: f64 = 1.5;
/// What a spot's repair reads around its centre, in spot radii: its donor ([`MAX_DONOR`]) and the donor's healed
/// area ([`FEATHER`]), plus the tone's blur (three [`heal::TEXTURE`] of the face, inside [`Blemish::REACH`]'s
/// margin) and half a radius to spare.
const READS: f64 = MAX_DONOR + FEATHER + 0.5;

/// The Blemishes settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Blemish {
    /// 0…100; 0 is off. Low values remove only the most visible spots; 100 removes every one found.
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

    /// The discs of the spots Blemishes repairs (`area::spots`).
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
        // every spot these settings can remove is healed once at full strength and kept (`repair`); Amount only
        // blends each one in by its weight
        let Some(b) = scene.mask_box(face, Part::FacialSkin) else { return };
        let removable: Vec<bool> = Blemish { amount: 100.0, keep_moles: self.keep_moles }.weights(&spots.list).iter().map(|w| *w > 0.0).collect();
        let Some(patches) = repair::patches(img, scene, face, b, &spots.list, &removable) else { return };
        patches.blend(img, &self.weights(&spots.list));
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
    /// Each spot's weight (0…1) at these settings: 0 for moles and freckles while they are kept. Amount takes the
    /// spots it removes in order of how much they show ([`Spot::strength`]: darkness and size), each one fully: a
    /// spot's turn comes at its rank among them ([`ORDER`] of the way along for the faintest), and it fades in over
    /// [`FADE`] of the slider. So 25 removes the most visible quarter or so, 50 every obvious spot, 100 every spot
    /// found, and each step shows. The Refine brush (#99) will scale these per spot.
    pub fn weights(&self, spots: &[Spot]) -> Vec<f32> {
        let a = if self.amount.is_finite() { (self.amount.clamp(0.0, 100.0) / 100.0) as f32 } else { 0.0 };
        let taken = |s: &Spot| !(self.keep_moles && s.kind != SpotKind::Blemish);
        let shows = |s: &Spot| if s.strength.is_finite() { s.strength.clamp(0.0, 1.0) } else { 0.0 };
        // each removed spot's rank from the most visible (0) to the least (1); ties share a rank
        let mut order: Vec<f32> = spots.iter().filter(|s| taken(s)).map(shows).collect();
        order.sort_by(|a, b| b.total_cmp(a));
        let n = order.len();
        spots
            .iter()
            .map(|s| {
                if !taken(s) {
                    return 0.0;
                }
                let v = shows(s);
                let ahead = order.partition_point(|o| *o > v);
                let rank = if n > 1 { ahead as f32 / (n - 1) as f32 } else { 0.0 };
                ((a - ORDER * rank) / FADE).clamp(0.0, 1.0)
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
pub(crate) struct Placed {
    pub(crate) c: (f64, f64),
    pub(crate) r: f64,
    pub(crate) offset: Option<(f64, f64)>,
}

/// Spot `s` in the image (whose mask's box is `b`); `None` when it is too small to see or its centre is outside the
/// box. Its radius is held to [`MAX_RADIUS`] and its donor to [`MAX_DONOR`] radii, so what the repair reads stays
/// within [`Blemish::REACH`] of the mask's area (zoom never changes a repair).
pub(crate) fn place(scene: &Scene<'_>, size: f64, b: [usize; 4], s: &Spot) -> Option<Placed> {
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
        .filter(|o| o.0.is_finite() && o.1.is_finite() && (o.0 * o.0 + o.1 * o.1).sqrt() <= MAX_DONOR * r + ROUNDING);
    Some(Placed { c, r, offset })
}
