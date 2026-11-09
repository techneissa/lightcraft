//! The band split that finds and lifts the lines of one region (see the module docs of [`super`]).
//!
//! The fine blur runs at full size. The two coarse blurs run on the region reduced by whole cells so that each is at
//! least [`PER_CELL`] cells wide, and are read back bilinearly; the cells are counted from the face's corner
//! ([`Sizes::grid`]), not from the region's, so a window of the photo cuts the same cells as the whole render.

use lightcraft_raster::blur::{gauss_taps, gaussian, gaussian_taps};
use lightcraft_raster::{Image, Plane, Rgb32f, par_rows};

use super::{DEEP, FINE};

/// The first blur's creases: a pixel this much darker (log) than its surroundings counts half in the second blur.
const CREASE: f32 = 0.03;
/// The share of a crease's fine dark core (finer than [`FINE`]) lifted with it, where the crease is at least
/// [`CORE_FULL`] deep. Pores outside creases are never touched.
const CORE: f32 = 0.5;
const CORE_FULL: f32 = 0.06;
/// A pixel darker (log) than the plain average of the skin around it by [`GATE_START`] or less is not changed (the
/// skin's own mottling and pores); one darker by [`GATE_FULL`] is wholly in a crease.
const GATE_START: f32 = 0.02;
const GATE_FULL: f32 = 0.07;
/// Most a crease's colour moves toward the skin around it (log, per channel).
const CHROMA: f32 = 0.3;
/// The smallest luminance (scene-linear) the log is taken of.
const DARK: f32 = 1e-4;
/// Blurs under this sigma (pixels) use exact Gaussian taps (the three-box blur snaps small sigmas to whole pixels).
const EXACT_BELOW: f32 = 2.0;
/// The smallest coarse blur, in cells, a reduction is made for (a blur of 2.5 cells or more read back bilinearly
/// differs from the full-size blur by well under a percent of the band).
const PER_CELL: f32 = 2.5;

/// The sizes one region is worked at.
#[derive(Clone, Copy, Debug)]
pub(super) struct Sizes {
    /// The face's size, in pixels.
    pub face: f64,
    /// The area's coarse blur, a fraction of the face ([`super::Area::coarse`]).
    pub coarse: f64,
    /// The region's top left pixel, counted from the face's grid corner (the floor of its box's corner).
    pub grid: [i64; 2],
}

/// One skin mask's pixels over a box, shared by every area it cuts: each channel's log (0 where the mask is 0) and
/// the fine blur of the masked log, built once for all of them.
pub(super) struct Skin<'a> {
    width: usize,
    mask: &'a Plane,
    log: Vec<[f32; 3]>,
    /// The fine blur over `fine_box` (x0, y0, x1, y1 in the box's pixels).
    fine: Image<[f32; 4]>,
    fine_box: [usize; 4],
}

