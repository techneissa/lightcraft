//! What a profile learns: the sliders of six groups, each turned into numbers that a regressor can average
//! (targets) and back into ordinary develop settings.
//!
//! A target is a slider's change from the photo's own defaults, so a raw's as-shot white balance and a JPEG's
//! relative one both start at 0. Temperature is learned in mireds (warmer = positive), where equal steps look
//! equal at any temperature. A colour grading wheel is learned as a point on the wheel (`x`, `y`), because its hue
//! means nothing when its saturation is 0 and wraps at 360. A point curve is learned as its height above the
//! diagonal at five places.

use lightcraft_color::spline::MonotoneCurve;
use lightcraft_develop::{DevelopSettings, ToneCurve, controls};
use lightcraft_geom::Point;
use serde::{Deserialize, Serialize};

/// The slider groups a profile can learn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Group {
    WhiteBalance,
    Tone,
    Presence,
    ColorMixer,
    ColorGrading,
    ToneCurve,
}

impl Group {
    pub const ALL: [Group; 6] = [Group::WhiteBalance, Group::Tone, Group::Presence, Group::ColorMixer, Group::ColorGrading, Group::ToneCurve];

    pub fn label(self) -> &'static str {
        match self {
            Group::WhiteBalance => "White Balance",
            Group::Tone => "Tone",
            Group::Presence => "Presence",
            Group::ColorMixer => "Color Mixer",
            Group::ColorGrading => "Color Grading",
            Group::ToneCurve => "Tone Curve",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Group::WhiteBalance => "whiteBalance",
            Group::Tone => "tone",
            Group::Presence => "presence",
            Group::ColorMixer => "colorMixer",
            Group::ColorGrading => "colorGrading",
            Group::ToneCurve => "toneCurve",
        }
    }

    pub fn from_id(id: &str) -> Option<Group> {
        Group::ALL.into_iter().find(|g| g.id() == id)
    }
}

