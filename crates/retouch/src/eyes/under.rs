//! Dark Circles and Eye Bags: under one eye, on its skin, in the crescents of [`super::weights`].
//!
//! A model of the skin under the eye, worked out on a grid in the eye's own frame (cells a fixed share of the
//! eye's width, so a draft, the loupe and an export work it out alike, and a close-up costs no more than a small
//! face), then laid on the pixels as a smooth gain per channel, so the skin keeps its texture:
//! - **Where**: the crescents below the lower lid that the face mesh places ([`super::weights`]), on the Facial
//!   Skin mask (and, through glasses, on the lens). The eye's masks and the lid line are kept out by place.
//! - **Skin, not lashes**: lashes, liner and a frame's edge are kept out by their shape: a thin line much darker
//!   than the skin around it (inside a cell, against the cell's other samples; across cells, against a closing of
//!   the cells' lightness; on a pixel, against the skin's level there). The dark circle itself is broad, so it is
//!   never taken for a lash, however dark or purple it is. Pale, off-colour things (a white or silver frame, a
//!   highlight) are left out too; through glasses, so are frames far darker than any skin.
//! - **Dark Circles**: the skin's colour under the eye, blurred, is taken to the cheek's below it: its luminance
//!   lifted to about the cheek's, and its hue (blue, purple, brown) turned most of the way to the cheek's. Lifting
//!   only: no channel darkens.
//! - **Eye Bags**: the shading between a fine and a coarse scale (the bag's bulge and the crease below it) is
//!   flattened; pores and fine lines are kept.

use lightcraft_raster::Rgb32f;

use super::Area;
use super::geom::{Eye, LID_MAX, smoothstep};
use super::grid::{Grid, around, grid_blur, luma, sample};
use super::weights::{self, LID_GAP, UNDER_BOX};
use crate::{Part, PartMask, Scene};

