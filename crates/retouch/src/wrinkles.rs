//! Wrinkles: Forehead, Crow's Feet, Cheeks and Smile Lines and Neck. Takes the lines in each area away and keeps the skin's pores
//! and the face's shape.
//!
//! Each area ([`Area`]) is a soft shape built from the face mesh ([`areas`]), cut by a skin mask (Facial Skin, or
//! Body Skin for the neck). Inside it, the skin's log brightness is split into three bands by blurs whose sizes are
//! fractions of the face:
//!
//! - **fine** (pores, fine hairs, grain): finer than [`FINE`]. Never changed.
//! - **lines**: between [`FINE`] and the area's own [`Area::coarse`] blur. A wrinkle is a narrow dark crease, so only
//!   where this band is darker than the skin around it is it lifted, by up to [`MOST`] of its depth at 100.
//! - **shape** (the face's form and light): coarser than [`Area::coarse`]. Never changed.
//!
//! The skin "around" a crease is a blur that leaves the creases out (a first blur finds the pixels darker than their
//! surroundings and a second one weighs them down), so a lifted crease meets its neighbours instead of leaving a
//! bright rim. Very deep darkness (a shadow, not a line) is lifted less ([`DEEP`]). The change is a gain on all three
//! channels, so the skin keeps its colour.
//!
//! Every blur reads only the skin (a blur of the picture weighted by the mask, divided by the blur of the mask), so
//! hair, eyes, lips, clothes and the background never bleed in, and pixels outside the masks never change.
//!
//! Zoom rule: every read stays inside the skin masks' boxes, and a pixel's change reads the picture at most
//! [`Wrinkles::REACH`] of the face's size away from it (less than the step allows past the masks' areas); no
//! statistic is taken from the image as a whole, and the cells and the weight's lattice are counted from the face's
//! corner, so a zoomed render equals the whole render inside it.
//!
//! Speed: each skin mask is sampled, and its log and fine blur built, once per face for all the areas it cuts;
//! pixels where the mask is 0 are skipped; an area's weight is measured only inside its bounds. A face too big for
//! full-size work ([`MAX_BOX`]) is worked on reduced, never skipped ([`Wrinkles::apply_capped`]).
//!
//! A later Refine brush (#99) paints over each area's weight: [`area_weight`] is where its strokes will be applied,
//! one weight per area.

pub mod areas;
mod exclude;
mod fix;
mod hairlike;
mod heal;
mod lines;
mod ridge;
pub mod search;

use lightcraft_raster::{Plane, Rgb32f, par_rows};
use serde::{Deserialize, Serialize};

pub use areas::Area;
pub use exclude::OVAL as FACE_OVAL;
pub use fix::Fix;
pub use search::{Crop, Fill, Filler, LAMA_FACES, NoFill, Picture, Source, WORK, with_fixes, with_fixes_from};

use crate::{Control, FaceCtx, Feature, Part, Scene, Scope, Spec, Stage};

/// The fine band's blur, a fraction of the face's size: what is finer than it is texture and is kept.
pub const FINE: f64 = 0.0025;
/// The share of a line's depth an area at 100 lifts.
pub const MOST: f32 = 0.9;
/// A line this deep (natural log of the brightness: about 30 % darker than the skin around it) is lifted half as
/// much: deep darkness is a shadow or a fold of the face's shape, not a line.
pub const DEEP: f32 = 1.0;
/// The spacing of the lattice an area's weight is measured on, a fraction of the face's size.
const LATTICE: f64 = 0.004;
/// Most pixels of a face's box the step works on at full size (2²⁶, about 67 M: a face filling a 100 MP export).
/// A bigger box is worked on reduced, as a draft is, and the change carried back ([`Wrinkles::apply_capped`]).
pub const MAX_BOX: usize = 1 << 26;

impl Area {
    /// The skin mask the area is cut by.
    pub fn part(self) -> Part {
        match self {
            Area::Neck => Part::BodySkin,
            _ => Part::FacialSkin,
        }
    }

    /// The blur that separates the area's lines from the face's shape, a fraction of the face's size: wider than the
    /// area's widest line.
    pub const fn coarse(self) -> f64 {
        match self {
            Area::Forehead => 0.03,
            Area::CrowsFeet => 0.015,
            Area::SmileLines => 0.035,
            Area::Neck => 0.03,
        }
    }

    /// The area's slider in `s`.
    pub fn amount(self, s: &Wrinkles) -> f64 {
        match self {
            Area::Forehead => s.forehead,
            Area::CrowsFeet => s.crows_feet,
            Area::SmileLines => s.smile_lines,
            Area::Neck => s.neck,
        }
    }
}

