//! Fork feature (`fork/glare-highlights.md`): finding glare, the Highlights mode's mask.
//!
//! Glare is a bright, nearly colourless spot that stands out from what is around it: a lamp on a lens, flash on
//! glass, shine on skin or a glossy product. [`find`] marks pixels that are near the brightest of the area, much
//! brighter than their surroundings and low in colour, grows each such seed over its softer edge, drops spots too small
//! (catchlights in eyes) or too large (a white wall), then grows the result by Expand and feathers it. Strokes the user
//! painted add to it or take from it last.
//!
//! Pure and deterministic: the same pixels and settings give the same mask, whatever the thread count.

use lightcraft_raster::{Plane, Rgb32f};

/// What [`find`] works with besides the pixels.
#[derive(Clone, Debug)]
pub struct Params {
    /// 0..=100, higher finds less.
    pub threshold: f64,
    /// 0..=100, how far the found spots grow.
    pub expand: f64,
    /// The long edge of the whole photo, in this picture's pixels: sizes are relative to it, so a preview-sized and
    /// a full-sized run find the same spots.
    pub photo_long: f64,
    /// Largest spot kept, as a fraction of the area searched.
    pub max_spot: f64,
    /// Whether the user chose where to look (a box or a mask): the glare then only has to be the area's brightest,
    /// not near white (a reflection on dark sunglasses), as the user has said where it is.
    pub chosen: bool,
    /// The picture is a box the user drew: a spot that runs into its edge goes on outside it (a white wall, the
    /// sky), so it is not glare.
    pub boxed: bool,
}

/// One painted stroke, in this picture's pixels.
#[derive(Clone, Debug)]
pub struct Stroke {
    pub erase: bool,
    pub radius: f32,
    pub points: Vec<(f32, f32)>,
}

/// Rec. 2020 luminance.
fn luma(p: [f32; 3]) -> f32 {
    (0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2]).max(0.0)
}

/// How colourful a pixel is: 0 for grey or white, 1 for a pure colour.
fn chroma(p: [f32; 3]) -> f32 {
    let hi = p[0].max(p[1]).max(p[2]);
    let lo = p[0].min(p[1]).min(p[2]).max(0.0);
    if hi <= 1e-6 { 0.0 } else { ((hi - lo) / hi).clamp(0.0, 1.0) }
}

/// The finder's limits for a Threshold of `t` (0..=1).
struct Limits {
    /// Share of the area's white a seed reaches.
    share: f32,
    /// Least linear brightness of a seed: glare is near clipping, whatever the area's brightest is.
    floor: f32,
    /// How much brighter than its surroundings a seed, and a whole spot than the ring around it, is.
    contrast: f32,
    /// Largest chroma of a seed.
    chroma: f32,
}

fn limits(t: f32) -> Limits {
    Limits { share: 0.55 + 0.4 * t, floor: 0.40 + 0.4 * t, contrast: 1.15 + 0.7 * t, chroma: 0.40 - 0.25 * t }
}

