//! Where Stray Hair's strand search ([`crate::Face::strands`]) found hairs or creases on the skin: a spot on one is
//! a hair stump or a crease, not a spot, and where stubble grows a spot among many is stubble.

use crate::Scene;
use crate::input::Strands;

/// A cell at least this weight (of 255) is on a strand.
const ON: u8 = 128;
/// A spot within this share of its radius of a strand cell lies on it.
const ON_REACH: f64 = 0.5;
/// Stubble: at least this share of the cells within [`AROUND`] of the face lie on strands.
pub const STUBBLE: f32 = 0.25;
const AROUND: f64 = 0.03;

/// One face's strands, mapped to the frame of its spots.
pub(super) struct Map<'a> {
    s: &'a Strands,
    /// A cell's width and height in normalised photo units.
    cell: (f64, f64),
}

impl<'a> Map<'a> {
    /// The strands of face `face` of `scene`; `None` when its strands were not searched or none were found.
    pub(super) fn of(scene: &'a Scene<'_>, face: usize) -> Option<Map<'a>> {
        let s = scene.faces.get(face)?.face.strands.as_ref()?;
        if s.is_empty() || s.width == 0 || s.height == 0 {
            return None;
        }
        let cell = (s.area.width() / s.width as f64, s.area.height() / s.height as f64);
        (cell.0 > 0.0 && cell.1 > 0.0 && cell.0.is_finite() && cell.1.is_finite()).then_some(Map { s, cell })
    }

    /// Whether a spot at `at` (normalised) of radius `r` lies on a strand, or (`stubbly`: where stubble grows) among
    /// many; `r` and `face` (the face's size) are in pixels of the picture the spots are found on, which is the one
    /// the strands were searched on when their sizes match. (Elsewhere strands among many are a wrinkled forehead
    /// or cheek, whose marks are spots.)
    pub(super) fn covers(&self, at: (f64, f64), r: f64, face: f64, stubbly: bool) -> bool {
        let (pw, ph) = (self.s.picture.0.max(1) as f64, self.s.picture.1.max(1) as f64);
        // cells per pixel of that picture, either way
        let (kx, ky) = (1.0 / (self.cell.0 * pw), 1.0 / (self.cell.1 * ph));
        let cx = (at.0 - self.s.area.x0) / self.cell.0;
        let cy = (at.1 - self.s.area.y0) / self.cell.1;
        if !(cx.is_finite() && cy.is_finite() && kx.is_finite() && ky.is_finite()) {
            return false;
        }
        if self.count(cx, cy, r * ON_REACH * kx, r * ON_REACH * ky).0 > 0 {
            return true;
        }
        let (hit, n) = if stubbly { self.count(cx, cy, face * AROUND * kx, face * AROUND * ky) } else { (0, 0) };
        n > 0 && hit as f32 >= STUBBLE * n as f32
    }

    /// Strand cells and all cells within an ellipse of radii (`rx`, `ry`) cells around (`cx`, `cy`) (the cell
    /// itself at least, at most [`MOST`] cells either way).
    fn count(&self, cx: f64, cy: f64, rx: f64, ry: f64) -> (usize, usize) {
        const MOST: isize = 64;
        let (rx, ry) = ((rx.ceil() as isize).clamp(0, MOST), (ry.ceil() as isize).clamp(0, MOST));
        let (x0, y0) = (cx.floor() as isize, cy.floor() as isize);
        let (mut hit, mut n) = (0, 0);
        for dy in -ry..=ry {
            for dx in -rx..=rx {
                let (ex, ey) = (dx as f64 / rx.max(1) as f64, dy as f64 / ry.max(1) as f64);
                if ex * ex + ey * ey > 1.0 {
                    continue;
                }
                let (x, y) = (x0 + dx, y0 + dy);
                n += 1;
                if x >= 0 && y >= 0 && self.s.cell(x as usize, y as usize).is_some_and(|c| c[0] >= ON) {
                    hit += 1;
                }
            }
        }
        (hit, n)
    }
}
