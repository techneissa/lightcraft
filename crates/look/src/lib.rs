//! Match Look (fork feature, `fork/match-look.md`): give a photo the colour and tone of a reference picture, as
//! ordinary develop slider values that stay editable.
//!
//! [`match_look`] measures the reference in OkLab ([`stats`]: lightness spread, the cast of shadows, midtones and
//! highlights, colourfulness, each colour mixer band), then fits the photo's sliders by rendering its small copy
//! through the real develop pipeline and keeping each slider move that brings its look closer ([`fit`]): white
//! balance, Exposure, Contrast, Highlights, Shadows, Whites, Blacks, Vibrance, Saturation, the colour mixer, the
//! shadow and highlight grading wheels and a master tone curve.
//!
//! The fitting takes a list of region pairs (a mask on the photo, a mask on the reference), so skin can later be
//! matched to skin and sky to sky; for now the caller passes [`RegionPair::whole`].
//!
//! [`reference`] reads a reference from a file, pasted bytes or an https address (size-capped, cached, never
//! imported). Layer L3: no UI dependencies.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod fit;
pub mod params;
pub mod reference;
mod residual;
pub mod stats;

use std::sync::atomic::AtomicBool;

use lightcraft_develop::{DevelopSettings, ResidualLook, WbMode, controls};
use lightcraft_geom::Point;
use lightcraft_pipeline::SourceInfo;
use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Plane, Rgb32f, Rgba8};

/// Why a match could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookError {
    /// The request itself is unusable (an empty picture, a bad option).
    Invalid(String),
    Cancelled,
    /// The reference is not an image this build reads, or it is damaged.
    Decode(String),
    /// The web address can't be used or the download failed.
    Download(String),
    /// A file or download past the size cap.
    TooLarge(String),
    Io(String),
}

impl std::fmt::Display for LookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LookError::Invalid(e) => write!(f, "{e}"),
            LookError::Cancelled => write!(f, "cancelled"),
            LookError::Decode(e) => write!(f, "the reference could not be read: {e}"),
            LookError::Download(e) => write!(f, "the reference could not be downloaded: {e}"),
            LookError::TooLarge(e) => write!(f, "{e}"),
            LookError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LookError {}

/// The photo being matched: its source (any size; it is fitted on a small copy), the facts it is rendered against,
/// and its current settings (where the fit starts).
pub struct Target<'a> {
    pub src: &'a Rgb32f,
    pub info: &'a SourceInfo,
    pub settings: &'a DevelopSettings,
}

/// A part of the photo matched to a part of the reference: masks 0..1 of any size (sampled at the same relative
/// place), `None` for the whole picture.
#[derive(Clone, Debug)]
pub struct RegionPair {
    pub target: Option<Plane>,
    pub reference: Option<Plane>,
    /// How much this pair counts against the others.
    pub weight: f64,
}

impl RegionPair {
    /// The whole photo to the whole reference.
    pub fn whole() -> Self {
        RegionPair { target: None, reference: None, weight: 1.0 }
    }
}

/// What the fit may change, and how hard it works.
#[derive(Clone, Debug)]
pub struct MatchOptions {
    /// Long edge of the copy the fit renders, px.
    pub size: usize,
    /// The most pipeline renders.
    pub budget: usize,
    pub white_balance: bool,
    pub mixer: bool,
    pub grading: bool,
    pub curve: bool,
    /// Also fit the residual Look (a colour lookup after the sliders, for what they cannot reach); off by default.
    pub look: bool,
}

impl Default for MatchOptions {
    fn default() -> Self {
        MatchOptions { size: 256, budget: 320, white_balance: true, mixer: true, grading: true, curve: true, look: false }
    }
}

/// One slider the match moved.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Change {
    /// The develop control id (`light.exposure`, `wb.temp`, `grading.shadows.hue`…).
    pub id: String,
    pub from: f64,
    pub to: f64,
}

/// The result of a match.
#[derive(Clone, Debug)]
pub struct MatchResult {
    /// The photo's settings with the match applied.
    pub settings: DevelopSettings,
    pub changes: Vec<Change>,
    /// The master tone curve the match set, if it set one.
    pub curve: Option<Vec<Point>>,
    /// The residual Look the match set (an empty one when it removed the photo's earlier Look), if it changed it.
    pub look: Option<ResidualLook>,
    /// How far the looks were apart before and after (see [`stats::loss`]).
    pub loss_before: f64,
    pub loss_after: f64,
    /// Pipeline renders used.
    pub evals: usize,
}