impl<'a> Skin<'a> {
    /// The skin of the box `c` (its pixels) under mask `m` (the same size), with the fine blur over `inner` (the
    /// areas' bounds, in the box's pixels) and as far around it as it reads; `face` is the face's size in pixels.
    /// `None` when the sizes differ or `inner` is empty.
    pub(super) fn new(c: &Rgb32f, m: &'a Plane, inner: [usize; 4], face: f64) -> Option<Skin<'a>> {
        let (w, h) = (c.width, c.height);
        let [ix0, iy0, ix1, iy1] = [inner[0].min(w), inner[1].min(h), inner[2].min(w), inner[3].min(h)];
        if w == 0 || h == 0 || ix0 >= ix1 || iy0 >= iy1 || c.data.len() != m.data.len() || m.width != w {
            return None;
        }
        let mask = |i: usize| m.data.get(i).copied().unwrap_or(0.0);
        let mut log = vec![[0.0f32; 3]; c.data.len()];
        par_rows(&mut log, w, |y, row| {
            for (x, o) in row.iter_mut().enumerate() {
                let i = y * w + x;
                if mask(i) > 0.0 {
                    *o = c.data.get(i).map_or([0.0; 3], |p| p.map(|v| v.max(DARK).ln()));
                }
            }
        });
        let sigma = px(FINE, face);
        let pad = (3.0 * sigma).ceil() as usize + 1;
        let fine_box = [ix0.saturating_sub(pad), iy0.saturating_sub(pad), (ix1 + pad).min(w), (iy1 + pad).min(h)];
        let fw = fine_box[2] - fine_box[0];
        let fine = blur(
            &filled(fw, fine_box[3] - fine_box[1], |_, x, y| {
                let i = (fine_box[1] + y) * w + fine_box[0] + x;
                let (l, mv) = (log.get(i).copied().unwrap_or_default(), mask(i));
                [l[0] * mv, l[1] * mv, l[2] * mv, mv]
            }),
            sigma,
        );
        Some(Skin { width: w, mask: m, log, fine, fine_box })
    }

    fn mask(&self, i: usize) -> f32 {
        self.mask.data.get(i).copied().unwrap_or(0.0)
    }

    fn log(&self, i: usize) -> [f32; 3] {
        self.log.get(i).copied().unwrap_or_default()
    }

    /// The fine blur at pixel (`x`, `y`) of the box, back to log colour; `l` outside its box or where it is empty.
    fn fine(&self, x: usize, y: usize, l: [f32; 3]) -> [f32; 3] {
        let [fx0, fy0, fx1, fy1] = self.fine_box;
        if !(fx0..fx1).contains(&x) || !(fy0..fy1).contains(&y) {
            return l;
        }
        self.fine.data.get((y - fy0) * self.fine.width + x - fx0).map_or(l, |p| unweight(*p, l))
    }
}

/// `f` of the face's size `face`, in pixels (0 for a value that is not a number).
fn px(f: f64, face: f64) -> f32 {
    let v = (f * face) as f32;
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

/// Lift the lines of one area in `c` (the box `skin` was built from; areas run one after another on it, each from
/// the box's original pixels, their lifts multiplying where two meet): its coarse blurs' cells over `region` and
/// the change inside `inner` (x0, y0, x1, y1 in the box's pixels), where the area weight `a` (over `inner`) is
/// above 0, by `k` of the lines' depth. False when nothing changed. Pixels where the mask is 0 are skipped
/// throughout: they weigh nothing in the blurs and never change.
///
/// Each channel's log is split into bands, so a crease (darker and, light passing through skin, redder than the
/// skin around it) takes the colour of the skin around it, not only its brightness. Whether a pixel is in a crease,
/// and how deep, is read from the luminance of the bands. The region's margin around `inner` only feeds the coarse
/// blurs' cells; the change runs inside `inner`.
pub(super) fn reduce(c: &mut Rgb32f, skin: &Skin<'_>, a: &Plane, sizes: Sizes, region: [usize; 4], inner: [usize; 4], k: f32) -> bool {
    let (w, h) = (c.width, c.height);
    let [rx0, ry0, rx1, ry1] = [region[0].min(w), region[1].min(h), region[2].min(w), region[3].min(h)];
    let [ix0, iy0, ix1, iy1] = [inner[0].max(rx0), inner[1].max(ry0), inner[2].min(rx1), inner[3].min(ry1)];
    if skin.width != w || c.data.len() != skin.log.len() || ix0 >= ix1 || iy0 >= iy1 || a.width != inner[2].saturating_sub(inner[0]) {
        return false;
    }
    let (aw, ah) = (a.width, a.height);
    let weight = |x: usize, y: usize| if y - inner[1] < ah { a.data.get((y - inner[1]) * aw + x - inner[0]).copied().unwrap_or(0.0) } else { 0.0 };
    let weigh = |l: [f32; 3], mv: f32| [l[0] * mv, l[1] * mv, l[2] * mv, mv];
    // (the grid's pixel (x, y) is the box's (rx0 + x, ry0 + y))
    let at = |x: usize, y: usize| (ry0 + y) * w + rx0 + x;
    let sigma = px(sizes.coarse, sizes.face);
    let grid = Grid::new(rx1 - rx0, ry1 - ry0, sigma, sizes.grid);
    let first = grid.blur(
        grid.cells(|x, y, _| {
            let i = at(x, y);
            weigh(skin.log(i), skin.mask(i))
        }),
        sigma,
    );
    // (each cell's average log luminance, where it has skin)
    let means: Vec<Option<f32>> = first.data.iter().map(|p| (p[3] > 1e-6).then(|| luma([p[0] / p[3], p[1] / p[3], p[2] / p[3]]))).collect();
    // the second blur leaves out the pixels darker than their surroundings: the creases
    let second = grid.blur(
        grid.cells(|x, y, cell| {
            let i = at(x, y);
            let mv = skin.mask(i);
            if mv <= 0.0 {
                return [0.0; 4];
            }
            let l = skin.log(i);
            let d = means.get(cell).copied().flatten().map_or(0.0, |m| ((m - luma(l)) / CREASE).max(0.0));
            weigh(l, mv / (1.0 + d * d))
        }),
        sigma,
    );
    par_rows(&mut c.data, w, |y, row| {
        if !(iy0..iy1).contains(&y) {
            return;
        }
        for (x, o) in row.iter_mut().enumerate().take(ix1).skip(ix0) {
            let i = y * w + x;
            let wt = skin.mask(i) * weight(x, y);
            if wt <= 0.0 {
                continue;
            }
            let l = skin.log(i);
            let lf = skin.fine(x, y, l);
            // only a pixel darker than the plain average around it is in a crease; the rest of the skin stays
            let (gx, gy) = (x - rx0, y - ry0);
            let mean = unweight(grid.read(&first, gx, gy), lf);
            let gate = smooth01((luma(sub(mean, lf)) - GATE_START) / (GATE_FULL - GATE_START));
            let band = sub(unweight(grid.read(&second, gx, gy), lf), lf);
            let depth = luma(band);
            if gate <= 0.0 || depth.is_nan() || depth <= 0.0 {
                continue;
            }
            // the crease's core, finer than the fine blur, is lifted with it
            let core = luma(sub(lf, l)).max(0.0) * CORE * (depth / CORE_FULL).min(1.0);
            let f = k * wt * gate;
            let lift = f * depth * keep_deep(depth);
            // the crease takes the colour of the skin around it, by a bounded step
            let new: [f32; 3] = std::array::from_fn(|ch| o[ch] * (lift + f * ((band[ch] - depth).clamp(-CHROMA, CHROMA) + core)).exp());
            if new.iter().all(|v| v.is_finite()) {
                *o = new;
            }
        }
    });
    true
}

/// A `w × h` image of `f(index, x, y)`, filled in parallel.
fn filled(w: usize, h: usize, f: impl Fn(usize, usize, usize) -> [f32; 4] + Sync) -> Image<[f32; 4]> {
    let mut img = Image::<[f32; 4]>::new(w, h);
    par_rows(&mut img.data, w.max(1), |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            *o = f(y * w + x, x, y);
        }
    });
    img
}

