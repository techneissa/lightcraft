//! Faces up close: each face is cut out upright (turned by its eye line), parsed into 19 face classes and given 478
//! mesh points; the classes become the person's face part planes, with the irises from the mesh and the teeth from the
//! mouth's bright, colourless pixels.

use candle_core::{Device, Tensor};

use crate::Result;
use crate::bisenet::BiSeNet;
use crate::eyes::{MeshEyes, smoothstep};
use crate::facemesh::{FaceMesh, MeshOut};
use crate::image::{Crop, RgbImage, imagenet, sample_grid};
use crate::models::FaceModel;
use crate::segformer::Segformer;
use crate::types::{Landmark, NormRect, Part, Plane};

/// The face parser's input side.
pub const SIDE: usize = 512;
/// The crop is this many face-box sizes across (CelebAMask-HQ's framing: face, hair and some shoulder).
const PARSE_CROP: f32 = 2.2;
/// The mesh's crop, as MediaPipe frames it from a detection box.
const MESH_CROP: f32 = 1.6;
/// The share of the crop's side over which face parts fade out toward its border.
const BORDER_FADE: f32 = 0.06;
/// A face plane has at most this many pixels across.
const MAX_PLANE: usize = 640;

/// The face parser of a set.
pub enum FaceNet {
    Segformer(Segformer),
    BiSeNet(Box<BiSeNet>),
}

/// Face groups, internal: what the classes add up to before the eyes and mouth are split.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum G {
    Person,
    Skin,
    Neck,
    Hair,
    Brows,
    Eyes,
    Lips,
    Mouth,
    Hat,
    Others,
}

const GROUPS: [G; 10] = [G::Person, G::Skin, G::Neck, G::Hair, G::Brows, G::Eyes, G::Lips, G::Mouth, G::Hat, G::Others];

/// The CelebAMask-HQ class names in each model's order.
fn names(model: FaceModel) -> [&'static str; 19] {
    match model {
        FaceModel::Segformer => [
            "background",
            "hair",
            "hat",
            "ear_r",
            "skin",
            "l_brow",
            "r_brow",
            "l_eye",
            "r_eye",
            "eye_g",
            "nose",
            "u_lip",
            "l_lip",
            "mouth",
            "cloth",
            "neck",
            "neck_l",
            "l_ear",
            "r_ear",
        ],
        FaceModel::BiSeNet => [
            "background",
            "skin",
            "l_brow",
            "r_brow",
            "l_eye",
            "r_eye",
            "eye_g",
            "l_ear",
            "r_ear",
            "ear_r",
            "nose",
            "mouth",
            "u_lip",
            "l_lip",
            "neck",
            "neck_l",
            "cloth",
            "hair",
            "hat",
        ],
    }
}

fn group_of(name: &str) -> Option<G> {
    Some(match name {
        "skin" | "nose" | "l_ear" | "r_ear" => G::Skin,
        "neck" => G::Neck,
        "hair" => G::Hair,
        "l_brow" | "r_brow" => G::Brows,
        "l_eye" | "r_eye" => G::Eyes,
        "u_lip" | "l_lip" => G::Lips,
        "mouth" => G::Mouth,
        "hat" => G::Hat,
        "eye_g" | "ear_r" | "neck_l" => G::Others,
        _ => return None,
    })
}

/// One face to parse.
pub struct Job {
    /// Person index (0-based).
    pub person: usize,
    pub rect: NormRect,
    /// Eye line angle in photo pixels (radians), from the hint's landmarks.
    pub angle: f32,
}

impl Job {
    fn crop(&self, img: &RgbImage, factor: f32, side: usize) -> Crop {
        let (w, h) = (img.width as f32, img.height as f32);
        let size = (self.rect.width() * w).max(self.rect.height() * h).max(1.0);
        let (cx, cy) = self.rect.centre();
        // the box centre is near the nose; the parser's framing has the eyes a little above the middle
        let up = if factor == PARSE_CROP { 0.08 * size } else { 0.0 };
        let (s, c) = self.angle.sin_cos();
        Crop { cx: cx * w + s * up, cy: cy * h - c * up, scale: size * factor / side as f32, angle: self.angle, side }
    }
}

impl Job {
    /// The face mesh's crop of this face.
    pub fn mesh_crop(&self, img: &RgbImage) -> Crop {
        self.crop(img, MESH_CROP, crate::facemesh::SIDE)
    }
}