/// Blur of the under-eye colour Dark Circles corrects, in half eye widths.
const DARK_SIGMA: f64 = 0.07;
/// Dark Circles at 100 takes the under-eye luminance to this share of the cheek's.
const LIFT_TO: f32 = 0.95;
/// Most Dark Circles lifts the luminance (a factor: 2.6 stops; a deep circle on dark skin needs 4–5×).
const MAX_LIFT: f32 = 6.0;
/// How far Dark Circles turns each channel against the luminance (a factor either way) …
const MAX_TINT: f32 = 2.5;
/// … and how much of the way to the cheek's hue it goes.
const TINT_SHARE: f32 = 0.9;
/// How a lift of `g` keeps the skin's texture: its luminance detail as a power `1 / (1 + DETAIL_K · ln g)` (a deep
/// shadow lifted 5× would otherwise show its noise and JPEG blocks 5× as strong) …
const DETAIL_K: f32 = 0.25;
/// … and its colour detail as a power `exp(−CHROMA_K · ln g)`: in a deep shadow the colour is mostly noise, so a
/// strong lift takes the colour from the skin around it, turned to the cheek's.
const CHROMA_K: f32 = 0.8;
/// A deep shadow (its luminance this share of the cheek's: wholly, then not at all) has lost its texture and colour
/// to the camera's noise and compression: as Dark Circles lifts it, it takes the skin's texture from
/// [`BORROW_BELOW`] half widths further down, where the skin is clean (as a healing brush would), and its colour
/// from the skin around it. Lighter skin (freckles, a mild circle) keeps its own …
const DEEP: (f32, f32) = (0.15, 0.35);
const BORROW_BELOW: f64 = 0.7;
/// … unless that texture is far from the skin's own there (a frame, a mole: by this much in log luminance, used
/// wholly, then not at all).
const BORROW_DETAIL: (f32, f32) = (0.3, 0.6);
/// The colour Dark Circles turns toward is the skin's around each place, from the cells at least this share of the
/// cheek's luminance (not at all, then wholly: the circle itself is left out) …
const CLEAN: (f32, f32) = (0.55, 0.85);
/// … blurred this much (in half eye widths), so a fold beside warm, lit skin takes that skin's colour, not a cooler
/// cheek's.
const HUE_SIGMA: f64 = 0.25;
/// How much of the colour turned toward is the skin's around (the rest is the cheek's).
const HUE_AROUND: f32 = 0.5;
/// Eye Bags' fine and coarse blurs, in half eye widths.
const BAG_FINE: f64 = 0.025;
const BAG_COARSE: f64 = 0.45;
/// How much of the shading Eye Bags removes at 100.
const BAG_STRENGTH: f32 = 1.0;
/// Most Eye Bags changes a pixel's luminance (a factor either way).
const BAG_RANGE: f32 = 2.2;
/// The cheek patch must have at least this much skin (in square half widths) to give Dark Circles a target.
const MIN_CHEEK: f64 = 0.02;
/// Plain Facial Skin under the eye (in square half widths) that gives the skin's colour; with less (glasses over
/// it), the skin seen through the lens gives it.
const MIN_SKIN: f64 = 0.1;
/// The cheek's and the skin's colour: the mean of the cells within this factor of their median's luminance.
const ROBUST: (f32, f32) = (0.6, 1.6);
/// A sample this share of its cell's median luminance is a lash or liner in the cell (counted not at all, then
/// wholly).
const IN_CELL: (f32, f32) = (0.45, 0.7);
/// How far a cell's lightness is closed (in cells): a dark line up to twice this wide is a lash, liner or a
/// frame's edge, not skin; a dark circle is far wider.
const CLOSE_CELLS: isize = 1;
/// A cell this share of its closed lightness is such a line (not skin at all, then wholly).
const CELL_LINE: (f32, f32) = (0.4, 0.6);
/// A pixel on a line this share of the skin's level around it is a lash, liner or frame: left as it is (then wholly
/// changed). A line, not a speck: the darkest mean along four directions through the pixel ([`LINE_STEP`] apart).
const PIXEL_LINE: (f32, f32) = (0.3, 0.55);
/// The spacing of a line's samples, in half eye widths (five samples: a lash is that long and much thinner).
const LINE_STEP: f64 = 0.01;
/// Lower lashes reach this far below the lid (in half widths, then not at all): below it, a dark line on Facial
/// Skin is a crease of the circle and is lifted with it. Through glasses a frame is kept out everywhere.
const LASH_ZONE: (f64, f64) = (0.3, 0.5);
/// Pale and off-hue (a white or silver frame, a highlight): this much lighter than the skin …
const PALE: (f64, f64) = (1.25, 1.6);
/// … and this far from its chromaticity.
const PALE_HUE: (f64, f64) = (0.04, 0.08);
/// Through glasses, how far from the skin's chromaticity a pixel still counts as skin (wholly, then not at all) …
const SEEN_HUE: (f64, f64) = (0.12, 0.2);
/// … and how dark it may be against the skin (a frame is darker: not skin at all, then wholly).
const FRAME_DARK: (f64, f64) = (0.1, 0.2);
/// The blur of the skin before it weighs the effects, in half eye widths.
const GATE_SIGMA: f64 = 0.08;
/// Grid cells per half eye width.
const CELLS_PER_HALF: f64 = 16.0;
/// Most cells along a grid's side (a grid is about 90 a side).
const MAX_SIDE: usize = 256;
/// How far, in sigmas of the widest blur, the grid reaches past the under-eye areas.
pub const PAD_SIGMAS: f64 = 2.5;
/// Samples averaged per cell, per side.
const SUB: usize = 3;

/// The masks a pixel is judged by.
struct Masks<'a> {
    skin: &'a PartMask,
    others: Option<&'a PartMask>,
    whites: Option<&'a PartMask>,
    iris: Option<&'a PartMask>,
}

impl Masks<'_> {
    /// Facial Skin, Others and the eye (Eye Whites or Iris) at normalised (`nx`, `ny`).
    fn at(&self, nx: f64, ny: f64) -> (f32, f32, f32) {
        let s = |m: Option<&PartMask>| m.map_or(0.0, |m| m.sample(nx, ny));
        (self.skin.sample(nx, ny), s(self.others), s(self.whites).max(s(self.iris)))
    }

    /// Others and the eye at normalised (`nx`, `ny`).
    fn lens_and_eye(&self, nx: f64, ny: f64) -> (f32, f32) {
        let s = |m: Option<&PartMask>| m.map_or(0.0, |m| m.sample(nx, ny));
        (s(self.others), s(self.whites).max(s(self.iris)))
    }
}