/// The controls a match may move, compared before and after to list the [`Change`]s.
fn touched_ids() -> Vec<&'static str> {
    let mut ids = vec![
        "wb.temp",
        "wb.tint",
        "light.exposure",
        "light.contrast",
        "light.highlights",
        "light.shadows",
        "light.whites",
        "light.blacks",
        "color.vibrance",
        "color.saturation",
    ];
    ids.extend(lightcraft_develop::CONTROLS.iter().map(|c| c.id).filter(|id| id.starts_with("mixer.")));
    for z in [params::Zone::Shadows, params::Zone::Midtones, params::Zone::Highlights] {
        for part in ["hue", "sat"] {
            if let Some(c) = lightcraft_develop::CONTROLS.iter().find(|c| c.id.strip_prefix(z.id()).and_then(|r| r.strip_prefix('.')) == Some(part)) {
                ids.push(c.id);
            }
        }
    }
    ids
}

fn wb_get(d: &DevelopSettings, id: &str) -> Option<f64> {
    match id {
        "wb.temp" => Some(d.wb.temp),
        "wb.tint" => Some(d.wb.tint),
        _ => controls::get(d, id),
    }
}

impl MatchResult {
    /// This result at `amount` (0..1) of its strength: each slider part of the way from where it was, the curve
    /// part of the way from straight. 1 keeps it as it is.
    pub fn scaled(&self, amount: f64) -> MatchResult {
        let k = if amount.is_finite() { amount.clamp(0.0, 1.0) } else { 1.0 };
        let mut r = self.clone();
        // a wheel that had no saturation has no hue to start from: its hue is taken as fitted, its saturation scales
        let from_grey = |hue_id: &str| {
            let sat_id = hue_id.replace(".hue", ".sat");
            self.changes.iter().find(|c| c.id == sat_id).is_some_and(|c| c.from.abs() < 1e-9)
        };
        for c in &mut r.changes {
            let d = c.to - c.from;
            c.to = if c.id.starts_with("grading.") && c.id.ends_with(".hue") {
                if from_grey(&c.id) && k > 0.0 {
                    c.to
                } else {
                    // along the shorter way round the wheel
                    (c.from + ((d + 180.0).rem_euclid(360.0) - 180.0) * k).rem_euclid(360.0)
                }
            } else {
                c.from + d * k
            };
        }
        r.changes.retain(|c| (c.to - c.from).abs() > 1e-9);
        r.curve = self.curve.as_ref().filter(|_| k > 0.0).map(|pts| pts.iter().map(|p| Point::new(p.x, p.x + (p.y - p.x) * k)).collect());
        r.look = self.look.as_ref().filter(|_| k > 0.0).map(|l| ResidualLook { amount: (l.amount * k).round(), ..l.clone() });
        r
    }

    /// Apply the match's changes to `d` (the photo's settings now, which may have moved on since the match started):
    /// only the sliders the match moved are written.
    pub fn apply_to(&self, d: &mut DevelopSettings, info: &SourceInfo) {
        if self.changes.iter().any(|c| c.id.starts_with("wb.") && (c.to - c.from).abs() > 1e-9) {
            let (t, tint) = lightcraft_pipeline::auto::current_wb(info, d);
            lightcraft_pipeline::local::normalize_wb(d, info);
            d.wb.temp = t;
            d.wb.tint = tint;
            d.wb.mode = WbMode::Custom;
        }
        for c in &self.changes {
            match c.id.as_str() {
                "wb.temp" => d.wb.temp = c.to,
                "wb.tint" => d.wb.tint = c.to,
                id => {
                    controls::set(d, id, c.to);
                }
            }
        }
        if let Some(pts) = &self.curve {
            d.curve.master = pts.clone();
        }
        if let Some(look) = &self.look {
            d.look = look.clone();
        }
    }
}