/// The mesh's points in fractions of the photo (none when the mesh was not sure it saw a face).
pub fn landmarks_of(m: &MeshOut, crop: &Crop, img: &RgbImage) -> Vec<Landmark> {
    // (`!(presence >= 0.5)`: a NaN presence drops the points too)
    if m.presence.is_nan() || m.presence < 0.5 {
        return Vec::new();
    }
    let (w, h) = (img.width as f32, img.height as f32);
    m.points
        .iter()
        .map(|q| {
            let (x, y) = crop.to_photo(q[0], q[1]);
            Landmark { x: x / w, y: y / h, z: q[2] * crop.scale / w }
        })
        .collect()
}

/// What one face gave.
pub struct FaceOut {
    pub person: usize,
    pub planes: Vec<(Part, Plane)>,
    pub landmarks: Vec<Landmark>,
    pub landmark_score: f32,
}

/// The crops' pixels, `crops.len() × 3 × side × side` (ImageNet-normalised when `normalise`).
pub fn batch_pixels(img: &RgbImage, crops: &[Crop], normalise: bool) -> Vec<f32> {
    let mut all = Vec::new();
    let side = crops.first().map_or(1, |c| c.side);
    for c in crops {
        let mut px = sample_grid(img, side, side, c.scale, [0.485, 0.456, 0.406], |u, v| c.to_photo(u, v));
        if normalise {
            imagenet(&mut px, side * side);
        }
        all.extend(px);
    }
    all
}

fn batch_tensor(img: &RgbImage, crops: &[Crop], normalise: bool, dev: &Device) -> Result<Tensor> {
    let side = crops.first().map_or(1, |c| c.side);
    Ok(Tensor::from_vec(batch_pixels(img, crops, normalise), (crops.len(), 3, side, side), dev)?)
}

/// The face mesh of `crops` with candle, in one call (each face of a call gives what it gives alone: `tests/models.rs`).
pub fn mesh_batch(mesh: &FaceMesh, img: &RgbImage, crops: &[Crop], dev: &Device) -> Result<Vec<MeshOut>> {
    mesh.forward(&batch_tensor(img, crops, false, dev)?)
}

/// Class probabilities at [`SIDE`]², per crop, summed into [`GROUPS`] (in that order): what a face parser gives.
pub type Groups = Vec<Vec<Vec<f32>>>;

/// The number of face groups a parser gives per crop.
pub const GROUP_COUNT: usize = GROUPS.len();

/// Parse a batch of faces with candle: class probabilities at [`SIDE`]², per crop, grouped.
pub fn parse_batch(net: &FaceNet, model: FaceModel, img: &RgbImage, crops: &[Crop], dev: &Device) -> Result<Groups> {
    let x = batch_tensor(img, crops, true, dev)?;
    let logits = match net {
        FaceNet::Segformer(m) => m.forward(&x)?,
        FaceNet::BiSeNet(m) => m.forward(&x)?,
    };
    let probs = candle_nn::ops::softmax(&crate::nn::resize(&logits, SIDE, SIDE, false)?, 1)?.to_dtype(candle_core::DType::F32)?;
    let n = SIDE * SIDE;
    let names = names(model);
    let mut out = Vec::with_capacity(crops.len());
    for b in 0..crops.len() {
        let all = probs.get(b)?.flatten_all()?.to_vec1::<f32>()?;
        let mut groups = vec![vec![0.0f32; n]; GROUPS.len()];
        for (c, name) in names.iter().enumerate() {
            let Some(src) = all.get(c * n..(c + 1) * n) else { continue };
            let mut add = |g: G| {
                if let Some(dst) = GROUPS.iter().position(|&x| x == g).and_then(|k| groups.get_mut(k)) {
                    for (d, s) in dst.iter_mut().zip(src) {
                        *d += s;
                    }
                }
            };
            if *name != "background" {
                add(G::Person);
            }
            if let Some(g) = group_of(name) {
                add(g);
            }
        }
        out.push(groups);
    }
    Ok(out)
}