/// How far a pixel's change reads around it, a fraction of the face's size: the first blur and the fine blur, then
/// the second blur, of the widest area (each Gaussian read to three sigmas).
const fn reach() -> f64 {
    let mut widest = 0.0;
    let mut i = 0;
    while i < Area::ALL.len() {
        let c = Area::ALL[i].coarse();
        if c > widest {
            widest = c;
        }
        i += 1;
    }
    6.0 * widest + 3.0 * FINE
}

/// The Wrinkles settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wrinkles {
    /// 0…100: how much the forehead's lines are softened; 0 is none.
    pub forehead: f64,
    /// 0…100: the lines beyond the eyes' outer corners.
    pub crows_feet: f64,
    /// 0…100: the folds from the nose to the mouth's corners.
    pub smile_lines: f64,
    /// 0…100: the lines of the neck.
    pub neck: f64,
}

impl Feature for Wrinkles {
    const KEY: &'static str = "wrinkles";
    const LABEL: &'static str = "Wrinkles";
    const STAGE: Stage = Stage::Retouch;
    const SCOPE: Scope = Scope::Face;
    const CONTROLS: &'static [Control<Self>] = &[
        Control { spec: Spec::amount("retouch.wrinkles.forehead", "Forehead"), get: |s| s.forehead, set: |s, v| s.forehead = v },
        Control { spec: Spec::amount("retouch.wrinkles.crowsFeet", "Crow's Feet"), get: |s| s.crows_feet, set: |s, v| s.crows_feet = v },
        Control {
            spec: Spec::amount("retouch.wrinkles.smileLines", "Cheeks and Smile Lines"),
            get: |s| s.smile_lines,
            set: |s, v| s.smile_lines = v,
        },
        Control { spec: Spec::amount("retouch.wrinkles.neck", "Neck"), get: |s| s.neck, set: |s, v| s.neck = v },
    ];
    const PARTS: &'static [Part] = &[Part::FacialSkin, Part::BodySkin];
    const REACH: f64 = reach();

    /// The four wrinkle areas, cut by their skin masks (`area::wrinkles`).
    fn area(scene: &Scene<'_>, faces: &[usize]) -> Option<lightcraft_raster::Plane> {
        crate::area::wrinkles(scene, faces)
    }

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>) {
        self.apply_capped(img, scene, face, MAX_BOX);
    }
}

