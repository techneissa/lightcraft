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
//! - **Dark Circles**: the skin under the eye that is darker than the skin just outside the circle (below and
//!   beside it, on the original pixels) is lifted toward that skin's lightness, the slider's share of the way in L*,
//!   never past it, and its hue (blue, purple, brown) turned toward that skin's; its texture is kept at that skin's
//!   strength. Lifting only: no channel darkens.
//! - **Eye Bags**: the shading between a fine and a coarse scale (the bag's bulge and the crease below it) is
//!   flattened; pores and fine lines are kept.

use lightcraft_raster::Rgb32f;

use super::Area;
use super::geom::{Eye, LID_MAX, smoothstep};
use super::grid::{Grid, around, grid_blur, luma, sample};
use super::weights::{self, LID_GAP, UNDER_BOX};
use crate::{Part, PartMask, Scene};

/// Blur of the under-eye colour, in half eye widths (what Eye Bags and the lash test read).
const DARK_SIGMA: f64 = 0.07;
/// The level Dark Circles lifts the circle to comes from the skin just outside it (below and beside it,
/// [`weights::ring`]), on the original pixels: the lightness this share of that skin is darker than (a low share, so
/// a lit cheekbone below the circle does not raise it) …
const REF_SHARE: f32 = 0.25;
/// The hue turned toward is that skin's, from its cells near that level (a Gaussian in L* of this width): a lit,
/// shiny cheekbone is cooler and paler than the skin, and its hue would make the lifted circle ashy.
const HUE_NEAR: f32 = 8.0;
/// … and never above the skin outside the circle near the place, blurred this much (in half eye widths): the darker
/// side of a side-lit face is lifted only to its own surroundings.
const REF_SIGMA: f64 = 0.35;
/// The skin outside the circle must cover at least this much (in square half widths) to give Dark Circles a level.
const MIN_RING: f64 = 0.02;
/// The fine level each pixel is lifted from: cells per half eye width, and their blur (in half eye widths). Fine
/// enough to follow a circle's sharp edge (no line is left along it), coarse enough to average grain out.
const FINE_PER_HALF: f64 = 40.0;
const FINE_SIGMA: f64 = 0.02;
const FINE_SUB: usize = 2;
/// Dark spots up to twice this many fine cells wide (0.05 half eye widths: a freckle) are the skin's texture, not
/// part of the level a pixel is lifted from.
const SPOT_CELLS: isize = 1;
/// Most Dark Circles lifts the luminance (a factor: 2.6 stops).
const MAX_LIFT: f32 = 6.0;
/// How much of the way to the hue of the skin outside the circle Dark Circles goes …
const TINT_SHARE: f32 = 0.9;
/// … and how much darker (in L*) than the level the skin must be for the hue to turn fully: skin about as light as
/// it keeps its hue.
const HUE_GAP: f32 = 6.0;
/// The hue turns as the slider's value to this power: sooner than the light, so a half-lifted shadow is the skin's
/// colour, not grey.
const HUE_SOONER: f32 = 0.35;
/// The skin's texture against the fine level is kept, its strength matched to the reference skin's (the spread of
/// log luminance inside the cells): a shadow's grain, stronger than the lit skin's once lifted, is scaled down to it,
/// by at most half. Never up: a stronger texture would raise the skin's mean past the level (a deep shadow with too
/// little texture takes the clean skin's instead, below).
const GRAIN_MATCH: (f32, f32) = (0.5, 1.0);
/// A deep shadow's colour detail is mostly noise: kept as a power `exp(−CHROMA_K · ln g)` of a lift `g`, so a
/// strong lift takes the colour from the skin around it.
const CHROMA_K: f32 = 0.8;
/// A deep shadow (its fine level this share of the reference's: wholly, then not at all) has lost its texture and
/// colour to the camera's noise and compression: as Dark Circles lifts it, it takes the skin's texture from
/// [`BORROW_BELOW`] half widths further down, where the skin is clean (as a healing brush would), and its colour
/// from the skin around it. Lighter skin (freckles, a mild circle) keeps its own …
const DEEP: (f32, f32) = (0.15, 0.35);
const BORROW_BELOW: f64 = 0.7;
/// … unless that texture is far from the skin's own there (a frame, a mole: by this much in log luminance, used
/// wholly, then not at all).
const BORROW_DETAIL: (f32, f32) = (0.3, 0.6);
/// Eye Bags' fine and coarse blurs, in half eye widths.
const BAG_FINE: f64 = 0.025;
const BAG_COARSE: f64 = 0.45;
/// How much of the shading Eye Bags removes at 100.
const BAG_STRENGTH: f32 = 1.0;
/// Most Eye Bags changes a pixel's luminance (a factor either way).
const BAG_RANGE: f32 = 2.2;
/// Plain Facial Skin under the eye (in square half widths) that gives the skin's colour; with less (glasses over
/// it), the skin seen through the lens gives it.
const MIN_SKIN: f64 = 0.1;
/// The skin's colour: the mean of the cells within this factor of their median's luminance.
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
    /// The spread of its samples' log luminance (the skin's grain at the cell's scale).
    grain: f32,
}

