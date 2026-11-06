//! The alpha planes of masks, handed to a frontend that tints them over the photo, and the signature that says whether the
//! planes it already holds are still the right ones.

use std::hash::{Hash, Hasher};

use lightcraft_develop::Mask;
use lightcraft_raster::Plane;

use super::{AlphaKeys, alpha_key};
use crate::Plan;

/// Which masks' planes are wanted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MaskWant {
    /// The mask with this [`Mask::id`].
    One(u32),
    /// Every visible mask that has components.
    All,
}

/// What a frontend asks for: the planes of `want`, unless they are the ones `known` (a signature from an earlier answer)
/// names, which it still holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanesAsk {
    pub want: MaskWant,
    pub known: Option<u64>,
}

/// The answer: the planes' signature and, unless the asker already holds them, the planes (each with its Mask's Amount applied,
/// cut to the request's window, in the masks' order).
#[derive(Debug)]
pub struct PlanesOut {
    pub signature: u64,
    pub planes: Option<Vec<(u32, Plane)>>,
}

/// The masks of `settings` that `want` names and a render evaluates: visible, with components.
pub fn wanted(masks: &[Mask], want: MaskWant) -> impl Iterator<Item = &Mask> {
    masks.iter().filter(move |m| {
        let named = match want {
            MaskWant::All => true,
            MaskWant::One(id) => id == m.id,
        };
        named && m.visible && !m.components.is_empty()
    })
}

/// What the planes of `want` in the render planned by `plan` are made from: each mask's alpha key and Amount, the size and
/// window of the planes, and the picture (`source`: its address, which a source keeps for its life). The same signature means
/// the same planes.
pub fn planes_signature(plan: &Plan<'_>, source: usize, want: MaskWant) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    let mut h = BuildHasherDefault::<DefaultHasher>::default().build_hasher();
    let keys = AlphaKeys { geo: plan.geo, image: plan.lin_key };
    let ev = plan.settings.light.exposure as f32;
    (want, source, plan.w, plan.h).hash(&mut h);
    plan.keep.map(|k| (k.x, k.y, k.w, k.h)).hash(&mut h);
    for m in wanted(&plan.settings.masks, want) {
        (m.id, alpha_key(m, keys, ev), m.adjust.amount.to_bits()).hash(&mut h);
    }
    h.finish()
}