/// The cells the coarse blurs run on: `factor × factor` pixels each, counted from the face's grid corner.
struct Grid {
    factor: usize,
    /// The region's top left pixel within its first cell (0…factor).
    offset: [usize; 2],
    /// The region's size in pixels, and in cells.
    size: [usize; 2],
    cells: [usize; 2],
}

impl Grid {
    fn new(w: usize, h: usize, sigma: f32, origin: [i64; 2]) -> Grid {
        let factor = if sigma.is_finite() { ((sigma / PER_CELL).floor() as usize).clamp(1, w.max(h).max(1)) } else { 1 };
        let f = factor as i64;
        let offset = origin.map(|o| o.rem_euclid(f) as usize);
        let cells = [(offset[0] + w).div_ceil(factor), (offset[1] + h).div_ceil(factor)];
        Grid { factor, offset, size: [w, h], cells }
    }

    /// The cells of `f(x, y, cell)` over the region's pixels (`cell`: the index of the cell that holds the pixel):
    /// each cell the mean of its `factor × factor` pixels (a cell cut by the region's edge counts the pixels it
    /// holds as that many of `factor²`).
    fn cells(&self, f: impl Fn(usize, usize, usize) -> [f32; 4] + Sync) -> Image<[f32; 4]> {
        let ([w, h], [cw, ch], k) = (self.size, self.cells, self.factor);
        let mut small = Image::<[f32; 4]>::new(cw, ch);
        let n = (k * k) as f32;
        par_rows(&mut small.data, cw.max(1), |cy, row| {
            let ys = (cy * k).saturating_sub(self.offset[1])..((cy + 1) * k).saturating_sub(self.offset[1]).min(h);
            for (cx, o) in row.iter_mut().enumerate() {
                let xs = (cx * k).saturating_sub(self.offset[0])..((cx + 1) * k).saturating_sub(self.offset[0]).min(w);
                let mut acc = [0.0f32; 4];
                for y in ys.clone() {
                    for x in xs.clone() {
                        let p = f(x, y, cy * cw + cx);
                        acc = std::array::from_fn(|c| acc[c] + p[c]);
                    }
                }
                *o = acc.map(|v| v / n);
            }
        });
        small
    }