/// The under-eye model, per cell: Eye Bags' gain and the skin's colour around the cell (blurred; lashes and frames
/// left out: what a pixel is judged against); Dark Circles' model; and the skin's colour under the eye.
struct Model {
    dark: Option<Dark>,
    bags: Vec<[f32; 1]>,
    /// The skin's colour at Eye Bags' coarse scale (what a lifted crease's colour turns toward).
    bag_hue: Vec<[f32; 3]>,
    low: Vec<[f32; 3]>,
    skin: [f32; 3],
}

/// Dark Circles' model: per cell its strength (the slider times the crescent's weight), the colour the skin is
/// lifted toward (the level of the skin outside the circle, with that skin's hue) and how much the texture is scaled
/// to match that skin's; and the fine level each pixel is lifted from, on its own finer grid.
struct Dark {
    strength: Vec<[f32; 1]>,
    target: Vec<[f32; 3]>,
    grain: Vec<[f32; 1]>,
    fine: Grid,
    level: Vec<[f32; 3]>,
}

/// Eye Bags' slider value (0…1) as the strength it uses: the square root, so its slow start (a share of the shading)
/// shows at every step. 0 stays 0 (off). Dark Circles needs none: it closes the gap in lightness (L*), which the eye
/// sees evenly.
fn bag_response(v: f32) -> f32 {
    if v > 0.0 { v.min(1.0).sqrt() } else { 0.0 }
}

/// Dark Circles (`dark`) and Eye Bags (`bags`), 0…1 each (the sliders' values), under `eye` of face `face`.
pub fn apply(img: &mut Rgb32f, scene: &Scene<'_>, face: usize, eye: &Eye, dark: f32, bags: f32) {
    let (dark, bags) = (dark.clamp(0.0, 1.0), bag_response(bags));
    let Some(f) = scene.faces.get(face).map(|c| c.face) else { return };
    let Some(skin) = f.mask(Part::FacialSkin) else { return };
    let masks = Masks { skin, others: f.mask(Part::Others), whites: f.mask(Part::EyeWhites), iris: f.mask(Part::Iris) };
    let pad = PAD_SIGMAS * BAG_COARSE.max(REF_SIGMA);
    let Some(grid) = Grid::new(eye, UNDER_BOX, pad, CELLS_PER_HALF, LID_MAX, MAX_SIDE) else { return };
    let Some(cells) = cells(img, scene, eye, &masks, &grid) else { return };
    let Some(model) = model(img, eye, &grid, &cells, dark, bags) else { return };
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
            let (colour, grain) = cell_colour(samples.get(..got).unwrap_or(&[]));
            out.push(Cell { colour, skin, others, eye: e, a, t: eye.below_lid(a, b), grain });
        }
    }
    Some(out)
}

