//! Teeth and lips: Teeth Whitening, Lip Color (toward a picked colour), Lip Hue and Lip Saturation.
//!
//! Everything works in OkLab on the face's Teeth and Lips masks and leaves every other pixel alone:
//! - **Teeth Whitening** takes the yellow out (OkLab chroma toward neutral) and lifts lightness toward a ceiling
//!   below white, by less on dark pixels (the gaps between teeth and the mouth's shade) and not at all on red ones
//!   (lips and gums the mask's soft edge reaches). The teeth keep their shading and never turn paper white.
//! - **Lip Color** gives the lips a picked colour the way Recolor does (`fork/recolour.md`): the lips' main
//!   colour is measured, and each pixel keeps its lightness and chroma in proportion to it, so the shading, the
//!   lines and the highlights stay.
//! - **Lip Hue** turns the lips' hue (up to [`MAX_HUE_TURN`], 30°, each way), **Lip Saturation** scales their chroma.
//!
//! Each is off at 0; the colour is only read while Lip Color is above 0. A face without the mask a slider needs (a
//! closed mouth has no Teeth mask) is left alone. The reference levels (the lips' main colour, the teeth's bright
//! level) are measured on a fixed grid over each mask's area in photo coordinates, so a draft, the loupe, a zoomed
//! window and an export measure the same points.

use lightcraft_raster::Rgb32f;
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, NormRect, Part, Scene, Scope, Spec, Stage};

/// Most hue turn of Lip Hue at ±100, in radians (30°).
pub const MAX_HUE_TURN: f32 = 30.0 * std::f32::consts::PI / 180.0;
/// OkLab lightness Teeth Whitening lifts toward (a white that is not paper white).
pub const TEETH_CEILING: f32 = 0.93;
/// Share of the room up to [`TEETH_CEILING`] Teeth Whitening 100 lifts a bright tooth by.
const TEETH_LIFT: f32 = 0.38;
/// Share of the yellow (OkLab chroma) Teeth Whitening 100 takes out.
const TEETH_NEUTRAL: f32 = 0.85;
/// Lip Saturation's chroma factor is 1 + this at +100 …
const LIP_SATURATION_UP: f32 = 0.5;
/// … and 1 − this at −100.
const LIP_SATURATION_DOWN: f32 = 0.7;
/// The lip colour every face starts with (sRGB 0…255): a rose red.
pub const DEFAULT_LIP_COLOR: [f64; 3] = [178.0, 52.0, 74.0];

/// The Teeth and Lips settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TeethLips {
    /// Teeth Whitening, 0…100; 0 is off.
    pub teeth_whitening: f64,
    /// How far the lips take [`TeethLips::lip_color`], 0…100; 0 is off.
    pub lip_color_amount: f64,
    /// The lip colour, sRGB 0…255 per channel.
    pub lip_color: [f64; 3],
    /// Lip Hue, −100…+100; 0 is off.
    pub lip_hue: f64,
    /// Lip Saturation, −100…+100; 0 is off.
    pub lip_saturation: f64,
}

impl Default for TeethLips {
    fn default() -> TeethLips {
        TeethLips { teeth_whitening: 0.0, lip_color_amount: 0.0, lip_color: DEFAULT_LIP_COLOR, lip_hue: 0.0, lip_saturation: 0.0 }
    }
}

/// A 0…255 colour channel slider (the panel shows them as one colour swatch).
const fn channel(id: &'static str, label: &'static str, default: f64) -> Spec {
    Spec { id, label, min: 0.0, max: 255.0, default, step: 1.0, decimals: 0, centered: false }
}

