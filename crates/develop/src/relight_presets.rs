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
//!
//! **Depth.** A light's depth is a number between the photo's nearest and farthest thing, so what it means depends on how
//! deep the scene is: a key at depth 40 is at the face's distance in one photo and far behind it in another. A preset
//! therefore writes the depth of a light that has one (a Point, a Spot, a Softbox) as a distance in front of the subject:
//! 50 is the subject's own depth, and every point below 50 is [`DEPTH_SPAN`] / 100 nearer the camera in log depth (a light
//! at `50 − 50·k` is `k` in log depth, about `e^k` times, nearer than the subject). Applying it uses the subject's own depth
//! ([`SubjectDepth`], the median depth of the face in the photo's field) and how much log depth a depth unit covers there;
//! a photo whose depth is not found yet places the depth as written.

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
    /// Its depth in the photo (`None` until the photo's depth is found).
    pub depth: Option<SubjectDepth>,
}

/// The log depth that 100 depth units of a preset's frame cover: a light written `k` log depth in front of the subject is at
/// depth `50 − 50·k` (with this span of 2: about 7 times the subject's distance at the largest).
pub const DEPTH_SPAN: f64 = 2.0;

/// Where the subject is in the photo's depth: its own depth as a light's depth value (0..100) and how much log depth 100 of
/// those values cover in this photo.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubjectDepth {
    pub own: f64,
    pub span: f64,
}

/// The smallest and largest a face is taken to be, as shares of the photo (a speck or the whole frame would send the
/// lights to a pin or far off the photo).
const MIN_SIZE: (f64, f64) = (0.08, 0.12);
const MAX_SIZE: (f64, f64) = (0.40, 0.60);

impl Subject {
    /// A photo the app found no subject on: a face of the canonical size, a little above its centre.
    pub const CENTRE: Subject = Subject { x: 0.5, y: 0.42, w: CANON.0, h: CANON.1, depth: None };

