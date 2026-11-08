//! The sliders the fitter moves, each as one number it can step: a develop control, white balance temperature on a
//! mired scale (even steps look even from 2000 K to 50000 K), or one axis of a colour grading wheel in cartesian form
//! (hue is meaningless while the saturation is 0, so the wheel moves on its x / y instead).

use lightcraft_develop::{DevelopSettings, Wheel, controls};

/// The white balance temperature range the fitter keeps to (the develop panel's).
const TEMP_RANGE: (f64, f64) = (2000.0, 50000.0);
/// The tint range.
const TINT_RANGE: (f64, f64) = (-150.0, 150.0);

/// A colour grading wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Shadows,
    Midtones,
    Highlights,
}

impl Zone {
    fn wheel(self, d: &DevelopSettings) -> Wheel {
        match self {
            Zone::Shadows => d.grading.shadows,
            Zone::Midtones => d.grading.midtones,
            Zone::Highlights => d.grading.highlights,
        }
    }
    fn wheel_mut(self, d: &mut DevelopSettings) -> &mut Wheel {
        match self {
            Zone::Shadows => &mut d.grading.shadows,
            Zone::Midtones => &mut d.grading.midtones,
            Zone::Highlights => &mut d.grading.highlights,
        }
    }
    /// The wheel's control id prefix (`grading.shadows`).
    pub fn id(self) -> &'static str {
        match self {
            Zone::Shadows => "grading.shadows",
            Zone::Midtones => "grading.midtones",
            Zone::Highlights => "grading.highlights",
        }
    }
}

/// One number the fitter steps.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    /// A develop control by id (`light.exposure`, `mixer.blue.sat`…), within its own range.
    Control(&'static str),
    /// White balance temperature in mired (1e6 / Kelvin).
    Mired,
    /// White balance tint.
    Tint,
    /// A grading wheel's x (`false`) or y (`true`): saturation × cos / sin of its hue.
    Wheel(Zone, bool),
}

impl Param {
    pub fn get(&self, d: &DevelopSettings) -> f64 {
        match self {
            Param::Control(id) => controls::get(d, id).unwrap_or(0.0),
            Param::Mired => 1e6 / d.wb.temp.clamp(TEMP_RANGE.0, TEMP_RANGE.1),
            Param::Tint => d.wb.tint,
            Param::Wheel(z, y) => {
                let w = z.wheel(d);
                let h = w.hue.to_radians();
                if *y { w.sat * h.sin() } else { w.sat * h.cos() }
            }
        }
    }

    /// Set the number, clamped into its range; returns the value it took.
    pub fn set(&self, d: &mut DevelopSettings, v: f64) -> f64 {
        if !v.is_finite() {
            return self.get(d);
        }
        match self {
            Param::Control(id) => {
                controls::set(d, id, v);
            }
            Param::Mired => d.wb.temp = (1e6 / v.clamp(1e6 / TEMP_RANGE.1, 1e6 / TEMP_RANGE.0)).clamp(TEMP_RANGE.0, TEMP_RANGE.1),
            Param::Tint => d.wb.tint = v.clamp(TINT_RANGE.0, TINT_RANGE.1),
            Param::Wheel(z, y) => {
                let w = z.wheel(d);
                let h = w.hue.to_radians();
                let (mut x, mut yy) = (w.sat * h.cos(), w.sat * h.sin());
                if *y {
                    yy = v;
                } else {
                    x = v;
                }
                let sat = x.hypot(yy).min(100.0);
                let out = z.wheel_mut(d);
                if sat > 1e-6 {
                    out.hue = yy.atan2(x).to_degrees().rem_euclid(360.0);
                }
                out.sat = sat;
            }
        }
        self.get(d)
    }

    /// A first step size: about a just-visible change.
    pub fn step(&self) -> f64 {
        match self {
            Param::Control(id) if *id == "light.exposure" => 0.25,
            Param::Control(id) if id.starts_with("mixer.") && id.ends_with(".hue") => 8.0,
            Param::Control(_) => 10.0,
            Param::Mired => 12.0,
            Param::Tint => 6.0,
            Param::Wheel(..) => 8.0,
        }
    }

    /// How much moving this number away from where the photo had it costs in the fit (squared, per its scale): a
    /// small prior that keeps the fit from big moves that buy little (an unused mixer band at 100) and from moves
    /// within the noise of the measurement. Broad sliders cost little, narrow ones more.
    pub fn prior(&self, from: f64, to: f64) -> f64 {
        let (scale, weight) = match self {
            Param::Control(id) if *id == "light.exposure" => (2.0, 2e-4),
            Param::Control(id) if id.starts_with("mixer.") => (100.0, 2e-3),
            Param::Control(id) if id.starts_with("light.") || id.starts_with("color.") => (100.0, 1e-3),
            Param::Control(_) => (100.0, 2e-4),
            Param::Mired => (50.0, 2e-4),
            Param::Tint => (50.0, 2e-4),
            Param::Wheel(..) => (50.0, 2e-3),
        };
        let d = (to - from) / scale;
        weight * d * d
    }

    /// The smallest step worth trying (below it the fitter stops refining this number).
    pub fn min_step(&self) -> f64 {
        self.step() / 16.0
    }
}