/// The glare mask of `img` (0..=1 per pixel), searched where `region` (same size) is above one half, or everywhere.
pub fn find(img: &Rgb32f, region: Option<&Plane>, p: &Params, strokes: &[Stroke]) -> Plane {
    let (w, h) = (img.width, img.height);
    let mut out = Plane::new(w, h);
    if w == 0 || h == 0 || img.data.len() != w * h || region.is_some_and(|r| r.width != w || r.height != h) {
        return out;
    }
    let inside = |i: usize| region.is_none_or(|r| r.data.get(i).copied().unwrap_or(0.0) > 0.5);
    let y: Vec<f32> = img.data.iter().map(|px| luma(*px)).collect();
    let c: Vec<f32> = img.data.iter().map(|px| chroma(*px)).collect();
    let searched: Vec<f32> = y.iter().enumerate().filter(|(i, _)| inside(*i)).map(|(_, v)| *v).collect();
    let white = percentile(searched, 0.998).max(1e-4);
    let unit = |v: f64, d: f64| if v.is_finite() { (v / 100.0).clamp(0.0, 1.0) } else { d };
    let t = unit(p.threshold, 0.5) as f32;
    let mut lim = limits(t);
    if p.chosen {
        lim.floor *= 0.35;
    }
    let (share, contrast, max_chroma) = (lim.share, lim.contrast, lim.chroma);
    let long = if p.photo_long.is_finite() && p.photo_long > 0.0 { p.photo_long } else { w.max(h) as f64 };
    // the surroundings: a box mean wide enough to hold a lens's glare, taken again with the glare held down to the
    // first mean, so a large bright spot does not raise its own surroundings
    let r_bg = ((0.03 * long).round() as usize).clamp(4, w.max(h));
    let first = box_mean(&y, w, h, r_bg);
    let held: Vec<f32> = y.iter().zip(&first).map(|(v, b)| v.min(*b)).collect();
    let bg = box_mean(&held, w, h, 2 * r_bg);
    let strong = |i: usize| {
        let (v, b) = (y[i], bg[i]);
        inside(i) && v >= share * white && v >= lim.floor && v >= contrast * b && c[i] <= max_chroma
    };
    // the soft edge of a spot: a little darker and more colourful than its core, still well above its surroundings
    let weak = |i: usize| {
        let (v, b) = (y[i], bg[i]);
        inside(i) && v >= 0.8 * share * white && v >= 0.75 * lim.floor && v >= 1.05 * b && c[i] <= (max_chroma * 1.4).min(0.6)
    };
    let mut mask = vec![false; w * h];
    let mut queue: Vec<usize> = (0..w * h).filter(|&i| strong(i)).collect();
    for &i in &queue {
        mask[i] = true;
    }
    while let Some(i) = queue.pop() {
        for j in neighbours(i, w, h) {
            if !mask[j] && weak(j) {
                mask[j] = true;
                queue.push(j);
            }
        }
    }
    let area = region.map_or(w * h, |r| r.data.iter().filter(|v| **v > 0.5).count()).max(1);
    let min_side = 0.003 * long;
    let min_px = ((min_side * min_side) as usize).max(4);
    let max_spot = if p.max_spot.is_finite() { p.max_spot.clamp(0.0, 1.0) } else { 0.25 };
    let max_px = ((max_spot * area as f64) as usize).max(min_px);
    let ring = Ring { luma: &y, contrast, max_width: ((0.02 * long) as usize).max(2), drop_at_edge: p.boxed && region.is_none() };
    keep_spots(&mut mask, w, h, min_px, max_px, &ring);
    // Expand: a near-circular growth (chamfer distance), then a feathered edge as wide as half of it
    let grow = 1.0 + unit(p.expand, 0.3) as f32 * (0.008 * long) as f32;
    let feather = (grow * 0.5).max(1.0);
    let dist = distance(&mask, w, h, grow + feather + 1.0);
    for (o, d) in out.data.iter_mut().zip(&dist) {
        *o = 1.0 - smoothstep(grow, grow + feather, *d);
    }
    paint(&mut out, strokes);
    if let Some(r) = region {
        // a mask region's own soft edge
        for (o, a) in out.data.iter_mut().zip(&r.data) {
            *o *= a.clamp(0.0, 1.0);
        }
    }
    out
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The value below which `q` of `values` lie (0 for none).
fn percentile(mut values: Vec<f32>, q: f64) -> f32 {
    values.retain(|v| v.is_finite());
    if values.is_empty() {
        return 0.0;
    }
    let k = ((values.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize;
    let k = k.min(values.len() - 1);
    let (_, v, _) = values.select_nth_unstable_by(k, f32::total_cmp);
    *v
}

/// Eight neighbours of pixel `i` inside the picture.
fn neighbours(i: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (x, y) = ((i % w) as isize, (i / w) as isize);
    [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)].into_iter().filter_map(move |(dx, dy)| {
        let (nx, ny) = (x + dx, y + dy);
        (nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h).then(|| ny as usize * w + nx as usize)
    })
}

/// Mean of `v` over a `(2r+1)²` box (clipped at the edges), by a summed-area table.
fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let stride = w + 1;
    let mut sat = vec![0.0f64; stride * (h + 1)];
    for y in 0..h {
        let mut row = 0.0f64;
        for x in 0..w {
            row += v.get(y * w + x).copied().unwrap_or(0.0) as f64;
            sat[(y + 1) * stride + x + 1] = sat[y * stride + x + 1] + row;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = sat[y1 * stride + x1] - sat[y0 * stride + x1] - sat[y1 * stride + x0] + sat[y0 * stride + x0];
            let n = ((y1 - y0) * (x1 - x0)).max(1) as f64;
            out[y * w + x] = (s / n) as f32;
        }
    }
    out
}

/// The test of a spot against the ring around it.
struct Ring<'a> {
    luma: &'a [f32],
    /// How much brighter than its ring a spot's mean is.
    contrast: f32,
    /// Widest ring, pixels.
    max_width: usize,
    /// Drop spots that touch the picture's edge.
    drop_at_edge: bool,
}