/// One grid cell: its colour (a lash or liner thinner than the cell left out), its masks and where it is.
#[derive(Clone, Copy, Debug, Default)]
struct Cell {
    colour: [f32; 3],
    /// Facial Skin, Others and the eye (Eye Whites or Iris) at its centre.
    skin: f32,
    others: f32,
    eye: f32,
    /// Across the eye (−1 outer corner, +1 inner) and below the lower lid, in half widths.
    a: f64,
    t: f64,
}

/// The under-eye model, per cell: Dark Circles' gain (per channel), Eye Bags' gain and the skin's colour around the
/// cell (blurred; lashes and frames left out: what a pixel is judged against); and the skin's colour under the eye.
struct Model {
    /// The cheek's luminance Dark Circles lifts toward (none: Dark Circles is off or has no target).
    cheek: Option<f32>,
    dark: Vec<[f32; 3]>,
    bags: Vec<[f32; 1]>,
    /// The skin's colour at Eye Bags' coarse scale (what a lifted crease's colour turns toward).
    bag_hue: Vec<[f32; 3]>,
    low: Vec<[f32; 3]>,
    skin: [f32; 3],
}

/// A slider's value (0…1) as the strength the effects use: the square root, so every step of the slider shows. The
/// effects' own response (a lift in stops, a share of the shading) is slow at first: linear, Dark Circles at 25 / 50 /
/// 75 closed 18 / 38 / 61 % of the circle's gap in lightness (L*) to the cheek on the test portraits, and 88 % at 100;
/// with the square root, about 38 / 56 / 74 %. 0 stays 0 (off).
fn response(v: f32) -> f32 {
    if v > 0.0 { v.min(1.0).sqrt() } else { 0.0 }
}

/// Dark Circles (`dark`) and Eye Bags (`bags`), 0…1 each (the sliders' values), under `eye` of face `face`.
pub fn apply(img: &mut Rgb32f, scene: &Scene<'_>, face: usize, eye: &Eye, dark: f32, bags: f32) {
    let (dark, bags) = (response(dark), response(bags));
    let Some(f) = scene.faces.get(face).map(|c| c.face) else { return };
    let Some(skin) = f.mask(Part::FacialSkin) else { return };
    let masks = Masks { skin, others: f.mask(Part::Others), whites: f.mask(Part::EyeWhites), iris: f.mask(Part::Iris) };
    let pad = PAD_SIGMAS * BAG_COARSE.max(DARK_SIGMA);
    let Some(grid) = Grid::new(eye, UNDER_BOX, pad, CELLS_PER_HALF, LID_MAX, MAX_SIDE) else { return };
    let Some(cells) = cells(img, scene, eye, &masks, &grid) else { return };
    let Some(model) = model(&grid, &cells, dark, bags) else { return };
    lay(img, scene, eye, &masks, &grid, &model);
}

/// The grid's cells: each one's samples (SUB × SUB over it), masks and place.
fn cells(img: &Rgb32f, scene: &Scene<'_>, eye: &Eye, masks: &Masks<'_>, grid: &Grid) -> Option<Vec<Cell>> {
    let n = grid.w.checked_mul(grid.h)?;
    let view = scene.view();
    let cell_px = grid.step * eye.half;
    // (a cell smaller than a pixel needs one sample: the image has no more in it)
    let sub = if cell_px < 1.0 { 1 } else { SUB };
    let mut out = Vec::with_capacity(n);
    for j in 0..grid.h {
        for i in 0..grid.w {
            let (a, b) = grid.local(i, j);
            let (px, py) = eye.to_px(a, b);
            let mut samples = [[0.0f32; 3]; SUB * SUB];
            let mut got = 0;
            for sj in 0..sub {
                for si in 0..sub {
                    let (da, db) = ((si as f64 + 0.5) / sub as f64 - 0.5, (sj as f64 + 0.5) / sub as f64 - 0.5);
                    let (sa, sb) = (da * cell_px, db * cell_px);
                    let (sx, sy) = (px + sa * eye.across.0 + sb * eye.down.0, py + sa * eye.across.1 + sb * eye.down.1);
                    if let (Some(p), Some(slot)) = (sample(img, sx, sy), samples.get_mut(got)) {
                        *slot = p;
                        got += 1;
                    }
                }
            }
            let (nx, ny) = view.to_norm(px, py);
            let (skin, others, e) = masks.at(nx, ny);
            let colour = cell_colour(samples.get(..got).unwrap_or(&[]));
            out.push(Cell { colour, skin, others, eye: e, a, t: eye.below_lid(a, b) });
        }
    }
    Some(out)
}

