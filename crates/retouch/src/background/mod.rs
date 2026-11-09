//! Background: Blur, Swap and Clean Backdrop, on the Entire Person / Background masks. Runs after every retouch, once
//! for the whole picture, with the settings for every face.
//!
//! - **Blur** (0…100): a lens-like blur of the background whose radius grows with how far behind the person each
//!   pixel is ([`falloff`]: the photo's depth map when it has one, else the distance from the person). At 100 the
//!   farthest background is blurred by a disc of [`BLUR_MAX`] of the photo's long side. The person is cut out
//!   before the blur (each blur is of the background alone, weighted by how much of each pixel is background), so
//!   the person's colour never spreads into the background: no halo round the hair.
//! - **Swap** (0…100): what is behind the person becomes a solid colour or another picture ([`Swap`]), fitted to
//!   cover the frame (and blurred like the background when Blur is on); Swap is how much of it shows.
//! - **Clean Backdrop** (0…100): on a studio backdrop, creases, scuffs and banding are smoothed away (the background
//!   blurred by [`CLEAN`] of the long side, faded out where it differs from the blur by more than a crease does, so a
//!   light stand or a chair stays). The paper's fine grain is kept ([`prepare::GRAIN`]), and a light dither stops
//!   the smoothed paper from banding in an 8-bit export.
//!
//! The person is drawn over the new background with their edge's own colours ([`matte`]: blur-fusion foreground
//! estimation, as a mask's Decontaminate), so no old background stays between strands. Pixels wholly person never
//! change. Sizes are fractions of the photo's long side or of the person's box, never pixels.
//!
//! Everything a slider does not change is computed once and kept ([`prepare`]): the matte, its edge's colours, the
//! blur's shares and the blurred and cleaned background. A slider drag then only mixes them per pixel ([`draw`]).
//! A window of the render (the zoomed loupe) must hold [`reach`] around itself; the blurs run on blocks anchored to
//! the whole render ([`Place`]) and the distances on a grid over the whole photo, so it draws what the whole render
//! draws inside it.

use std::sync::Arc;

use lightcraft_raster::{Plane, Rgb32f};
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Part, Scene, Scope, Spec, Stage};

mod draw;
pub mod falloff;
mod grid;
mod matte;
mod prepare;

pub use prepare::{GRAIN, forget, kept};

/// The blur radius at Blur 100 on the farthest background, a fraction of the photo's long side.
pub const BLUR_MAX: f64 = 0.03;
/// Clean Backdrop's smoothing (a Gaussian's sigma), a fraction of the photo's long side.
pub const CLEAN: f64 = 0.025;

/// The Background settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Background {
    /// 0…100; 0 is off.
    pub blur: f64,
    /// 0…100: how much of [`Background::with`] replaces what is behind the person.
    pub swap: f64,
    /// 0…100: Clean Backdrop.
    pub clean: f64,
    /// What Swap puts behind the person.
    pub with: Swap,
}

/// What Background Swap puts behind the person.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Swap {
    /// A solid colour: sRGB, 0…1 per channel (as a colour picker shows it), turned linear for the render.
    Colour { rgb: [f64; 3] },
    /// A picture file (any format the app reads), by its path.
    File { path: String },
    /// A photo of the library, as it is developed, by its id.
    Photo { id: u64 },
}

/// A light grey, a studio backdrop's colour.
pub const DEFAULT_COLOUR: [f64; 3] = [0.85, 0.85, 0.85];

impl Default for Swap {
    fn default() -> Swap {
        Swap::Colour { rgb: DEFAULT_COLOUR }
    }
}

impl Swap {
    /// The picture this swap needs loaded, when it is one: `file:<path>` or `photo:<id>` (the engine's key).
    pub fn picture(&self) -> Option<String> {
        match self {
            Swap::Colour { .. } => None,
            Swap::File { path } => Some(format!("file:{path}")),
            Swap::Photo { id } => Some(format!("photo:{id}")),
        }
    }

    /// `#RRGGBB` for a colour.
    pub fn hex(&self) -> Option<String> {
        let Swap::Colour { rgb } = self else { return None };
        let b = |v: f64| {
            let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
            (v * 255.0).round() as u8
        };
        Some(format!("#{:02X}{:02X}{:02X}", b(rgb[0]), b(rgb[1]), b(rgb[2])))
    }

    /// A colour from `#RRGGBB` (or `RRGGBB`); `None` for anything else.
    pub fn from_hex(s: &str) -> Option<Swap> {
        let h = s.trim().trim_start_matches('#');
        if h.len() != 6 || !h.is_ascii() {
            return None;
        }
        let c = |i: usize| h.get(i..i + 2).and_then(|v| u8::from_str_radix(v, 16).ok()).map(|v| f64::from(v) / 255.0);
        Some(Swap::Colour { rgb: [c(0)?, c(2)?, c(4)?] })
    }
}

/// Where the image being retouched lies in the whole render (a zoomed loupe renders a window of it): its top-left
/// pixel and the render's size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Place {
    pub x: i64,
    pub y: i64,
    pub width: usize,
    pub height: usize,
}