/// How a target is read from and written to the settings.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// A develop control, as a change from the default.
    Control(&'static str),
    /// `wb.temp` in mireds, warmer positive.
    Mired,
    /// One axis of a colour grading wheel (`0` x, `1` y), saturation units.
    Wheel(Wheel, u8),
    /// A point curve's height above the diagonal at `x`, in 0..100 (percent of the range).
    Curve(Channel, u8),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Wheel {
    Shadows,
    Midtones,
    Highlights,
    Global,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Channel {
    Master,
    Red,
    Green,
    Blue,
}

/// One learned number.
#[derive(Clone, Copy, Debug)]
pub struct Slider {
    /// Stable id, written in profile files.
    pub id: &'static str,
    pub label: &'static str,
    pub group: Group,
    /// What one unit of error means to a person, for the accuracy report (`unit` per step of the slider).
    pub unit: &'static str,
    kind: Kind,
}

/// Where the point curves are sampled.
pub const CURVE_X: [f64; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

macro_rules! sliders {
    ($( $id:literal, $label:literal, $group:ident, $unit:literal, $kind:expr; )*) => {
        /// Every slider a profile can learn, in a fixed order.
        pub static SLIDERS: &[Slider] = &[ $( Slider { id: $id, label: $label, group: Group::$group, unit: $unit, kind: $kind }, )* ];
    };
}

use Channel::{Blue, Green, Master, Red};
use Kind::{Control, Curve, Mired};
use Wheel::{Global, Highlights, Midtones, Shadows};

sliders! {
    "wb.temp", "Temp", WhiteBalance, "mired", Mired;
    "wb.tint", "Tint", WhiteBalance, "", Control("wb.tint");
    "light.exposure", "Exposure", Tone, "EV", Control("light.exposure");
    "light.contrast", "Contrast", Tone, "", Control("light.contrast");
    "light.highlights", "Highlights", Tone, "", Control("light.highlights");
    "light.shadows", "Shadows", Tone, "", Control("light.shadows");
    "light.whites", "Whites", Tone, "", Control("light.whites");
    "light.blacks", "Blacks", Tone, "", Control("light.blacks");
    "effects.texture", "Texture", Presence, "", Control("effects.texture");
    "effects.clarity", "Clarity", Presence, "", Control("effects.clarity");
    "effects.dehaze", "Dehaze", Presence, "", Control("effects.dehaze");
    "color.vibrance", "Vibrance", Presence, "", Control("color.vibrance");
    "color.saturation", "Saturation", Presence, "", Control("color.saturation");
    "mixer.red.hue", "Red Hue", ColorMixer, "", Control("mixer.red.hue");
    "mixer.red.sat", "Red Saturation", ColorMixer, "", Control("mixer.red.sat");
    "mixer.red.lum", "Red Luminance", ColorMixer, "", Control("mixer.red.lum");
    "mixer.orange.hue", "Orange Hue", ColorMixer, "", Control("mixer.orange.hue");
    "mixer.orange.sat", "Orange Saturation", ColorMixer, "", Control("mixer.orange.sat");
    "mixer.orange.lum", "Orange Luminance", ColorMixer, "", Control("mixer.orange.lum");
    "mixer.yellow.hue", "Yellow Hue", ColorMixer, "", Control("mixer.yellow.hue");
    "mixer.yellow.sat", "Yellow Saturation", ColorMixer, "", Control("mixer.yellow.sat");
    "mixer.yellow.lum", "Yellow Luminance", ColorMixer, "", Control("mixer.yellow.lum");
    "mixer.green.hue", "Green Hue", ColorMixer, "", Control("mixer.green.hue");
    "mixer.green.sat", "Green Saturation", ColorMixer, "", Control("mixer.green.sat");
    "mixer.green.lum", "Green Luminance", ColorMixer, "", Control("mixer.green.lum");
    "mixer.aqua.hue", "Aqua Hue", ColorMixer, "", Control("mixer.aqua.hue");
    "mixer.aqua.sat", "Aqua Saturation", ColorMixer, "", Control("mixer.aqua.sat");
    "mixer.aqua.lum", "Aqua Luminance", ColorMixer, "", Control("mixer.aqua.lum");
    "mixer.blue.hue", "Blue Hue", ColorMixer, "", Control("mixer.blue.hue");
    "mixer.blue.sat", "Blue Saturation", ColorMixer, "", Control("mixer.blue.sat");
    "mixer.blue.lum", "Blue Luminance", ColorMixer, "", Control("mixer.blue.lum");
    "mixer.purple.hue", "Purple Hue", ColorMixer, "", Control("mixer.purple.hue");
    "mixer.purple.sat", "Purple Saturation", ColorMixer, "", Control("mixer.purple.sat");
    "mixer.purple.lum", "Purple Luminance", ColorMixer, "", Control("mixer.purple.lum");
    "mixer.magenta.hue", "Magenta Hue", ColorMixer, "", Control("mixer.magenta.hue");
    "mixer.magenta.sat", "Magenta Saturation", ColorMixer, "", Control("mixer.magenta.sat");
    "mixer.magenta.lum", "Magenta Luminance", ColorMixer, "", Control("mixer.magenta.lum");
    "grading.shadows.x", "Shadows Color", ColorGrading, "", Kind::Wheel(Shadows, 0);
    "grading.shadows.y", "Shadows Color", ColorGrading, "", Kind::Wheel(Shadows, 1);
    "grading.shadows.lum", "Shadows Luminance", ColorGrading, "", Control("grading.shadows.lum");
    "grading.midtones.x", "Midtones Color", ColorGrading, "", Kind::Wheel(Midtones, 0);
    "grading.midtones.y", "Midtones Color", ColorGrading, "", Kind::Wheel(Midtones, 1);
    "grading.midtones.lum", "Midtones Luminance", ColorGrading, "", Control("grading.midtones.lum");
    "grading.highlights.x", "Highlights Color", ColorGrading, "", Kind::Wheel(Highlights, 0);
    "grading.highlights.y", "Highlights Color", ColorGrading, "", Kind::Wheel(Highlights, 1);
    "grading.highlights.lum", "Highlights Luminance", ColorGrading, "", Control("grading.highlights.lum");
    "grading.global.x", "Global Color", ColorGrading, "", Kind::Wheel(Global, 0);
    "grading.global.y", "Global Color", ColorGrading, "", Kind::Wheel(Global, 1);
    "grading.global.lum", "Global Luminance", ColorGrading, "", Control("grading.global.lum");
    "grading.blending", "Blending", ColorGrading, "", Control("grading.blending");
    "grading.balance", "Balance", ColorGrading, "", Control("grading.balance");
    "curve.highlights", "Curve Highlights", ToneCurve, "", Control("curve.highlights");
    "curve.lights", "Curve Lights", ToneCurve, "", Control("curve.lights");
    "curve.darks", "Curve Darks", ToneCurve, "", Control("curve.darks");
    "curve.shadows", "Curve Shadows", ToneCurve, "", Control("curve.shadows");
    "curve.master.0", "Point Curve 0 %", ToneCurve, "%", Curve(Master, 0);
    "curve.master.25", "Point Curve 25 %", ToneCurve, "%", Curve(Master, 1);
    "curve.master.50", "Point Curve 50 %", ToneCurve, "%", Curve(Master, 2);
    "curve.master.75", "Point Curve 75 %", ToneCurve, "%", Curve(Master, 3);
    "curve.master.100", "Point Curve 100 %", ToneCurve, "%", Curve(Master, 4);
    "curve.red.0", "Red Curve 0 %", ToneCurve, "%", Curve(Red, 0);
    "curve.red.25", "Red Curve 25 %", ToneCurve, "%", Curve(Red, 1);
    "curve.red.50", "Red Curve 50 %", ToneCurve, "%", Curve(Red, 2);
    "curve.red.75", "Red Curve 75 %", ToneCurve, "%", Curve(Red, 3);
    "curve.red.100", "Red Curve 100 %", ToneCurve, "%", Curve(Red, 4);
    "curve.green.0", "Green Curve 0 %", ToneCurve, "%", Curve(Green, 0);
    "curve.green.25", "Green Curve 25 %", ToneCurve, "%", Curve(Green, 1);
    "curve.green.50", "Green Curve 50 %", ToneCurve, "%", Curve(Green, 2);
    "curve.green.75", "Green Curve 75 %", ToneCurve, "%", Curve(Green, 3);
    "curve.green.100", "Green Curve 100 %", ToneCurve, "%", Curve(Green, 4);
    "curve.blue.0", "Blue Curve 0 %", ToneCurve, "%", Curve(Blue, 0);
    "curve.blue.25", "Blue Curve 25 %", ToneCurve, "%", Curve(Blue, 1);
    "curve.blue.50", "Blue Curve 50 %", ToneCurve, "%", Curve(Blue, 2);
    "curve.blue.75", "Blue Curve 75 %", ToneCurve, "%", Curve(Blue, 3);
    "curve.blue.100", "Blue Curve 100 %", ToneCurve, "%", Curve(Blue, 4);
}

/// The slider with `id`.
pub fn find(id: &str) -> Option<&'static Slider> {
    SLIDERS.iter().find(|s| s.id == id)
}

fn wheel(s: &DevelopSettings, w: Wheel) -> &lightcraft_develop::Wheel {
    match w {
        Shadows => &s.grading.shadows,
        Midtones => &s.grading.midtones,
        Highlights => &s.grading.highlights,
        Global => &s.grading.global,
    }
}

fn wheel_mut(s: &mut DevelopSettings, w: Wheel) -> &mut lightcraft_develop::Wheel {
    match w {
        Shadows => &mut s.grading.shadows,
        Midtones => &mut s.grading.midtones,
        Highlights => &mut s.grading.highlights,
        Global => &mut s.grading.global,
    }
}

fn wheel_xy(w: &lightcraft_develop::Wheel) -> (f64, f64) {
    let (sat, hue) = (if w.sat.is_finite() { w.sat } else { 0.0 }, if w.hue.is_finite() { w.hue.to_radians() } else { 0.0 });
    (sat * hue.cos(), sat * hue.sin())
}

fn channel(c: &ToneCurve, ch: Channel) -> &[Point] {
    match ch {
        Master => &c.master,
        Red => &c.red,
        Green => &c.green,
        Blue => &c.blue,
    }
}

fn channel_mut(c: &mut ToneCurve, ch: Channel) -> &mut Vec<Point> {
    match ch {
        Master => &mut c.master,
        Red => &mut c.red,
        Green => &mut c.green,
        Blue => &mut c.blue,
    }
}

/// A point curve's height above the diagonal at `x`, in percent.
fn curve_offset(points: &[Point], x: f64) -> f64 {
    if ToneCurve::point_curve_is_identity(points) {
        return 0.0;
    }
    let pts: Vec<(f64, f64)> = points.iter().map(|p| (p.x, p.y)).collect();
    (MonotoneCurve::new(&pts).eval(x) - x) * 100.0
}

fn mired(k: f64) -> f64 {
    1e6 / k.clamp(1000.0, 100_000.0)
}

fn value(s: &DevelopSettings, defaults: &DevelopSettings, kind: Kind) -> f64 {
    let v = match kind {
        Control(id) => controls::get(s, id).unwrap_or(0.0) - controls::get(defaults, id).unwrap_or(0.0),
        Mired => mired(defaults.wb.temp) - mired(s.wb.temp),
        Kind::Wheel(w, axis) => {
            let (x, y) = wheel_xy(wheel(s, w));
            let (dx, dy) = wheel_xy(wheel(defaults, w));
            if axis == 0 { x - dx } else { y - dy }
        }
        Curve(ch, i) => {
            let x = CURVE_X.get(i as usize).copied().unwrap_or(0.0);
            curve_offset(channel(&s.curve, ch), x) - curve_offset(channel(&defaults.curve, ch), x)
        }
    };
    if v.is_finite() { v } else { 0.0 }
}

/// Every slider's target for a photo: its settings against its defaults, both with the white balance it really
/// renders with (the engine fills in an as-shot white balance first).
pub fn targets(settings: &DevelopSettings, defaults: &DevelopSettings) -> Vec<f32> {
    SLIDERS.iter().map(|sl| value(settings, defaults, sl.kind) as f32).collect()
}

/// Write predicted targets (`(slider index, target)`) into `out`, starting from the photo's `defaults`. Only the
/// named sliders change: a wheel or a curve changes only when one of its numbers is named.
pub fn write(out: &mut DevelopSettings, defaults: &DevelopSettings, predicted: &[(usize, f32)]) {
    let get = |i: usize| predicted.iter().find(|(j, _)| *j == i).map(|(_, v)| *v as f64);
    let mut wheels = Vec::new();
    let mut curves = Vec::new();
    for (i, v) in predicted {
        let Some(sl) = SLIDERS.get(*i) else { continue };
        let v = *v as f64;
        if !v.is_finite() {
            continue;
        }
        match sl.kind {
            Control(id) => {
                controls::set(out, id, controls::get(defaults, id).unwrap_or(0.0) + v);
            }
            Mired => {
                let m = mired(defaults.wb.temp) - v;
                controls::set(out, "wb.temp", 1e6 / m.max(1e6 / 50_000.0));
            }
            Kind::Wheel(w, _) if !wheels.contains(&w) => wheels.push(w),
            Curve(ch, _) if !curves.contains(&ch) => curves.push(ch),
            _ => {}
        }
    }
    let index = |kind: Kind| SLIDERS.iter().position(|s| s.kind == kind);
    for w in wheels {
        let (dx, dy) = wheel_xy(wheel(defaults, w));
        let x = dx + index(Kind::Wheel(w, 0)).and_then(get).unwrap_or(0.0);
        let y = dy + index(Kind::Wheel(w, 1)).and_then(get).unwrap_or(0.0);
        let sat = x.hypot(y).clamp(0.0, 100.0);
        let target = wheel_mut(out, w);
        if sat < 0.5 {
            target.sat = 0.0;
        } else {
            target.sat = sat.round();
            target.hue = y.atan2(x).to_degrees().rem_euclid(360.0).round() % 360.0;
        }
    }
    for ch in curves {
        let mut pts = Vec::with_capacity(CURVE_X.len());
        let mut identity = true;
        for (i, x) in CURVE_X.iter().enumerate() {
            let base = curve_offset(channel(&defaults.curve, ch), *x);
            let t = index(Curve(ch, i as u8)).and_then(get).unwrap_or(0.0);
            let y = (x + (base + t) / 100.0).clamp(0.0, 1.0);
            identity &= (y - x).abs() < 0.002;
            pts.push(Point::new(*x, y));
        }
        *channel_mut(&mut out.curve, ch) = if identity { Vec::new() } else { pts };
    }
}