    /// The face inside box `[x0, y0, x1, y1]` (normalized); `None` for a box that is not one.
    pub fn from_face(b: [f64; 4]) -> Option<Subject> {
        let [x0, y0, x1, y1] = b;
        if !b.iter().all(|v| v.is_finite()) || x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Subject { x: (x0 + x1) / 2.0, y: (y0 + y1) / 2.0, w: x1 - x0, h: y1 - y0, depth: None }.kept())
    }

    /// The face of whoever a mask's box `[x0, y0, x1, y1]` holds: a face a little over a third as wide and a third as
    /// tall as the box, in its upper part (a person's head sits at the top and their shoulders are about three heads wide).
    pub fn from_mask_box(b: [f64; 4]) -> Option<Subject> {
        let [x0, y0, x1, y1] = b;
        if !b.iter().all(|v| v.is_finite()) || x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (w, h) = (x1 - x0, y1 - y0);
        Some(Subject { x: (x0 + x1) / 2.0, y: y0 + 0.2 * h, w: 0.38 * w, h: 0.3 * h, depth: None }.kept())
    }

    /// The numbers within the photo and the sizes the presets are written for.
    fn kept(self) -> Subject {
        let f = |v: f64, (lo, hi): (f64, f64)| if v.is_finite() { v.clamp(lo, hi) } else { lo };
        Subject {
            x: f(self.x, (0.0, 1.0)),
            y: f(self.y, (0.0, 1.0)),
            w: f(self.w, (MIN_SIZE.0, MAX_SIZE.0)),
            h: f(self.h, (MIN_SIZE.1, MAX_SIZE.1)),
            depth: self.depth,
        }
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

    /// This subject with its depth (own depth as a light's depth value, 0..100, and the log depth 100 of them cover).
    pub fn with_depth(self, own: f64, span: f64) -> Subject {
        let ok = own.is_finite() && span.is_finite() && span > 0.0;
        Subject { depth: ok.then_some(SubjectDepth { own, span }), ..self }
    }

    /// A light's depth in the preset frame (`frame` true) or in the photo, mapped to the other. Without the subject's
    /// depth it is kept as it is.
    fn map_depth(&self, v: f64, to_photo: bool) -> f64 {
        match self.depth {
            None => v,
            Some(d) if to_photo => d.own + (v - 50.0) * DEPTH_SPAN / d.span,
            Some(d) => 50.0 + (v - d.own) * d.span / DEPTH_SPAN,
        }
    }

    /// The lights of preset `r` (written in the preset frame) round this face.
    pub fn place(&self, r: &Relight) -> Relight {
        let mut out = r.clone();
        let (kx, ky) = (self.w / CANON.0, self.h / CANON.1);
        for l in &mut out.lights {
            if matches!(l.kind.index(), 0 | 1 | 3) {
                l.depth = self.map_depth(l.depth, true);
            }
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
            if matches!(l.kind.index(), 0 | 1 | 3) {
                l.depth = self.map_depth(l.depth, false);
            }
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

/// A light `(dx, dy)` of the photo's width and height from the face's centre (right and down are positive), `ahead` in log
/// depth nearer the camera than the face (negative: behind it).
fn light(amount: f64, (dx, dy): (f64, f64), ahead: f64, softness: f64, warmth: f64) -> LightSettings {
    let depth = 50.0 - 100.0 * ahead / DEPTH_SPAN;
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

/// A built-in preset as written: not clamped here, so a test can tell a number out of its control's range from one that is
/// not ([`built_in`] clamps them when it makes the list).
fn preset(id: &str, name: &str, darken: f64, lights: Vec<LightSettings>) -> LightPreset {
    LightPreset { id: id.into(), name: name.into(), lights: Relight { lights, darken }, builtin: true }
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

/// A stage fixture (issue #334): a cone `angle` degrees wide with edge softness `edge` (0..100), aimed at `aim` (`(dx, dy)`
/// from the face, in the preset frame) from where the light stands. A soft edge: a real fixture's pool on a person is soft and
/// wraps the form; the hard edge is for a pool on a flat floor or wall, which the Sharp Cone control gives.
fn stage_spot(l: LightSettings, (dx, dy): (f64, f64), angle: f64, edge: f64) -> LightSettings {
    with_kind(l, |k| {
        k.which = 1.0;
        k.spot_aim_x = 0.5 + dx;
        k.spot_aim_y = 0.5 + dy;
        k.spot_angle = angle;
        k.spot_edge = edge;
    })
}

/// The haze the light's beam shows in: its amount, Forward Scatter and Dust, and how much of the fixture itself shows as a
/// bright point.
fn hazy(mut l: LightSettings, haze: f64, scatter: f64, dust: f64, source: f64) -> LightSettings {
    (l.effects.haze, l.effects.scatter, l.effects.dust, l.effects.source) = (haze, scatter, dust, source);
    l
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
        written()
            .into_iter()
            .map(|mut p| {
                p.lights.sanitize();
                p
            })
            .collect()
    })
}

/// The built-in presets as written (see [`built_in`]).
fn written() -> Vec<LightPreset> {
    vec![
        // A key about 45° to one side and 45° above the face, close to it: the near cheek bright, the far cheek in
        // shadow with the nose's shadow reaching towards it; a weak fill, so the key is several times the shadow side.
        preset(
            "rembrandt",
            "Rembrandt",
            75.0,
            vec![shadows(wrap(spot(light(100.0, (-0.34, -0.34), 0.15, 40.0, 6.0), 30.0, 60.0), 15.0), 100.0), fill(4.0, 0.0, 45.0)],
        ),
        // The key straight in front and high: a small shadow under the nose and chin, a soft fill from below.
        preset(
            "butterfly",
            "Butterfly",
            65.0,
            vec![shadows(wrap(spot(light(100.0, (0.0, -0.42), 0.15, 40.0, 0.0), 30.0, 60.0), 15.0), 100.0), fill(15.0, 0.0, 55.0)],
        ),
        // One light exactly from the side, level with the face: half of it lit, half in shadow.
        preset("split", "Split", 70.0, vec![shadows(wrap(spot(light(100.0, (-0.5, 0.0), 0.1, 40.0, 0.0), 30.0, 50.0), 0.0), 100.0)]),
        // The subject's whole outline lit from behind, the face evenly and softly lit from the front by a broad light (a
        // Fill alone lifts only what is dark, and left a dark stripe between the rim and the lit part of the face).
        preset(
            "rim-fill",
            "Rim + Fill",
            30.0,
            vec![rim(85.0, 0.0, (45.0, 0.0, 0.0)), wrap(light(40.0, (0.0, -0.05), 0.3, 100.0, 4.0), 100.0), fill(20.0, 4.0, 60.0)],
        ),
        // A low warm light raking across the face and body from the left, the shadow side and the wall away from it
        // cooler and darker.
        preset(
            "golden-hour-window",
            "Golden-Hour Window",
            60.0,
            vec![
                gel(shadows(wrap(spot(light(100.0, (-0.6, 0.0), 0.1, 40.0, 100.0), 44.0, 80.0), 20.0), 100.0), 38.0, 45.0),
                wrap(light(10.0, (0.5, 0.1), 0.1, 80.0, -45.0), 40.0),
            ],
        ),
        // A magenta light from the left and a teal one from the right, the room dark so the colours show.
        preset(
            "neon-two-tone",
            "Neon Two-Tone",
            75.0,
            vec![
                gel(shadows(spot(light(50.0, (-0.5, 0.05), 0.2, 40.0, 0.0), 32.0, 70.0), 60.0), 305.0, 85.0),
                gel(shadows(spot(light(50.0, (0.5, 0.05), 0.2, 40.0, 0.0), 32.0, 70.0), 60.0), 178.0, 85.0),
            ],
        ),
        // One small, warm flame held in front of the person at chin height, close: it lights the face from below (chin, under
        // the nose, cheeks) and falls off fast down the body, the rest of the room dark. Not too small a source, or a hot spot
        // forms on the nearest surface.
        preset(
            "candle",
            "Candle",
            70.0,
            vec![wrap(
                with_effects(light(60.0, (0.04, 0.17), 0.07, 28.0, 100.0), |e| {
                    e.gel_hue = 38.0;
                    e.gel_strength = 30.0;
                    e.haze = 10.0;
                    e.shadows = 25.0;
                }),
                75.0,
            )],
        ),
        // Stage lights hung behind and above the performer, their narrow beams streaming down and towards the camera through
        // haze, the subject dark against them and caught only along the outline; the fixtures themselves show as bright
        // points when they hang inside the frame. Wrap 0: lights behind a person don't light the front of them. The beams
        // aim beside the person, onto the floor or wall, and are thin and faint enough that a dark photo isn't flooded.
        preset(
            "concert-backlight",
            "Concert Backlight",
            85.0,
            vec![
                hazy(
                    wrap(gel(stage_spot(light(35.0, (-0.45, -0.42), -0.55, 45.0, 0.0), (0.2, 0.85), 6.0, 40.0), 36.0, 60.0), 0.0),
                    45.0,
                    35.0,
                    25.0,
                    70.0,
                ),
                hazy(
                    wrap(gel(stage_spot(light(35.0, (0.45, -0.42), -0.55, 45.0, 0.0), (-0.2, 0.85), 6.0, 40.0), 36.0, 60.0), 0.0),
                    45.0,
                    35.0,
                    25.0,
                    70.0,
                ),
                hazy(
                    wrap(gel(stage_spot(light(40.0, (0.0, -0.5), -0.7, 45.0, 0.0), (0.0, 0.9), 5.0, 40.0), 215.0, 35.0), 0.0),
                    45.0,
                    35.0,
                    25.0,
                    70.0,
                ),
                gel(rim(45.0, 0.0, (30.0, 0.0, 0.0)), 36.0, 50.0),
            ],
        ),
        // Two wide washes from the upper left and the upper right, a little behind the performer, red-orange on one side and blue
        // (a little stronger: blue lights a dark shirt poorly) on the other, both aimed at the person so they meet on them, each colouring its own side; thin haze shows the cones,
        // and the fixtures show as bright points when they hang inside the frame. Soft cones, high Wrap and no cast shadows:
        // the light follows the face's form.
        preset(
            "two-colour-stage",
            "Two-Colour Stage",
            80.0,
            vec![
                hazy(
                    wrap(gel(stage_spot(light(22.0, (-0.5, -0.4), -0.2, 45.0, 0.0), (0.0, 0.25), 15.0, 75.0), 8.0, 85.0), 65.0),
                    50.0,
                    35.0,
                    20.0,
                    60.0,
                ),
                hazy(
                    wrap(gel(stage_spot(light(32.0, (0.5, -0.4), -0.2, 45.0, 0.0), (0.0, 0.25), 15.0, 75.0), 228.0, 85.0), 65.0),
                    50.0,
                    35.0,
                    20.0,
                    60.0,
                ),
            ],
        ),
        // One spotlight from above and to one side, a soft pool that wraps the performer's face and shoulders and the rest of the
        // stage dark, the haze showing the cone across the picture.
        preset(
            "single-spotlight",
            "Single Spotlight",
            92.0,
            vec![hazy(
                wrap(gel(stage_spot(light(55.0, (-0.32, -0.5), 0.0, 30.0, 25.0), (0.0, 0.1), 13.0, 70.0), 40.0, 15.0), 60.0),
                70.0,
                70.0,
                15.0,
                40.0,
            )],
        ),
    ]
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
        let centre = rembrandt.placed(&Subject { x: 0.5, y: 0.5, w: 0.25, h: 0.25, depth: None });
        let moved = rembrandt.placed(&Subject { x: 0.3, y: 0.6, w: 0.25, h: 0.25, depth: None });
        let (a, b) = (&centre.lights[0], &moved.lights[0]);
        assert!((b.x - a.x + 0.2).abs() < 1e-9 && (b.y - a.y - 0.1).abs() < 1e-9, "{a:?} {b:?}");
        // a bigger subject takes its lights further out, in the same direction
        let big = rembrandt.placed(&Subject { x: 0.5, y: 0.5, w: 0.5, h: 0.5, depth: None });
        assert!((big.lights[0].x - 0.5).abs() > (a.x - 0.5).abs() && big.lights[0].x < 0.5);
        // what has no place on the photo stays as it was
        assert_eq!(centre.lights[1], big.lights[1]);
        // capturing what was placed gives the preset back
        let s = Subject { x: 0.34, y: 0.58, w: 0.3, h: 0.4, depth: None };
        let back = s.capture(&rembrandt.placed(&s));
        for (p, q) in rembrandt.lights.lights.iter().zip(&back.lights) {
            assert!((p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9 && p.amount == q.amount, "{p:?} {q:?}");
            assert!((p.kind.box_width - q.kind.box_width).abs() < 1e-9 && (p.kind.spot_aim_y - q.kind.spot_aim_y).abs() < 1e-9);
        }
    }

    #[test]
    fn a_lights_depth_is_placed_in_front_of_the_subjects_own() {
        let r = &built_in()[0];
        let face = Subject { x: 0.5, y: 0.4, w: 0.15, h: 0.25, depth: None };
        // the same preset in a shallow scene (the face at depth 20 of a span of 1) and in a deep one (60, span 3): the key is the same
        // distance (0.15 of log depth) in front of the face in both
        for (own, span) in [(20.0, 1.0), (60.0, 3.0)] {
            let key = face.with_depth(own, span).place(&r.lights).lights[0].clone();
            assert!(((own - key.depth) * span / 100.0 - 0.15).abs() < 1e-9, "{own} {span}: {}", key.depth);
        }
        // a light with no depth (a fill) keeps what it has, and a photo without depth yet places the depth as written
        let placed = face.with_depth(20.0, 1.0).place(&r.lights);
        assert_eq!(placed.lights[1].depth, r.lights.lights[1].depth);
        assert_eq!(face.place(&r.lights).lights[0].depth, r.lights.lights[0].depth);
        // capturing gives the preset's depth back
        let back = face.with_depth(60.0, 3.0).capture(&face.with_depth(60.0, 3.0).place(&r.lights));
        assert!((back.lights[0].depth - r.lights.lights[0].depth).abs() < 1e-9);
        // nonsense depth is no depth
        assert_eq!(face.with_depth(f64::NAN, 1.0).depth, None);
        assert_eq!(face.with_depth(20.0, 0.0).depth, None);
    }

    #[test]
    fn built_in_presets_are_whole_distinct_and_within_range() {
        let list = built_in();
        assert_eq!(list.len(), 10);
        for (i, p) in list.iter().enumerate() {
            assert!(p.builtin && !p.id.is_empty() && list[..i].iter().all(|q| q.id != p.id && q.name != p.name), "{}", p.id);
            assert!(p.lights.is_on() && !p.lights.lights.is_empty());
        }
        // (the numbers as written, before `built_in` clamps them: one outside its control's range would be silently changed)
        for p in written() {
            let mut r = p.lights.clone();
            r.sanitize();
            assert_eq!(r, p.lights, "{} holds a number out of its control's range", p.id);
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
