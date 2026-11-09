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
//!   light stand or a chair stays, and by colour, so a coloured object does too). A light dither of about one 8-bit
//!   step stops the smoothed paper from banding in an export and gives grainy paper a grain of its own strength.
//!
//! The person is drawn over the new background with their edge's own colours ([`matte`]: blur-fusion foreground
//! estimation, as a mask's Decontaminate), so no old background stays between strands. Pixels wholly person never
//! change. Sizes are fractions of the photo's long side or of the person's box, never pixels.
//!
//! Everything a slider does not change is made once from the [`Analysis`] image (the whole frame before this step, at
//! a fixed size, from the pipeline) and kept with it ([`prepare`]): the matte, its edge's colours, the blur's shares,
//! the blurred and cleaned background and the fitted Swap picture. A slider drag, a loupe pan or another render size
//! then only mixes them per pixel ([`draw`]), so a window of the render (the zoomed loupe) draws what the whole render
//! draws inside it and reads nothing around itself.

use std::sync::Arc;

use lightcraft_raster::{Plane, Rgb32f};
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Part, Scene, Scope, Spec, Stage};

mod draw;
pub mod falloff;
mod grid;
pub mod holes;
mod matte;
mod prepare;

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

/// What Background Swap puts behind the person. Never a file path: a picture the user picks is copied into the
/// library's own folder and named by its content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Swap {
    /// A solid colour: sRGB, 0…1 per channel (as a colour picker shows it), turned linear for the render.
    Colour { rgb: [f64; 3] },
    /// A picture copied into the library (`<library>/Backgrounds/<hash>.<ext>`), by the SHA-256 of its bytes (hex).
    Picture { hash: String },
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
    /// The picture this swap needs loaded, when it is one: `picture:<hash>` or `photo:<id>` (the engine's key).
    pub fn picture(&self) -> Option<String> {
        match self {
            Swap::Colour { .. } => None,
            Swap::Picture { hash } => Some(format!("picture:{hash}")),
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

/// The long side (pixels) of the [`Analysis`] image the pipeline makes.
pub const ANALYSIS_LONG: usize = 1600;

/// The whole frame of a render before the Background step, at a fixed size ([`ANALYSIS_LONG`] on its long side for
/// the pipeline's), scene-linear: what Background's blurs, matte colours and cleaned backdrop are made from, and where
/// that work is kept (for the newest few faces, depth maps and Swap pictures).
pub struct Analysis {
    pub image: Arc<Rgb32f>,
    /// What the image is made from (the source, the settings before the step, the frame and the size).
    pub key: u64,
    kept: prepare::Kept,
}

impl Analysis {
    pub fn new(image: Arc<Rgb32f>, key: u64) -> Analysis {
        Analysis { image, key, kept: prepare::Kept::default() }
    }

    /// How many sets of work are kept with it.
    pub fn kept(&self) -> usize {
        self.kept.len()
    }

    /// Memory held (bytes): the image and the work kept with it.
    pub fn bytes(&self) -> usize {
        self.image.data.len() * std::mem::size_of::<[f32; 3]>() + self.kept.bytes()
    }
}

impl std::fmt::Debug for Analysis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Analysis({}×{}, {:016x}, {} kept)", self.image.width, self.image.height, self.key, self.kept())
    }
}

/// `LIGHTCRAFT_PROFILE=1` prints how long Background took (as the pipeline's stages do).
fn profiling() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("LIGHTCRAFT_PROFILE").is_some_and(|v| v != "0"))
}

impl Background {
    /// These settings as a portrait preset keeps them: Blur, Clean Backdrop and a Swap colour, never a Swap picture
    /// (a picture of one library, or a photo of it, means nothing elsewhere): a picture's Swap is left off.
    pub fn for_preset(&self) -> Background {
        match self.with {
            Swap::Colour { .. } => self.clone(),
            _ => Background { swap: 0.0, with: Swap::default(), ..self.clone() },
        }
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
    /// The person, and their hair (small gaps in it are the person: [`holes`]).
    const PARTS: &'static [Part] = &[Part::EntirePerson, Part::Hair];
    /// Nothing around a face: Background reads the [`Analysis`] image of the whole frame, so a zoomed window needs no
    /// more pixels around itself.
    const REACH: f64 = 0.0;
    const READS_AROUND: bool = false;

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, _face: Option<usize>) {
        let k = draw::Strength::of(self);
        if k.is_off() || img.width == 0 || img.height == 0 || img.data.len() != img.width * img.height {
            return;
        }
        let t = profiling().then(std::time::Instant::now);
        // the pipeline's analysis image; without one (the examples, the crate's tests) the image itself, when it is
        // the whole render
        let own;
        let analysis = match scene.view().analysis() {
            Some(a) => a,
            None => {
                let whole = scene.view().place().is_none_or(|p| p.x == 0 && p.y == 0 && (p.width, p.height) == (img.width, img.height));
                if !whole {
                    return;
                }
                own = Analysis::new(Arc::new(img.clone()), 0);
                &own
            }
        };
        let Some((prep, kept)) = prepare::prepared(analysis, scene, img.width, img.height) else { return };
        let frame = prepare::frame_of(analysis, scene, img.width, img.height);
        let t1 = t.map(|t| t.elapsed());
        draw::draw(img, scene, &prep, &frame, &k, &self.with);
        if let (Some(t), Some(t1)) = (t, t1) {
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
            let what = if kept { "kept" } else { "made" };
            eprintln!("    retouch background {}×{}: {what} {:.1} ms, drawn {:.1} ms", img.width, img.height, ms(t1), ms(t.elapsed() - t1));
        }
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
        assert_eq!(Swap::Picture { hash: "ab12".into() }.picture().as_deref(), Some("picture:ab12"));
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
    fn a_preset_keeps_a_colour_and_never_a_picture() {
        let colour = Background { blur: 30.0, swap: 80.0, clean: 10.0, with: Swap::from_hex("#112233").unwrap_or_default() };
        assert_eq!(colour.for_preset(), colour);
        let picture = Background { blur: 30.0, swap: 80.0, clean: 10.0, with: Swap::Picture { hash: "ab".into() } };
        assert_eq!(picture.for_preset(), Background { blur: 30.0, clean: 10.0, ..Background::default() });
        let photo = Background { swap: 50.0, with: Swap::Photo { id: 3 }, ..Background::default() };
        assert!(photo.for_preset().with.picture().is_none());
    }
}