impl Feature for TeethLips {
    const KEY: &'static str = "teethLips";
    const LABEL: &'static str = "Teeth and Lips";
    const STAGE: Stage = Stage::Retouch;
    const SCOPE: Scope = Scope::Face;
    const PARTS: &'static [Part] = &[Part::Lips, Part::Teeth];
    /// It reads only inside its masks' areas: the pixels it changes, and the fixed grid its reference levels come
    /// from (bilinear, a pixel past a grid point at most).
    const REACH: f64 = 0.05;
    /// Refine Add paints a tooth the teeth mask missed.
    const ADD_PARTS: &'static [Part] = &[Part::Teeth];
    const CONTROLS: &'static [Control<Self>] = &[
        Control {
            spec: Spec::amount("retouch.teethLips.teethWhitening", "Teeth Whitening"),
            get: |s| s.teeth_whitening,
            set: |s, v| s.teeth_whitening = v,
        },
        Control {
            spec: Spec::amount("retouch.teethLips.lipColorAmount", "Lip Color"),
            get: |s| s.lip_color_amount,
            set: |s, v| s.lip_color_amount = v,
        },
        Control {
            spec: channel("retouch.teethLips.lipColorR", "Lip Color Red", DEFAULT_LIP_COLOR[0]),
            get: |s| s.lip_color[0],
            set: |s, v| s.lip_color[0] = v,
        },
        Control {
            spec: channel("retouch.teethLips.lipColorG", "Lip Color Green", DEFAULT_LIP_COLOR[1]),
            get: |s| s.lip_color[1],
            set: |s, v| s.lip_color[1] = v,
        },
        Control {
            spec: channel("retouch.teethLips.lipColorB", "Lip Color Blue", DEFAULT_LIP_COLOR[2]),
            get: |s| s.lip_color[2],
            set: |s, v| s.lip_color[2] = v,
        },
        Control { spec: Spec::signed("retouch.teethLips.lipHue", "Lip Hue"), get: |s| s.lip_hue, set: |s, v| s.lip_hue = v },
        Control {
            spec: Spec::signed("retouch.teethLips.lipSaturation", "Lip Saturation"),
            get: |s| s.lip_saturation,
            set: |s, v| s.lip_saturation = v,
        },
    ];

    /// Off while every amount is 0, whatever the colour.
    fn is_off(&self) -> bool {
        [self.teeth_whitening, self.lip_color_amount, self.lip_hue, self.lip_saturation].iter().all(|v| !v.is_finite() || *v == 0.0)
    }

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>) {
        let Some(face) = face else { return };
        if img.width == 0 || img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let teeth = unit(self.teeth_whitening, 0.0);
        if teeth > 0.0 {
            whiten_teeth(img, scene, face, teeth);
        }
        let lips = Lips {
            colour: unit(self.lip_color_amount, 0.0),
            target: oklab_of_srgb8(self.lip_color),
            hue: unit(self.lip_hue, -1.0) * MAX_HUE_TURN,
            saturation: unit(self.lip_saturation, -1.0),
        };
        if lips.colour > 0.0 || lips.hue != 0.0 || lips.saturation != 0.0 {
            recolour_lips(img, scene, face, &lips);
        }
    }
}

