//! Fork (issue #301, `fork/relight.md`): light presets. A preset is a set of lights (and Darken Surroundings) written
//! for a face, not for a photo: where its lights are is measured from the face the app found on the photo
//! ([`Subject`]: a face's box; else a face worked out from a person or subject mask's box; else a face at the photo's
//! centre), so one preset fits a portrait, a group and a landscape alike.
//!
//! **The preset frame.** A preset's lights are saved as ordinary Relight settings for a face of [`CANON`] size standing at
//! the photo's centre: a key 0.4 of the photo's width to the left of the face and 0.45 of its height above it is saved at
//! `x = 0.1`, `y = 0.05`. Applying it to a photo whose face is `k` times as wide moves the light `k` times as far from the
//! face (the same along the height). A Spot's aim and a Softbox's size scale the same way. Light types that have no
//! place (a Rim, a Fill, a Sun's direction) and every other number (depth, Amount, Softness, colour, gel, pattern, haze,
//! Cast Shadows) are carried as they are. The frame keeps every number inside the ranges its control has, so a saved
//! preset reads with the lenient reader of [`Relight`] and is never clamped out of shape.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::relight::{KindSettings, LightSettings, Relight};
use crate::relight_effects::LightEffects;
use crate::{DevelopSettings, SegMask};

/// The face a preset's lights are written for: its width and height as shares of the photo (a head and shoulders
/// portrait of a 3:2 photo).
pub const CANON: (f64, f64) = (0.15, 0.25);

/// The most presets of one's own a library keeps, and the longest name (characters; a longer one is refused).
pub const MAX_USER: usize = 100;
pub const MAX_NAME: usize = 40;

/// The subject: a face's box, in the same normalized photo coordinates as a light's place (before the crop and the lens
/// corrections): where its centre is and how wide and tall it is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Subject {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// The smallest and largest a face is taken to be, as shares of the photo (a speck or the whole frame would send the
/// lights to a pin or far off the photo).
const MIN_SIZE: (f64, f64) = (0.08, 0.12);
const MAX_SIZE: (f64, f64) = (0.40, 0.60);

impl Subject {
    /// A photo the app found no subject on: a face of the canonical size, a little above its centre.
    pub const CENTRE: Subject = Subject { x: 0.5, y: 0.42, w: CANON.0, h: CANON.1 };

