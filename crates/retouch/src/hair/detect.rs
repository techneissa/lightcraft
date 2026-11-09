//! Finding stray strands on the fine grid: thin lines that stand out from both sides alike, run on along their own
//! direction, and stand out from the texture around them. On the face, a strand also has the hair's polarity (darker
//! than the skin for dark hair) and skin on both sides: a beard's hairs, with beard beside them, are left alone.

use lightcraft_raster::blur::{gauss_taps, gaussian, gaussian_taps};
use lightcraft_raster::{Plane, Rgb32f, par_rows};

use super::grid::{Grid, bilinear, smoothstep};

/// A strand's blur, in faces (about a strand's half width in a portrait: a hair is a few pixels wide on a face
/// a thousand pixels tall).
pub const STRAND: f64 = 0.0025;
/// The fewest cells a strand's blur spans (a smaller face's strands are thinner than a cell).
const MIN_SIGMA: f64 = 0.7;
/// How far across a strand its sides are read: this many blurs, and a cell.
pub const SIDE: f64 = 2.5;
/// How much a line must stand out from its sides (cube-root luminance; a dark hair on skin is about 0.3, a crease
/// of the skin 0.04): its least, and the rise above that to wholly. A side that differs from the other counts
/// against.
const CONTRAST: (f32, f32) = (0.015, 0.04);
const ASYMMETRY: f32 = 1.0;
/// The least a line stands out on the face's skin, where creases and wrinkles are lines too.
const SKIN_CONTRAST: f32 = 0.05;
/// A strand must stand out this many times the texture around it.
const NOISE: f32 = 1.5;
/// A cell that stands out less than this from its sides is not on a line (it measures the texture).
const QUIET: f32 = 0.01;
/// The fine detail that measures texture: the tone less its blur by this many strand blurs.
const TEXTURE_BLUR: f64 = 3.0;
/// How line-like (1 − the smaller curvature over the larger): a spot or a pore is not a strand.
const LINE: (f32, f32) = (0.45, 0.75);
/// How far along its direction a strand must run, in cells either side, and how much of that must be strand.
const RUN: isize = 5;
const RUN_SHARE: (f32, f32) = (0.05, 0.15);
/// The blur that measures the texture around a cell (in faces), and the texture at which the lines there are a
/// texture and not stray strands (not at all, then wholly).
const DENSITY_SIGMA: f64 = 0.03;
const DENSITY: (f32, f32) = (0.035, 0.06);
/// On the face: how much darker than the skin's tone a strand's sides may be (wholly, then not at all), so a beard
/// or a shadow is not taken for skin.
const BESIDE_SKIN: (f32, f32) = (0.06, 0.12);
/// A beard: skin this much darker than the lit skin's tone, over this share of the skin around (none, then wholly).
const BEARD_TONE: f32 = 0.15;
const BEARD: (f32, f32) = (0.12, 0.25);
/// The skin's tone is this percentile of the Facial Skin's (the lit skin: a beard and shadows are darker).
const SKIN_SHARE: f64 = 0.75;
/// The surety at which a cell is wholly strand (a strand is repaired wholly, not faded).
const SURE: (f32, f32) = (0.04, 0.3);
/// How far a kept strand is grown, in cells.
const GROW: isize = 2;
/// Cells whose search weight is below this are not searched.
const MIN_WEIGHT: f32 = 0.02;

/// Where to look, per cell of the fine grid (see [`super::region::Area`]).
pub(super) struct Near<'a> {
    pub weight: &'a [f32],
    pub skin: &'a [f32],
    pub core: &'a [f32],
}

/// What the search found per cell of the fine grid.
pub(super) struct Strands {
    /// How surely a strand covers the cell (0…1), grown by a cell and softened.
    pub weight: Vec<f32>,
    /// The direction across the strand (unit), from the surest strand cell beside it.
    pub across: Vec<(f32, f32)>,
    /// +1 for a strand lighter than its sides, −1 for a darker one.
    pub polarity: Vec<f32>,
    /// The strand's blur, in cells.
    pub sigma: f64,
}