    /// The cells blurred by `sigma` pixels.
    fn blur(&self, cells: Image<[f32; 4]>, sigma: f32) -> Image<[f32; 4]> {
        blur(&cells, sigma / self.factor as f32)
    }

    /// The blurred cells `b` read back at pixel (`x`, `y`) of the region, bilinearly.
    fn read(&self, b: &Image<[f32; 4]>, x: usize, y: usize) -> [f32; 4] {
        if self.factor == 1 {
            return b.data.get(y * b.width + x).copied().unwrap_or_default();
        }
        let ([cw, ch], f) = (self.cells, self.factor as f32);
        if cw == 0 || ch == 0 {
            return [0.0; 4];
        }
        let fx = (((x + self.offset[0]) as f32 + 0.5) / f - 0.5).clamp(0.0, (cw - 1) as f32);
        let fy = (((y + self.offset[1]) as f32 + 0.5) / f - 0.5).clamp(0.0, (ch - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(cw - 1), (y0 + 1).min(ch - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |cx: usize, cy: usize| b.data.get(cy * cw + cx).copied().unwrap_or_default();
        let (p00, p10, p01, p11) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        std::array::from_fn(|c| (p00[c] * (1.0 - tx) + p10[c] * tx) * (1.0 - ty) + (p01[c] * (1.0 - tx) + p11[c] * tx) * ty)
    }
}

/// Luminance weights (Rec.2020; close enough for the sRGB of the examples: only contrast is measured).
fn luma(p: [f32; 3]) -> f32 {
    p[0] * 0.2627 + p[1] * 0.678 + p[2] * 0.0593
}

/// How much of a lift is kept at depth `d` (log): 1 for a shallow line, ½ at [`DEEP`].
fn keep_deep(d: f32) -> f32 {
    let r = d / DEEP;
    1.0 / (1.0 + r * r)
}

/// A mask-weighted blur back to log colour; `fallback` where the mask's blur is about 0.
fn unweight(p: [f32; 4], fallback: [f32; 3]) -> [f32; 3] {
    if p[3] > 1e-6 { [p[0] / p[3], p[1] / p[3], p[2] / p[3]] } else { fallback }
}

/// `img` blurred by Gaussian `sigma` (pixels): exact taps under [`EXACT_BELOW`], three box passes above.
fn blur(img: &Image<[f32; 4]>, sigma: f32) -> Image<[f32; 4]> {
    if sigma < EXACT_BELOW && sigma.is_finite() && sigma > 0.0 { gaussian_taps(img, &gauss_taps(sigma)) } else { gaussian(img, sigma) }
}

/// 0 at or below 0, 1 at or above 1, smooth between.
fn smooth01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