/// The mean of a cell's samples, leaving out those far darker than the cell's median (a lash or liner crossing it).
fn cell_colour(samples: &[[f32; 3]]) -> [f32; 3] {
    let mut l: Vec<f32> = samples.iter().map(|p| luma(*p)).collect();
    l.sort_by(f32::total_cmp);
    let median = l.get(l.len() / 2).copied().unwrap_or(0.0);
    let (mut acc, mut wsum) = ([0.0f32; 3], 0.0f32);
    for p in samples {
        let w = if median > 1e-6 { smoothstep(f64::from(IN_CELL.0), f64::from(IN_CELL.1), f64::from(luma(*p) / median)) as f32 } else { 1.0 };
        acc = std::array::from_fn(|c| acc[c] + p[c] * w);
        wsum += w;
    }
    if wsum > 0.0 { acc.map(|v| v / wsum) } else { [0.0; 3] }
}

/// The mean colour of `items` (colour, weight), leaving out those far from their median's luminance (freckles, a
/// frame, a highlight), and the weight it is made of.
fn robust_mean(items: impl Iterator<Item = ([f32; 3], f32)>) -> Option<([f32; 3], f64)> {
    let items: Vec<([f32; 3], f32)> = items.filter(|(c, w)| *w > 0.0 && w.is_finite() && c.iter().all(|v| v.is_finite())).collect();
    let mut l: Vec<f32> = items.iter().map(|(c, _)| luma(*c)).collect();
    l.sort_by(f32::total_cmp);
    let median = *l.get(l.len() / 2)?;
    if median.is_nan() || median <= 1e-6 {
        return None;
    }
    let (mut sum, mut total) = ([0.0f64; 3], 0.0f64);
    for (c, w) in &items {
        let r = luma(*c) / median;
        if r >= ROBUST.0 && r <= ROBUST.1 {
            for ch in 0..3 {
                sum[ch] += f64::from(c[ch] * w);
            }
            total += f64::from(*w);
        }
    }
    (total > 0.0).then(|| (sum.map(|v| (v / total) as f32), total))
}

/// Per cell, 0 for a thin dark line across the skin (a lash, liner, a frame's edge), 1 for skin: its lightness
/// against a closing of the skin cells' lightness (a line up to 2 × [`CLOSE_CELLS`] wide closes over; a dark circle,
/// far wider, does not).
fn lines(grid: &Grid, cells: &[Cell], kind: &[f32]) -> Vec<f32> {
    let l: Vec<f32> = cells.iter().map(|c| luma(c.colour)).collect();
    let skin: Vec<f32> = l.iter().zip(kind).map(|(v, k)| if *k >= 0.5 { *v } else { 0.0 }).collect();
    let dilated: Vec<f32> = (0..cells.len()).map(|k| around(grid, k, CLOSE_CELLS).filter_map(|q| skin.get(q).copied()).fold(0.0, f32::max)).collect();
    let line: Vec<f32> = (0..cells.len())
        .map(|k| {
            // (cells with no skin around them close to nothing: they are judged as skin)
            let closed = around(grid, k, CLOSE_CELLS).filter_map(|q| dilated.get(q).copied()).filter(|v| *v > 1e-6).fold(f32::INFINITY, f32::min);
            match l.get(k) {
                Some(v) if closed.is_finite() => smoothstep(f64::from(CELL_LINE.0), f64::from(CELL_LINE.1), f64::from(v / closed)) as f32,
                _ => 1.0,
            }
        })
        .collect();
    // a cell beside a line holds the line's edge (its samples blend into it): it counts as little
    (0..cells.len()).map(|k| around(grid, k, 1).filter_map(|q| line.get(q).copied()).fold(1.0, f32::min)).collect()
}