/// The smallest box holding `a` and `b`.
fn union(a: [usize; 4], b: [usize; 4]) -> [usize; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

impl Wrinkles {
    /// [`Feature::apply`] with `max_box` the most pixels of the face's areas' box worked on at full size
    /// ([`MAX_BOX`] in the step): a bigger box is worked on reduced by a whole factor, as a draft of it is, and the
    /// change is carried back to the full-size skin as a gain, so a huge face is retouched as its draft shows,
    /// never skipped.
    pub fn apply_capped(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>, max_box: usize) {
        let Some(face) = face else { return };
        if img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let Some(ctx) = scene.faces.get(face) else { return };
        // the change the face's search found, when it has been done: the sliders scale it (`fix.rs`)
        if let Some(fix) = ctx.face.wrinkles.as_deref() {
            self.apply_fix(img, scene, face, fix);
            return;
        }
        let plans: Vec<Plan> = Area::ALL.iter().filter_map(|&area| Plan::new(scene, face, area, strength(area.amount(self)))).collect();
        let Some(all) = plans.iter().map(|p| p.b).reduce(union) else { return };
        if let Some(f) = crate::reduced::factor(all, max_box) {
            let parts: Vec<Part> = plans.iter().map(|p| p.area.part()).collect();
            let keep = |x, y| parts.iter().any(|p| scene.mask_at(face, *p, x, y) > 0.0);
            crate::reduced::retouch(img, scene, face, all, f, keep, |small, sub, at| self.apply_capped(small, sub, Some(at), usize::MAX));
            return;
        }
        // each skin mask is sampled, and its log and fine blur built, once, over the boxes of every area it cuts
        for part in [Part::FacialSkin, Part::BodySkin] {
            let mine: Vec<&Plan> = plans.iter().filter(|p| p.area.part() == part).collect();
            let Some(u) = mine.iter().map(|p| p.b).reduce(union) else {
                continue;
            };
            let Some(mask) = skin_mask(scene, face, part, u) else { continue };
            // (the areas' bounds, in the box's pixels)
            let inners: Vec<[usize; 4]> = mine.iter().map(|p| p.inner(u)).collect();
            let Some(all) = inners.iter().copied().filter(|b| b[0] < b[2] && b[1] < b[3]).reduce(union) else { continue };
            let mut c = crop(img, u);
            let Some(skin) = lines::Skin::new(&c, &mask, all, ctx.size) else { continue };
            let mut changed = false;
            for (plan, inner) in mine.iter().zip(inners) {
                changed |= plan.run(&mut c, &skin, ctx, u, inner);
            }
            if changed {
                paste(img, u, &c, &mask, all);
            }
        }
    }
}

/// One area of one face, ready to run: its regions, their bounds and the box it works on.
struct Plan {
    area: Area,
    k: f32,
    regions: Vec<areas::Region>,
    /// The regions' bounds (pixels, not clamped).
    r: [f64; 4],
    /// The box worked on: the bounds and the blurs' reach, within the skin mask's box and the image.
    b: [usize; 4],
}

impl Plan {
    fn new(scene: &Scene<'_>, face: usize, area: Area, k: f32) -> Option<Plan> {
        if k <= 0.0 {
            return None;
        }
        let ctx = scene.faces.get(face)?;
        let mbox = scene.mask_box(face, area.part())?;
        let regions = areas::regions(ctx, area);
        // one box for the area (both sides of the face share its margin)
        let r = regions.iter().map(areas::Region::bounds).reduce(|u, r| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])])?;
        // the blurs read this far around the area
        let margin = (2.0 * 3.0 * area.coarse() + 3.0 * FINE) * ctx.size + 2.0;
        let b = [
            clamp_px(r[0] - margin, mbox[0], mbox[2]),
            clamp_px(r[1] - margin, mbox[1], mbox[3]),
            clamp_px(r[2] + margin, mbox[0], mbox[2]),
            clamp_px(r[3] + margin, mbox[1], mbox[3]),
        ];
        (b[0] < b[2] && b[1] < b[3]).then_some(Plan { area, k, regions, r, b })
    }

    /// The area's bounds within the box `u` (in its pixels; empty when they miss the plan's box).
    fn inner(&self, u: [usize; 4]) -> [usize; 4] {
        let (b, r) = (self.b, self.r);
        let ib = [clamp_px(r[0], b[0], b[2]), clamp_px(r[1], b[1], b[3]), clamp_px(r[2] + 1.0, b[0], b[2]), clamp_px(r[3] + 1.0, b[1], b[3])];
        [ib[0].saturating_sub(u[0]), ib[1].saturating_sub(u[1]), ib[2].saturating_sub(u[0]), ib[3].saturating_sub(u[1])]
    }

    /// Lift the area's lines in `c` (the box `u` of the picture, `skin` built from it), inside `inner` (its
    /// [`Plan::inner`]). False when nothing changed.
    fn run(&self, c: &mut Rgb32f, skin: &lines::Skin<'_>, ctx: &FaceCtx<'_>, u: [usize; 4], inner: [usize; 4]) -> bool {
        if inner[0] >= inner[2] || inner[1] >= inner[3] {
            return false;
        }
        // the coarse blurs' cells and the weight's lattice are anchored to the face, so a window of the photo uses
        // the same ones; the weight is above 0 only inside the area's bounds, so it is measured there only
        let corner = [ctx.bounds[0].floor(), ctx.bounds[1].floor()];
        let a = area_weight(&self.regions, [u[0] + inner[0], u[1] + inner[1], u[0] + inner[2], u[1] + inner[3]], corner, ctx.px(LATTICE));
        let b = self.b;
        let region = [b[0].saturating_sub(u[0]), b[1].saturating_sub(u[1]), b[2].saturating_sub(u[0]), b[3].saturating_sub(u[1])];
        let grid = [b[0] as i64 - corner[0] as i64, b[1] as i64 - corner[1] as i64];
        lines::reduce(c, skin, &a, lines::Sizes { face: ctx.size, coarse: self.area.coarse(), grid }, region, inner, self.k)
    }
}

/// A slider (0…100) as the share of a line's depth lifted (a value that is not a number is 0).
fn strength(v: f64) -> f32 {
    if v.is_finite() { (v.clamp(0.0, 100.0) / 100.0) as f32 * MOST } else { 0.0 }
}

/// `v` (pixels) as a whole pixel within `lo..=hi`.
fn clamp_px(v: f64, lo: usize, hi: usize) -> usize {
    if !v.is_finite() {
        return if v > 0.0 { hi } else { lo };
    }
    (v.round().max(0.0) as usize).clamp(lo, hi)
}

