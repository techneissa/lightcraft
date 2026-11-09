//! The person's matte and the two colours of its soft edge.
//!
//! The matte is the largest Entire Person mask of every face at each pixel (the people parse's, with the hair edge
//! Settings ▸ Faces ▸ Hair Edges chooses, `fork/hair-edges.md`). At its soft edge a pixel is a mix,
//! `I = a·F + (1 − a)·U`, of the person's colour `F` and the background behind it `U`. Both are estimated by
//! blur-fusion foreground estimation (Forte and Pitié 2021), as a mask's Decontaminate does
//! (`fork/decontaminate.md`, `crates/pipeline/src/decontam/`): box means of the person's and the background's colours
//! in a wide and a narrow window (fractions of the photo's long side), corrected so that they mix back to the pixel.
//! The person is then drawn over a new background with `F`, so no old background colour stays between strands.

use lightcraft_raster::{Plane, Rgb32f, par_rows};

use crate::{Part, Scene};

/// Below and above these values of the matte a pixel is wholly background or wholly person.
pub(crate) const EDGE: (f32, f32) = (0.002, 0.998);
/// The wide and narrow windows, fractions of the photo's long side (as Decontaminate's).
pub(crate) const WIDE: f64 = 0.045;
pub(crate) const NARROW: f64 = 0.003;
/// Rows estimated together.
const BAND: usize = 512;

/// The matte of every person in the image's pixels (0 everywhere in a window of the render the person is not in);
/// `None` when no face has an Entire Person mask with a person in it.
pub(crate) fn person(scene: &Scene<'_>) -> Option<Plane> {
    let (w, h) = (scene.width, scene.height);
    let any =
        scene.faces.iter().filter_map(|f| f.face.mask(Part::EntirePerson)).any(|m| {
            m.area.is_valid() && m.plane.data.len() == m.plane.width.saturating_mul(m.plane.height) && m.plane.data.iter().any(|v| *v > EDGE.0)
        });
    if !any || w == 0 || h == 0 {
        return None;
    }
    let boxes: Vec<(usize, [usize; 4])> = (0..scene.faces.len()).filter_map(|i| Some((i, scene.mask_box(i, Part::EntirePerson)?))).collect();
    let view = scene.view();
    let mut a = Plane::new(w, h);
    par_rows(&mut a.data, w, |y, row| {
        for (i, b) in &boxes {
            if y < b[1] || y >= b[3] {
                continue;
            }
            let Some(m) = scene.faces.get(*i).and_then(|f| f.face.mask(Part::EntirePerson)) else { continue };
            for x in b[0]..b[2].min(w) {
                let (nx, ny) = view.to_norm(x as f64 + 0.5, y as f64 + 0.5);
                if let Some(v) = row.get_mut(x) {
                    *v = v.max(m.sample(nx, ny));
                }
            }
        }
    });
    Some(a)
}

/// One edge pixel: its index in the image and its person and background colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EdgePx {
    pub i: usize,
    pub person: [f32; 3],
    pub behind: [f32; 3],
}

/// The edge pixels of a matte, in image order, and where each row's start.
#[derive(Debug, Default)]
pub(crate) struct Edge {
    rows: Vec<usize>,
    px: Vec<EdgePx>,
}

impl Edge {
    /// The edge pixels of row `y`.
    pub fn row(&self, y: usize) -> &[EdgePx] {
        let (Some(&a), Some(&b)) = (self.rows.get(y), self.rows.get(y + 1)) else { return &[] };
        self.px.get(a..b).unwrap_or(&[])
    }

    pub fn bytes(&self) -> usize {
        self.px.len() * std::mem::size_of::<EdgePx>() + self.rows.len() * std::mem::size_of::<usize>()
    }
}

pub(crate) fn is_edge(v: f32) -> bool {
    v > EDGE.0 && v < EDGE.1
}

/// A rectangle of pixels, right and bottom exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Rect {
    fn w(self) -> usize {
        self.x1.saturating_sub(self.x0)
    }

    fn h(self) -> usize {
        self.y1.saturating_sub(self.y0)
    }

    fn grow(self, r: usize, w: usize, h: usize) -> Rect {
        Rect {
            x0: self.x0.saturating_sub(r),
            y0: self.y0.saturating_sub(r),
            x1: self.x1.saturating_add(r).min(w),
            y1: self.y1.saturating_add(r).min(h),
        }
    }
}

/// Colours over a rectangle.
struct Region {
    at: Rect,
    data: Vec<[f32; 3]>,
}