/// Turn one face's grouped probabilities into planes over the photo (axis-aligned, clamped to the photo).
fn planes(img: &RgbImage, crop: &Crop, groups: &[Vec<f32>], mesh: &MeshEyes, scene: &Scene, person: usize) -> Vec<(Part, Plane)> {
    let keep = |x: f32, y: f32| (scene.claims)(person, x, y);
    let (iw, ih) = (img.width as f32, img.height as f32);
    let (bx0, by0, bx1, by1) = crop.photo_bounds();
    let Some(rect) = NormRect::new(bx0 / iw, by0 / ih, bx1 / iw, by1 / ih).clamped() else { return Vec::new() };
    let res = (SIDE as f32 / (crop.side as f32 * crop.scale)).min(4.0);
    let pw = ((rect.width() * iw * res).round() as usize).clamp(1, MAX_PLANE);
    let ph = ((rect.height() * ih * res).round() as usize).clamp(1, MAX_PLANE);
    let n = pw * ph;
    let g = |k: G| GROUPS.iter().position(|&x| x == k).and_then(|i| groups.get(i)).map_or(&[][..], Vec::as_slice);
    let mut out: Vec<Vec<f32>> = vec![vec![0.0; n]; GROUPS.len()];
    let mut photo = vec![(0.0f32, 0.0f32); n];
    let mut fade = vec![0.0f32; n];
    for y in 0..ph {
        for x in 0..pw {
            let px = (rect.x0 + (x as f32 + 0.5) / pw as f32 * rect.width()) * iw;
            let py = (rect.y0 + (y as f32 + 0.5) / ph as f32 * rect.height()) * ih;
            let i = y * pw + x;
            if let Some(p) = photo.get_mut(i) {
                *p = (px, py);
            }
            let (u, v) = crop.to_crop(px, py);
            if u < 0.0 || v < 0.0 || u > SIDE as f32 || v > SIDE as f32 || !keep(px / iw, py / ih) {
                continue;
            }
            // fade out toward the crop's border, so a part the crop cuts through ends softly
            if let Some(f) = fade.get_mut(i) {
                *f = smoothstep(0.0, BORDER_FADE * SIDE as f32, u.min(v).min(SIDE as f32 - u).min(SIDE as f32 - v));
            }
            for (k, src) in GROUPS.iter().map(|&k| g(k)).enumerate() {
                if let Some(d) = out.get_mut(k).and_then(|o| o.get_mut(i)) {
                    *d = crate::image::bilinear(src, SIDE, SIDE, u - 0.5, v - 0.5).clamp(0.0, 1.0);
                }
            }
        }
    }
    let at = |k: G| {
        GROUPS
            .iter()
            .position(|&x| x == k)
            .and_then(|i| out.get(i))
            .map(|v| v.iter().zip(&fade).map(|(&p, &f)| crate::eyes::sharpen(p) * f).collect::<Vec<f32>>())
            .unwrap_or_default()
    };
    let luma = |i: usize| photo.get(i).map_or(0.0, |&(x, y)| img.luma(x as usize, y as usize));
    let sat = |i: usize| {
        photo.get(i).and_then(|&(x, y)| img.sample(x, y)).map_or(0.0, |c| {
            let (mx, mn) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
            if mx > 1e-3 { (mx - mn) / mx } else { 0.0 }
        })
    };
    let skin = at(G::Skin);
    let mut skin_l: Vec<f32> = skin.iter().enumerate().filter(|(_, v)| **v > 0.5).map(|(i, _)| luma(i)).collect();
    skin_l.sort_by(f32::total_cmp);
    let skin_luma = skin_l.get(skin_l.len() / 2).copied().unwrap_or(0.5).max(0.05);
    // eyes: cut to the mesh's eye openings and split by its iris discs; without a mesh, the parser's eyes, and the
    // darker part of them is the iris
    let parser_eyes = at(G::Eyes);
    let eyes: Vec<f32> = if mesh.has_openings() { photo.iter().map(|&(x, y)| mesh.opening(x, y)).collect() } else { parser_eyes };
    let eye_l: Vec<f32> = eyes.iter().enumerate().filter(|(_, v)| **v > 0.5).map(|(i, _)| luma(i)).collect();
    let eye_mean = eye_l.iter().sum::<f32>() / eye_l.len().max(1) as f32;
    let iris_w = |i: usize| -> f32 {
        let Some(&(x, y)) = photo.get(i) else { return 0.0 };
        mesh.iris(x, y).unwrap_or_else(|| smoothstep(eye_mean * 0.95, eye_mean * 0.7, luma(i)))
    };
    let iris_p: Vec<f32> = eyes.iter().enumerate().map(|(i, e)| e * iris_w(i)).collect();
    let whites: Vec<f32> = eyes.iter().zip(&iris_p).map(|(e, i)| (e - i).max(0.0)).collect();
    let teeth: Vec<f32> =
        at(G::Mouth).iter().enumerate().map(|(i, m)| m * smoothstep(0.7, 0.95, luma(i) / skin_luma) * smoothstep(0.5, 0.25, sat(i))).collect();
    let mut body_skin = at(G::Neck);
    for (b, s) in body_skin.iter_mut().zip(&skin) {
        *b = b.min(1.0 - s);
    }
    // the face parser calls short dark hair a hat now and then: its hat stands only where the body model sees one too
    let face_hat = at(G::Hat);
    let hat: Vec<f32> = face_hat.iter().zip(&photo).map(|(h, &(x, y))| h * smoothstep(0.15, 0.4, (scene.body_hat)(x / iw, y / ih))).collect();
    let hair: Vec<f32> = at(G::Hair).iter().zip(face_hat.iter().zip(&hat)).map(|(r, (f, h))| (r + f - h).min(1.0)).collect();
    let plane = |data: Vec<f32>| Plane { rect, width: pw, height: ph, data, split: false };
    vec![
        (Part::EntirePerson, plane(at(G::Person))),
        (Part::FacialSkin, plane(skin)),
        (Part::BodySkin, plane(body_skin)),
        (Part::Hair, plane(hair)),
        (Part::Eyebrows, plane(at(G::Brows))),
        (Part::EyeWhites, plane(whites)),
        (Part::Iris, plane(iris_p)),
        (Part::Lips, plane(at(G::Lips))),
        (Part::Teeth, plane(teeth)),
        (Part::Hat, plane(hat)),
        (Part::Others, plane(at(G::Others))),
    ]
}

