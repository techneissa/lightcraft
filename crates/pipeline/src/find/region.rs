//! Fork feature (`fork/object-remove.md`): the shape of a candidate, so Remove takes it out with one spot in its shape
//! instead of dabs ([`lightcraft_develop::SpotArea`]).

use lightcraft_develop::SegMask;
use lightcraft_geom::Point;

use super::shape::Mask;

/// A wire's path (`pts`, grid pixels) as the pixels within half its width of it, on the `w × h` grid.
pub fn path_region(pts: &[(f64, f64)], width_px: f64, w: usize, h: usize) -> Option<Mask> {
    let mut m = Mask::new(w, h);
    if pts.is_empty() || m.bits.len() != w.saturating_mul(h) || !width_px.is_finite() {
        return None;
    }
    let r = (width_px / 2.0).clamp(0.5, w.max(h) as f64);
    let mut stamp = |c: (f64, f64)| {
        if !(c.0.is_finite() && c.1.is_finite()) {
            return;
        }
        let (x0, x1) = ((c.0 - r).floor().max(0.0) as usize, ((c.0 + r).ceil().max(0.0) as usize).min(w));
        let (y0, y1) = ((c.1 - r).floor().max(0.0) as usize, ((c.1 + r).ceil().max(0.0) as usize).min(h));
        for y in y0..y1 {
            for x in x0..x1 {
                if (x as f64 + 0.5 - c.0).hypot(y as f64 + 0.5 - c.1) <= r
                    && let Some(b) = m.bits.get_mut(y * w + x)
                {
                    *b = true;
                }
            }
        }
    };
    stamp(pts[0]);
    for seg in pts.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        if !len.is_finite() {
            continue;
        }
        // (a step of half a pixel, at most a few thousand per segment)
        let n = ((len * 2.0).ceil() as usize).clamp(1, 8192);
        for k in 1..=n {
            let t = k as f64 / n as f64;
            stamp((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
        }
    }
    (m.count() > 0).then_some(m)
}

/// A candidate's shape on the working grid (spanning the photo, as the spots' coordinates) as a stored spot shape.
pub fn area_of(m: &Mask) -> Option<SegMask> {
    let (w, h) = (m.w, m.h);
    if w == 0 || h == 0 || m.bits.len() != w.saturating_mul(h) {
        return None;
    }
    let cov: Vec<f32> = m.bits.iter().map(|b| if *b { 1.0 } else { 0.0 }).collect();
    let (fw, fh) = (w as f64, h as f64);
    crate::spot_area::area_from_coverage(&cov, w, h, |p| Point::new(p.x / fw, p.y / fh), |n| Point::new(n.x * fw, n.y * fh))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_becomes_a_strip_of_its_width() {
        let m = path_region(&[(10.0, 20.0), (90.0, 20.0)], 4.0, 100, 40).unwrap();
        assert!(m.get(50, 20) && m.get(50, 21) && !m.get(50, 24) && !m.get(5, 20));
        assert!(path_region(&[], 4.0, 100, 40).is_none());
        assert!(path_region(&[(f64::NAN, 1.0), (1e300, -1e300)], f64::INFINITY, 100, 40).is_none());
        assert!(path_region(&[(-50.0, -50.0)], 2.0, 100, 40).is_none(), "off the grid: nothing");
    }

    #[test]
    fn a_grid_shape_becomes_an_area_around_it() {
        let m = Mask::from_fn(200, 100, |x, y| (80..120).contains(&x) && (30..70).contains(&y));
        let seg = area_of(&m).unwrap();
        let r = seg.bounds().unwrap();
        assert!(r[0] < 0.4 && r[2] > 0.6 && r[1] < 0.3 && r[3] > 0.7, "{r:?}");
        assert!(r[0] > 0.3 && r[2] < 0.7, "{r:?}");
        assert!(area_of(&Mask::new(10, 10)).is_none());
    }
}