/// The mean of a cell's samples, leaving out those far darker than the cell's median (a lash or liner crossing it),
/// and the spread of the kept samples' log luminance.
fn cell_colour(samples: &[[f32; 3]]) -> ([f32; 3], f32) {
    let mut l: Vec<f32> = samples.iter().map(|p| luma(*p)).collect();
    l.sort_by(f32::total_cmp);
    let median = l.get(l.len() / 2).copied().unwrap_or(0.0);
    let (mut acc, mut wsum, mut log, mut log2) = ([0.0f32; 3], 0.0f32, 0.0f32, 0.0f32);
    for p in samples {
        let w = if median > 1e-6 { smoothstep(f64::from(IN_CELL.0), f64::from(IN_CELL.1), f64::from(luma(*p) / median)) as f32 } else { 1.0 };
        acc = std::array::from_fn(|c| acc[c] + p[c] * w);
        wsum += w;
        let v = luma(*p).max(1e-6).ln();
        log += w * v;
        log2 += w * v * v;
    }
    if wsum > 0.0 {
        let mean = log / wsum;
        let spread = (log2 / wsum - mean * mean).max(0.0).sqrt();
        (acc.map(|v| v / wsum), if spread.is_finite() { spread } else { 0.0 })
    } else {
        ([0.0; 3], 0.0)
    }
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

/// The model from the cells: where the skin is, its level, and each effect's per-cell values.
fn model(img: &Rgb32f, eye: &Eye, grid: &Grid, cells: &[Cell], dark: f32, bags: f32) -> Option<Model> {
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
    let low = normalised(grid, cells, &gate, DARK_SIGMA);
    // the effects' weights: the skin smoothed (a patchy one, through glasses, gives no blotches), kept off the eye
    let smooth = grid_blur(&gate.iter().map(|g| [*g]).collect::<Vec<_>>(), grid.w, grid.h, GATE_SIGMA / grid.step);
    let weight = |area: Area, k: usize| -> f32 {
        let (Some(c), Some(&[g])) = (cells.get(k), smooth.get(k)) else { return 0.0 };
        if c.t <= 0.0 { 0.0 } else { weights::of(area, c.a, c.t) * g * (1.0 - c.eye) * smoothstep(0.0, LID_GAP, c.t) as f32 }
    };
    let mut dark_gain = vec![[1.0f32; 3]; n];
    let dark = if dark > 0.0 {
        dark_circles(img, eye, grid, cells, &gate, &low, |k| dark * weight(Area::DarkCircles, k)).map(|(d, gain)| {
            dark_gain = gain;
            d
        })
    } else {
        None
    };
    let mut bag_gain = vec![[1.0f32]; n];
    let mut bag_hue = vec![[0.0f32; 3]; n];
    if bags > 0.0 {
        eye_bags(grid, cells, &gate, &dark_gain, &mut bag_gain, |k| bags * BAG_STRENGTH * weight(Area::EyeBags, k));
        bag_hue = normalised(grid, cells, &gate, BAG_COARSE).into_iter().map(|l| l.unwrap_or([0.0; 3])).collect();
    }
    Some(Model { dark, bags: bag_gain, bag_hue, low: low.into_iter().map(|l| l.unwrap_or([0.0; 3])).collect(), skin })
}

/// The cells' colour weighed by `weight` and blurred `sigma` half widths, normalised (none where no weight reaches).
fn normalised(grid: &Grid, cells: &[Cell], weight: &[f32], sigma: f64) -> Vec<Option<[f32; 3]>> {
    let packed: Vec<[f32; 4]> = cells.iter().zip(weight).map(|(c, g)| [c.colour[0] * g, c.colour[1] * g, c.colour[2] * g, *g]).collect();
    grid_blur(&packed, grid.w, grid.h, sigma / grid.step)
        .into_iter()
        .map(|l| (l[3] > 1e-6).then(|| [l[0] / l[3], l[1] / l[3], l[2] / l[3]]).filter(|c| luma(*c) > 1e-6))
        .collect()
}

/// CIE lightness L* (0…100) of a luminance (1 = white).
fn lstar(y: f32) -> f32 {
    if y > 216.0 / 24389.0 { 116.0 * y.cbrt() - 16.0 } else { y * 24389.0 / 27.0 }
}

/// The luminance of CIE lightness `l`.
fn from_lstar(l: f32) -> f32 {
    if l > 8.0 { ((l + 16.0) / 116.0).powi(3) } else { l * 27.0 / 24389.0 }
}

/// The luminance a skin of luminance `y` is lifted to, `s` (0…1) of the way in lightness (L*) to `to`; never past it,
/// and skin as light as it or lighter stays as it is.
fn lifted_luma(y: f32, to: f32, s: f32) -> f32 {
    if !(y > 0.0 && to > y && s > 0.0) {
        return y;
    }
    let (l0, l1) = (lstar(y), lstar(to));
    from_lstar(l0 + s.min(1.0) * (l1 - l0)).clamp(y, to)
}

/// Dark Circles' model and each cell's luminance gain (for Eye Bags, which works on the colours as Dark Circles leaves
/// them); none without enough skin outside the circle to lift toward.
fn dark_circles(
    img: &Rgb32f,
    eye: &Eye,
    grid: &Grid,
    cells: &[Cell],
    gate: &[f32],
    low: &[Option<[f32; 3]>],
    weight: impl Fn(usize) -> f32,
) -> Option<(Dark, Vec<[f32; 3]>)> {
    // the skin just outside the circle, below and beside it, on the original pixels
    let ring: Vec<f32> = cells.iter().zip(gate).map(|(c, g)| weights::ring(c.a, c.t) * g).collect();
    let total: f64 = ring.iter().map(|w| f64::from(*w)).sum();
    if total.is_nan() || total < MIN_RING / (grid.step * grid.step) {
        return None;
    }
    // its level: the lightness REF_SHARE of it is darker than
    let mut lights: Vec<(f32, f32)> = cells.iter().zip(&ring).filter(|(_, w)| **w > 0.01).map(|(c, w)| (lstar(luma(c.colour)), *w)).collect();
    lights.sort_by(|p, q| p.0.total_cmp(&q.0));
    let sum: f32 = lights.iter().map(|(_, w)| w).sum();
    let mut seen = 0.0f32;
    let level = lights.iter().find(|(_, w)| {
        seen += w;
        seen >= REF_SHARE * sum
    })?;
    let (level_l, level) = (level.0, from_lstar(level.0));
    // its hue: from its cells near that level
    let near = cells.iter().zip(&ring).map(|(c, w)| {
        let d = (lstar(luma(c.colour)) - level_l) / HUE_NEAR;
        (c.colour, w * (-0.5 * d * d).exp())
    });
    let (hue, _) = robust_mean(near)?;
    let hy = luma(hue);
    if hy.is_nan() || hy <= 1e-6 {
        return None;
    }
    // the skin outside the circle near each place: a level the place is not lifted past
    let around = normalised(grid, cells, &ring, REF_SIGMA);
    let target: Vec<[f32; 3]> = around.iter().map(|c| hue.map(|v| v * c.map_or(level, |c| luma(c).min(level)) / hy)).collect();
    // the texture's strength, matched to the skin's outside the circle
    let grain_ring = cells.iter().zip(&ring).map(|(c, w)| c.grain * w).sum::<f32>() / ring.iter().sum::<f32>().max(1e-6);
    let own: Vec<[f32; 2]> = cells.iter().zip(gate).map(|(c, g)| [c.grain * g, *g]).collect();
    let grain: Vec<[f32; 1]> = grid_blur(&own, grid.w, grid.h, DARK_SIGMA / grid.step)
        .into_iter()
        .map(|o| [if o[1] > 1e-6 && o[0] > 1e-6 { (grain_ring / (o[0] / o[1])).clamp(GRAIN_MATCH.0, GRAIN_MATCH.1) } else { 1.0 }])
        .collect();
    let strength: Vec<[f32; 1]> = (0..cells.len()).map(|k| [weight(k).clamp(0.0, 1.0)]).collect();
    // each cell's luminance gain, for Eye Bags
    let gain: Vec<[f32; 3]> = (0..cells.len())
        .map(|k| {
            let (Some(Some(l)), Some(t), Some([s])) = (low.get(k), target.get(k), strength.get(k)) else { return [1.0; 3] };
            let y = luma(*l);
            let g = if y > 1e-6 { (lifted_luma(y, luma(*t), *s) / y).clamp(1.0, MAX_LIFT) } else { 1.0 };
            [g; 3]
        })
        .collect();
    let (fine, level) = fine_level(img, eye, grid, gate)?;
    Some((Dark { strength, target, grain, fine, level }, gain))
}

/// `packed` (colour × weight, weight) with each cell's luminance closed (the most of its neighbours' least, over
/// [`SPOT_CELLS`]): a dark spot up to twice that wide takes the skin's level around it; the colour keeps its hue.
fn close_spots(grid: &Grid, packed: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let l: Vec<f32> = packed.iter().map(|p| if p[3] > 1e-4 { luma([p[0] / p[3], p[1] / p[3], p[2] / p[3]]) } else { 0.0 }).collect();
    let dilated: Vec<f32> = (0..l.len()).map(|k| around(grid, k, SPOT_CELLS).filter_map(|q| l.get(q).copied()).fold(0.0, f32::max)).collect();
    (0..l.len())
        .map(|k| {
            let closed = around(grid, k, SPOT_CELLS).filter_map(|q| dilated.get(q).copied()).filter(|v| *v > 1e-6).fold(f32::INFINITY, f32::min);
            let (Some(&p), Some(&own)) = (packed.get(k), l.get(k)) else { return [0.0; 4] };
            if closed.is_finite() && own > 1e-6 && closed > own {
                let g = closed / own;
                [p[0] * g, p[1] * g, p[2] * g, p[3]]
            } else {
                p
            }
        })
        .collect()
}

/// The skin's level on a finer grid over the effects' area (lashes and frames left out by the cells' gate), what each
/// pixel is lifted from: it follows a circle's sharp edge, which the cells' blur would round off into a dark line.
fn fine_level(img: &Rgb32f, eye: &Eye, grid: &Grid, gate: &[f32]) -> Option<(Grid, Vec<[f32; 3]>)> {
    let fine = Grid::new(eye, weights::EFFECT_BOX, 3.0 * FINE_SIGMA, FINE_PER_HALF, LID_MAX, MAX_SIDE)?;
    let gate: Vec<[f32; 1]> = gate.iter().map(|g| [*g]).collect();
    let cell_px = fine.step * eye.half;
    // (2 × 2 samples: the fine blur averages the cells' neighbours too)
    let sub = if cell_px < 1.0 { 1 } else { FINE_SUB };
    let mut packed: Vec<[f32; 4]> = Vec::with_capacity(fine.w.checked_mul(fine.h)?);
    for j in 0..fine.h {
        for i in 0..fine.w {
            let (a, b) = fine.local(i, j);
            let (px, py) = eye.to_px(a, b);
            let mut samples = [[0.0f32; 3]; SUB * SUB];
            let mut got = 0;
            for sj in 0..sub {
                for si in 0..sub {
                    let (da, db) = (((si as f64 + 0.5) / sub as f64 - 0.5) * cell_px, ((sj as f64 + 0.5) / sub as f64 - 0.5) * cell_px);
                    let (sx, sy) = (px + da * eye.across.0 + db * eye.down.0, py + da * eye.across.1 + db * eye.down.1);
                    if let (Some(p), Some(slot)) = (sample(img, sx, sy), samples.get_mut(got)) {
                        *slot = p;
                        got += 1;
                    }
                }
            }
            let (c, _) = cell_colour(samples.get(..got).unwrap_or(&[]));
            let [g] = grid.at(&gate, a, b, [0.0]);
            packed.push([c[0] * g, c[1] * g, c[2] * g, g]);
        }
    }
    // small dark spots (freckles, a mole) closed over: they stay the skin's texture, not a level to lift from; a
    // circle's edge, a step, is kept where it is
    let packed = close_spots(&fine, &packed);
    let level = grid_blur(&packed, fine.w, fine.h, FINE_SIGMA / fine.step)
        .into_iter()
        .map(|l| if l[3] > 1e-4 { [l[0] / l[3], l[1] / l[3], l[2] / l[3]] } else { [0.0; 3] })
        .collect();
    Some((fine, level))
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
            let [s] = m.dark.as_ref().map_or([0.0], |d| grid.at(&d.strength, a, b, [0.0]));
            let [gb] = grid.at(&m.bags, a, b, [1.0]);
            // Dark Circles here: none where the skin is already as light as the skin outside the circle
            let lift = m.dark.as_ref().filter(|_| s > 0.0).map(|d| Lift {
                fine: d.fine.at(&d.level, a, b, [0.0; 3]),
                target: grid.at(&d.target, a, b, [0.0; 3]),
                strength: s,
                grain: grid.at(&d.grain, a, b, [1.0])[0],
            });
            let lift = lift.filter(|l| luma(l.target) > luma(l.fine));
            if lift.is_none() && gb == 1.0 {
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
            let q = match lift {
                Some(at) => {
                    // the texture of the clean skin below, for a deep shadow
                    let deep = at.deep();
                    let below = if deep > 0.0 {
                        let (sa, sb) = (a, b + BORROW_BELOW);
                        let (sx, sy) = eye.to_px(sa, sb);
                        Below::at(sample(img, sx, sy), grid.at(&m.low, sa, sb, [0.0; 3]))
                    } else {
                        None
                    };
                    at.apply(p, deep, below)
                }
                None => p,
            };
            let q = if gb == 1.0 { q } else { bag(q.map(|v| v * gb), grid.at(&m.bag_hue, a, b, [0.0; 3]), gb) };
            out.push((i, std::array::from_fn(|c| p[c] + keep * (q[c] - p[c]))));
        }
    }
    for (i, q) in out {
        if let Some(v) = img.data.get_mut(i) {
            *v = q;
        }
    }
}