/// How much colour `p`, where Others is `o` and Facial Skin `s`, is skin of colour `skin`: Facial Skin, or through
/// glasses a colour like the skin's (a frame is far darker or off-hue); neither when pale and off-hue (a white or
/// silver frame, a highlight). However dark or off-colour, Facial Skin is skin: a dark circle is. 0…1.
fn kind(p: [f32; 3], skin: [f32; 3], s: f32, o: f32) -> f32 {
    let (sp, ss) = (p[0] + p[1] + p[2], skin[0] + skin[1] + skin[2]);
    if !(sp > 1e-6 && ss > 1e-6) {
        return 0.0;
    }
    let ratio = f64::from(luma(p) / luma(skin).max(1e-6));
    let (dr, dg) = (f64::from(p[0] / sp - skin[0] / ss), f64::from(p[1] / sp - skin[1] / ss));
    let hue = (dr * dr + dg * dg).sqrt();
    let pale = 1.0 - smoothstep(PALE.0, PALE.1, ratio) * smoothstep(PALE_HUE.0, PALE_HUE.1, hue);
    let o = f64::from(o.clamp(0.0, 1.0));
    let lens = o * smoothstep(FRAME_DARK.0, FRAME_DARK.1, ratio) * (1.0 - smoothstep(SEEN_HUE.0, SEEN_HUE.1, hue));
    let plain = f64::from(s.clamp(0.0, 1.0)) * (1.0 - o);
    ((plain + lens).min(1.0) * pale) as f32
}

/// The model from the cells: where the skin is, its level, and each cell's gain.
fn model(grid: &Grid, cells: &[Cell], dark: f32, bags: f32) -> Option<Model> {
    let n = cells.len();
    // the skin's colour under the eye: plain Facial Skin below the lid, else (glasses over the whole of it) the skin
    // seen through the lens
    let below = |c: &&Cell| c.eye <= 0.0 && c.t > LID_GAP;
    let plain = cells.iter().filter(below).filter(|c| c.skin >= 0.5 && c.others < 0.5).map(|c| (c.colour, 1.0));
    let lens = cells.iter().filter(below).filter(|c| c.skin.max(c.others) >= 0.5).map(|c| (c.colour, 1.0));
    let enough = MIN_SKIN / (grid.step * grid.step);
    let (skin, _) = robust_mean(plain).filter(|(_, n)| *n >= enough).or_else(|| robust_mean(lens))?;
    // skin per cell: its masks and colour, off the eye and the lid line, without lashes, liner and frames' edges
    let kind: Vec<f32> = cells.iter().map(|c| if c.t > 0.0 { kind(c.colour, skin, c.skin, c.others) * (1.0 - c.eye) } else { 0.0 }).collect();
    let line = lines(grid, cells, &kind);
    let gate: Vec<f32> = cells.iter().zip(kind.iter().zip(&line)).map(|(c, (k, l))| k * l * smoothstep(0.0, LID_GAP, c.t) as f32).collect();
    // the skin's colour around each cell (lashes and frames left out, normalised, so nothing of them bleeds in)
    let packed: Vec<[f32; 4]> = cells.iter().zip(&gate).map(|(c, g)| [c.colour[0] * g, c.colour[1] * g, c.colour[2] * g, *g]).collect();
    let low: Vec<Option<[f32; 3]>> = grid_blur(&packed, grid.w, grid.h, DARK_SIGMA / grid.step)
        .into_iter()
        .map(|l| (l[3] > 1e-4).then(|| [l[0] / l[3], l[1] / l[3], l[2] / l[3]]).filter(|c| luma(*c) > 1e-6))
        .collect();
    // the effects' weights: the skin smoothed (a patchy one, through glasses, gives no blotches), kept off the eye
    let smooth = grid_blur(&gate.iter().map(|g| [*g]).collect::<Vec<_>>(), grid.w, grid.h, GATE_SIGMA / grid.step);
    let weight = |area: Area, k: usize| -> f32 {
        let (Some(c), Some(&[g])) = (cells.get(k), smooth.get(k)) else { return 0.0 };
        if c.t <= 0.0 { 0.0 } else { weights::of(area, c.a, c.t) * g * (1.0 - c.eye) * smoothstep(0.0, LID_GAP, c.t) as f32 }
    };
    let mut dark_gain = vec![[1.0f32; 3]; n];
    let cheek = if dark > 0.0 { dark_circles(grid, cells, &gate, &low, &mut dark_gain, |k| dark * weight(Area::DarkCircles, k)) } else { None };
    let mut bag_gain = vec![[1.0f32]; n];
    let mut bag_hue = vec![[0.0f32; 3]; n];
    if bags > 0.0 {
        eye_bags(grid, cells, &gate, &dark_gain, &mut bag_gain, |k| bags * BAG_STRENGTH * weight(Area::EyeBags, k));
        let packed: Vec<[f32; 4]> = cells.iter().zip(&gate).map(|(c, g)| [c.colour[0] * g, c.colour[1] * g, c.colour[2] * g, *g]).collect();
        bag_hue = grid_blur(&packed, grid.w, grid.h, BAG_COARSE / grid.step)
            .into_iter()
            .map(|l| if l[3] > 1e-4 { [l[0] / l[3], l[1] / l[3], l[2] / l[3]] } else { [0.0; 3] })
            .collect();
    }
    Some(Model { cheek, dark: dark_gain, bags: bag_gain, bag_hue, low: low.into_iter().map(|l| l.unwrap_or([0.0; 3])).collect(), skin })
}