    /// The face inside box `[x0, y0, x1, y1]` (normalized); `None` for a box that is not one.
    pub fn from_face(b: [f64; 4]) -> Option<Subject> {
        let [x0, y0, x1, y1] = b;
        if !b.iter().all(|v| v.is_finite()) || x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Subject { x: (x0 + x1) / 2.0, y: (y0 + y1) / 2.0, w: x1 - x0, h: y1 - y0 }.kept())
    }

    /// The face of whoever a mask's box `[x0, y0, x1, y1]` holds: a face a little over a third as wide and a third as
    /// tall as the box, in its upper part (a person's head sits at the top and their shoulders are about three heads wide).
    pub fn from_mask_box(b: [f64; 4]) -> Option<Subject> {
        let [x0, y0, x1, y1] = b;
        if !b.iter().all(|v| v.is_finite()) || x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (w, h) = (x1 - x0, y1 - y0);
        Some(Subject { x: (x0 + x1) / 2.0, y: y0 + 0.2 * h, w: 0.38 * w, h: 0.3 * h }.kept())
    }

    /// The numbers within the photo and the sizes the presets are written for.
    fn kept(self) -> Subject {
        let f = |v: f64, (lo, hi): (f64, f64)| if v.is_finite() { v.clamp(lo, hi) } else { lo };
        Subject { x: f(self.x, (0.0, 1.0)), y: f(self.y, (0.0, 1.0)), w: f(self.w, (MIN_SIZE.0, MAX_SIZE.0)), h: f(self.h, (MIN_SIZE.1, MAX_SIZE.1)) }
    }

    /// Every number of `l` that is a place or a size on the photo, mapped with `f(value, axis_is_x, is_size)`: its place
    /// (a Rim and a Fill have none), a Spot's aim, a Softbox's size. The numbers of a type the light isn't stay as they are.
    fn map_light(l: &mut LightSettings, f: impl Fn(f64, bool, bool) -> f64) {
        let kind = l.kind.index();
        if l.kind.has_place() {
            l.x = f(l.x, true, false);
            l.y = f(l.y, false, false);
        }
        let k = &mut l.kind;
        if kind == 1 {
            k.spot_aim_x = f(k.spot_aim_x, true, false);
            k.spot_aim_y = f(k.spot_aim_y, false, false);
        }
        if kind == 3 {
            k.box_width = f(k.box_width, true, true);
            k.box_height = f(k.box_height, false, true);
        }
    }

    /// The lights of preset `r` (written in the preset frame) round this face.
    pub fn place(&self, r: &Relight) -> Relight {
        let mut out = r.clone();
        let (kx, ky) = (self.w / CANON.0, self.h / CANON.1);
        for l in &mut out.lights {
            Subject::map_light(l, |v, x, size| {
                let (c, k) = if x { (self.x, kx) } else { (self.y, ky) };
                if size { v * k } else { c + (v - 0.5) * k }
            });
        }
        out.sanitize();
        out
    }

    /// The lights `r` has on this face, in the preset frame (the inverse of [`Subject::place`], to the range a light can
    /// hold).
    pub fn capture(&self, r: &Relight) -> Relight {
        let mut out = r.clone();
        let (kx, ky) = (self.w / CANON.0, self.h / CANON.1);
        for l in &mut out.lights {
            Subject::map_light(l, |v, x, size| {
                let (c, k) = if x { (self.x, kx) } else { (self.y, ky) };
                if size { v / k } else { 0.5 + (v - c) / k }
            });
        }
        out.sanitize();
        out
    }
}

/// The box of what a segmentation selects (a cell above zero), normalized `[x0, y0, x1, y1]`; `None` when it selects
/// nothing or its data is damaged. Cells are the grid's, so the box is as fine as the grid is.
pub fn selection_box(seg: &SegMask) -> Option<[f64; 4]> {
    let side = seg.side as usize;
    let logits = seg.logits()?;
    let r = seg.bounds()?;
    if side == 0 || logits.len() < side.checked_mul(side)? {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    let mut cells = 0usize;
    for (i, v) in logits.iter().take(side * side).enumerate() {
        if *v > 0.0 {
            let (cx, cy) = (i % side, i / side);
            (x0, y0, x1, y1) = (x0.min(cx), y0.min(cy), x1.max(cx), y1.max(cy));
            cells += 1;
        }
    }
    // (a few stray cells are noise, not a subject)
    if cells < 4 {
        return None;
    }
    let at = |c: usize, lo: f64, hi: f64| lo + (hi - lo) * c as f64 / side as f64;
    Some([at(x0, r[0], r[2]), at(y0, r[1], r[3]), at(x1 + 1, r[0], r[2]), at(y1 + 1, r[1], r[3])])
}

/// A light preset: a name and lights in the preset frame (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LightPreset {
    pub id: String,
    pub name: String,
    /// The lights and Darken Surroundings, in the preset frame.
    pub lights: Relight,
    /// Ships with the app: never saved, never renamed or deleted.
    #[serde(skip)]
    pub builtin: bool,
}

impl LightPreset {
    /// The lights of this preset round `subject`.
    pub fn placed(&self, subject: &Subject) -> Relight {
        subject.place(&self.lights)
    }

    /// `d` with this preset's lights round `subject` instead of its own, and the Relight section's eye on (a preset that
    /// changes nothing the photo shows would look like a button that does nothing).
    pub fn onto(&self, d: &DevelopSettings, subject: &Subject) -> DevelopSettings {
        let mut out = d.clone();
        out.relight = self.placed(subject);
        out.disabled_sections.retain(|s| s != crate::relight::SECTION);
        out
    }