/// One cell's line measures.
#[derive(Clone, Copy, Default)]
struct Cell {
    /// 1 − the smaller curvature over the larger.
    line: f32,
    /// How much it stands out from its sides (0 or more).
    stands: f32,
    /// Its sides' mean tone.
    beside: f32,
    across: (f32, f32),
    polarity: f32,
}

/// The strands of the fine `grid`, where `near` says to look.
pub(super) fn strands(img: &Rgb32f, grid: &Grid, near: &Near<'_>) -> Strands {
    let (w, h) = (grid.w, grid.h);
    let sigma = grid.cells(STRAND).max(MIN_SIGMA);
    let mut out = Strands { weight: vec![0.0; grid.len()], across: vec![(1.0, 0.0); grid.len()], polarity: vec![0.0; grid.len()], sigma };
    let n = grid.len();
    if near.weight.len() != n || near.skin.len() != n || near.core.len() != n || !near.weight.iter().any(|v| *v > MIN_WEIGHT) {
        return out;
    }
    let weight = |k: usize| near.weight.get(k).copied().unwrap_or(0.0);
    let g = tone(img, grid);
    let cells = measure(&g, sigma, near.weight);
    // the texture around each cell: the mean fine detail over the cells searched that are not on a line (the main
    // hair mass, what is not searched and the strands themselves stay out of it: flyaways against a clear sky are not
    // a texture, a beard's mottle is)
    let fine = g.zip_map(&blur(&g, TEXTURE_BLUR * sigma), |a, b| (a - b).abs());
    let quiet = Plane::from_fn(w, h, |x, y| {
        let k = y * w + x;
        if weight(k) > MIN_WEIGHT && cells.get(k).is_some_and(|c| c.stands < QUIET) { 1.0 } else { 0.0 }
    });
    let spread = grid.cells(DENSITY_SIGMA) as f32;
    let texture = gaussian(&fine.zip_map(&quiet, |a, b| a * b), spread).zip_map(&gaussian(&quiet, spread), |a, b| if b > 1e-3 { a / b } else { 0.0 });
    // the face's lit skin tone (a beard and the shadows are darker) and the hair's, measured over the face's grid,
    // so a window and a draft measure the same
    let skin_tone = percentile(&g.data, near.skin, 0.8, SKIN_SHARE);
    let hair_tone = percentile(&g.data, near.core, 0.8, 0.5);
    // on the face, only strands of the hair's polarity (dark hair on lighter skin: dark strands)
    let face_polarity = match (skin_tone, hair_tone) {
        (Some(s), Some(h)) if (h - s).abs() > 0.05 => (h - s).signum(),
        _ => -1.0,
    };
    // a beard or stubble: much of the skin around is darker than the lit skin
    let dark = skin_tone.map(|t| {
        let skin = Plane::from_fn(w, h, |x, y| if near.skin.get(y * w + x).copied().unwrap_or(0.0) > 0.5 { 1.0 } else { 0.0 });
        let dark = g.zip_map(&skin, |v, s| if s > 0.0 && v < t - BEARD_TONE { 1.0 } else { 0.0 });
        gaussian(&dark, spread).zip_map(&gaussian(&skin, spread), |a, b| if b > 1e-3 { a / b } else { 0.0 })
    });
    let score: Vec<f32> = cells
        .iter()
        .enumerate()
        .map(|(k, c)| {
            let on_skin = near.skin.get(k).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            let least = CONTRAST.0 + on_skin * (SKIN_CONTRAST - CONTRAST.0);
            let floor = least.max(NOISE * texture.data.get(k).copied().unwrap_or(0.0));
            let mut s = smoothstep(floor, floor + CONTRAST.1, c.stands) * smoothstep(LINE.0, LINE.1, c.line);
            if on_skin > 0.0 {
                let skin_ok = skin_tone.map_or(1.0, |t| 1.0 - smoothstep(BESIDE_SKIN.0, BESIDE_SKIN.1, t - c.beside));
                let pol_ok = if c.polarity == face_polarity { 1.0 } else { 0.0 };
                let beard = dark.as_ref().and_then(|d| d.data.get(k)).copied().unwrap_or(0.0);
                let skin_ok = skin_ok * (1.0 - smoothstep(BEARD.0, BEARD.1, beard));
                s *= 1.0 - on_skin * (1.0 - skin_ok * pol_ok);
            }
            if s.is_finite() { s } else { 0.0 }
        })
        .collect();
    // keep lines that run on along their direction and are not part of a dense texture
    let mut kept = vec![0.0f32; n];
    par_rows(&mut kept, w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let k = y * w + x;
            let s = score.get(k).copied().unwrap_or(0.0);
            let Some(c) = cells.get(k) else { continue };
            if s <= 0.0 {
                continue;
            }
            let (tx, ty) = (-f64::from(c.across.1), f64::from(c.across.0));
            let run: f32 =
                (-RUN..=RUN).map(|t| bilinear(&score, w, h, x as f64 + t as f64 * tx, y as f64 + t as f64 * ty)).sum::<f32>() / (2 * RUN + 1) as f32;
            let dense = texture.data.get(k).copied().unwrap_or(0.0);
            let v = s * smoothstep(RUN_SHARE.0, RUN_SHARE.1, run) * (1.0 - smoothstep(DENSITY.0, DENSITY.1, dense));
            let v = smoothstep(SURE.0, SURE.1, v) * weight(k);
            *o = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        }
    });
    for (k, c) in cells.iter().enumerate() {
        if let (Some(a), Some(p)) = (out.across.get_mut(k), out.polarity.get_mut(k)) {
            *a = c.across;
            *p = c.polarity;
        }
    }
    grow(&mut out, &kept, w, h);
    out
}

