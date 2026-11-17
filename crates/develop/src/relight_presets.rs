//! Fork (issue #301, `fork/relight.md`): light presets. A preset is a set of lights (and Darken Surroundings) written
//! for a subject, not for a photo: where its lights are is measured in the subject's own size. Applying it places them
//! round the subject the app found on the photo ([`Subject`]: a face, a person or subject mask, the photo's centre), so
//! one preset fits a portrait, a group and a landscape alike.
//!
//! **The preset frame.** A preset's lights are saved as ordinary Relight settings for a subject standing at the photo's
//! centre, with one subject width (or height) equal to [`UNIT`] of the photo. A light `0.8` subject widths to the left
//! and `1.0` subject heights above the subject is saved at `x = 0.5 − 0.8·UNIT`, `y = 0.5 − 1.0·UNIT`. The same holds for
//! a Spot's aim and a Softbox's size. Light types that have no place (a Rim, a Fill, a Sun's direction) and every other
//! number (depth, Amount, Softness, colour, gel, pattern, haze, Cast Shadows) are carried as they are. The frame keeps
//! every number a light holds inside the ranges its controls have, so a saved preset reads with the lenient reader of
//! [`Relight`] and is never clamped out of shape.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::relight::{KindSettings, LightSettings, Relight};
use crate::relight_effects::LightEffects;
use crate::{DevelopSettings, SegMask};

/// One subject width (or height) in the preset frame, as a share of the photo.
pub const UNIT: f64 = 0.25;

/// The most presets of one's own a library keeps, and the longest name (characters; a longer one is refused).
pub const MAX_USER: usize = 100;
pub const MAX_NAME: usize = 40;

/// The subject's box on the photo: where its centre is and how wide and tall it is, in the same normalized photo
/// coordinates as a light's place (before the crop and the lens corrections).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Subject {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// The smallest and largest a subject's box is taken to be (a speck, or the whole frame, would send the lights to a
/// pin or off the photo).
const MIN_SIZE: f64 = 0.12;
const MAX_SIZE: f64 = 1.0;

impl Subject {
    /// A photo the app found no subject on: its centre, a little over a third of its width.
    pub const CENTRE: Subject = Subject { x: 0.5, y: 0.5, w: 0.4, h: 0.5 };