/// The working copy the fit starts from: `s` with its white balance resolved to the numbers it renders with, on the
/// scale the source uses, so the fitter can step them.
fn working_copy(s: &DevelopSettings, info: &SourceInfo) -> DevelopSettings {
    let mut d = s.clone();
    let (t, tint) = lightcraft_pipeline::auto::current_wb(info, &d);
    lightcraft_pipeline::local::normalize_wb(&mut d, info);
    d.wb.temp = t;
    d.wb.tint = tint;
    d.wb.mode = WbMode::Custom;
    // a new match starts without the photo's earlier residual Look
    d.look = ResidualLook::default();
    d
}

/// A copy of `src` whose long edge is at most `edge`.
fn small(src: &Rgb32f, edge: usize) -> Rgb32f {
    let long = src.width.max(src.height);
    if long <= edge || long == 0 {
        return src.clone();
    }
    let k = edge as f64 / long as f64;
    let (w, h) = (((src.width as f64 * k).round() as usize).max(1), ((src.height as f64 * k).round() as usize).max(1));
    resize(src, w, h, Filter::Mitchell)
}

/// Render a library photo as a reference: its source with its own settings, at most `edge` px on the long side.
pub fn render_reference(src: &Rgb32f, info: &SourceInfo, settings: &DevelopSettings, edge: usize) -> Rgba8 {
    let edge = edge.clamp(16, 2048);
    lightcraft_pipeline::render(src, info, settings, &lightcraft_pipeline::RenderRequest::fit(edge, edge)).image
}

/// Fit `target`'s sliders so its look matches `reference` (a display-ready sRGB picture), region pair by region pair.
/// `progress` is told the renders done and the budget. Stops with [`LookError::Cancelled`] when `cancel` is raised.
pub fn match_look(
    target: &Target<'_>,
    reference: &Rgba8,
    regions: &[RegionPair],
    opts: &MatchOptions,
    cancel: &AtomicBool,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<MatchResult, LookError> {
    if reference.width == 0 || reference.height == 0 || reference.data.len() != reference.width.saturating_mul(reference.height) {
        return Err(LookError::Invalid("the reference picture is empty".into()));
    }
    if target.src.width == 0 || target.src.height == 0 {
        return Err(LookError::Invalid("the photo has no pixels".into()));
    }
    if regions.is_empty() {
        return Err(LookError::Invalid("no regions to match".into()));
    }
    let size = opts.size.clamp(32, 1024);
    let src = small(target.src, size);
    let mut info = target.info.clone();
    if info.native_long.is_none() {
        info.native_long = Some(target.src.width.max(target.src.height));
    }
    let mut goals = Vec::new();
    for r in regions {
        let stats = stats::measure(reference, r.reference.as_ref());
        if stats.weight > 0.0 && r.weight.is_finite() && r.weight > 0.0 {
            goals.push(fit::Goal { mask: r.target.clone(), stats, weight: r.weight });
        }
    }
    if goals.is_empty() {
        return Err(LookError::Invalid("the reference regions are empty".into()));
    }
    let start = working_copy(target.settings, &info);
    let mut d = start.clone();
    let mut fitter = fit::Fitter {
        src: &src,
        info: &info,
        req: fit::request(size),
        goals,
        evals: 0,
        budget: opts.budget.clamp(8, 5000),
        cancel,
        progress,
        start: start.clone(),
        all: fit::all_params(),
        large_bands: Vec::new(),
    };
    let (loss_before, loss_after) = fitter.run(&mut d, opts)?;
    let evals = fitter.evals;
    let mut changes = Vec::new();
    for id in touched_ids() {
        let (Some(from), Some(to)) = (wb_get(&start, id), wb_get(&d, id)) else { continue };
        let decimals = if id == "wb.temp" { 0 } else { controls::find(id).map_or(2, |c| c.decimals as i32) };
        let k = 10f64.powi(decimals);
        let to = (to * k).round() / k;
        if (to - from).abs() > 0.5 / k {
            changes.push(Change { id: id.to_string(), from: (from * k).round() / k, to });
        }
    }
    let curve = (d.curve.master != start.curve.master).then(|| d.curve.master.clone());
    let look = (d.look != target.settings.look && !(d.look.is_identity() && target.settings.look.is_identity())).then(|| d.look.clone());
    let mut settings = target.settings.clone();
    let result = MatchResult { settings: settings.clone(), changes, curve, look, loss_before, loss_after, evals };
    result.apply_to(&mut settings, &info);
    Ok(MatchResult { settings, ..result })
}

#[cfg(test)]
mod tests;