impl Region {
    fn get(&self, x: usize, y: usize) -> [f32; 3] {
        let (Some(rx), Some(ry)) = (x.checked_sub(self.at.x0), y.checked_sub(self.at.y0)) else { return [0.0; 3] };
        if rx >= self.at.w() {
            return [0.0; 3];
        }
        self.data.get(ry * self.at.w() + rx).copied().unwrap_or([0.0; 3])
    }
}

/// Box means of radius `r` of `v` (read over `inp`) at every pixel of `out`, each window clamped to `inp`.
fn box_mean(v: &(impl Fn(usize, usize) -> f32 + Sync), inp: Rect, out: Rect, r: usize) -> Vec<f32> {
    let (iw, ow) = (inp.w(), out.w());
    let mut rows = vec![0f64; inp.h() * ow];
    par_rows(&mut rows, ow, |ry, t| {
        let y = inp.y0 + ry;
        let mut prefix = Vec::with_capacity(iw + 1);
        let mut sum = 0f64;
        prefix.push(sum);
        for x in inp.x0..inp.x1 {
            sum += f64::from(v(x, y));
            prefix.push(sum);
        }
        for (ox, t) in t.iter_mut().enumerate() {
            let x = out.x0 + ox;
            let (a, b) = (x.saturating_sub(r).max(inp.x0), x.saturating_add(r + 1).min(inp.x1));
            let s = prefix.get(b - inp.x0).copied().unwrap_or(0.0) - prefix.get(a.min(b) - inp.x0).copied().unwrap_or(0.0);
            *t = s / b.saturating_sub(a).max(1) as f64;
        }
    });
    for ry in 1..inp.h() {
        let (done, rest) = rows.split_at_mut(ry * ow);
        if let (Some(prev), Some(row)) = (done.get((ry - 1) * ow..), rest.get_mut(..ow)) {
            row.iter_mut().zip(prev).for_each(|(v, p)| *v += *p);
        }
    }
    let mut o = vec![0f32; out.h() * ow];
    par_rows(&mut o, ow.max(1), |oy, row| {
        let y = out.y0 + oy;
        let (a, b) = (y.saturating_sub(r).max(inp.y0), y.saturating_add(r + 1).min(inp.y1));
        let n = b.saturating_sub(a).max(1) as f64;
        let sum_to = |end: usize, ox: usize| if end <= inp.y0 { 0.0 } else { rows.get((end - inp.y0 - 1) * ow + ox).copied().unwrap_or(0.0) };
        for (ox, v) in row.iter_mut().enumerate() {
            *v = ((sum_to(b, ox) - sum_to(a, ox)) / n) as f32;
        }
    });
    o
}

