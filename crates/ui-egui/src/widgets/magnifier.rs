//! The eyedropper magnifier (issue #459): while an eyedropper is armed and the pointer is over the photo, a round
//! magnifier follows it. It shows the photo's pixels around the pointer as square cells with a thin grid, the centre
//! cell outlined (the pixel a click picks), inside a ring; under it, a chip with the pixel's hex value and the name of
//! the nearest named colour.
//!
//! Every eyedropper uses it the same way: [`sample`] reads a [`Patch`] from the pixels on screen, [`paint`] draws it.
//! The colour that is picked is [`Patch::rgb`], the very pixel the outlined cell shows.

use egui::{Color32, ColorImage, Context, CursorIcon, Id, LayerId, Order, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};
use lightcraft_color::names::nearest_name;

use crate::i18n::tr;
use crate::theme::Tokens;

/// Cells across the magnifier (odd: the pixel under the pointer is the middle one).
pub const CELLS: usize = 11;
/// The gap kept between the magnifier and the window's edge, and between the ring and its chip (points).
const EDGE: f32 = 6.0;
/// Corners of the polygon the cells are cut to (the ring is a true circle).
const ROUND: usize = 96;

/// The pixels around one pixel of a picture, and the colour of that pixel.
#[derive(Clone, Debug)]
pub struct Patch {
    /// [`CELLS`] × [`CELLS`] cells, row by row, as drawn on screen; `None` beyond the picture's edge.
    cells: Vec<Option<Color32>>,
    /// The middle cell's pixel in the picture.
    pub pixel: [usize; 2],
    /// The middle cell's colour as sRGB (what is picked); `None` when what is on screen is not sRGB and cannot be turned back.
    srgb: Option<[u8; 3]>,
}

impl Patch {
    /// The colour the picker takes, `#RRGGBB` (`None` when it is not known, see [`Patch::set_srgb`]).
    pub fn hex(&self) -> Option<String> {
        self.srgb.map(hex_of)
    }

    /// The picked colour as 8-bit sRGB.
    pub fn rgb(&self) -> Option<[u8; 3]> {
        self.srgb
    }

    /// The English name of the nearest named colour.
    pub fn name(&self) -> Option<&'static str> {
        self.srgb.map(nearest_name)
    }

    /// The middle pixel as it is on screen.
    fn shown(&self) -> Option<Color32> {
        self.cells.get(CELLS * (CELLS / 2) + CELLS / 2).copied().flatten()
    }

    /// The middle pixel's value on screen, for a caller that turns it into sRGB itself (a display profile is in use).
    pub fn shown_rgb(&self) -> Option<[u8; 3]> {
        self.shown().map(|c| [c.r(), c.g(), c.b()])
    }

    /// The picked colour, when what is on screen is not sRGB: `rgb` is the middle pixel in sRGB, or `None` if that
    /// cannot be told (the magnifier then shows no value and the eyedropper picks the way it did without it).
    pub fn set_srgb(&mut self, rgb: Option<[u8; 3]>) {
        self.srgb = rgb;
    }
}

/// `#RRGGBB`.
pub fn hex_of(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// The pixels of `img` around the point `at` (each 0..1 across the picture; clamped), the pixel the point is in in the
/// middle.
pub fn sample(img: &ColorImage, at: Vec2) -> Option<Patch> {
    let [w, h] = img.size;
    if w == 0 || h == 0 || img.pixels.len() < w * h {
        return None;
    }
    let px = |v: f32, n: usize| ((v.clamp(0.0, 1.0) * n as f32) as usize).min(n - 1);
    let (cx, cy) = (px(at.x, w), px(at.y, h));
    let half = (CELLS / 2) as i64;
    let mut cells = Vec::with_capacity(CELLS * CELLS);
    for dy in -half..=half {
        for dx in -half..=half {
            let (x, y) = (cx as i64 + dx, cy as i64 + dy);
            let inside = x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h;
            cells.push(if inside { img.pixels.get(y as usize * w + x as usize).copied() } else { None });
        }
    }
    let mut patch = Patch { cells, pixel: [cx, cy], srgb: None };
    patch.srgb = patch.shown_rgb();
    Some(patch)
}

/// Where the magnifier goes for a pointer at `at`: the ring's centre and the chip's rectangle, all inside `bounds`
/// (shifted off the pointer where it is too near an edge for them to fit; the chip goes above the ring where there is
/// no room below).
fn place(at: Pos2, bounds: Rect, ring: f32, chip: Option<Vec2>) -> (Pos2, Option<Rect>) {
    let half = ring / 2.0;
    let mut chip_rect = chip.map(|size| {
        let below = Rect::from_min_size(pos2(at.x - size.x / 2.0, at.y + half + EDGE), size);
        if below.bottom() <= bounds.bottom() { below } else { below.translate(vec2(0.0, -(size.y + ring + 2.0 * EDGE))) }
    });
    let mut all = Rect::from_center_size(at, vec2(ring, ring));
    if let Some(c) = chip_rect {
        all = all.union(c);
    }
    let shift = |lo: f32, hi: f32, b_lo: f32, b_hi: f32| {
        if lo < b_lo {
            b_lo - lo
        } else if hi > b_hi {
            b_hi - hi
        } else {
            0.0
        }
    };
    let by = shift(all.top(), all.bottom(), bounds.top(), bounds.bottom());
    let bx = shift(all.left(), all.right(), bounds.left(), bounds.right());
    let moved = vec2(bx, by);
    chip_rect = chip_rect.map(|c| c.translate(moved));
    (at + moved, chip_rect)
}

/// The polygon of a circle (counter-clockwise on screen).
fn disc(centre: Pos2, radius: f32) -> Vec<Pos2> {
    (0..ROUND)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / ROUND as f32;
            centre + radius * vec2(a.cos(), a.sin())
        })
        .collect()
}