/// A slider value as −1…1 or 0…1 (`min` −1 or 0); 0 for a value that is not a number.
fn unit(v: f64, min: f32) -> f32 {
    if v.is_finite() { (v / 100.0).clamp(f64::from(min), 1.0) as f32 } else { 0.0 }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x >= e1 { 1.0 } else { 0.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The part of the mask of `part` of `face` that is above 0, in normalised photo coordinates (a sample's cell and its
/// neighbours, for the bilinear sampling); `None` without the mask or when it is 0 everywhere. A parser's masks cover
/// the whole face crop while the lips and teeth are a small part of it: walking only this keeps a slider cheap.
fn live_area(scene: &Scene<'_>, face: usize, part: Part) -> Option<NormRect> {
    let mask = scene.faces.get(face)?.face.mask(part)?;
    let (pw, ph) = (mask.plane.width, mask.plane.height);
    if pw == 0 || ph == 0 || mask.plane.data.len() != pw.saturating_mul(ph) || !mask.area.is_valid() {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (pw, ph, 0, 0);
    for (i, v) in mask.plane.data.iter().enumerate() {
        if *v > 0.0 && v.is_finite() {
            let (x, y) = (i % pw, i / pw);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
    }
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let a = mask.area;
    let (cx, cy) = (a.width() / pw as f64, a.height() / ph as f64);
    Some(NormRect::new(
        (a.x0 + (x0 as f64 - 1.0) * cx).max(a.x0),
        (a.y0 + (y0 as f64 - 1.0) * cy).max(a.y0),
        (a.x0 + (x1 as f64 + 1.0) * cx).min(a.x1),
        (a.y0 + (y1 as f64 + 1.0) * cy).min(a.y1),
    ))
}

/// The pixels of the image `area` (normalised photo coordinates) can touch: `[x0, y0, x1, y1)`, clamped to the image.
/// The area's edges are followed at 16 points each (a lens or transform warp bends them), with a pixel to spare.
fn pixel_box(scene: &Scene<'_>, a: NormRect) -> Option<[usize; 4]> {
    const STEPS: usize = 16;
    let view = scene.view();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for i in 0..=STEPS {
        let t = i as f64 / STEPS as f64;
        let (u, v) = (a.x0 + t * a.width(), a.y0 + t * a.height());
        for (nx, ny) in [(u, a.y0), (u, a.y1), (a.x0, v), (a.x1, v)] {
            if let Some((px, py)) = view.to_px(nx, ny).filter(|(x, y)| x.is_finite() && y.is_finite()) {
                (x0, y0, x1, y1) = (x0.min(px), y0.min(py), x1.max(px), y1.max(py));
            }
        }
    }
    if x0 > x1 || y0 > y1 {
        return None;
    }
    let clamp = |v: f64, max: usize| v.clamp(0.0, max as f64) as usize;
    let b = [
        clamp(x0.floor() - 1.0, scene.width),
        clamp(y0.floor() - 1.0, scene.height),
        clamp(x1.ceil() + 1.0, scene.width),
        clamp(y1.ceil() + 1.0, scene.height),
    ];
    (b[0] < b[2] && b[1] < b[3]).then_some(b)
}

/// Each pixel of the mask of `part` of `face` in the image (x, y, mask value > 0), row by row.
fn masked(scene: &Scene<'_>, face: usize, part: Part, w: usize, h: usize) -> Vec<(usize, usize, f32)> {
    let Some([x0, y0, x1, y1]) = live_area(scene, face, part).and_then(|a| pixel_box(scene, a)) else { return Vec::new() };
    let mut out = Vec::new();
    for y in y0..y1.min(h) {
        for x in x0..x1.min(w) {
            let m = scene.mask_at(face, part, x, y);
            if m > 0.0 && m.is_finite() {
                out.push((x, y, m.min(1.0)));
            }
        }
    }
    out
}

/// Grid points per side of a mask's area that reference levels are measured on.
const GRID: usize = 64;

/// The cell centres of a [`GRID`] × [`GRID`] grid over the live area ([`live_area`]) of the mask of `part` of `face`, in normalised photo
/// coordinates, with the mask's value there (only those above 0). Anchored to the mask, not to the image: a draft,
/// the loupe, a zoomed window and an export measure the same points.
fn grid(scene: &Scene<'_>, face: usize, part: Part) -> Vec<(f64, f64, f32)> {
    let Some(mask) = scene.faces.get(face).and_then(|f| f.face.mask(part)) else { return Vec::new() };
    // (over the part of the mask above 0: the same in every render, and dense where the lips or teeth are)
    let Some(a) = live_area(scene, face, part) else { return Vec::new() };
    let n = GRID as f64;
    (0..GRID * GRID)
        .filter_map(|i| {
            let (nx, ny) = (a.x0 + ((i % GRID) as f64 + 0.5) / n * a.width(), a.y0 + ((i / GRID) as f64 + 0.5) / n * a.height());
            let m = mask.sample(nx, ny);
            (m > 0.0 && m.is_finite()).then_some((nx, ny, m.min(1.0)))
        })
        .collect()
}

/// The image at pixel position (`x`, `y`), bilinear between pixel centres (held at the edge); `None` outside the
/// image.
fn sample(img: &Rgb32f, x: f64, y: f64) -> Option<[f32; 3]> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || !(0.0..=w as f64).contains(&x) || !(0.0..=h as f64).contains(&y) {
        return None;
    }
    let fx = (x - 0.5).clamp(0.0, (w - 1) as f64);
    let fy = (y - 0.5).clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let at = |x: usize, y: usize| img.data.get(y * w + x).copied();
    let (a, b, c, d) = (at(x0, y0)?, at(x1, y0)?, at(x0, y1)?, at(x1, y1)?);
    Some(std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bottom = c[k] + (d[k] - c[k]) * tx;
        top + (bottom - top) * ty
    }))
}

/// The teeth's bright level: the lightness most of the solid teeth reach (the 80th percentile on the grid over the
/// Teeth mask), so gaps and shade are judged against these teeth, at any exposure.
fn teeth_level(img: &Rgb32f, scene: &Scene<'_>, face: usize) -> Option<f32> {
    let view = scene.view();
    let samples: Vec<(f32, f32)> = grid(scene, face, Part::Teeth)
        .into_iter()
        .filter_map(|(nx, ny, m)| {
            let (x, y) = view.to_px(nx, ny)?;
            let l = oklab(sample(img, x, y)?)[0];
            l.is_finite().then_some((l, m))
        })
        .collect();
    let mut ls: Vec<f32> = samples.iter().filter(|(_, m)| *m >= 0.5).map(|(l, _)| *l).collect();
    if ls.is_empty() {
        ls = samples.iter().map(|(l, _)| *l).collect();
    }
    ls.sort_by(f32::total_cmp);
    ls.get(ls.len() * 4 / 5).copied().filter(|b| *b > 1e-4)
}

fn whiten_teeth(img: &mut Rgb32f, scene: &Scene<'_>, face: usize, amount: f32) {
    let w = img.width;
    let px = masked(scene, face, Part::Teeth, w, img.height);
    if px.is_empty() {
        return;
    }
    let Some(bright) = teeth_level(img, scene, face) else { return };
    for (x, y, m) in px {
        let Some(p) = img.data.get_mut(y * w + x) else { continue };
        let [l, a, b] = oklab(*p);
        if !(l.is_finite() && a.is_finite() && b.is_finite()) {
            continue;
        }
        // lips and gums (clearly red) are not teeth, whatever the mask's edge says
        let tooth = 1.0 - smoothstep(0.035, 0.08, a - 0.4 * b.max(0.0));
        // gaps and the mouth's shade get less lift and less neutralising
        let lit = smoothstep(0.35, 0.8, l / bright);
        let k = amount * m * tooth;
        if k <= 0.0 {
            continue;
        }
        let keep = 1.0 - TEETH_NEUTRAL * k * (0.6 + 0.4 * lit);
        // yellow (b > 0) goes toward neutral; a cool cast (b ≤ 0) is kept as it is
        let (a2, b2) = if b > 0.0 { (a * keep, b * keep) } else { (a, b) };
        let l2 = l + TEETH_LIFT * k * lit * (TEETH_CEILING - l).max(0.0);
        *p = in_gamut(*p, [l2, a2, b2]);
    }
}

struct Lips {
    /// Lip Color 0…1.
    colour: f32,
    /// The picked colour in OkLab.
    target: [f32; 3],
    /// Lip Hue as a turn in radians.
    hue: f32,
    /// Lip Saturation −1…1.
    saturation: f32,
}

/// The lips' main colour: lightness, chroma and hue of `samples` (weight, colour), lightness weighted by the
/// weight², hue and chroma also by chroma (a highlight or the dark line between the lips counts little).
fn main_colour(samples: impl Iterator<Item = (f32, [f32; 3])>) -> Option<(f32, f32, f32)> {
    let (mut sw, mut sl, mut sa, mut sb) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut sw_l = 0.0f64;
    for (m, p) in samples {
        let [l, a, b] = oklab(p);
        if !(l.is_finite() && a.is_finite() && b.is_finite()) {
            continue;
        }
        let c = (a * a + b * b).sqrt();
        let wt = f64::from(m * m * c);
        sw += wt;
        sa += wt * f64::from(a);
        sb += wt * f64::from(b);
        let wl = f64::from(m * m);
        sw_l += wl;
        sl += wl * f64::from(l);
    }
    if sw <= 1e-9 || sw_l <= 1e-9 {
        return None;
    }
    let (a, b) = ((sa / sw) as f32, (sb / sw) as f32);
    let l = (sl / sw_l) as f32;
    let c = (a * a + b * b).sqrt();
    (l > 1e-4 && c.is_finite()).then_some((l, c, b.atan2(a)))
}

/// The face mesh's lip outlines in pixels, the outer one's margin ([`LIP_MARGIN`]) and the soft edges in pixels
/// (`edge` inside the outer one's margin, `feather` inside the inner one).
struct Mesh {
    outer: Vec<(f64, f64)>,
    inner: Vec<(f64, f64)>,
    edge: f64,
    feather: f64,
    margin: f64,
}

/// How much of the lips a pixel position is, given the Lips and Teeth masks there: the lips less the teeth (a
/// parser's lips can take in an open mouth), held inside the face mesh's outer lip outline (a parser's mask at a
/// small face spills onto the skin around) and out of its inner one (the open mouth).
fn lip_share(lips: f32, teeth: f32, mesh: Option<&Mesh>, x: f64, y: f64) -> f32 {
    let m = lips * (1.0 - teeth.clamp(0.0, 1.0));
    match mesh {
        Some(k) if m > 0.0 => m * inside(&k.outer, x, y, k.edge, k.margin) * (1.0 - inside(&k.inner, x, y, k.feather, 0.0)),
        _ => m,
    }
}

fn recolour_lips(img: &mut Rgb32f, scene: &Scene<'_>, face: usize, lips: &Lips) {
    let w = img.width;
    let mesh = scene.faces.get(face).and_then(|f| {
        Some(Mesh {
            outer: outline(f, &OUTER_LIPS)?,
            inner: outline(f, &INNER_LIPS)?,
            edge: f.px(LIP_EDGE),
            feather: f.px(MOUTH_FEATHER),
            margin: f.px(LIP_MARGIN),
        })
    });
    let teeth = |x: usize, y: usize| scene.mask_at(face, Part::Teeth, x, y);
    let px: Vec<(usize, usize, f32)> = masked(scene, face, Part::Lips, w, img.height)
        .into_iter()
        .map(|(x, y, m)| (x, y, lip_share(m, teeth(x, y), mesh.as_ref(), x as f64 + 0.5, y as f64 + 0.5)))
        .filter(|(_, _, m)| *m > 0.0)
        .collect();
    if px.is_empty() {
        return;
    }
    // the main colour from the fixed grid over the Lips mask, never from the pixels this render happens to hold
    let view = scene.view();
    let teeth_mask = scene.faces.get(face).and_then(|f| f.face.mask(Part::Teeth));
    let main = main_colour(grid(scene, face, Part::Lips).into_iter().filter_map(|(nx, ny, m)| {
        let (x, y) = view.to_px(nx, ny)?;
        let share = lip_share(m, teeth_mask.map_or(0.0, |t| t.sample(nx, ny)), mesh.as_ref(), x, y);
        (share > 0.0).then_some((share, sample(img, x, y)?))
    }));
    let (tl, ta, tb) = (lips.target[0], lips.target[1], lips.target[2]);
    let (tc, th) = ((ta * ta + tb * tb).sqrt(), tb.atan2(ta));
    let sat = if lips.saturation >= 0.0 { 1.0 + LIP_SATURATION_UP * lips.saturation } else { 1.0 + LIP_SATURATION_DOWN * lips.saturation };
    for (x, y, mask) in px {
        let Some(p) = img.data.get_mut(y * w + x) else { continue };
        let [l, a, b] = oklab(*p);
        if !(l.is_finite() && a.is_finite() && b.is_finite()) {
            continue;
        }
        // what is not lip colour inside the mask stays: a clearly other hue (teeth the Teeth mask missed, skin), or
        // a bright near-grey (teeth, a highlight)
        let c = (a * a + b * b).sqrt();
        let m = match main {
            Some((ml, _, mh)) => {
                let other_hue = smoothstep(0.5, 1.0, wrap(b.atan2(a) - mh).abs()) * smoothstep(0.015, 0.04, c);
                let bright_grey = smoothstep(ml + 0.08, ml + 0.2, l) * (1.0 - smoothstep(0.02, 0.05, c));
                mask * (1.0 - other_hue) * (1.0 - bright_grey)
            }
            None => mask,
        };
        if m <= 0.0 {
            continue;
        }
        let (mut nl, mut na, mut nb) = (l, a, b);
        if let Some((ml, mc, mh)) = main.filter(|_| lips.colour > 0.0) {
            let k = lips.colour * m;
            // lightness in proportion to the main colour's below it; above it, the room left up to 1 shared out
            let rl = if l <= ml { l * tl / ml } else { tl + (l - ml) * (1.0 - tl).max(0.0) / (1.0 - ml).max(1e-3) };
            // chroma in proportion, softly held under 1.6 × the target's, faded on the darkest and brightest pixels
            let fade = smoothstep(0.02, 0.18, rl) * (1.0 - smoothstep(0.85, 1.0, rl));
            let rc = (tc * (c / mc.max(1e-4))).min(1.6 * tc) * (0.35 + 0.65 * fade);
            // the pixel keeps its own offset from the lips' hue (only where it is clearly coloured)
            let off = wrap(b.atan2(a) - mh).clamp(-0.25, 0.25) * smoothstep(0.02, 0.06, c);
            let rh = th + off;
            nl = l + (rl - l) * k;
            na = a + (rc * rh.cos() - a) * k;
            nb = b + (rc * rh.sin() - b) * k;
        }
        if lips.hue != 0.0 {
            let (s, co) = (lips.hue * m).sin_cos();
            (na, nb) = (na * co - nb * s, na * s + nb * co);
        }
        if lips.saturation != 0.0 {
            let f = 1.0 + (sat - 1.0) * m;
            (na, nb) = (na * f, nb * f);
        }
        *p = in_gamut(*p, [nl, na, nb]);
    }
}

/// The face mesh's inner lip outline, in order around the mouth (MediaPipe's numbering).
const INNER_LIPS: [usize; 20] = [78, 191, 80, 81, 82, 13, 312, 311, 310, 415, 308, 324, 318, 402, 317, 14, 87, 178, 88, 95];
/// Where the face's landmarks hold the 478 of the face mesh: after YuNet's five.
const MESH_START: usize = 5;
/// The soft edge inside the inner lip outline (into the open mouth), as a fraction of the face.
const MOUTH_FEATHER: f64 = 0.006;
/// How far past the mesh's outer lip outline the lips may reach, as a fraction of the face; nothing past it changes.
/// The mesh's outline can lie this far inside the lips' real edge (a wide smile's upper lip: about 0.012), so the
/// outline guards against a mask spilling onto the skin around while the Lips mask draws the edge.
pub const LIP_MARGIN: f64 = 0.015;
/// The soft edge inside that margin, as a fraction of the face.
const LIP_EDGE: f64 = 0.0015;

/// The face mesh's outer lip outline, in order around the mouth.
const OUTER_LIPS: [usize; 20] = [61, 185, 40, 39, 37, 0, 267, 269, 270, 409, 291, 375, 321, 405, 314, 17, 84, 181, 91, 146];

/// A face mesh outline (`INNER_LIPS`, `OUTER_LIPS`) in pixels; `None` without the mesh.
fn outline(f: &crate::FaceCtx<'_>, points: &[usize]) -> Option<Vec<(f64, f64)>> {
    points.iter().map(|i| f.landmarks.get(MESH_START + i).copied().flatten()).collect()
}

/// How far (`x`, `y`) is inside polygon `poly` grown by `grow` pixels: 0 on and past the grown edge, 1 from `feather`
/// pixels inside it, a smooth step between (a hard edge for a feather of 0).
fn inside(poly: &[(f64, f64)], x: f64, y: f64, feather: f64, grow: f64) -> f32 {
    let (mut inside, mut d2) = (false, f64::INFINITY);
    for (i, a) in poly.iter().enumerate() {
        let Some(b) = poly.get((i + 1) % poly.len()) else { continue };
        if (a.1 > y) != (b.1 > y) && x < a.0 + (y - a.1) * (b.0 - a.0) / (b.1 - a.1) {
            inside = !inside;
        }
        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
        let len2 = ex * ex + ey * ey;
        let t = if len2 > 0.0 { (((x - a.0) * ex + (y - a.1) * ey) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let (dx, dy) = (x - a.0 - t * ex, y - a.1 - t * ey);
        d2 = d2.min(dx * dx + dy * dy);
    }
    let d = if inside { d2.sqrt() } else { -d2.sqrt() } + grow;
    if !d.is_finite() || d <= 0.0 {
        return 0.0;
    }
    smoothstep(0.0, feather as f32, d as f32)
}

/// An angle in −π…π.
fn wrap(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a + PI).rem_euclid(TAU) - PI
}

/// `lab` as linear RGB, its chroma reduced as far as needed so that no channel goes below where `before` was (or
/// 0): more colour than the primaries hold turns into less chroma, not a wrong hue.
fn in_gamut(before: [f32; 3], lab: [f32; 3]) -> [f32; 3] {
    let floor = before.iter().fold(0.0f32, |a, v| if v.is_finite() { a.min(*v) } else { a });
    let ok = |p: [f32; 3]| p.iter().all(|v| v.is_finite() && *v >= floor - 1e-6);
    let full = rgb(lab);
    if ok(full) {
        return full;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..12 {
        let t = 0.5 * (lo + hi);
        if ok(rgb([lab[0], lab[1] * t, lab[2] * t])) { lo = t } else { hi = t }
    }
    let out = rgb([lab[0], lab[1] * lo, lab[2] * lo]);
    if out.iter().all(|v| v.is_finite()) { out.map(|v| v.max(floor)) } else { before }
}

/// Linear Rec.2020 → OkLab.
pub fn oklab(p: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = p;
    let l = 0.616_688_44 * r + 0.360_159_07 * g + 0.023_043_294 * b;
    let m = 0.265_140_2 * r + 0.635_856_5 * g + 0.099_030_23 * b;
    let s = 0.100_150_645 * r + 0.204_004_32 * g + 0.696_324_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OkLab → linear Rec.2020.
pub fn rgb(lab: [f32; 3]) -> [f32; 3] {
    let [ll, a, b] = lab;
    let l = ll + 0.396_337_78 * a + 0.215_803_76 * b;
    let m = ll - 0.105_561_346 * a - 0.063_854_17 * b;
    let s = ll - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        2.140_140_4 * l - 1.246_356 * m + 0.106_431_73 * s,
        -0.884_832_45 * l + 2.163_172_7 * m - 0.278_361_6 * s,
        -0.048_579_06 * l - 0.454_490_9 * m + 1.502_356_3 * s,
    ]
}

/// An sRGB colour (0…255 per channel, as the colour picker shows it) in OkLab, through linear Rec.2020.
pub fn oklab_of_srgb8(c: [f64; 3]) -> [f32; 3] {
    let lin = |v: f64| {
        let v = if v.is_finite() { (v / 255.0).clamp(0.0, 1.0) } else { 0.0 };
        (if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }) as f32
    };
    oklab(srgb_to_rec2020([lin(c[0]), lin(c[1]), lin(c[2])]))
}

/// Linear sRGB → linear Rec.2020 (the examples read sRGB pictures).
pub fn srgb_to_rec2020(p: [f32; 3]) -> [f32; 3] {
    [
        0.627_403_9 * p[0] + 0.329_283_04 * p[1] + 0.043_313_07 * p[2],
        0.069_097_29 * p[0] + 0.919_540_4 * p[1] + 0.011_362_3 * p[2],
        0.016_391_44 * p[0] + 0.088_013_31 * p[1] + 0.895_595_25 * p[2],
    ]
}

/// Linear Rec.2020 → linear sRGB.
pub fn rec2020_to_srgb(p: [f32; 3]) -> [f32; 3] {
    [
        1.660_491 * p[0] - 0.587_641_14 * p[1] - 0.072_849_88 * p[2],
        -0.124_550_47 * p[0] + 1.132_899_9 * p[1] - 0.008_349_402 * p[2],
        -0.018_150_763 * p[0] - 0.100_578_9 * p[1] + 1.118_729_7 * p[2],
    ]
}