/// Each cell's line measures, on cells whose search weight is above [`MIN_WEIGHT`] (the others stay zero).
fn measure(g: &Plane, sigma: f64, weight: &[f32]) -> Vec<Cell> {
    let (w, h) = (g.width, g.height);
    let smooth = blur(g, sigma);
    let side = SIDE * sigma + 1.0;
    let s2 = (sigma * sigma) as f32;
    let mut cells = vec![Cell::default(); w * h];
    par_rows(&mut cells, w, |y, row| {
        if y == 0 || y + 1 >= h {
            return;
        }
        for (x, o) in row.iter_mut().enumerate() {
            if x == 0 || x + 1 >= w || weight.get(y * w + x).copied().unwrap_or(0.0) <= MIN_WEIGHT {
                continue;
            }
            let at = |dx: isize, dy: isize| smooth.get((x as isize + dx) as usize, (y as isize + dy) as usize);
            let c = at(0, 0);
            let hxx = at(1, 0) - 2.0 * c + at(-1, 0);
            let hyy = at(0, 1) - 2.0 * c + at(0, -1);
            let hxy = (at(1, 1) - at(1, -1) - at(-1, 1) + at(-1, -1)) / 4.0;
            let mean = (hxx + hyy) / 2.0;
            let r = (((hxx - hyy) / 2.0).powi(2) + hxy * hxy).sqrt();
            let (hi, lo) = (mean + r, mean - r);
            let (l1, l2) = if hi.abs() >= lo.abs() { (hi, lo) } else { (lo, hi) };
            if l1.abs() * s2 <= 1e-5 || !l1.is_finite() {
                continue;
            }
            // the eigenvector of `hi` is at angle θ; `lo`'s is across it
            let theta = 0.5 * (2.0 * hxy).atan2(hxx - hyy);
            let (nx, ny) = if l1 == hi { (theta.cos(), theta.sin()) } else { (-theta.sin(), theta.cos()) };
            // a valley (curving up across it) is a dark strand
            let polarity = if l1 > 0.0 { -1.0 } else { 1.0 };
            let (cx, cy) = (x as f64, y as f64);
            let read = |d: f64| bilinear(&g.data, w, h, cx + d * f64::from(nx), cy + d * f64::from(ny));
            let (a, b, mid) = (read(side), read(-side), g.get(x, y));
            let stands = polarity * (mid - (a + b) / 2.0) - ASYMMETRY * (a - b).abs();
            let line = 1.0 - l2.abs() / l1.abs();
            if line.is_finite() && stands.is_finite() {
                *o = Cell { line, stands: stands.max(0.0), beside: (a + b) / 2.0, across: (nx, ny), polarity };
            }
        }
    });
    cells
}