fn sane(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// One blur-fusion pass with window `r`, reading `inp` and estimating the colours at `out`.
fn pass(img: &Rgb32f, alpha: &Plane, prev: Option<(&Region, &Region)>, inp: Rect, out: Rect, r: usize) -> (Region, Region) {
    const EPS: f32 = 1e-5;
    let w = img.width;
    let photo = |x: usize, y: usize| img.data.get(y * w + x).copied().unwrap_or([0.0; 3]);
    let f_at = |x: usize, y: usize| prev.map_or_else(|| photo(x, y), |(f, _)| f.get(x, y));
    let u_at = |x: usize, y: usize| prev.map_or_else(|| photo(x, y), |(_, u)| u.get(x, y));
    let a = |x: usize, y: usize| sane(alpha.data.get(y * w + x).copied().unwrap_or(0.0));
    let ba = box_mean(&a, inp, out, r);
    let bf: [Vec<f32>; 3] = std::array::from_fn(|c| box_mean(&|x, y| f_at(x, y)[c] * a(x, y), inp, out, r));
    let bu: [Vec<f32>; 3] = std::array::from_fn(|c| box_mean(&|x, y| u_at(x, y)[c] * (1.0 - a(x, y)), inp, out, r));
    let n = out.w() * out.h();
    let (mut f, mut u) = (vec![[0.0f32; 3]; n], vec![[0.0f32; 3]; n]);
    for (i, (fp, up)) in f.iter_mut().zip(u.iter_mut()).enumerate() {
        let (x, y) = (out.x0 + i % out.w().max(1), out.y0 + i / out.w().max(1));
        let (al, m, px) = (a(x, y), ba.get(i).copied().unwrap_or(0.0), photo(x, y));
        for c in 0..3 {
            let ff = bf[c].get(i).copied().unwrap_or(0.0) / (m + EPS);
            let fu = bu[c].get(i).copied().unwrap_or(0.0) / (1.0 - m + EPS);
            let res = px[c] - al * ff - (1.0 - al) * fu;
            fp[c] = (ff + al * res).max(0.0);
            up[c] = (fu + (1.0 - al) * res).max(0.0);
        }
    }
    (Region { at: out, data: f }, Region { at: out, data: u })
}

/// The person and background colours of the edge pixels of `img` under `alpha` (same size), with windows for a photo
/// whose long side is `long_px` pixels.
pub(crate) fn edge(img: &Rgb32f, alpha: &Plane, long_px: f64) -> Edge {
    let (w, h) = (img.width, img.height);
    if alpha.width != w || alpha.height != h || w == 0 || h == 0 {
        return Edge::default();
    }
    let radius = |f: f64| ((f * long_px).round().max(1.0) as usize).min(w.max(h));
    let (rw, rn) = (radius(WIDE), radius(NARROW));
    let spans: Vec<Option<(usize, usize)>> = (0..h)
        .map(|y| {
            let row = alpha.data.get(y * w..(y + 1) * w)?;
            let first = row.iter().position(|v| is_edge(*v))?;
            let last = row.iter().rposition(|v| is_edge(*v)).unwrap_or(first);
            Some((first, last + 1))
        })
        .collect();
    let mut edge = Edge { rows: Vec::with_capacity(h + 1), px: Vec::new() };
    edge.rows.push(0);
    for y0 in (0..h).step_by(BAND) {
        let y1 = (y0 + BAND).min(h);
        let (x0, x1) = spans.get(y0..y1).unwrap_or(&[]).iter().flatten().fold((usize::MAX, 0), |(a, b), &(l, r)| (a.min(l), b.max(r)));
        if x1 <= x0 {
            edge.rows.extend(std::iter::repeat_n(edge.px.len(), y1 - y0));
            continue;
        }
        let o = Rect { x0, y0, x1, y1 };
        let p = o.grow(rn, w, h);
        let q = p.grow(rw, w, h);
        let (f1, u1) = pass(img, alpha, None, q, p, rw);
        let (f, u) = pass(img, alpha, Some((&f1, &u1)), p, o, rn);
        drop((f1, u1));
        for y in y0..y1 {
            for x in x0..x1 {
                let raw = alpha.data.get(y * w + x).copied().unwrap_or(0.0);
                if !is_edge(raw) {
                    continue;
                }
                let al = sane(raw);
                let px = img.data.get(y * w + x).copied().unwrap_or([0.0; 3]);
                let (mut fp, mut up) = (f.get(x, y), u.get(x, y));
                // the smallest change that makes `a·F + (1 − a)·U` the pixel, shared by the two colours
                let norm = al * al + (1.0 - al) * (1.0 - al);
                for c in 0..3 {
                    let r = (px[c] - (al * fp[c] + (1.0 - al) * up[c])) / norm;
                    fp[c] = (fp[c] + al * r).max(0.0);
                    up[c] = (up[c] + (1.0 - al) * r).max(0.0);
                }
                edge.px.push(EdgePx { i: y * w + x, person: fp, behind: up });
            }
            edge.rows.push(edge.px.len());
        }
    }
    edge
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A strand half over a blue sky: the person's colour comes out without the blue, the background's without the
    /// strand, and the two mix back to the pixel.
    #[test]
    fn an_edge_pixel_splits_into_the_person_and_what_is_behind_and_mixes_back() {
        let (w, h) = (120, 80);
        let hair = [0.30, 0.18, 0.08];
        let sky = [0.10, 0.30, 0.80];
        let alpha = Plane::from_fn(w, h, |x, _| ((x as f32 - 50.0) / 20.0).clamp(0.0, 1.0));
        let img = Rgb32f::from_fn(w, h, |x, y| {
            let a = alpha.get(x, y);
            std::array::from_fn(|c| a * hair[c] + (1.0 - a) * sky[c])
        });
        let e = edge(&img, &alpha, 1000.0);
        let row = e.row(40);
        assert!(!row.is_empty());
        for p in row {
            let a = alpha.data[p.i];
            let px = img.data[p.i];
            for c in 0..3 {
                assert!((a * p.person[c] + (1.0 - a) * p.behind[c] - px[c]).abs() < 1e-4);
            }
            if (0.3..0.7).contains(&a) {
                assert!((p.person[2] - hair[2]).abs() < 0.1, "no blue in the strand: {:?} at {a}", p.person);
                assert!((p.behind[0] - sky[0]).abs() < 0.1, "no hair in the sky: {:?}", p.behind);
            }
        }
        assert!(e.row(500).is_empty());
    }
}