/// The weight of an area (its `regions`, the largest where they meet) over the box `b` (0…1), measured on a lattice
/// `step` pixels apart from `corner` and read between its points bilinearly (the weight's edges are soft over several
/// lattice steps). The Refine brush (#99) will add and take away its strokes here: one weight per area.
pub fn area_weight(regions: &[areas::Region], b: [usize; 4], corner: [f64; 2], step: f64) -> Plane {
    let [x0, y0, x1, y1] = b;
    let w = x1 - x0;
    let mut p = Plane::new(w, y1 - y0);
    let step = if step.is_finite() { step.floor().max(1.0) } else { 1.0 };
    let bounds: Vec<[f64; 4]> = regions.iter().map(areas::Region::bounds).collect();
    let at = |lx: f64, ly: f64| -> f32 {
        let q = (corner[0] + lx * step + 0.5, corner[1] + ly * step + 0.5);
        regions
            .iter()
            .zip(&bounds)
            .filter(|(_, r)| q.0 >= r[0] && q.0 <= r[2] && q.1 >= r[1] && q.1 <= r[3])
            .fold(0.0f32, |v, (g, _)| v.max(g.weight(q)))
    };
    // the lattice over the box, one point beyond it on each side
    let lx0 = ((x0 as f64 - corner[0]) / step).floor() - 1.0;
    let ly0 = ((y0 as f64 - corner[1]) / step).floor() - 1.0;
    let (lw, lh) = (((w as f64) / step).ceil() as usize + 3, ((p.height as f64) / step).ceil() as usize + 3);
    let mut lattice = Plane::new(lw, lh);
    par_rows(&mut lattice.data, lw.max(1), |j, row| {
        for (i, v) in row.iter_mut().enumerate() {
            *v = at(lx0 + i as f64, ly0 + j as f64);
        }
    });
    par_rows(&mut p.data, w.max(1), |y, row| {
        let fy = ((y0 + y) as f64 - corner[1]) / step - ly0;
        let (j, ty) = (fy.floor().max(0.0) as usize, (fy - fy.floor()) as f32);
        for (x, v) in row.iter_mut().enumerate() {
            let fx = ((x0 + x) as f64 - corner[0]) / step - lx0;
            let (i, tx) = (fx.floor().max(0.0) as usize, (fx - fx.floor()) as f32);
            let l = |i: usize, j: usize| lattice.data.get(j * lw + i).copied().unwrap_or(0.0);
            *v = (l(i, j) * (1.0 - tx) + l(i + 1, j) * tx) * (1.0 - ty) + (l(i, j + 1) * (1.0 - tx) + l(i + 1, j + 1) * tx) * ty;
        }
    });
    p
}

/// The mask of `part` of `face` over the box `b` (0…1); `None` when it is 0 everywhere there.
fn skin_mask(scene: &Scene<'_>, face: usize, part: Part, b: [usize; 4]) -> Option<Plane> {
    let mask = scene.faces.get(face)?.face.mask(part)?.sampler()?;
    let view = scene.view();
    let [x0, y0, x1, y1] = b;
    let w = x1 - x0;
    let mut m = Plane::new(w, y1 - y0);
    par_rows(&mut m.data, w.max(1), |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            let (nx, ny) = view.to_norm((x0 + x) as f64 + 0.5, (y0 + y) as f64 + 0.5);
            let s = mask.at(nx, ny);
            *v = if s.is_finite() { s.clamp(0.0, 1.0) } else { 0.0 };
        }
    });
    m.data.iter().any(|v| *v > 0.0).then_some(m)
}

fn crop(img: &Rgb32f, b: [usize; 4]) -> Rgb32f {
    let [x0, y0, x1, y1] = b;
    let w = x1 - x0;
    let mut c = Rgb32f::new(w, y1 - y0);
    par_rows(&mut c.data, w.max(1), |y, row| {
        let start = (y0 + y) * img.width + x0;
        if let Some(src) = img.data.get(start..start + w) {
            row.copy_from_slice(src);
        }
    });
    c
}

/// Write `out` into `img` over the box `b`, inside `inner` (in the box's pixels: what the areas can change) and
/// only where the mask `m` is above 0 (elsewhere `out` holds the picture's own pixels).
fn paste(img: &mut Rgb32f, b: [usize; 4], out: &Rgb32f, m: &Plane, inner: [usize; 4]) {
    let [x0, y0, x1, _] = b;
    let w = x1 - x0;
    let [ix0, iy0, ix1, iy1] = [inner[0].min(w), inner[1], inner[2].min(w), inner[3].min(out.height)];
    if ix0 >= ix1 {
        return;
    }
    par_rows(&mut img.data, img.width.max(1), |y, row| {
        let Some(oy) = y.checked_sub(y0).filter(|oy| (iy0..iy1).contains(oy)) else { return };
        let (Some(dst), Some(src), Some(mrow)) =
            (row.get_mut(x0 + ix0..x0 + ix1), out.data.get(oy * w + ix0..oy * w + ix1), m.data.get(oy * w + ix0..oy * w + ix1))
        else {
            return;
        };
        for ((d, o), mv) in dst.iter_mut().zip(src).zip(mrow) {
            if *mv > 0.0 {
                *d = *o;
            }
        }
    });
}