impl Ring<'_> {
    /// Whether `spot` (pixel indices of one spot of `mask`) is brighter than the ring around it by the contrast: a ring
    /// half as wide as the spot's size, at least 2 pixels. A white wall broken into pieces has white around each piece;
    /// glare has a darker lens, skin or paint.
    fn stands_out(&self, spot: &[usize], mask: &[bool], stamp: &mut [u32], id: u32, (w, h): (usize, usize)) -> bool {
        let width = ((spot.len() as f64).sqrt() * 0.5).round().clamp(2.0, self.max_width.max(2) as f64) as usize;
        let (mut sum, mut n) = (0.0f64, 0usize);
        let mut front: Vec<usize> = spot.to_vec();
        for &i in spot {
            stamp[i] = id;
        }
        for _ in 0..width {
            let mut next = Vec::new();
            for &i in &front {
                for j in neighbours(i, w, h) {
                    if stamp[j] != id && !mask[j] {
                        stamp[j] = id;
                        sum += self.luma.get(j).copied().unwrap_or(0.0) as f64;
                        n += 1;
                        next.push(j);
                    }
                }
            }
            front = next;
        }
        if n == 0 {
            return true;
        }
        let inner = spot.iter().map(|&i| self.luma.get(i).copied().unwrap_or(0.0) as f64).sum::<f64>() / spot.len().max(1) as f64;
        inner >= self.contrast as f64 * (sum / n as f64)
    }
}

/// Keep the 8-connected spots of `mask` whose size is within `min..=max` pixels and that stand out from their ring.
fn keep_spots(mask: &mut [bool], w: usize, h: usize, min: usize, max: usize, ring: &Ring<'_>) {
    let mut seen = vec![false; w * h];
    let mut stamp = vec![0u32; w * h];
    let mut id = 0u32;
    let mut drop = Vec::new();
    let mut spot = Vec::new();
    for start in 0..w * h {
        if !mask[start] || seen[start] {
            continue;
        }
        spot.clear();
        seen[start] = true;
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            spot.push(i);
            for j in neighbours(i, w, h) {
                if mask[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        id = id.wrapping_add(1).max(1);
        let at_edge = ring.drop_at_edge && spot.iter().any(|&i| i % w == 0 || i / w == 0 || i % w + 1 == w || i / w + 1 == h);
        if at_edge || spot.len() < min || spot.len() > max || !ring.stands_out(&spot, mask, &mut stamp, id, (w, h)) {
            drop.extend_from_slice(&spot);
        }
    }
    for i in drop {
        mask[i] = false;
    }
}

/// Distance of every pixel to the nearest set pixel of `mask` (3-4 chamfer, in pixels), at most `cap`.
fn distance(mask: &[bool], w: usize, h: usize, cap: f32) -> Vec<f32> {
    let big = cap.max(1.0) * 3.0 + 4.0;
    let mut d: Vec<f32> = mask.iter().map(|m| if *m { 0.0 } else { big }).collect();
    let at = |d: &[f32], x: isize, y: isize| -> f32 {
        if x < 0 || y < 0 || x as usize >= w || y as usize >= h { big } else { d[y as usize * w + x as usize] }
    };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let i = y as usize * w + x as usize;
            let v = d[i].min(at(&d, x - 1, y) + 3.0).min(at(&d, x, y - 1) + 3.0).min(at(&d, x - 1, y - 1) + 4.0).min(at(&d, x + 1, y - 1) + 4.0);
            d[i] = v;
        }
    }
    for y in (0..h as isize).rev() {
        for x in (0..w as isize).rev() {
            let i = y as usize * w + x as usize;
            let v = d[i].min(at(&d, x + 1, y) + 3.0).min(at(&d, x, y + 1) + 3.0).min(at(&d, x + 1, y + 1) + 4.0).min(at(&d, x - 1, y + 1) + 4.0);
            d[i] = v;
        }
    }
    d.iter_mut().for_each(|v| *v = (*v / 3.0).min(cap));
    d
}