/// Dark Circles' gains; the cheek's luminance they lift toward (none without a cheek to lift toward).
fn dark_circles(
    grid: &Grid,
    cells: &[Cell],
    gate: &[f32],
    low: &[Option<[f32; 3]>],
    gain: &mut [[f32; 3]],
    weight: impl Fn(usize) -> f32,
) -> Option<f32> {
    // the target: the cheek's colour below the eye
    let cheek = cells.iter().zip(gate).map(|(c, g)| (c.colour, weights::cheek(c.a, c.t) * g));
    let (target, total) = robust_mean(cheek)?;
    if total.is_nan() || total < MIN_CHEEK / (grid.step * grid.step) {
        return None;
    }
    let ty = luma(target);
    if !(ty.is_finite() && ty > 0.0) {
        return None;
    }
    // the colour of the clean skin around each cell (the circle left out), else the cheek's
    let clean: Vec<[f32; 4]> = cells
        .iter()
        .zip(gate)
        .map(|(c, g)| {
            let w = g * smoothstep(f64::from(CLEAN.0), f64::from(CLEAN.1), f64::from(luma(c.colour) / ty)) as f32;
            [c.colour[0] * w, c.colour[1] * w, c.colour[2] * w, w]
        })
        .collect();
    let around = grid_blur(&clean, grid.w, grid.h, HUE_SIGMA / grid.step);
    for (k, (out, l)) in gain.iter_mut().zip(low).enumerate() {
        let w = weight(k);
        let Some(l) = l else { continue };
        let ly = luma(*l);
        if !(w > 0.0 && ly > 1e-6) {
            continue;
        }
        let hue = match around.get(k) {
            Some(s) if s[3] > 1e-3 => {
                let s = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
                // (scaled to the cheek's luminance: only its hue is used, half and half with the cheek's)
                let sy = luma(s);
                if sy > 1e-6 { std::array::from_fn(|c| target[c] + HUE_AROUND * (s[c] * ty / sy - target[c])) } else { target }
            }
            _ => target,
        };
        let lift = (LIFT_TO * ty / ly).clamp(1.0, MAX_LIFT);
        for c in 0..3 {
            // the channel's share of the luminance, turned toward the clean skin's around it
            let tint = if l[c] > 1e-6 && hue[c] > 0.0 { ((hue[c] / ty) / (l[c] / ly)).clamp(1.0 / MAX_TINT, MAX_TINT) } else { 1.0 };
            // (lifting only: no channel darkens; the weight as a power, so half the slider is half the stops; the hue
            // turns sooner than the light, so a half-lifted shadow is not grey)
            let g = (lift.powf(w) * tint.powf(TINT_SHARE * w.sqrt())).clamp(1.0, MAX_LIFT * MAX_TINT);
            if g.is_finite() {
                out[c] = g;
            }
        }
    }
    Some(ty)
}

fn eye_bags(grid: &Grid, cells: &[Cell], gate: &[f32], dark: &[[f32; 3]], gain: &mut [[f32; 1]], weight: impl Fn(usize) -> f32) {
    // on the colours as Dark Circles left them
    let packed: Vec<[f32; 2]> =
        cells.iter().zip(gate).zip(dark).map(|((c, g), d)| [luma(std::array::from_fn(|k| c.colour[k] * d[k])) * g, *g]).collect();
    let (fine, coarse) = (grid_blur(&packed, grid.w, grid.h, BAG_FINE / grid.step), grid_blur(&packed, grid.w, grid.h, BAG_COARSE / grid.step));
    for (k, (out, (f, c))) in gain.iter_mut().zip(fine.iter().zip(&coarse)).enumerate() {
        let w = weight(k);
        if !(w > 0.0 && f[1] > 1e-4 && c[1] > 1e-4) {
            continue;
        }
        let (fy, cy) = (f[0] / f[1], c[0] / c[1]);
        if !(fy > 1e-6 && cy > 1e-6) {
            continue;
        }
        let m = 1.0 + w * ((cy / fy).clamp(1.0 / BAG_RANGE, BAG_RANGE) - 1.0);
        if m.is_finite() {
            out[0] = m;
        }
    }
}