/// `subject` cut to the convex polygon `clip` (Sutherland–Hodgman).
fn clip_convex(subject: Vec<Pos2>, clip: &[Pos2]) -> Vec<Pos2> {
    let area: f32 = clip.iter().zip(clip.iter().cycle().skip(1)).map(|(a, b)| a.x * b.y - b.x * a.y).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut out = subject;
    for (a, b) in clip.iter().zip(clip.iter().cycle().skip(1)) {
        let side = |p: Pos2| sign * ((b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x));
        let input = std::mem::take(&mut out);
        for (p, q) in input.iter().zip(input.iter().cycle().skip(1)) {
            let (sp, sq) = (side(*p), side(*q));
            if sp >= 0.0 {
                out.push(*p);
            }
            if (sp >= 0.0) != (sq >= 0.0) {
                let t = sp / (sp - sq);
                out.push(*p + t * (*q - *p));
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

/// Draw the magnifier for `patch` at the pointer `at`, over everything else in the window and inside it. The pointer's
/// cursor is hidden while the ring is centred on it (the outlined cell is the pointer), and a crosshair where the ring
/// had to move away from an edge.
pub fn paint(ctx: &Context, at: Pos2, patch: &Patch) {
    let t = Tokens::get(ctx);
    let ppp = ctx.pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let window = ctx.content_rect();
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("lc-magnifier"))).with_clip_rect(window);
    let cell = snap(t.magnifier_size / CELLS as f32).max(1.0 / ppp);
    let ring = cell * CELLS as f32;
    let chip = patch.hex().zip(patch.name()).map(|(hex, name)| Chip::new(&painter, &t, hex, tr(name).to_string()));
    let (centre, chip_rect) = place(at, window.shrink(EDGE), ring, chip.as_ref().map(|c| c.size));
    let centre = pos2(snap(centre.x), snap(centre.y));
    let exact = (centre - at).length() < 1.0;
    ctx.set_cursor_icon(if exact { CursorIcon::None } else { CursorIcon::Crosshair });

    let line = 3.0;
    let inner = ring / 2.0 - line / 2.0;
    let edge = disc(centre, inner);
    painter.circle_filled(centre, inner, Color32::BLACK);
    let origin = centre - vec2(ring, ring) / 2.0;
    let cell_rect = |col: usize, row: usize| Rect::from_min_size(origin + vec2(col as f32, row as f32) * cell, vec2(cell, cell));
    for (i, colour) in patch.cells.iter().enumerate() {
        let Some(colour) = *colour else { continue };
        let r = cell_rect(i % CELLS, i / CELLS);
        let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
        if corners.iter().all(|c| (*c - centre).length() <= inner) {
            painter.rect_filled(r, 0.0, colour);
        } else {
            let cut = clip_convex(corners.to_vec(), &edge);
            if cut.len() >= 3 {
                painter.add(Shape::convex_polygon(cut, colour, Stroke::NONE));
            }
        }
    }
    // the grid: a dark line under a light one, so it shows on a cell of any brightness
    let (dark, light) = (Stroke::new(1.5, t.ring_inner.gamma_multiply(0.28)), Stroke::new(0.75, t.magnifier_grid));
    for k in 1..CELLS {
        let off = k as f32 * cell - ring / 2.0;
        let reach = (inner * inner - off * off).max(0.0).sqrt();
        for stroke in [dark, light] {
            painter.line_segment([centre + vec2(off, -reach), centre + vec2(off, reach)], stroke);
            painter.line_segment([centre + vec2(-reach, off), centre + vec2(reach, off)], stroke);
        }
    }
    // the ring: white over a dark line, so it reads on any photo
    painter.circle_stroke(centre, ring / 2.0, Stroke::new(line + 2.0, t.ring_inner.gamma_multiply(0.6)));
    painter.circle_stroke(centre, ring / 2.0, Stroke::new(line, t.ring_active));
    // the pixel that is picked
    let mid = cell_rect(CELLS / 2, CELLS / 2).expand(0.5);
    painter.rect_stroke(mid, 1.5, Stroke::new(3.5, t.ring_inner), StrokeKind::Middle);
    painter.rect_stroke(mid, 1.5, Stroke::new(1.75, t.ring_active), StrokeKind::Middle);

    if let (Some(chip), Some(rect)) = (chip, chip_rect) {
        chip.paint(&painter, &t, rect, patch.shown());
    }
}

/// The hex value and colour name under the ring.
struct Chip {
    hex: std::sync::Arc<egui::Galley>,
    name: std::sync::Arc<egui::Galley>,
    size: Vec2,
}

impl Chip {
    const PAD: f32 = 8.0;
    const SWATCH: f32 = 18.0;
    const GAP: f32 = 8.0;

    fn new(painter: &Painter, t: &Tokens, hex: String, name: String) -> Chip {
        let hex = painter.layout_no_wrap(hex, t.semibold(13.0), t.text);
        let name = painter.layout_no_wrap(name, t.font(11.0), t.text_label);
        let text = vec2(hex.size().x.max(name.size().x), hex.size().y + name.size().y);
        let size = vec2(Self::PAD * 2.0 + Self::SWATCH + Self::GAP + text.x, Self::PAD * 2.0 + text.y.max(Self::SWATCH));
        Chip { hex, name, size }
    }

    fn paint(&self, painter: &Painter, t: &Tokens, rect: Rect, colour: Option<Color32>) {
        painter.rect(rect, 6.0, t.chrome.gamma_multiply(0.94), Stroke::new(1.0, t.button_border), StrokeKind::Inside);
        let swatch = Rect::from_min_size(pos2(rect.left() + Self::PAD, rect.center().y - Self::SWATCH / 2.0), vec2(Self::SWATCH, Self::SWATCH));
        painter.rect_filled(swatch, 3.0, colour.unwrap_or(Color32::TRANSPARENT));
        painter.rect_stroke(swatch, 3.0, Stroke::new(1.0, t.button_border), StrokeKind::Outside);
        let x = swatch.right() + Self::GAP;
        let top = rect.center().y - (self.hex.size().y + self.name.size().y) / 2.0;
        painter.galley(pos2(x, top), self.hex.clone(), t.text);
        painter.galley(pos2(x, top + self.hex.size().y), self.name.clone(), t.text_label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture whose every pixel has its own colour, so a cell that came from the wrong pixel shows.
    fn picture(w: usize, h: usize) -> ColorImage {
        let pixels = (0..w * h).map(|i| Color32::from_rgb((i % w) as u8 * 9, (i / w) as u8 * 9, (i % 7) as u8)).collect();
        ColorImage::new([w, h], pixels)
    }

    /// The value shown and picked is the pixel the point is in, and the cells around it are its neighbours.
    #[test]
    fn the_middle_cell_is_the_pixel_at_the_point() {
        let img = picture(20, 20);
        // the point is inside pixel (7, 3)
        let patch = sample(&img, vec2(7.5 / 20.0, 3.5 / 20.0)).expect("a patch");
        assert_eq!(patch.pixel, [7, 3]);
        let want = img.pixels[3 * 20 + 7];
        assert_eq!(patch.rgb(), Some([want.r(), want.g(), want.b()]));
        assert_eq!(patch.hex(), Some(hex_of([want.r(), want.g(), want.b()])));
        // the cell right of the middle one is pixel (8, 3); beyond the picture's left edge there is none
        assert_eq!(patch.cells[CELLS * (CELLS / 2) + CELLS / 2 + 1], Some(img.pixels[3 * 20 + 8]));
        let at_edge = sample(&img, vec2(0.0, 0.5)).expect("a patch");
        assert_eq!(at_edge.pixel[0], 0);
        assert_eq!(at_edge.cells[CELLS * (CELLS / 2)], None, "nothing left of the first column");
    }
}