/// Draw the painted strokes over `alpha`: an added stroke raises it to the stroke's own soft disc, an erased one
/// lowers it by as much.
fn paint(alpha: &mut Plane, strokes: &[Stroke]) {
    let (w, h) = (alpha.width, alpha.height);
    for s in strokes {
        let r = s.radius.max(0.5);
        if !r.is_finite() || s.points.is_empty() {
            continue;
        }
        let inner = r * 0.8;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &s.points {
            x0 = x0.min(p.0 - r);
            y0 = y0.min(p.1 - r);
            x1 = x1.max(p.0 + r);
            y1 = y1.max(p.1 + r);
        }
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
            continue;
        }
        let (bx0, by0) = (x0.floor().max(0.0) as usize, y0.floor().max(0.0) as usize);
        let (bx1, by1) = ((x1.ceil().max(0.0) as usize).min(w), (y1.ceil().max(0.0) as usize).min(h));
        for y in by0..by1 {
            for x in bx0..bx1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = segments_distance(&s.points, px, py);
                let a = 1.0 - smoothstep(inner, r, d);
                if a <= 0.0 {
                    continue;
                }
                let i = y * w + x;
                if let Some(v) = alpha.data.get_mut(i) {
                    *v = if s.erase { *v * (1.0 - a) } else { v.max(a) };
                }
            }
        }
    }
}

