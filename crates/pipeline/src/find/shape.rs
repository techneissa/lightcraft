//! Mask geometry for Find People / Find Wires: connected components, exact distance transform,
//! dilation, outline tracing and polyline simplification. Plain data in, plain data out; every
//! index is range-checked (the masks come from models and photos).

/// A boolean grid, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub bits: Vec<bool>,
}

/// Most pixels a [`Mask`] may hold (callers work on grids of a few hundred pixels a side).
pub const MAX_MASK_PIXELS: usize = 16 << 20;

impl Mask {
    pub fn new(w: usize, h: usize) -> Mask {
        let n = w.saturating_mul(h).min(MAX_MASK_PIXELS);
        Mask { w, h, bits: vec![false; n] }
    }

    pub fn from_fn(w: usize, h: usize, f: impl Fn(usize, usize) -> bool) -> Mask {
        let mut m = Mask::new(w, h);
        if m.bits.len() != w.saturating_mul(h) {
            return Mask { w: 0, h: 0, bits: Vec::new() };
        }
        for y in 0..h {
            for x in 0..w {
                m.bits[y * w + x] = f(x, y);
            }
        }
        m
    }

    pub fn get(&self, x: i64, y: i64) -> bool {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return false;
        }
        self.bits.get(y as usize * self.w + x as usize).copied().unwrap_or(false)
    }

    pub fn count(&self) -> usize {
        self.bits.iter().filter(|b| **b).count()
    }
}

/// Squared Euclidean distance (in pixels) from every pixel to the nearest pixel where `feature` holds;
/// a huge value when there is none. Exact (Felzenszwalb & Huttenlocher's lower envelope).
pub fn distance_sq(w: usize, h: usize, feature: impl Fn(usize) -> bool) -> Vec<f32> {
    const FAR: f64 = 1e12;
    if w == 0 || h == 0 || w.saturating_mul(h) > MAX_MASK_PIXELS {
        return Vec::new();
    }
    let mut grid: Vec<f64> = (0..w * h).map(|i| if feature(i) { 0.0 } else { FAR }).collect();
    let n = w.max(h);
    let (mut f, mut d, mut v, mut z) = (vec![0.0f64; n], vec![0.0f64; n], vec![0usize; n], vec![0.0f64; n + 1]);
    let mut pass = |len: usize, get: &dyn Fn(usize) -> f64, set: &mut dyn FnMut(usize, f64)| {
        for q in 0..len {
            f[q] = get(q);
        }
        let mut k = 0usize;
        v[0] = 0;
        z[0] = f64::NEG_INFINITY;
        z[1] = f64::INFINITY;
        for q in 1..len {
            let s = loop {
                let p = v[k];
                let s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * (q as f64 - p as f64));
                // (z[0] is -inf, so k stops at 0)
                if s <= z[k] && k > 0 {
                    k -= 1;
                } else {
                    break s;
                }
            };
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f64::INFINITY;
        }
        k = 0;
        for q in 0..len {
            while z[k + 1] < q as f64 {
                k += 1;
            }
            let p = v[k];
            d[q] = (q as f64 - p as f64).powi(2) + f[p];
        }
        for q in 0..len {
            set(q, d[q]);
        }
    };
    for x in 0..w {
        let col: Vec<f64> = (0..h).map(|y| grid[y * w + x]).collect();
        let mut out = vec![0.0; h];
        pass(h, &|q| col[q], &mut |q, val| out[q] = val);
        for y in 0..h {
            grid[y * w + x] = out[y];
        }
    }
    for y in 0..h {
        let row: Vec<f64> = grid[y * w..(y + 1) * w].to_vec();
        let mut out = vec![0.0; w];
        pass(w, &|q| row[q], &mut |q, val| out[q] = val);
        grid[y * w..(y + 1) * w].copy_from_slice(&out);
    }
    grid.into_iter().map(|v| v.min(FAR) as f32).collect()
}

/// `m` grown by `radius` pixels.
pub fn dilate(m: &Mask, radius: f32) -> Mask {
    let d = distance_sq(m.w, m.h, |i| m.bits.get(i).copied().unwrap_or(false));
    let r2 = radius.max(0.0) * radius.max(0.0);
    Mask { w: m.w, h: m.h, bits: d.iter().map(|v| *v <= r2).collect() }
}

/// One 8-connected component.
#[derive(Clone, Debug)]
pub struct Component {
    pub mask: Mask,
    pub pixels: usize,
    /// `[x0, y0, x1, y1]`, inclusive.
    pub bbox: [usize; 4],
    pub centroid: (f64, f64),
}

