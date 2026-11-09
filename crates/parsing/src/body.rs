//! The body model's view: the whole photo through the SegFormer clothes parser (ATR's 18 classes), as class
//! probabilities, and each person's body part planes cut from them.

use candle_core::{Device, Tensor};

use crate::image::{RgbImage, imagenet, sample_grid};
use crate::people::Owners;
use crate::segformer::Segformer;
use crate::types::{NormRect, Part, Plane};
use crate::{Error, Result};

/// The clothes parser's classes.
pub const CLASSES: usize = 18;
pub const BACKGROUND: usize = 0;
pub const FACE: usize = 11;
pub const HAT: usize = 1;

/// Which classes make each part (ATR order: 0 background, 1 hat, 2 hair, 3 sunglasses, 4 upper clothes, 5 skirt,
/// 6 pants, 7 dress, 8 belt, 9–10 shoes, 11 face, 12–13 legs, 14–15 arms, 16 bag, 17 scarf).
const GROUPS: [(Part, &[usize]); 9] = [
    (Part::Hat, &[HAT]),
    (Part::Hair, &[2]),
    (Part::ClothesTop, &[4]),
    (Part::ClothesBottom, &[5, 6]),
    (Part::Dress, &[7]),
    (Part::Shoes, &[9, 10]),
    (Part::BodySkin, &[12, 13, 14, 15]),
    (Part::Bag, &[16]),
    (Part::Others, &[3, 8, 17]),
];

/// Class probabilities over a `w × h` grid that covers the whole photo (class-major).
pub struct BodyMap {
    pub w: usize,
    pub h: usize,
    pub probs: Vec<f32>,
}

impl BodyMap {
    pub fn class(&self, c: usize) -> &[f32] {
        let n = self.w * self.h;
        self.probs.get(c * n..(c + 1) * n).unwrap_or(&[])
    }

    /// The chance each pixel is a person.
    pub fn person(&self) -> Vec<f32> {
        self.class(BACKGROUND).iter().map(|b| (1.0 - b).clamp(0.0, 1.0)).collect()
    }

    /// The pixels that are hair, a face, an arm or a leg (more likely than not).
    pub fn skin_or_hair(&self) -> Vec<bool> {
        let n = self.w * self.h;
        (0..n).map(|i| [2usize, FACE, 12, 13, 14, 15].iter().map(|&c| self.probs.get(c * n + i).copied().unwrap_or(0.0)).sum::<f32>() > 0.5).collect()
    }

    /// Class `c`'s probability at a point of the photo (fractions), bilinear.
    pub fn at(&self, c: usize, x: f32, y: f32) -> f32 {
        crate::image::bilinear(self.class(c), self.w, self.h, x * self.w as f32 - 0.5, y * self.h as f32 - 0.5)
    }

    /// The owner at a point of the photo (fractions), nearest pixel.
    pub fn owner_at(&self, owners: &Owners, x: f32, y: f32) -> u16 {
        if !(0.0..1.0).contains(&x) || !(0.0..1.0).contains(&y) {
            return 0;
        }
        let (i, j) = ((x * self.w as f32) as usize, (y * self.h as f32) as usize);
        owners.owner.get(j.min(self.h.saturating_sub(1)) * self.w + i.min(self.w.saturating_sub(1))).copied().unwrap_or(0)
    }
}

/// The grid the photo is resampled to: the long side `side` (a multiple of 32), the short side in proportion.
pub fn grid(width: usize, height: usize, side: usize) -> (usize, usize) {
    let side = (side / 32).max(1) * 32;
    let round = |v: f32| ((v / 32.0).round() as usize).max(1) * 32;
    if width >= height {
        (side, round(side as f32 * height as f32 / width.max(1) as f32))
    } else {
        (round(side as f32 * width as f32 / height.max(1) as f32), side)
    }
}

/// The photo resampled to the body model's grid for a long side of `side`, ImageNet-normalised: (width, height, three
/// planes).
pub fn pixels(img: &RgbImage, side: usize) -> (usize, usize, Vec<f32>) {
    let (w, h) = grid(img.width, img.height, side);
    let (sx, sy) = (img.width as f32 / w as f32, img.height as f32 / h as f32);
    let mut px = sample_grid(img, w, h, sx.max(sy), [0.485, 0.456, 0.406], |u, v| (u * sx, v * sy));
    imagenet(&mut px, w * h);
    (w, h, px)
}

/// Run the clothes parser (candle) on [`pixels`]' output.
pub fn run(model: &Segformer, (w, h, px): (usize, usize, Vec<f32>), dev: &Device) -> Result<BodyMap> {
    if model.classes() != CLASSES {
        return Err(Error::Model(format!("the body model has {} classes, not the clothes parser's {CLASSES}", model.classes())));
    }
    let x = Tensor::from_vec(px, (1, 3, h, w), dev)?;
    let logits = model.forward(&x)?;
    let up = crate::nn::resize(&logits, h, w, false)?;
    let probs = candle_nn::ops::softmax(&up, 1)?.flatten_all()?.to_dtype(candle_core::DType::F32)?.to_vec1::<f32>()?;
    Ok(BodyMap { w, h, probs })
}