/// Distance from `(x, y)` to the polyline through `pts`.
fn segments_distance(pts: &[(f32, f32)], x: f32, y: f32) -> f32 {
    let point = |a: (f32, f32)| ((x - a.0).powi(2) + (y - a.1).powi(2)).sqrt();
    let Some(first) = pts.first() else { return f32::MAX };
    let mut best = point(*first);
    for pair in pts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 { (((x - a.0) * dx + (y - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
        best = best.min(point((a.0 + t * dx, a.1 + t * dy)));
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mid-grey picture (skin-like orange tint) with a white disc of radius `r` at `(cx, cy)`.
    fn picture(w: usize, h: usize, spots: &[(f32, f32, f32, [f32; 3])]) -> Rgb32f {
        Rgb32f::from_fn(w, h, |x, y| {
            for (cx, cy, r, c) in spots {
                if ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt() <= *r {
                    return *c;
                }
            }
            [0.30, 0.20, 0.14]
        })
    }

    fn params() -> Params {
        Params { threshold: 50.0, expand: 0.0, photo_long: 400.0, max_spot: 0.25, chosen: false, boxed: false }
    }

    #[test]
    fn a_white_spot_is_found_and_its_surroundings_are_not() {
        let img = picture(400, 300, &[(200.0, 150.0, 12.0, [1.0, 1.0, 0.98])]);
        let a = find(&img, None, &params(), &[]);
        assert!(a.get(200, 150) > 0.99);
        assert_eq!(a.get(20, 20), 0.0);
        assert_eq!(a.get(200, 180), 0.0);
    }

    #[test]
    fn a_bright_colourful_spot_is_not_glare() {
        let img = picture(400, 300, &[(200.0, 150.0, 12.0, [1.0, 0.15, 0.1])]);
        assert!(find(&img, None, &params(), &[]).data.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn tiny_spots_like_catchlights_and_huge_ones_are_kept_out() {
        // this picture is a crop of a 2000 px photo: a catchlight 4 pixels across is under the smallest spot (6 px
        // across); a white area larger than the size cap goes too, and a glare spot between the two stays
        let spots = [(50.0, 50.0, 2.0, [1.0; 3]), (300.0, 150.0, 90.0, [1.0; 3]), (80.0, 220.0, 8.0, [1.0; 3])];
        let img = picture(400, 300, &spots);
        let a = find(&img, None, &Params { max_spot: 0.1, photo_long: 2000.0, ..params() }, &[]);
        assert_eq!(a.get(50, 50), 0.0);
        assert_eq!(a.get(300, 150), 0.0);
        assert!(a.get(80, 220) > 0.99);
    }

    #[test]
    fn a_higher_threshold_finds_less_and_expand_grows_the_mask() {
        let img = picture(400, 300, &[(100.0, 150.0, 10.0, [1.0; 3]), (300.0, 150.0, 10.0, [0.62, 0.6, 0.58])]);
        let loose = find(&img, None, &Params { threshold: 0.0, ..params() }, &[]);
        let strict = find(&img, None, &Params { threshold: 100.0, ..params() }, &[]);
        assert!(loose.get(300, 150) > 0.99, "a dimmer spot passes a low threshold");
        assert_eq!(strict.get(300, 150), 0.0, "and fails a high one");
        let grown = find(&img, None, &Params { expand: 100.0, ..params() }, &[]);
        let sum = |p: &Plane| p.data.iter().sum::<f32>();
        assert!(sum(&grown) > sum(&find(&img, None, &params(), &[])) * 1.3);
        // the edge is soft: some pixels are partly in
        assert!(grown.data.iter().any(|v| *v > 0.05 && *v < 0.95));
    }

    #[test]
    fn only_the_region_is_searched_and_its_soft_edge_is_kept() {
        let img = picture(400, 300, &[(100.0, 150.0, 10.0, [1.0; 3]), (300.0, 150.0, 10.0, [1.0; 3])]);
        let region = Plane::from_fn(400, 300, |x, _| if x < 200 { 1.0 } else { 0.0 });
        let a = find(&img, Some(&region), &params(), &[]);
        assert!(a.get(100, 150) > 0.99);
        assert_eq!(a.get(300, 150), 0.0);
    }

    #[test]
    fn in_a_drawn_box_a_bright_area_running_into_its_edge_is_not_glare() {
        // a white wall enters the box from the left; a glare spot sits inside
        let img = Rgb32f::from_fn(400, 300, |x, y| {
            if x < 40 || ((x as f32 - 250.0).powi(2) + (y as f32 - 150.0).powi(2)).sqrt() < 10.0 { [1.0; 3] } else { [0.3, 0.2, 0.14] }
        });
        let boxed = find(&img, None, &Params { chosen: true, boxed: true, max_spot: 0.35, ..params() }, &[]);
        assert_eq!(boxed.get(10, 150), 0.0);
        assert!(boxed.get(250, 150) > 0.99);
    }

    #[test]
    fn painted_strokes_add_and_erase() {
        let img = picture(400, 300, &[(100.0, 150.0, 10.0, [1.0; 3])]);
        let strokes = [
            Stroke { erase: false, radius: 8.0, points: vec![(300.0, 100.0), (320.0, 100.0)] },
            Stroke { erase: true, radius: 30.0, points: vec![(100.0, 150.0)] },
        ];
        let a = find(&img, None, &params(), &strokes);
        assert!(a.get(310, 100) > 0.99, "added");
        assert_eq!(a.get(100, 150), 0.0, "erased");
    }

    #[test]
    fn hostile_input_gives_an_empty_mask_not_a_panic() {
        let mut img = picture(40, 30, &[]);
        img.data.pop();
        assert!(find(&img, None, &params(), &[]).data.iter().all(|v| *v == 0.0));
        let img = Rgb32f::filled(40, 30, [f32::NAN, f32::INFINITY, -1.0]);
        let bad = Params { threshold: f64::NAN, expand: f64::INFINITY, photo_long: f64::NAN, max_spot: f64::NAN, chosen: false, boxed: false };
        let strokes = [Stroke { erase: false, radius: f32::NAN, points: vec![(f32::NAN, 1.0)] }];
        let _ = find(&img, None, &bad, &strokes);
        let wrong = Plane::new(3, 3);
        let _ = find(&picture(40, 30, &[]), Some(&wrong), &params(), &[]);
        let _ = find(&Rgb32f::new(0, 0), None, &params(), &[]);
    }
}
