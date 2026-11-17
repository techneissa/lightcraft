//! Fork (issue #301): where a thing stands in the field's depth, in the units a light's depth is set in. Light presets
//! place their lights relative to the subject's own depth, so a key "at the face's distance" lands on the face and not
//! on what is far behind it.

use crate::field::Field;

/// How far in front of the photo's nearest thing a light at depth 0 is (log depth): `Field::rig` sets the light's
/// log depth as `near − NEAR_MARGIN + depth · span`.
pub const NEAR_MARGIN: f32 = 0.6;

impl Field {
    /// The log depth a light at depth 0 sits at and the log depth between that and the farthest thing (what a light's depth
    /// 0..1 spans), as [`Field::rig`] reads them.
    pub fn light_depth_span(&self) -> (f32, f32) {
        let near = self.near - NEAR_MARGIN;
        (near, (self.far - near).max(0.1))
    }

    /// The median log depth of what the box `[x0, y0, x1, y1]` (normalized photo coordinates) holds, sampled on a grid
    /// over its middle part (a face's edges are hair and background); `None` for a box that is not one.
    pub fn depth_in(&self, b: [f64; 4]) -> Option<f32> {
        let [x0, y0, x1, y1] = b;
        if !b.iter().all(|v| v.is_finite()) || x1 <= x0 || y1 <= y0 {
            return None;
        }
        const N: usize = 9;
        let (cx, cy, hw, hh) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) * 0.3, (y1 - y0) * 0.3);
        let mut v: Vec<f32> = Vec::with_capacity(N * N);
        for j in 0..N {
            for i in 0..N {
                let (fx, fy) = (i as f64 / (N - 1) as f64 * 2.0 - 1.0, j as f64 / (N - 1) as f64 * 2.0 - 1.0);
                let (u, w) = ((cx + fx * hw).clamp(0.0, 1.0) as f32, (cy + fy * hh).clamp(0.0, 1.0) as f32);
                let lz = self.at(u, w).0;
                if lz.is_finite() {
                    v.push(lz);
                }
            }
        }
        if v.is_empty() {
            return None;
        }
        v.sort_by(f32::total_cmp);
        v.get(v.len() / 2).copied()
    }

    /// The depth (0..1 of a light's span) at which a light sits at log depth `lz`, and the span (log depth) one whole unit
    /// of it covers.
    pub fn light_depth_of(&self, lz: f32) -> (f64, f64) {
        let (near, span) = self.light_depth_span();
        (f64::from((lz - near) / span), f64::from(span))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A near square in the middle of a far wall: the box over it reads its depth, the box over the wall the wall's.
    #[test]
    fn a_box_reads_the_median_depth_of_what_it_holds() {
        let (w, h) = (80usize, 60usize);
        let lz: Vec<f32> = (0..w * h)
            .map(|i| {
                let (u, v) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                if (0.4..0.6).contains(&u) && (0.3..0.7).contains(&v) { -1.0 } else { 0.0 }
            })
            .collect();
        let f = Field::from_parts(w, h, lz, vec![-2.0; w * h]).unwrap();
        let near = f.depth_in([0.4, 0.3, 0.6, 0.7]).unwrap();
        let wall = f.depth_in([0.0, 0.0, 0.2, 0.3]).unwrap();
        assert!(near < wall - 0.5, "{near} {wall}");
        assert_eq!(f.depth_in([0.5, 0.5, 0.5, 0.9]), None);
        assert_eq!(f.depth_in([f64::NAN, 0.0, 1.0, 1.0]), None);
        // a light at the subject's depth is between the nearest and the farthest
        let (d, span) = f.light_depth_of(near);
        assert!((0.0..1.0).contains(&d) && span > 0.1, "{d} {span}");
    }
}