/// The `share` percentile (0…1) of `v` over the cells where `mask` is above `at` (a 512-bin histogram over 0…2);
/// `None` for none.
fn percentile(v: &[f32], mask: &[f32], at: f32, share: f64) -> Option<f32> {
    const BINS: usize = 512;
    const TOP: f32 = 2.0;
    let mut hist = [0u32; BINS];
    let mut count = 0u64;
    for (x, m) in v.iter().zip(mask) {
        if *m > at && x.is_finite() {
            let b = ((x / TOP).clamp(0.0, 1.0) * (BINS - 1) as f32) as usize;
            if let Some(h) = hist.get_mut(b) {
                *h += 1;
                count += 1;
            }
        }
    }
    let mut seen = 0u64;
    for (b, n) in hist.iter().enumerate() {
        seen += u64::from(*n);
        if count > 0 && seen as f64 >= share * count as f64 {
            return Some((b as f32 + 0.5) / (BINS - 1) as f32 * TOP);
        }
    }
    None
}

/// The cube root of each cell's luminance (linear Rec.2020), the image averaged over the cell.
fn tone(img: &Rgb32f, grid: &Grid) -> Plane {
    let sub = grid.cell.round().clamp(1.0, 4.0) as usize;
    let mut g = Plane::new(grid.w, grid.h);
    par_rows(&mut g.data, grid.w, |j, row| {
        for (i, o) in row.iter_mut().enumerate() {
            let (x, y) = grid.centre(i, j);
            let mut sum = 0.0f32;
            for sy in 0..sub {
                for sx in 0..sub {
                    let dx = ((sx as f64 + 0.5) / sub as f64 - 0.5) * grid.cell;
                    let dy = ((sy as f64 + 0.5) / sub as f64 - 0.5) * grid.cell;
                    let p = img.sample_bilinear((x + dx) as f32, (y + dy) as f32);
                    sum += 0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2];
                }
            }
            let l = sum / (sub * sub) as f32;
            *o = if l.is_finite() { l.max(0.0).cbrt() } else { 0.0 };
        }
    });
    g
}

/// Blur with a Gaussian of `sigma` cells (exact taps for a small one).
fn blur(p: &Plane, sigma: f64) -> Plane {
    let s = sigma as f32;
    if !(s.is_finite() && s > 0.05) {
        return p.clone();
    }
    if s < 2.5 { gaussian_taps(p, &gauss_taps(s)) } else { gaussian(p, s) }
}

/// Grow the kept strands by [`GROW`] cells (each cell takes the surest around it) and soften them, so a strand's soft
/// edge and the sharpening halo beside it are covered. Each cell the softened weight reaches takes the direction and
/// polarity of the surest strand cell within a cell more (the softening's reach); a cell with none has no polarity,
/// and is never repaired.
fn grow(out: &mut Strands, kept: &[f32], w: usize, h: usize) {
    let surest = |x: usize, y: usize, r: isize| -> (f32, usize) {
        let mut best = (0.0f32, y * w + x);
        for dy in -r..=r {
            for dx in -r..=r {
                let (sx, sy) = (x as isize + dx, y as isize + dy);
                if sx < 0 || sy < 0 || sx >= w as isize || sy >= h as isize {
                    continue;
                }
                let k = sy as usize * w + sx as usize;
                let v = kept.get(k).copied().unwrap_or(0.0);
                if v > best.0 {
                    best = (v, k);
                }
            }
        }
        best
    };
    let mut weight = vec![0.0f32; w * h];
    let mut across = vec![(1.0f32, 0.0f32); w * h];
    let mut polarity = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let k = y * w + x;
            let near = surest(x, y, GROW);
            let source = surest(x, y, GROW + 1);
            if let (Some(o), Some(a), Some(p)) = (weight.get_mut(k), across.get_mut(k), polarity.get_mut(k)) {
                *o = near.0;
                if source.0 > 0.0 {
                    *a = out.across.get(source.1).copied().unwrap_or((1.0, 0.0));
                    *p = out.polarity.get(source.1).copied().unwrap_or(0.0);
                }
            }
        }
    }
    let soft = gaussian_taps(&Plane { width: w, height: h, data: weight }, &gauss_taps(0.7));
    out.weight = soft.data;
    out.across = across;
    out.polarity = polarity;
}