impl Component {
    /// A mask (any shape, not necessarily connected) described as one component; `None` when it is empty.
    pub fn of(mask: Mask) -> Option<Component> {
        let w = mask.w;
        if w == 0 || mask.bits.len() != w.saturating_mul(mask.h) {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (w, mask.h, 0usize, 0usize);
        let (mut n, mut sx, mut sy) = (0usize, 0.0f64, 0.0f64);
        for (i, b) in mask.bits.iter().enumerate() {
            if *b {
                let (x, y) = (i % w, i / w);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                n += 1;
                sx += x as f64 + 0.5;
                sy += y as f64 + 0.5;
            }
        }
        (n > 0).then(|| Component { mask, pixels: n, bbox: [x0, y0, x1, y1], centroid: (sx / n as f64, sy / n as f64) })
    }
}

/// The 8-connected components of `m`, largest first, at most `limit` of them.
pub fn components(m: &Mask, limit: usize) -> Vec<Component> {
    let (w, h) = (m.w, m.h);
    if w == 0 || h == 0 || m.bits.len() != w * h {
        return Vec::new();
    }
    let mut seen = vec![false; w * h];
    let mut out: Vec<(usize, Vec<usize>)> = Vec::new();
    for start in 0..w * h {
        if seen[start] || !m.bits[start] {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let mut px = Vec::new();
        while let Some(i) = stack.pop() {
            px.push(i);
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !seen[j] && m.bits[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        out.push((px.len(), px));
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.0));
    out.truncate(limit);
    out.into_iter()
        .map(|(n, px)| {
            let mut mask = Mask::new(w, h);
            let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
            let (mut sx, mut sy) = (0.0f64, 0.0f64);
            for i in &px {
                mask.bits[*i] = true;
                let (x, y) = (i % w, i / w);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                sx += x as f64 + 0.5;
                sy += y as f64 + 0.5;
            }
            Component { mask, pixels: n, bbox: [x0, y0, x1, y1], centroid: (sx / n as f64, sy / n as f64) }
        })
        .collect()
}

/// The outer outline of the component `m` (assumed 8-connected) as pixel-centre points, clockwise on screen.
pub fn trace_outline(m: &Mask) -> Vec<(f64, f64)> {
    let Some(start) = m.bits.iter().position(|b| *b) else { return Vec::new() };
    if m.w == 0 {
        return Vec::new();
    }
    let s = ((start % m.w) as i64, (start / m.w) as i64);
    // clockwise on screen (y down): W, NW, N, NE, E, SE, S, SW
    const DIRS: [(i64, i64); 8] = [(-1, 0), (-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1)];
    let mut out = vec![(s.0 as f64 + 0.5, s.1 as f64 + 0.5)];
    let (mut p, mut back) = (s, 0usize);
    for _ in 0..(8 * m.w.saturating_mul(m.h)).min(1 << 24) {
        let mut moved = false;
        for k in 1..=8 {
            let d = (back + k) % 8;
            let q = (p.0 + DIRS[d].0, p.1 + DIRS[d].1);
            if m.get(q.0, q.1) {
                let before = (d + 7) % 8;
                let bp = (p.0 + DIRS[before].0, p.1 + DIRS[before].1);
                let rel = (bp.0 - q.0, bp.1 - q.1);
                back = DIRS.iter().position(|v| *v == rel).unwrap_or(0);
                p = q;
                moved = true;
                break;
            }
        }
        if !moved || p == s {
            break;
        }
        out.push((p.0 as f64 + 0.5, p.1 as f64 + 0.5));
    }
    out
}

/// Douglas-Peucker: `pts` reduced so no dropped point is further than `eps` from the result.
/// `closed` treats the list as a ring.
pub fn simplify(pts: &[(f64, f64)], eps: f64, closed: bool) -> Vec<(f64, f64)> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    fn dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = dx * dx + dy * dy;
        if len2 <= f64::EPSILON {
            return ((p.0 - a.0).powi(2) + (p.1 - a.1).powi(2)).sqrt();
        }
        let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
        ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
    }
    fn rec(pts: &[(f64, f64)], lo: usize, hi: usize, eps: f64, keep: &mut [bool]) {
        let mut stack = vec![(lo, hi)];
        while let Some((lo, hi)) = stack.pop() {
            if hi <= lo + 1 {
                continue;
            }
            let (mut far, mut best) = (lo, 0.0);
            for i in lo + 1..hi {
                let d = dist(pts[i], pts[lo], pts[hi]);
                if d > best {
                    (far, best) = (i, d);
                }
            }
            if best > eps {
                keep[far] = true;
                stack.push((lo, far));
                stack.push((far, hi));
            }
        }
    }
    let n = pts.len();
    let mut keep = vec![false; n];
    if closed {
        // split the ring at the point furthest from the first
        let far = (1..n)
            .max_by(|a, b| {
                let da = (pts[*a].0 - pts[0].0).powi(2) + (pts[*a].1 - pts[0].1).powi(2);
                let db = (pts[*b].0 - pts[0].0).powi(2) + (pts[*b].1 - pts[0].1).powi(2);
                da.total_cmp(&db)
            })
            .unwrap_or(n - 1);
        keep[0] = true;
        keep[far] = true;
        rec(pts, 0, far, eps, &mut keep);
        let mut tail: Vec<(f64, f64)> = pts[far..].to_vec();
        tail.push(pts[0]);
        let mut tk = vec![false; tail.len()];
        rec(&tail, 0, tail.len() - 1, eps, &mut tk);
        for (i, k) in tk.iter().enumerate().take(tail.len() - 1).skip(1) {
            if *k {
                keep[far + i] = true;
            }
        }
    } else {
        keep[0] = true;
        keep[n - 1] = true;
        rec(pts, 0, n - 1, eps, &mut keep);
    }
    pts.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect()
}

/// Whether `(x, y)` is inside the polygon `ring` (even-odd).
pub fn in_polygon(ring: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut inside = false;
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + n - 1) % n]);
        if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    inside
}