/// The picture Swap uses, loaded by the engine: scene-linear, in the render's working space.
#[derive(Clone)]
pub struct Backdrop {
    pub image: Arc<Rgb32f>,
    /// Which picture it is (its path or photo and its pixels), for cache keys.
    pub key: u64,
}

impl std::fmt::Debug for Backdrop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Backdrop({}×{}, {:016x})", self.image.width, self.image.height, self.key)
    }
}

/// A photo's depth map: 1 nearest, 0 farthest (an iPhone's disparity, normalised), over the photo in the faces'
/// normalised frame.
#[derive(Clone)]
pub struct Depth {
    pub plane: Arc<Plane>,
    /// For cache keys.
    pub key: u64,
}

impl std::fmt::Debug for Depth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Depth({}×{}, {:016x})", self.plane.width, self.plane.height, self.key)
    }
}

/// How far (pixels) Background reads around a pixel in a render whose photo has a long side of `long_px` pixels,
/// with every slider on: a window of the render must hold this much around itself ([`Background::reach`] for given
/// settings).
pub fn reach(long_px: f64) -> usize {
    Background { blur: 1.0, swap: 1.0, clean: 1.0, with: Swap::default() }.reach(long_px)
}

impl Background {
    /// How far (pixels) these settings read around a pixel in a render whose photo has a long side of `long_px`
    /// pixels: the edge's colours read their windows, and the blur and the clean read the background (and those
    /// colours) around them, a block more each. 0 while off.
    pub fn reach(&self, long_px: f64) -> usize {
        let k = draw::Strength::of(self);
        if k.is_off() {
            return 0;
        }
        let blur = if k.blur > 0.0 { BLUR_MAX * (1.0 + 2.0 / 3.0) } else { 0.0 };
        let clean = if k.clean > 0.0 { CLEAN * 4.0 } else { 0.0 };
        let r = (blur.max(clean) + matte::WIDE + matte::NARROW) * long_px;
        if r.is_finite() { (r.ceil().max(0.0) as usize).saturating_add(4) } else { 0 }
    }
}

impl Feature for Background {
    const KEY: &'static str = "background";
    const LABEL: &'static str = "Background";
    const STAGE: Stage = Stage::Background;
    const SCOPE: Scope = Scope::Picture;
    const CONTROLS: &'static [Control<Self>] = &[
        Control { spec: Spec::amount("retouch.background.blur", "Blur"), get: |s| s.blur, set: |s, v| s.blur = v },
        Control { spec: Spec::amount("retouch.background.swap", "Swap"), get: |s| s.swap, set: |s, v| s.swap = v },
        Control { spec: Spec::amount("retouch.background.clean", "Clean Backdrop"), get: |s| s.clean, set: |s, v| s.clean = v },
    ];
    const PARTS: &'static [Part] = &[Part::EntirePerson];

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, _face: Option<usize>) {
        let k = draw::Strength::of(self);
        if k.is_off() || img.width == 0 || img.height == 0 || img.data.len() != img.width * img.height {
            return;
        }
        let Some(prep) = prepare::prepared(img, scene) else { return };
        draw::draw(img, scene, &prep, &k, &self.with);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_reads_and_writes_as_hex() {
        assert_eq!(Swap::from_hex("#FF8000").and_then(|s| s.hex()).as_deref(), Some("#FF8000"));
        assert_eq!(Swap::from_hex("00ff00"), Some(Swap::Colour { rgb: [0.0, 1.0, 0.0] }));
        for bad in ["", "#12345", "#GG0000", "#ééé", "#1234567"] {
            assert_eq!(Swap::from_hex(bad), None, "{bad}");
        }
        assert_eq!(Swap::Colour { rgb: [f64::NAN, 2.0, -1.0] }.hex().as_deref(), Some("#00FF00"));
        assert_eq!(Swap::File { path: "/a b.jpg".into() }.picture().as_deref(), Some("file:/a b.jpg"));
    }

    #[test]
    fn settings_saved_without_the_swap_read_its_default_and_round_trip() {
        let b: Background = serde_json::from_str(r#"{"blur": 40}"#).unwrap_or_default();
        assert_eq!(b, Background { blur: 40.0, ..Background::default() });
        let b = Background { swap: 60.0, with: Swap::Photo { id: 7 }, ..Background::default() };
        let back: Background = serde_json::from_value(serde_json::to_value(&b).unwrap_or_default()).unwrap_or_default();
        assert_eq!(back, b);
    }

    #[test]
    fn the_reach_grows_with_the_render() {
        assert!(reach(2000.0) > 250 && reach(2000.0) < 330, "{}", reach(2000.0));
        assert_eq!(reach(f64::NAN), 0);
        assert!(reach(4000.0) > reach(2000.0));
        let blur = Background { blur: 50.0, ..Background::default() };
        assert!(blur.reach(2000.0) < reach(2000.0) && blur.reach(2000.0) > 150);
        assert_eq!(Background::default().reach(2000.0), 0);
    }
}