/// The face models of a set, each candle or Core ML: the parser (crops in, [`Groups`] out) and the mesh.
pub struct Nets<'a> {
    pub parse: &'a dyn Fn(&RgbImage, &[Crop]) -> Result<Groups>,
    pub mesh: &'a dyn Fn(&RgbImage, &[Crop]) -> Result<Vec<MeshOut>>,
}

/// What the whole-photo body map tells the face parts, at points in fractions of the photo.
pub struct Scene<'a> {
    /// Whether person `p` may claim a point (another person's body is not theirs).
    pub claims: &'a dyn Fn(usize, f32, f32) -> bool,
    /// The body model's hat probability.
    pub body_hat: &'a dyn Fn(f32, f32) -> f32,
}

/// Parse `jobs` in batches of `batch`: face classes, mesh points and planes. `cancelled` is asked between batches.
pub fn run(nets: &Nets, img: &RgbImage, jobs: &[Job], batch: usize, scene: &Scene, cancelled: &dyn Fn() -> bool) -> Result<Vec<FaceOut>> {
    let Nets { parse, mesh } = *nets;
    let mut out = Vec::with_capacity(jobs.len());
    for chunk in jobs.chunks(batch.max(1)) {
        if cancelled() {
            return Err(crate::Error::Cancelled);
        }
        let parse_crops: Vec<Crop> = chunk.iter().map(|j| j.crop(img, PARSE_CROP, SIDE)).collect();
        let mesh_crops: Vec<Crop> = chunk.iter().map(|j| j.mesh_crop(img)).collect();
        crate::profile::mark("face crops");
        let groups = parse(img, &parse_crops)?;
        crate::profile::mark("face parser");
        let meshes = mesh(img, &mesh_crops)?;
        crate::profile::mark("face mesh");
        for (((job, pc), mc), (g, m)) in chunk.iter().zip(&parse_crops).zip(&mesh_crops).zip(groups.iter().zip(&meshes)) {
            let eyes = MeshEyes::from_mesh(m, mc);
            let p = job.person;
            let planes = planes(img, pc, g, &eyes, scene, p);
            out.push(FaceOut { person: p, planes, landmarks: landmarks_of(m, mc, img), landmark_score: m.presence });
        }
        crate::profile::mark("face planes");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mesh the network is not sure of (low or NaN presence) gives no points; a sure one gives them.
    #[test]
    fn landmarks_need_a_presence_of_at_least_half() {
        let img = RgbImage { width: 4, height: 4, data: vec![0; 48] };
        let crop = Crop { cx: 2.0, cy: 2.0, scale: 1.0, angle: 0.0, side: 4 };
        let out = |presence: f32| landmarks_of(&MeshOut { points: vec![[1.0, 1.0, 0.0]; 3], presence }, &crop, &img).len();
        assert_eq!((out(f32::NAN), out(0.2), out(0.5), out(0.9)), (0, 0, 3, 3));
    }
}