/// Lay the model on the pixels the under-eye areas cover, keeping the eye, the lid line, lashes, liner and frames
/// out at the pixels' resolution. Every other pixel stays as it was.
fn lay(img: &mut Rgb32f, scene: &Scene<'_>, eye: &Eye, masks: &Masks<'_>, grid: &Grid, m: &Model) {
    let Some([x0, y0, x1, y1]) = eye.pixel_box(weights::EFFECT_BOX, 1.0, img.width, img.height) else { return };
    let view = scene.view();
    let step = LINE_STEP * eye.half;
    let mut out: Vec<(usize, [f32; 3])> = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let (a, b) = eye.local(px, py);
            let t = eye.below_lid(a, b);
            if t <= 0.0 || (weights::of(Area::DarkCircles, a, t) <= 0.0 && weights::of(Area::EyeBags, a, t) <= 0.0) {
                continue;
            }
            let (gd, [gb]) = (grid.at(&m.dark, a, b, [1.0; 3]), grid.at(&m.bags, a, b, [1.0]));
            if gd == [1.0; 3] && gb == 1.0 {
                continue;
            }
            let i = y * img.width + x;
            let Some(&p) = img.data.get(i) else { continue };
            let (nx, ny) = view.to_norm(px, py);
            let (o, e) = masks.lens_and_eye(nx, ny);
            // (the cells' gate holds Facial Skin's extent; here only what a pixel's colour and shape say)
            let mut keep = (1.0 - e) * smoothstep(0.0, LID_GAP, t) as f32 * kind(p, m.skin, 1.0, o);
            if keep.is_nan() || keep <= 0.0 {
                continue;
            }
            let low = grid.at(&m.low, a, b, [0.0; 3]);
            let level = luma(low);
            if level.is_nan() || level <= 1e-6 {
                continue;
            }
            // a line far darker than the skin around it is a lash, liner or a frame: kept (the circle is the skin)
            let zone = (1.0 - smoothstep(LASH_ZONE.0, LASH_ZONE.1, t)).max(f64::from(o.clamp(0.0, 1.0)));
            // (a pixel as light as the skin's level is not on a dark line: most of them, and the test is costly)
            if zone > 0.0 && luma(p) < PIXEL_LINE.1 * level {
                let on_line =
                    1.0 - smoothstep(f64::from(PIXEL_LINE.0), f64::from(PIXEL_LINE.1), f64::from(line_light(img, px, py, eye, step) / level));
                keep *= (1.0 - zone * on_line) as f32;
            }
            if keep <= 0.0 {
                continue;
            }
            // how deep a shadow it lies in, and how far toward the cheek it is lifted now (0 at 0, 1 at the top)
            let deep = m.cheek.map_or(0.0, |ty| {
                let depth = 1.0 - smoothstep(f64::from(DEEP.0), f64::from(DEEP.1), f64::from(level / ty)) as f32;
                let full = (LIFT_TO * ty / level).clamp(1.0, MAX_LIFT).ln();
                let now = luma(std::array::from_fn(|c| low[c] * gd[c])) / level;
                let progress = if depth > 0.0 && full > 1e-3 && now > 1.0 { (now.ln() / full).clamp(0.0, 1.0).sqrt() } else { 0.0 };
                depth * progress
            });
            // the texture of the clean skin below, for a deep shadow
            let below = if deep > 0.0 {
                let (sa, sb) = (a, b + BORROW_BELOW);
                let (sx, sy) = eye.to_px(sa, sb);
                let l = luma(grid.at(&m.low, sa, sb, [0.0; 3]));
                sample(img, sx, sy).map(luma).filter(|_| l > 1e-6).map(|v| v / l)
            } else {
                None
            };
            let hue = if gb > 1.0 { grid.at(&m.bag_hue, a, b, [0.0; 3]) } else { [0.0; 3] };
            let q = lifted(p, low, gd, gb, below, deep, hue);
            out.push((i, std::array::from_fn(|c| p[c] + keep * (q[c] - p[c]))));
        }
    }
    for (i, q) in out {
        if let Some(v) = img.data.get_mut(i) {
            *v = q;
        }
    }
}

