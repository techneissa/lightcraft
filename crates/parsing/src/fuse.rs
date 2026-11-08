//! The Sapiens2 set's body map: Sapiens2 decides where the person, skin, hair and clothes are; the SegFormer clothes
//! parser only names the garments inside Sapiens2's clothes (Top, Bottom, Dress, Hat, Bag, …), which Sapiens2 has no
//! classes for. The result is a body map in the clothes parser's 18 classes, so the rest of the parse (people, parts,
//! faces) runs as for the other sets.

use candle_core::Device;

use crate::body::{BodyMap, CLASSES as ATR};
use crate::image::{RgbImage, sample_grid};
use crate::sapiens::{self, Sapiens2, class as s};
use crate::{Error, Result};

/// The clothes parser's classes (ATR order).
mod atr {
    pub const BACKGROUND: usize = 0;
    pub const HAT: usize = 1;
    pub const HAIR: usize = 2;
    pub const SUNGLASSES: usize = 3;
    pub const UPPER: usize = 4;
    pub const SKIRT: usize = 5;
    pub const PANTS: usize = 6;
    pub const DRESS: usize = 7;
    pub const BELT: usize = 8;
    pub const LEFT_SHOE: usize = 9;
    pub const RIGHT_SHOE: usize = 10;
    pub const FACE: usize = 11;
    pub const LEFT_LEG: usize = 12;
    pub const RIGHT_LEG: usize = 13;
    pub const LEFT_ARM: usize = 14;
    pub const RIGHT_ARM: usize = 15;
    pub const BAG: usize = 16;
    pub const SCARF: usize = 17;
}

/// How much a garment class's own default counts when the clothes parser has no opinion there (it calls the pixel
/// background, as it does on dark and light clothes): its probability, plus this for the default.
const PRIOR: f32 = 0.05;

