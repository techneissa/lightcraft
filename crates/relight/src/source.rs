//! A light's source seen as a bright point (issue #334): a stage fixture, a lamp or a bulb that sits inside the photo shows
//! as a glowing spot where it hangs, in the light's own colour, whether or not there is haze for a beam. A light outside the
//! photo shows nothing (there is nothing to see: its beam still enters from the edge); a nearer thing hides the glow (the
//! glow sits at the light's own depth, so a person standing before it cuts it, with the depth map's outline).
//!
//! The plane is made in the same units as the beam's ([`crate::Field::beam_plane`]: 18 % grey per unit of the light's
//! strength), so the finish stage adds it in the same way and `Amount` dims it with the rest of the light.

use rayon::prelude::*;

use crate::field::Field;
use crate::light::{Light, Lobe, Rig};

/// The glow's radius at Source 0 (barely a point) and at Source 1, as shares of the field's longer side.
const R_MIN: f32 = 0.004;
const R_MAX: f32 = 0.024;
/// The glow's peak in units of 18 % grey per unit of the light's strength: well past white at any Amount a light is used at.
const PEAK: f32 = 8.0;
/// A surface this much nearer than the light (log depth) begins to hide it, and hides it fully at `HIDDEN`.
const VISIBLE: f32 = 0.02;
const HIDDEN: f32 = 0.14;

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Field {
    /// The glow of `light`'s source (set up as `rig`) with `strength` (0..1): `None` when it has none to show (no strength, no
    /// place, or a place outside the photo). The caller keeps it until the light moves.
    pub fn source(&self, light: &Light, rig: &Rig, strength: f64) -> Option<Lobe> {
        if !(strength.is_finite() && strength > 0.0 && light.kind.shows_source()) {
            return None;
        }
        let strength = strength.min(1.0) as f32;
        let (w, h) = (self.width, self.height);
        if !(light.x.is_finite() && light.y.is_finite() && (0.0..=1.0).contains(&light.x) && (0.0..=1.0).contains(&light.y)) || w == 0 || h == 0 {
            return None;
        }
        let lz = rig.position()[2].ln();
        if !lz.is_finite() {
            return None;
        }
        let long = w.max(h) as f32;
        let r = (R_MIN + (R_MAX - R_MIN) * strength) * long;
        let (cx, cy) = (light.x as f32 * w as f32, light.y as f32 * h as f32);
        // (the halo fades smoothly to nothing from 5 radii to 9, round: it used to stop at the square round 9 radii, a hard
        // rectangle in a dark photo's haze, issue #346)
        let reach = (9.0 * r).ceil();
        let (x0, x1) = ((cx - reach).floor().max(0.0) as usize, ((cx + reach).ceil() as usize).min(w));
        let (y0, y1) = ((cy - reach).floor().max(0.0) as usize, ((cy + reach).ceil() as usize).min(h));
        let mut m = vec![0f32; w * h];
        m.par_chunks_mut(w).enumerate().skip(y0).take(y1.saturating_sub(y0)).for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate().skip(x0).take(x1.saturating_sub(x0)) {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let d = (dx * dx + dy * dy).sqrt() / r;
                // (a hot core and a soft halo round it)
                let glow = ((-(d / 0.5) * (d / 0.5)).exp() + 0.2 * (-d / 1.6).exp()) * (1.0 - smooth(5.0, 9.0, d));
                let (surface, _, _) = self.at_px(y * w + x);
                let seen = 1.0 - smooth(VISIBLE, HIDDEN, lz - surface);
                let v = PEAK * glow * seen;
                *out = if v.is_finite() { v.max(0.0) } else { 0.0 };
            }
        });
        Some(Lobe { m })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Field, Light, LightKind};

    #[test]
    fn the_glow_fades_to_nothing_before_the_square_it_is_drawn_in() {
        // (issue #346) along the row and the column through the source, the glow only falls, and the last values before it is cut
        // off are nothing next to its peak: no edge of a rectangle shows in a dark photo's haze
        let (w, h) = (400, 300);
        #[allow(clippy::unwrap_used)]
        let f = Field::from_parts(w, h, vec![0.4; w * h], vec![-4.0; w * h]).unwrap();
        let l = Light { x: 0.5, y: 0.5, depth: 0.2, amount: 1.0, softness: 0.5, warmth: 0.0, wrap: 0.4, kind: LightKind::Point };
        let rig = f.rig(&l, 0.0, 0.0);
        #[allow(clippy::unwrap_used)]
        let m = f.source(&l, &rig, 1.0).unwrap().m;
        let peak = m.iter().copied().fold(0f32, f32::max);
        let row: Vec<f32> = (200..w).map(|x| m[150 * w + x]).collect();
        let col: Vec<f32> = (150..h).map(|y| m[y * w + 200]).collect();
        for line in [row, col] {
            assert!(line.windows(2).all(|p| p[1] <= p[0]), "the glow rises again");
            let last = line.iter().rposition(|v| *v > 0.0).unwrap_or(0);
            assert!(line[last] < 1e-5 * peak, "the glow is cut off at {} of a peak of {peak}", line[last]);
        }
    }
}