/// The clean skin's texture below a deep shadow: a pixel's luminance against the skin's level there, and each
/// channel's share against the level's (its colour grain).
#[derive(Clone, Copy, Debug)]
struct Below {
    luma: f32,
    colour: [f32; 3],
}

impl Below {
    fn at(p: Option<[f32; 3]>, level: [f32; 3]) -> Option<Below> {
        let p = p?;
        let (py, ly) = (luma(p), luma(level));
        if !(py > 1e-6 && ly > 1e-6) {
            return None;
        }
        let colour = std::array::from_fn(|c| if level[c] > 1e-6 { (p[c] / py) / (level[c] / ly) } else { 1.0 });
        Some(Below { luma: py / ly, colour })
    }
}

/// Dark Circles at one pixel: the skin's fine level there, the colour it is lifted toward, the strength (0…1) and
/// the texture's scale.
#[derive(Clone, Copy, Debug)]
struct Lift {
    fine: [f32; 3],
    target: [f32; 3],
    strength: f32,
    grain: f32,
}

impl Lift {
    /// How deep a shadow the pixel lies in (0…1), as far as the slider lifts it.
    fn deep(&self) -> f32 {
        let (fy, ty) = (luma(self.fine), luma(self.target));
        if !(fy > 1e-6 && ty > fy && self.strength > 0.0) {
            return 0.0;
        }
        (1.0 - smoothstep(f64::from(DEEP.0), f64::from(DEEP.1), f64::from(fy / ty)) as f32) * self.strength.sqrt()
    }