/// Pixel `p` with Dark Circles' gain `gd` and Eye Bags' `gb`, where the skin around it is `low`: its luminance
/// detail against `low` kept (compressed as the lift grows; in a `deep` shadow, 0…1, the texture `below` of the
/// clean skin further down instead, the pixel's luminance against the skin's there) and its colour detail kept
/// (faded as the lift grows and in a deep shadow), on `low` lifted. Dark Circles never darkens a channel.
fn lifted(p: [f32; 3], low: [f32; 3], gd: [f32; 3], gb: f32, below: Option<f32>, deep: f32, bag_hue: [f32; 3]) -> [f32; 3] {
    let (py, ly) = (luma(p), luma(low));
    let lit: [f32; 3] = std::array::from_fn(|c| low[c] * gd[c]);
    let lift = luma(lit) / ly;
    if !(py > 1e-6 && lift.is_finite() && lift > 1.0) {
        return bag(p.map(|v| v * gb), bag_hue, gb);
    }
    let strength = lift.ln();
    let (k, keep_colour) = (1.0 / (1.0 + DETAIL_K * strength), (-CHROMA_K * strength).exp() * (1.0 - deep));
    let d = (py / ly).clamp(0.05, 20.0);
    // the luminance texture: its own (compressed), or for a deep shadow lifted far, the clean skin's below it
    let own = k * d.ln();
    let texture = match below {
        Some(r) if r > 0.0 && r.is_finite() => {
            let r = r.ln();
            let share = f64::from(deep) * (1.0 - smoothstep(f64::from(BORROW_DETAIL.0), f64::from(BORROW_DETAIL.1), f64::from(r.abs())));
            (own + share as f32 * (r - own)).exp()
        }
        _ => own.exp(),
    };
    let q: [f32; 3] = std::array::from_fn(|c| {
        let colour = if low[c] > 1e-6 { (p[c] / (low[c] * d)).clamp(0.25, 4.0).powf(keep_colour) } else { 1.0 };
        let q = lit[c] * texture * colour;
        (if q.is_finite() { q.max(p[c]) } else { p[c] }) * gb
    });
    bag(q, bag_hue, gb)
}

/// Eye Bags' lift of a crease (`gb` above 1) also turns the crease's colour toward the skin's around it (`low`, at
/// the bag's scale), as far as the lift takes its luminance toward that skin's: a shadow in skin is more saturated
/// than the lit skin, so a crease lifted alone reads as an orange band, and one given the lit skin's colour while
/// still dark reads as grey.
fn bag(q: [f32; 3], low: [f32; 3], gb: f32) -> [f32; 3] {
    let (qy, ly) = (luma(q), luma(low));
    let before = qy / gb;
    if !(gb > 1.0 && before > 1e-6 && ly > before) {
        return q;
    }
    let share = (gb.ln() / (ly / before).ln()).clamp(0.0, 1.0);
    let out: [f32; 3] = std::array::from_fn(|c| q[c] + share * (low[c] * qy / ly - q[c]));
    if out.iter().all(|v| v.is_finite() && *v >= 0.0) { out } else { q }
}

/// The darkest mean luminance along four directions through pixel (`px`, `py`) (five samples `step` apart): low on
/// a lash or a frame's edge (dark along it), not on a dark speck of grain (light around it).
fn line_light(img: &Rgb32f, px: f64, py: f64, eye: &Eye, step: f64) -> f32 {
    let (u, v) = (eye.across, eye.down);
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let dirs = [u, v, ((u.0 + v.0) * s, (u.1 + v.1) * s), ((u.0 - v.0) * s, (u.1 - v.1) * s)];
    let mut darkest = f32::INFINITY;
    for d in dirs {
        let (mut sum, mut n) = (0.0f32, 0.0f32);
        for k in -2..=2 {
            let k = f64::from(k) * step;
            if let Some(q) = sample(img, px + k * d.0, py + k * d.1) {
                sum += luma(q);
                n += 1.0;
            }
        }
        if n > 0.0 {
            darkest = darkest.min(sum / n);
        }
    }
    if darkest.is_finite() { darkest } else { 0.0 }
}