/// Distance from `(x, y)` to the polyline `pts`.
pub fn distance_to_polyline(pts: &[(f64, f64)], x: f64, y: f64) -> f64 {
    let seg = |a: (f64, f64), b: (f64, f64)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = dx * dx + dy * dy;
        let t = if len2 <= f64::EPSILON { 0.0 } else { (((x - a.0) * dx + (y - a.1) * dy) / len2).clamp(0.0, 1.0) };
        ((x - a.0 - t * dx).powi(2) + (y - a.1 - t * dy).powi(2)).sqrt()
    };
    match pts {
        [] => f64::INFINITY,
        [p] => seg(*p, *p),
        _ => pts.windows(2).map(|w| seg(w[0], w[1])).fold(f64::INFINITY, f64::min),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_transform_is_exact() {
        let (w, h) = (9, 7);
        let d = distance_sq(w, h, |i| i == 3 * w + 4);
        for y in 0..h {
            for x in 0..w {
                let want = ((x as f32 - 4.0).powi(2) + (y as f32 - 3.0).powi(2)).round();
                assert_eq!(d[y * w + x].round(), want, "{x},{y}");
            }
        }
        // no feature at all: far, not NaN
        assert!(distance_sq(5, 5, |_| false).iter().all(|v| *v > 1e9 && v.is_finite()));
    }

    #[test]
    fn dilate_grows_a_dot_to_a_disc() {
        let mut m = Mask::new(21, 21);
        m.bits[10 * 21 + 10] = true;
        let g = dilate(&m, 3.0);
        assert!(g.get(13, 10) && g.get(10, 7) && !g.get(14, 10) && !g.get(13, 13));
    }

    #[test]
    fn components_split_and_sort() {
        let m = Mask::from_fn(20, 10, |x, y| (x < 4 && y < 4) || (10..12).contains(&x) && y < 2);
        let c = components(&m, 8);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].pixels, 16);
        assert_eq!(c[1].pixels, 4);
        assert_eq!(c[0].bbox, [0, 0, 3, 3]);
        assert!(components(&Mask { w: 3, h: 3, bits: vec![true; 4] }, 4).is_empty(), "a mask of the wrong size is nothing");
    }

    #[test]
    fn outline_of_a_block_is_its_corners() {
        let m = Mask::from_fn(12, 12, |x, y| (3..9).contains(&x) && (3..9).contains(&y));
        let ring = trace_outline(&m);
        let simple = simplify(&ring, 0.5, true);
        assert_eq!(simple.len(), 4, "{simple:?}");
        assert!(in_polygon(&ring, 6.0, 6.0) && !in_polygon(&ring, 1.0, 1.0));
        // a single pixel and an empty mask do not loop or panic
        assert_eq!(trace_outline(&Mask::from_fn(3, 3, |x, y| x == 1 && y == 1)).len(), 1);
        assert!(trace_outline(&Mask::new(0, 0)).is_empty());
    }

    #[test]
    fn simplify_keeps_a_bend() {
        let pts: Vec<(f64, f64)> = (0..10).map(|i| (i as f64, 0.0)).chain((1..10).map(|i| (9.0, i as f64))).collect();
        let s = simplify(&pts, 0.1, false);
        assert_eq!(s, vec![(0.0, 0.0), (9.0, 0.0), (9.0, 9.0)]);
        assert!((distance_to_polyline(&s, 4.0, 2.0) - 2.0).abs() < 1e-9);
    }
}