/// Where a Sapiens2 class goes: one clothes-parser class, or shared among `names` by the clothes parser's own
/// probabilities, `default` when it has none.
enum To {
    One(usize),
    Named { names: &'static [usize], default: usize },
}

/// Each Sapiens2 class's place in the clothes parser's classes.
fn route(c: usize) -> To {
    match c {
        s::BACKGROUND => To::One(atr::BACKGROUND),
        s::HAIR => To::One(atr::HAIR),
        s::FACE_NECK | s::LOWER_LIP | s::UPPER_LIP | s::LOWER_TEETH | s::UPPER_TEETH | s::TONGUE => To::One(atr::FACE),
        s::EYEGLASS => To::One(atr::SUNGLASSES),
        s::LEFT_SHOE | s::LEFT_SOCK => To::One(atr::LEFT_SHOE),
        s::RIGHT_SHOE | s::RIGHT_SOCK => To::One(atr::RIGHT_SHOE),
        s::LEFT_FOOT | s::LEFT_LOWER_LEG | s::LEFT_UPPER_LEG => To::One(atr::LEFT_LEG),
        s::RIGHT_FOOT | s::RIGHT_LOWER_LEG | s::RIGHT_UPPER_LEG => To::One(atr::RIGHT_LEG),
        s::LEFT_HAND | s::LEFT_LOWER_ARM | s::LEFT_UPPER_ARM | s::TORSO => To::One(atr::LEFT_ARM),
        s::RIGHT_HAND | s::RIGHT_LOWER_ARM | s::RIGHT_UPPER_ARM => To::One(atr::RIGHT_ARM),
        s::UPPER_CLOTHING => To::Named { names: &[atr::UPPER, atr::DRESS, atr::SCARF], default: atr::UPPER },
        s::LOWER_CLOTHING => To::Named { names: &[atr::PANTS, atr::SKIRT, atr::DRESS, atr::BELT], default: atr::PANTS },
        // Apparel: what is worn but is not a top, bottom or shoe (hats, bags, scarves, belts, gloves)
        s::APPAREL => To::Named {
            names: &[atr::HAT, atr::BAG, atr::SCARF, atr::BELT, atr::SUNGLASSES, atr::DRESS, atr::UPPER, atr::SKIRT, atr::PANTS],
            default: atr::SCARF,
        },
        // there are no other classes
        _ => To::One(atr::BACKGROUND),
    }
}

/// The whole photo's pass sees this share of the trained number of patches: it serves the people without a pass of
/// their own ([`crate::crops`]), mostly a portrait that fills the frame (the same masks, in about half the time).
const WHOLE_AREA: f32 = 0.5;

/// The input Sapiens2 sees for a `width × height` photo: [`WHOLE_AREA`] of the trained number of patches (1024 × 768),
/// in the photo's own shape, sides in multiples of the patch.
pub fn input_size(width: usize, height: usize) -> (usize, usize) {
    let area = (sapiens::TRAIN_H * sapiens::TRAIN_W) as f32 * WHOLE_AREA;
    let aspect = (width.max(1) as f32 / height.max(1) as f32).clamp(0.25, 4.0);
    let p = sapiens::PATCH as f32;
    let w = ((area * aspect).sqrt() / p).round().max(1.0) * p;
    let h = ((area / aspect).sqrt() / p).round().max(1.0) * p;
    (w as usize, h as usize)
}

/// Sapiens2's class probabilities over the whole photo, on a `w × h` grid (class-major), for the clothes parser's grid.
pub fn run(model: &Sapiens2, img: &RgbImage, w: usize, h: usize, dev: &Device) -> Result<Vec<f32>> {
    let (iw, ih) = input_size(img.width, img.height);
    let (sx, sy) = (img.width as f32 / iw as f32, img.height as f32 / ih as f32);
    let px = sample_grid(img, iw, ih, sx.max(sy), [0.485, 0.456, 0.406], |u, v| (u * sx, v * sy));
    let x = sapiens::input(px, iw, ih, dev)?;
    let logits = model.forward(&x)?;
    let down = crate::nn::resize(&logits, h, w, false)?;
    let probs = candle_nn::ops::softmax(&down, 1)?.flatten_all()?.to_vec1::<f32>()?;
    if probs.len() != sapiens::CLASSES * w * h {
        return Err(Error::Model(format!("Sapiens2 gave {} values, not {}", probs.len(), sapiens::CLASSES * w * h)));
    }
    Ok(probs)
}

/// The clothes parser's map with Sapiens2's say on where things are, as much as `presence` gives it at each cell (1
/// inside the people's boxes, 0 away from them; missing cells: 0): each Sapiens2 class's probability goes to its
/// clothes-parser class, and a garment's is shared among the garments the clothes parser sees there.
pub fn fuse(map: &BodyMap, parts: &[f32], presence: &[f32]) -> BodyMap {
    let n = map.w * map.h;
    let mut probs = vec![0.0f32; ATR * n];
    let routes: Vec<To> = (0..sapiens::CLASSES).map(route).collect();
    for i in 0..n {
        let atr_p = |c: usize| map.probs.get(c * n + i).copied().unwrap_or(0.0);
        for (c, to) in routes.iter().enumerate() {
            let p = parts.get(c * n + i).copied().unwrap_or(0.0);
            if p <= 0.0 {
                continue;
            }
            match to {
                To::One(k) => {
                    if let Some(o) = probs.get_mut(k * n + i) {
                        *o += p;
                    }
                }
                To::Named { names, default } => {
                    let weight = |k: usize| atr_p(k) + if k == *default { PRIOR } else { 0.0 };
                    let total: f32 = names.iter().map(|&k| weight(k)).sum();
                    for &k in names.iter() {
                        if let Some(o) = probs.get_mut(k * n + i) {
                            *o += p * weight(k) / total.max(f32::MIN_POSITIVE);
                        }
                    }
                }
            }
        }
        let say = presence.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        for c in 0..ATR {
            if let Some(o) = probs.get_mut(c * n + i) {
                *o = say * *o + (1.0 - say) * atr_p(c);
            }
        }
    }
    BodyMap { w: map.w, h: map.h, probs }
}

/// Logits to probabilities on the CPU (for tests).
#[cfg(test)]
fn softmax(t: &candle_core::Tensor) -> Vec<f32> {
    candle_nn::ops::softmax(t, 1).and_then(|p| p.flatten_all()).and_then(|p| p.to_vec1::<f32>()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::Tensor;

    fn one_pixel(atr_probs: &[(usize, f32)], parts: &[(usize, f32)]) -> BodyMap {
        let mut a = vec![0.0; ATR];
        for &(c, p) in atr_probs {
            a[c] = p;
        }
        let mut s = vec![0.0; sapiens::CLASSES];
        for &(c, p) in parts {
            s[c] = p;
        }
        fuse(&BodyMap { w: 1, h: 1, probs: a }, &s, &[1.0])
    }

    #[test]
    fn sapiens_decides_where_the_clothes_are_and_the_clothes_parser_names_them() {
        // a dark shirt the clothes parser calls background: Sapiens2's upper clothing makes it a top
        let m = one_pixel(&[(atr::BACKGROUND, 0.95), (atr::UPPER, 0.05)], &[(s::UPPER_CLOTHING, 0.9), (s::BACKGROUND, 0.1)]);
        assert!((m.probs[atr::BACKGROUND] - 0.1).abs() < 1e-5);
        assert!(m.probs[atr::UPPER] > 0.85, "{:?}", m.probs);
        // a dress: Sapiens2 sees upper clothing, the clothes parser a dress
        let m = one_pixel(&[(atr::DRESS, 0.8), (atr::UPPER, 0.1)], &[(s::UPPER_CLOTHING, 1.0)]);
        assert!(m.probs[atr::DRESS] > 0.8 && m.probs[atr::UPPER] < 0.2, "{:?}", m.probs);
        // a hat is apparel the clothes parser names
        let m = one_pixel(&[(atr::HAT, 0.6), (atr::HAIR, 0.4)], &[(s::APPAREL, 1.0)]);
        assert!(m.probs[atr::HAT] > 0.9);
        // apparel the clothes parser can't name is Others (scarf)
        let m = one_pixel(&[(atr::BACKGROUND, 1.0)], &[(s::APPAREL, 1.0)]);
        assert!((m.probs[atr::SCARF] - 1.0).abs() < 1e-5);
        // skin and the face go where they are, whatever the clothes parser says
        let m = one_pixel(&[(atr::UPPER, 1.0)], &[(s::TORSO, 0.5), (s::FACE_NECK, 0.3), (s::UPPER_LIP, 0.2)]);
        assert!((m.probs[atr::LEFT_ARM] - 0.5).abs() < 1e-5 && (m.probs[atr::FACE] - 0.5).abs() < 1e-5);
        assert_eq!(m.probs[atr::UPPER], 0.0);
    }

    #[test]
    fn the_fused_map_still_sums_to_one() {
        let atr: Vec<f32> = (0..ATR).map(|c| (c as f32 + 1.0) / ((ATR * (ATR + 1)) as f32 / 2.0)).collect();
        let logits = Tensor::arange(0f32, sapiens::CLASSES as f32, &Device::Cpu).unwrap().affine(0.3, 0.0).unwrap().sin().unwrap();
        let parts = softmax(&logits.reshape((1, sapiens::CLASSES, 1, 1)).unwrap());
        let m = fuse(&BodyMap { w: 1, h: 1, probs: atr.clone() }, &parts, &[0.5]);
        assert!((m.probs.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        // and a map missing its values is empty, not a panic
        let m = fuse(&BodyMap { w: 2, h: 2, probs: vec![] }, &[], &[]);
        assert_eq!(m.probs.len(), ATR * 4);
        // away from the people Sapiens2 has no say: the clothes parser's map stays
        let m = fuse(&BodyMap { w: 1, h: 1, probs: atr.clone() }, &parts, &[0.0]);
        assert_eq!(m.probs, atr);
    }

    #[test]
    fn the_input_keeps_the_photo_shape_at_about_the_trained_size() {
        assert_eq!(input_size(768, 1024), (544, 720));
        let (w, h) = input_size(6000, 4000);
        assert!(w % 16 == 0 && h % 16 == 0 && w > h);
        assert!(((w * h) as f32 / (1024.0 * 768.0 * WHOLE_AREA) - 1.0).abs() < 0.05);
        let (w, h) = input_size(1, 100_000);
        assert!(w >= 16 && h <= 4096, "{w} {h}");
        assert_eq!(input_size(0, 0), input_size(1, 1));
    }
}