    /// The subject inside box `[x0, y0, x1, y1]` (normalized); `None` for a box that is not one.
    pub fn from_box(b: [f64; 4]) -> Option<Subject> {
        let [x0, y0, x1, y1] = b;
        if !b.iter().all(|v| v.is_finite()) || x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Subject { x: (x0 + x1) / 2.0, y: (y0 + y1) / 2.0, w: x1 - x0, h: y1 - y0 }.kept())
    }

    /// The head and shoulders of a person whose face is inside `face` (normalized): the face's centre is a little above
    /// the box's, which is two and a half faces wide and three tall.
    pub fn from_face(face: [f64; 4]) -> Option<Subject> {
        let f = Subject::from_box(face)?;
        Some(Subject { x: f.x, y: f.y + 0.6 * f.h, w: 2.5 * f.w, h: 3.0 * f.h }.kept())
    }

    /// The numbers within the photo and the sizes the presets are written for.
    fn kept(self) -> Subject {
        let f = |v: f64, lo: f64, hi: f64| if v.is_finite() { v.clamp(lo, hi) } else { lo };
        Subject { x: f(self.x, 0.0, 1.0), y: f(self.y, 0.0, 1.0), w: f(self.w, MIN_SIZE, MAX_SIZE), h: f(self.h, MIN_SIZE, MAX_SIZE) }
    }

    /// The photo coordinate of `v` of the preset frame along an axis whose subject is at `centre` with extent `size`.
    fn to_photo(v: f64, centre: f64, size: f64) -> f64 {
        centre + (v - 0.5) / UNIT * size
    }

    /// The preset-frame coordinate of photo coordinate `v`, the inverse of [`Subject::to_photo`].
    fn to_frame(v: f64, centre: f64, size: f64) -> f64 {
        0.5 + (v - centre) / size * UNIT
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

    /// The lights of preset `r` (written in the preset frame) round this subject.
    pub fn place(&self, r: &Relight) -> Relight {
        let mut out = r.clone();
        for l in &mut out.lights {
            Subject::map_light(l, |v, x, size| {
                let (c, e) = if x { (self.x, self.w) } else { (self.y, self.h) };
                if size { v / UNIT * e } else { Subject::to_photo(v, c, e) }
            });
        }
        out.sanitize();
        out
    }

    /// The lights `r` has on this subject, in the preset frame (the inverse of [`Subject::place`], to the range a light
    /// can hold).
    pub fn capture(&self, r: &Relight) -> Relight {
        let mut out = r.clone();
        for l in &mut out.lights {
            Subject::map_light(l, |v, x, size| {
                let (c, e) = if x { (self.x, self.w) } else { (self.y, self.h) };
                if size { v / e * UNIT } else { Subject::to_frame(v, c, e) }
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

/// A light at `(ox, oy)` subject widths and heights from the subject's centre (right and down are positive), at `depth`.
fn light(amount: f64, (ox, oy): (f64, f64), depth: f64, softness: f64, warmth: f64) -> LightSettings {
    LightSettings { amount, softness, warmth, x: 0.5 + ox * UNIT, y: 0.5 + oy * UNIT, depth, ..LightSettings::default() }
}

fn with_kind(mut l: LightSettings, f: impl FnOnce(&mut KindSettings)) -> LightSettings {
    f(&mut l.kind);
    l
}

fn with_effects(mut l: LightSettings, f: impl FnOnce(&mut LightEffects)) -> LightSettings {
    f(&mut l.effects);
    l
}

/// A softbox of `w × h` subject widths and heights, at the light's place.
fn softbox(l: LightSettings, (w, h): (f64, f64), rotation: f64) -> LightSettings {
    with_kind(l, |k| {
        k.which = 3.0;
        k.box_width = w * UNIT;
        k.box_height = h * UNIT;
        k.box_rotation = rotation;
    })
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

/// The presets that ship with the app, all original setups: the names say what a photographer calls them, the numbers
/// are this app's own.
pub fn built_in() -> &'static [LightPreset] {
    static LIST: OnceLock<Vec<LightPreset>> = OnceLock::new();
    LIST.get_or_init(|| {
        vec![
            // A soft key high and to the side (about 45° up and across), a whisper of fill, a darker room: the lit side
            // of the face carries a small triangle of light under the eye on the shadow side.
            preset(
                "rembrandt",
                "Rembrandt",
                40.0,
                vec![
                    with_effects(softbox(light(95.0, (-0.75, -0.75), 12.0, 35.0, 8.0), (0.9, 0.7), 0.0), |e| e.shadows = 40.0),
                    fill(10.0, 0.0, 45.0),
                ],
            ),
            // The key straight in front and high, its shadow a small butterfly under the nose; a fill from below.
            preset(
                "butterfly",
                "Butterfly",
                30.0,
                vec![with_effects(softbox(light(95.0, (0.0, -0.95), 10.0, 35.0, 0.0), (1.0, 0.6), 0.0), |e| e.shadows = 35.0), fill(20.0, 0.0, 55.0)],
            ),
            // One tight light from the side, aimed at the face: half the face lit, half in shadow, the backdrop left alone.
            preset(
                "split",
                "Split",
                45.0,
                vec![with_effects(
                    with_kind(light(85.0, (-1.4, -0.2), 20.0, 25.0, 0.0), |k| {
                        k.which = 1.0;
                        k.spot_aim_x = 0.5;
                        k.spot_aim_y = 0.5 - 0.2 * UNIT;
                        k.spot_angle = 26.0;
                        k.spot_edge = 55.0;
                    }),
                    |e| e.shadows = 35.0,
                )],
            ),
            // The subject's outline lit from behind-right in a cool light, a warm fill keeping the face open.
            preset("rim-fill", "Rim + Fill", 22.0, vec![rim(90.0, -20.0, (60.0, 70.0, 65.0)), fill(25.0, 8.0, 50.0)]),
            // Low warm sun through a window: a warm broad light from the left, a cool fill from the sky.
            preset(
                "golden-hour-window",
                "Golden-Hour Window",
                30.0,
                vec![
                    with_effects(
                        LightSettings {
                            wrap: 5.0,
                            ..with_kind(light(32.0, (-1.5, -0.4), 20.0, 65.0, 40.0), |k| {
                                k.which = 2.0;
                                k.sun_azimuth = -55.0;
                                k.sun_elevation = 16.0;
                            })
                        },
                        |e| {
                            e.shadows = 30.0;
                            e.haze = 8.0;
                        },
                    ),
                    fill(12.0, -25.0, 40.0),
                ],
            ),
            // A magenta light from the left and a teal one from the right, the room dark so the colours show.
            preset(
                "neon-two-tone",
                "Neon Two-Tone",
                55.0,
                vec![
                    with_effects(softbox(light(50.0, (-1.3, -0.1), 25.0, 40.0, 0.0), (0.8, 1.4), 0.0), |e| {
                        (e.gel_hue, e.gel_strength) = (305.0, 60.0)
                    }),
                    with_effects(softbox(light(50.0, (1.3, 0.1), 25.0, 40.0, 0.0), (0.8, 1.4), 0.0), |e| (e.gel_hue, e.gel_strength) = (178.0, 60.0)),
                ],
            ),
            // One small, warm flame close and low in front: it falls off fast, the rest of the room is dark.
            preset(
                "candle",
                "Candle",
                65.0,
                vec![with_effects(light(75.0, (0.25, 0.3), 4.0, 18.0, 100.0), |e| {
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
    fn a_box_and_a_face_make_a_subject_and_nonsense_makes_none() {
        let s = Subject::from_box([0.2, 0.1, 0.6, 0.9]).unwrap();
        assert!((s.x - 0.4).abs() < 1e-9 && (s.y - 0.5).abs() < 1e-9 && (s.w - 0.4).abs() < 1e-9 && (s.h - 0.8).abs() < 1e-9);
        for bad in [[0.5, 0.5, 0.5, 0.9], [f64::NAN, 0.0, 1.0, 1.0], [0.6, 0.1, 0.2, 0.9]] {
            assert_eq!(Subject::from_box(bad), None);
        }
        // a speck is taken as the smallest subject, the whole frame as the largest
        let speck = Subject::from_box([0.5, 0.5, 0.51, 0.51]).unwrap();
        assert_eq!((speck.w, speck.h), (MIN_SIZE, MIN_SIZE));
        let face = Subject::from_face([0.4, 0.2, 0.5, 0.35]).unwrap();
        assert!(face.y > 0.275 && face.w > 0.2, "head and shoulders reach below and beyond the face: {face:?}");
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
            assert!((p.kind.box_width - q.kind.box_width).abs() < 1e-9);
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