/// Person `k`'s (0-based) planes from the whole-photo body map: Entire Person and the body parts, cut to the pixels
/// the person owns. `facial_skin`: also the body model's face class as Facial Skin (for a person whose face was not
/// parsed).
pub fn planes(map: &BodyMap, owners: &Owners, k: usize, facial_skin: bool) -> Vec<(Part, Plane)> {
    let Ok(me) = u16::try_from(k + 1) else { return Vec::new() };
    parts_from(map, NormRect::new(0.0, 0.0, 1.0, 1.0), &|i| owners.owner.get(i) == Some(&me), facial_skin)
}

/// The parts of a map that covers `rect` of the photo, cut to its pixels where `mine(i)`.
fn parts_from(map: &BodyMap, rect: NormRect, mine: &dyn Fn(usize) -> bool, facial_skin: bool) -> Vec<(Part, Plane)> {
    let (w, h) = (map.w, map.h);
    let mut bx = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if mine(y * w + x) {
                bx = (bx.0.min(x), bx.1.min(y), bx.2.max(x + 1), bx.3.max(y + 1));
            }
        }
    }
    if bx.0 >= bx.2 || bx.1 >= bx.3 {
        return Vec::new();
    }
    let (x0, y0, x1, y1) = bx;
    let (pw, ph) = (x1 - x0, y1 - y0);
    let fx = |x: usize| rect.x0 + x as f32 / w as f32 * rect.width();
    let fy = |y: usize| rect.y0 + y as f32 / h as f32 * rect.height();
    let plane_rect = NormRect::new(fx(x0), fy(y0), fx(x1), fy(y1));
    let cut = |values: &dyn Fn(usize) -> f32| -> Plane {
        let mut data = vec![0.0f32; pw * ph];
        for y in 0..ph {
            for x in 0..pw {
                let i = (y0 + y) * w + x0 + x;
                if mine(i)
                    && let Some(d) = data.get_mut(y * pw + x)
                {
                    *d = crate::eyes::sharpen(values(i));
                }
            }
        }
        Plane { rect: plane_rect, width: pw, height: ph, data, split: true }
    };
    let n = w * h;
    let sum = |classes: &[usize], i: usize| classes.iter().map(|&c| map.probs.get(c * n + i).copied().unwrap_or(0.0)).sum::<f32>();
    let mut out = vec![(Part::EntirePerson, cut(&|i| 1.0 - sum(&[BACKGROUND], i)))];
    for (part, classes) in GROUPS {
        out.push((part, cut(&|i| sum(classes, i))));
    }
    if facial_skin {
        out.push((Part::FacialSkin, cut(&|i| sum(&[FACE], i))));
    }
    out
}

/// Faces the body model sees (its face class), for a parse given no face boxes: each piece of face larger than
/// `min_px` body-map pixels across, as a box in fractions of the photo.
pub fn faces(map: &BodyMap, min_px: usize) -> Vec<NormRect> {
    let (w, h) = (map.w, map.h);
    let face = map.class(FACE);
    let mask: Vec<bool> = face.iter().map(|&p| p > 0.5).collect();
    let (label, sizes) = crate::people::components(&mask, w, h);
    let mut boxes: Vec<(usize, usize, usize, usize)> = vec![(usize::MAX, usize::MAX, 0, 0); sizes.len()];
    for (i, &l) in label.iter().enumerate() {
        if let Some(b) = boxes.get_mut(l as usize)
            && l != 0
        {
            let (x, y) = (i % w, i / w);
            *b = (b.0.min(x), b.1.min(y), b.2.max(x + 1), b.3.max(y + 1));
        }
    }
    boxes
        .into_iter()
        .skip(1)
        .filter(|b| b.2 > b.0 && b.3 > b.1 && (b.2 - b.0).max(b.3 - b.1) >= min_px.max(2))
        .map(|(x0, y0, x1, y1)| NormRect::new(x0 as f32 / w as f32, y0 as f32 / h as f32, x1 as f32 / w as f32, y1 as f32 / h as f32))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_keeps_the_aspect_in_multiples_of_32() {
        assert_eq!(grid(6000, 4000, 768), (768, 512));
        assert_eq!(grid(4000, 6000, 768), (512, 768));
        assert_eq!(grid(1, 1, 768), (768, 768));
        assert_eq!(grid(10_000, 1, 768), (768, 32));
        assert_eq!(grid(5, 5, 0), (32, 32));
    }
}