    /// Pixel `p` lifted: its fine level taken `strength` of the way in lightness to the target's (never past it), its
    /// hue turned toward the target's (sooner than the light, and only where the skin is darker than the target), its
    /// texture against the fine level kept at the reference skin's strength (in a deep shadow the clean skin's
    /// texture `below` instead) and its colour detail faded as the lift grows. No channel darkens.
    fn apply(&self, p: [f32; 3], deep: f32, below: Option<Below>) -> [f32; 3] {
        let (f, t, s) = (self.fine, self.target, self.strength);
        let (py, fy, ty) = (luma(p), luma(f), luma(t));
        if !(py > 1e-6 && fy > 1e-6 && ty > fy && s > 0.0) {
            return p;
        }
        let yt = lifted_luma(fy, ty, s);
        let lift = (yt / fy).clamp(1.0, MAX_LIFT);
        let hue = (TINT_SHARE * s.powf(HUE_SOONER) * ((lstar(ty) - lstar(fy)) / HUE_GAP).clamp(0.0, 1.0)).max(deep);
        // (the skin's colour mixed with the target's at the same luminance: wholly the target's in a deep shadow,
        // whose own colour is the camera's noise)
        let base: [f32; 3] = std::array::from_fn(|c| yt * ((1.0 - hue) * f[c] / fy + hue * t[c] / ty));
        // the luminance texture: its own at the reference's strength, or for a deep shadow the clean skin's below it
        let d = (py / fy).clamp(0.05, 20.0);
        let own = (1.0 + s * (self.grain - 1.0)) * d.ln();
        let keep_colour = (-CHROMA_K * lift.ln()).exp() * (1.0 - deep);
        // (a colour detail kept by under 1 % is none: the powers are the costliest part of a pixel)
        let own_colour: [f32; 3] = if keep_colour < 0.01 {
            [1.0; 3]
        } else {
            std::array::from_fn(|c| if f[c] > 1e-6 { (p[c] / (f[c] * d)).clamp(0.25, 4.0).powf(keep_colour) } else { 1.0 })
        };
        let (texture, colour) = match below {
            Some(b) if b.luma > 0.0 && b.luma.is_finite() && deep > 0.0 => {
                let r = b.luma.ln();
                let share = deep * (1.0 - smoothstep(f64::from(BORROW_DETAIL.0), f64::from(BORROW_DETAIL.1), f64::from(r.abs())) as f32);
                let colour = std::array::from_fn(|c| own_colour[c] * (b.colour[c].clamp(0.5, 2.0)).powf(share));
                ((own + share * (r - own)).exp(), colour)
            }
            _ => (own.exp(), own_colour),
        };
        std::array::from_fn(|c| {
            let q = base[c] * texture * colour[c];
            if q.is_finite() { q.max(p[c]) } else { p[c] }
        })
    }
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