    /// A preset of the lights `r` has round `subject`.
    pub fn capture(id: &str, name: &str, r: &Relight, subject: &Subject) -> LightPreset {
        LightPreset { id: id.into(), name: name.into(), lights: subject.capture(r), builtin: false }
    }
}

/// A name as a preset keeps it: control characters dropped, trimmed, inner runs of spaces made one; `None` for nothing.
pub fn clean_name(name: &str) -> Option<String> {
    let s = name.chars().filter(|c| !c.is_control() || c.is_whitespace()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

/// A light `(dx, dy)` of the photo's width and height from the face's centre (right and down are positive), at `depth`.
fn light(amount: f64, (dx, dy): (f64, f64), depth: f64, softness: f64, warmth: f64) -> LightSettings {
    LightSettings { amount, softness, warmth, x: 0.5 + dx, y: 0.5 + dy, depth, ..LightSettings::default() }
}

fn with_kind(mut l: LightSettings, f: impl FnOnce(&mut KindSettings)) -> LightSettings {
    f(&mut l.kind);
    l
}

fn with_effects(mut l: LightSettings, f: impl FnOnce(&mut LightEffects)) -> LightSettings {
    f(&mut l.effects);
    l
}

/// A rim light from `angle` (0 above, 90 the right, −90 the left), `side` how one-sided it is.
fn rim(amount: f64, warmth: f64, (width, angle, side): (f64, f64, f64)) -> LightSettings {
    let l = LightSettings { amount, warmth, ..LightSettings::default() };
    with_kind(l, |k| {
        k.which = 4.0;
        k.rim_width = width;
        k.rim_angle = angle;
        k.rim_side = side;
    })
}

fn fill(amount: f64, warmth: f64, near: f64) -> LightSettings {
    let l = LightSettings { amount, warmth, ..LightSettings::default() };
    with_kind(l, |k| {
        k.which = 5.0;
        k.fill_near = near;
    })
}

fn preset(id: &str, name: &str, darken: f64, lights: Vec<LightSettings>) -> LightPreset {
    let mut r = Relight { lights, darken };
    r.sanitize();
    LightPreset { id: id.into(), name: name.into(), lights: r, builtin: true }
}

/// A spot aimed at the face (a little below its centre), `angle` degrees wide with edge `edge`.
fn spot(l: LightSettings, angle: f64, edge: f64) -> LightSettings {
    with_kind(l, |k| {
        k.which = 1.0;
        k.spot_aim_x = 0.5;
        k.spot_aim_y = 0.53;
        k.spot_angle = angle;
        k.spot_edge = edge;
    })
}

fn shadows(mut l: LightSettings, amount: f64) -> LightSettings {
    l.effects.shadows = amount;
    l
}

fn wrap(mut l: LightSettings, wrap: f64) -> LightSettings {
    l.wrap = wrap;
    l
}

fn gel(mut l: LightSettings, hue: f64, strength: f64) -> LightSettings {
    (l.effects.gel_hue, l.effects.gel_strength) = (hue, strength);
    l
}

/// The presets that ship with the app, all original setups: the names say what a photographer calls them, the numbers
/// are this app's own. Positions are in the preset frame (see the module docs): `(dx, dy)` from the face. Each light is
/// a spot or a point rather than a Sun / Window: a Sun at a grazing angle lights the wall along the subject's outline
/// (the depth model's soft depth steps tilt the wall's normals towards it) and leaves a pale halo there. The room is
/// darkened (Darken Surroundings) so the lit side and the shadow side differ at a glance.
pub fn built_in() -> &'static [LightPreset] {
    static LIST: OnceLock<Vec<LightPreset>> = OnceLock::new();
    LIST.get_or_init(|| {
        vec![
            // A key about 45° to one side and 45° above the face, close to it: the near cheek bright, the far cheek in
            // shadow with the nose's shadow reaching towards it; a weak fill, so the key is several times the shadow side.
            preset(
                "rembrandt",
                "Rembrandt",
                75.0,
                vec![shadows(wrap(spot(light(100.0, (-0.34, -0.34), 40.0, 40.0, 6.0), 30.0, 60.0), 15.0), 100.0), fill(4.0, 0.0, 45.0)],
            ),
            // The key straight in front and high: a small shadow under the nose and chin, a soft fill from below.
            preset(
                "butterfly",
                "Butterfly",
                65.0,
                vec![shadows(wrap(spot(light(100.0, (0.0, -0.42), 40.0, 40.0, 0.0), 30.0, 60.0), 15.0), 100.0), fill(15.0, 0.0, 55.0)],
            ),
            // One light exactly from the side, level with the face: half of it lit, half in shadow.
            preset("split", "Split", 70.0, vec![shadows(wrap(spot(light(100.0, (-0.5, 0.0), 45.0, 40.0, 0.0), 30.0, 50.0), 0.0), 100.0)]),
            // The subject's whole outline lit from behind, the face softly lit from the front.
            preset("rim-fill", "Rim + Fill", 75.0, vec![rim(85.0, 0.0, (45.0, 0.0, 0.0)), fill(45.0, 4.0, 70.0)]),
            // A low warm light raking across the face and body from the left, the shadow side and the wall away from it
            // cooler and darker.
            preset(
                "golden-hour-window",
                "Golden-Hour Window",
                60.0,
                vec![
                    gel(shadows(wrap(spot(light(100.0, (-0.6, 0.0), 60.0, 40.0, 100.0), 44.0, 80.0), 20.0), 100.0), 38.0, 45.0),
                    wrap(light(10.0, (0.5, 0.1), 60.0, 80.0, -45.0), 40.0),
                ],
            ),
            // A magenta light from the left and a teal one from the right, the room dark so the colours show.
            preset(
                "neon-two-tone",
                "Neon Two-Tone",
                75.0,
                vec![
                    gel(shadows(spot(light(50.0, (-0.5, 0.05), 50.0, 40.0, 0.0), 32.0, 70.0), 60.0), 305.0, 85.0),
                    gel(shadows(spot(light(50.0, (0.5, 0.05), 50.0, 40.0, 0.0), 32.0, 70.0), 60.0), 178.0, 85.0),
                ],
            ),
            // One small, warm flame close and low in front: it falls off fast, the rest of the room is dark.
            preset(
                "candle",
                "Candle",
                65.0,
                vec![with_effects(light(75.0, (0.08, 0.38), 4.0, 18.0, 100.0), |e| {
                    e.gel_hue = 38.0;
                    e.gel_strength = 30.0;
                    e.haze = 18.0;
                    e.shadows = 40.0;
                })],
            ),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_face_makes_a_subject_and_nonsense_makes_none() {
        let s = Subject::from_face([0.2, 0.1, 0.4, 0.5]).unwrap();
        assert!((s.x - 0.3).abs() < 1e-9 && (s.y - 0.3).abs() < 1e-9 && (s.w - 0.2).abs() < 1e-9 && (s.h - 0.4).abs() < 1e-9);
        for bad in [[0.5, 0.5, 0.5, 0.9], [f64::NAN, 0.0, 1.0, 1.0], [0.6, 0.1, 0.2, 0.9]] {
            assert_eq!(Subject::from_face(bad), None);
            assert_eq!(Subject::from_mask_box(bad), None);
        }
        // a speck is taken as the smallest face, the whole frame as the largest
        let speck = Subject::from_face([0.5, 0.5, 0.51, 0.51]).unwrap();
        assert_eq!((speck.w, speck.h), MIN_SIZE);
        let all = Subject::from_face([0.0, 0.0, 1.0, 1.0]).unwrap();
        assert_eq!((all.w, all.h), MAX_SIZE);
        // a person's mask gives a face in its upper part
        let m = Subject::from_mask_box([0.3, 0.2, 0.7, 1.0]).unwrap();
        assert!((m.x - 0.5).abs() < 1e-9 && m.y < 0.5 && m.w < 0.2, "{m:?}");
    }

    #[test]
    fn placing_follows_the_subject_and_capturing_undoes_it() {
        let rembrandt = &built_in()[0];
        let centre = rembrandt.placed(&Subject { x: 0.5, y: 0.5, w: 0.25, h: 0.25 });
        let moved = rembrandt.placed(&Subject { x: 0.3, y: 0.6, w: 0.25, h: 0.25 });
        let (a, b) = (&centre.lights[0], &moved.lights[0]);
        assert!((b.x - a.x + 0.2).abs() < 1e-9 && (b.y - a.y - 0.1).abs() < 1e-9, "{a:?} {b:?}");
        // a bigger subject takes its lights further out, in the same direction
        let big = rembrandt.placed(&Subject { x: 0.5, y: 0.5, w: 0.5, h: 0.5 });
        assert!((big.lights[0].x - 0.5).abs() > (a.x - 0.5).abs() && big.lights[0].x < 0.5);
        // what has no place on the photo stays as it was
        assert_eq!(centre.lights[1], big.lights[1]);
        // capturing what was placed gives the preset back
        let s = Subject { x: 0.34, y: 0.58, w: 0.3, h: 0.4 };
        let back = s.capture(&rembrandt.placed(&s));
        for (p, q) in rembrandt.lights.lights.iter().zip(&back.lights) {
            assert!((p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9 && p.amount == q.amount, "{p:?} {q:?}");
            assert!((p.kind.box_width - q.kind.box_width).abs() < 1e-9 && (p.kind.spot_aim_y - q.kind.spot_aim_y).abs() < 1e-9);
        }
    }

    #[test]
    fn built_in_presets_are_whole_distinct_and_within_range() {
        let list = built_in();
        assert_eq!(list.len(), 7);
        for (i, p) in list.iter().enumerate() {
            assert!(p.builtin && !p.id.is_empty() && list[..i].iter().all(|q| q.id != p.id && q.name != p.name), "{}", p.id);
            let mut r = p.lights.clone();
            r.sanitize();
            assert_eq!(r, p.lights, "{} holds a number out of range", p.id);
            assert!(p.lights.is_on() && !p.lights.lights.is_empty());
        }
    }

    #[test]
    fn a_saved_preset_reads_back_and_a_damaged_one_reads_as_empty() {
        let s = Subject::CENTRE;
        let p = LightPreset::capture("mine-1", "My Look", &built_in()[5].placed(&s), &s);
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<LightPreset>(&json).unwrap(), p);
        let odd: LightPreset = serde_json::from_str(r#"{"id": 4, "name": null, "lights": "x"}"#).unwrap_or_default();
        assert!(odd.id.is_empty());
        assert_eq!(clean_name("  A   Warm \n Look  ").as_deref(), Some("A Warm Look"));
        assert_eq!(clean_name("   "), None);
    }

    #[test]
    fn a_selection_has_the_box_of_its_cells() {
        let side = 16usize;
        let mut logits = vec![-5.0f32; side * side];
        for y in 4..12 {
            for x in 2..6 {
                logits[y * side + x] = 5.0;
            }
        }
        let seg = SegMask::from_logits(side, &logits);
        let b = selection_box(&seg).unwrap();
        assert!(
            (b[0] - 2.0 / 16.0).abs() < 1e-9
                && (b[2] - 6.0 / 16.0).abs() < 1e-9
                && (b[1] - 4.0 / 16.0).abs() < 1e-9
                && (b[3] - 12.0 / 16.0).abs() < 1e-9,
            "{b:?}"
        );
        assert_eq!(selection_box(&SegMask::from_logits(side, &vec![-5.0; side * side])), None);
    }
}
