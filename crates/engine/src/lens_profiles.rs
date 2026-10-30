//! Lens profile corrections from a lens database the user supplies ([`lightcraft_lensdb`]).
//!
//! When a raw carries no lens correction of its own (DNG opcodes, or a camera correction a raw reader converts,
//! [`crate::files::embedded_lens`]), the lens named in its EXIF is looked up in a lensfun-format database: the
//! directory in `LIGHTCRAFT_LENSFUN_DIR`, else the folder chosen in Settings (e.g. lensfun's `data/db`, CC-BY-SA, which is
//! never bundled with the app).
//! The profile is converted to the same [`lightcraft_develop::EmbeddedLens`] form, so "Enable Profile Corrections", its
//! amount sliders, the GPU path and DNG export treat both alike. Without a folder nothing changes.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_develop::EmbeddedLens;
use lightcraft_lensdb::{LensDb, Shot};

/// The environment variable naming the lens database directory. When set (and not empty) it wins over
/// the folder chosen in Settings.
pub const ENV: &str = "LIGHTCRAFT_LENSFUN_DIR";

/// Where the database folder in use comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderSource {
    /// No folder is configured.
    None,
    /// [`ENV`].
    Env,
    /// The folder chosen in Settings ([`set_folder`]).
    Setting,
}

/// Why a configured folder gives no profiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The folder doesn't exist.
    Missing,
    /// It exists but can't be listed (a file, no permission…); the system's words.
    Unreadable(String),
    /// It holds no `*.xml` files.
    NoXml,
    /// It holds XML files, but none gave a lens profile (not lensfun files, or all invalid).
    NoLenses,
}

/// What the database in use holds, for Settings and agents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub source: FolderSource,
    /// The folder in use ([`ENV`] when it is set, else the setting).
    pub dir: Option<PathBuf>,
    /// The folder chosen in Settings, even while [`ENV`] overrides it.
    pub setting: Option<PathBuf>,
    /// Lens profiles read.
    pub profiles: usize,
    /// XML files skipped as invalid.
    pub skipped: usize,
    pub problem: Option<Problem>,
}

/// A folder read.
pub(crate) struct Loaded {
    pub(crate) db: Option<Arc<LensDb>>,
    pub(crate) profiles: usize,
    pub(crate) skipped: usize,
    pub(crate) problem: Option<Problem>,
}

#[derive(Default)]
struct State {
    /// The folder chosen in Settings.
    setting: Option<PathBuf>,
    /// The last folder read and what it gave; read again when the folder in use changes.
    loaded: Option<(PathBuf, Arc<Loaded>)>,
}

static STATE: Mutex<State> = Mutex::new(State { setting: None, loaded: None });

/// The folder in use for an environment value and a setting: the variable wins when it isn't empty.
pub(crate) fn resolve(env: Option<std::ffi::OsString>, setting: Option<&Path>) -> (FolderSource, Option<PathBuf>) {
    match (env.filter(|v| !v.is_empty()), setting) {
        (Some(v), _) => (FolderSource::Env, Some(PathBuf::from(v))),
        (None, Some(dir)) => (FolderSource::Setting, Some(dir.to_path_buf())),
        (None, None) => (FolderSource::None, None),
    }
}

/// Read the lensfun-format files in `dir`.
pub(crate) fn load(dir: &Path) -> Loaded {
    let fail = |problem| Loaded { db: None, profiles: 0, skipped: 0, problem: Some(problem) };
    match std::fs::read_dir(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return fail(Problem::Missing),
        Err(e) => return fail(Problem::Unreadable(e.to_string())),
        Ok(entries) => {
            let any_xml = entries.flatten().any(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")));
            if !any_xml {
                return fail(Problem::NoXml);
            }
        }
    }
    match LensDb::load_dir(dir) {
        Ok((db, errors)) => {
            let skipped = errors.len();
            for (file, e) in errors {
                log::warn!("lens database: skipped {file}: {e}");
            }
            let profiles = db.lenses.len();
            if db.lenses.is_empty() {
                return Loaded { db: None, profiles: 0, skipped, problem: Some(Problem::NoLenses) };
            }
            Loaded { db: Some(Arc::new(db)), profiles, skipped, problem: None }
        }
        Err(e) => {
            log::warn!("{e}");
            fail(Problem::Unreadable(e.to_string()))
        }
    }
}

/// The folder in use and what reading it gave.
struct Current {
    source: FolderSource,
    dir: Option<PathBuf>,
    setting: Option<PathBuf>,
    loaded: Option<Arc<Loaded>>,
}

/// The current folder's contents, read again when the folder in use is not the one last read.
fn current() -> Current {
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let (source, dir) = resolve(std::env::var_os(ENV), state.setting.as_deref());
    let setting = state.setting.clone();
    let Some(dir) = dir else { return Current { source, dir: None, setting, loaded: None } };
    if state.loaded.as_ref().is_none_or(|(d, _)| *d != dir) {
        state.loaded = Some((dir.clone(), Arc::new(load(&dir))));
    }
    let loaded = state.loaded.as_ref().map(|(_, l)| l.clone());
    Current { source, dir: Some(dir), setting, loaded }
}

/// Choose the folder the database is read from (Settings ▸ lens database); `None` or an empty path
/// clears it. [`ENV`] still wins while it is set. The folder is read again on next use, and
/// [`reload`] forces that for the same folder.
pub fn set_folder(dir: Option<&Path>) {
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let dir = dir.filter(|d| !d.as_os_str().is_empty()).map(Path::to_path_buf);
    if state.setting != dir {
        state.setting = dir;
        state.loaded = None;
    }
}

/// Read the folder in use again (new files were added to it).
pub fn reload() {
    STATE.lock().unwrap_or_else(PoisonError::into_inner).loaded = None;
}

/// The configured database; `None` when none is configured or it holds no lenses.
pub fn database() -> Option<Arc<LensDb>> {
    current().loaded?.db.clone()
}

/// How many profiles the folder in use gave, or why none.
pub fn status() -> Status {
    let c = current();
    let (profiles, skipped, problem) = c.loaded.map_or((0, 0, None), |l| (l.profiles, l.skipped, l.problem.clone()));
    Status { source: c.source, dir: c.dir, setting: c.setting, profiles, skipped, problem }
}

/// The database profile for a photo with metadata `m`, for its `w × h` (unoriented, default-cropped) image,
/// re-expressed after `orientation`. `None` without a database, lens name, focal length or matching profile.
pub fn lookup(db: &LensDb, m: &lightcraft_meta::Metadata, w: f64, h: f64, orientation: lightcraft_geom::Orientation) -> Option<EmbeddedLens> {
    let focal = m.focal_length.filter(|f| f.is_finite() && *f > 0.0)?;
    let camera = db.find_camera(m.make.as_deref().unwrap_or_default(), m.model.as_deref().unwrap_or_default());
    let lens = match m.lens_model.as_deref() {
        Some(model) => db.find_lens(m.lens_make.as_deref(), model, camera)?,
        // Nikon DSLRs record only the lens specification ("24-70mm f/2.8"); a profile must be named exactly that
        // (a user's alias), since the specification fits other makers' lenses too
        None => {
            let name = lightcraft_lensdb::spec_name(m.lens_spec?);
            db.find_lens_named(m.lens_make.as_deref(), &name, camera)?
        }
    };
    let crop = frame_crop_factor(camera.map(|c| c.crop_factor), focal, m.focal_length_35mm);
    let shot = Shot { focal, aperture: m.f_number, distance: None, crop_factor: crop, width: w, height: h };
    let mut corr = lens.correction(&shot)?;
    // (a pincushion correction would leave blank bands along the frame's edges)
    corr.warp = corr.warp.map(|warp| lightcraft_lensdb::fill_frame(warp, w, h));
    Some(lightcraft_pipeline::optics::reorient_lens(&corr, orientation, w, h))
}

/// The crop factor of the recorded frame, which scales the profile's radii: the body's (`camera`), else what the
/// 35 mm equivalent focal length implies. A body shooting a crop of its sensor (a full-frame camera in APS-C /
/// Super 35 mode, or switched there by an APS-C lens) records a larger 35 mm equivalent than its own crop factor
/// gives; the frame's corners are then nearer the optical axis, so that crop wins. Within 10 % of the body's, the
/// difference is the tag's whole-millimetre rounding and the body's value stands.
fn frame_crop_factor(camera: Option<f64>, focal: f64, focal_35mm: Option<f64>) -> Option<f64> {
    let frame = focal_35mm.map(|f35| f35 / focal).filter(|c| c.is_finite() && (0.1..=20.0).contains(c));
    match (camera, frame) {
        (Some(body), Some(frame)) if frame > body * 1.1 => Some(frame),
        (body, frame) => body.or(frame),
    }
}

/// [`crate::files::embedded_lens`], else the database profile ([`lookup`]) for a raw with metadata `meta`.
pub fn for_raw(raw: &lightcraft_raw::RawInfo, meta: impl FnOnce() -> lightcraft_meta::Metadata) -> Option<EmbeddedLens> {
    crate::files::embedded_lens(raw).or_else(|| {
        let db = database()?;
        let (w, h) = raw.developed_size();
        lookup(&db, &meta(), w as f64, h as f64, raw.orientation)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_geom::Orientation;

    const DB: &str = r#"<lensdatabase>
      <camera><maker>Testcam</maker><model>TC-1</model><mount>T</mount><cropfactor>1.5</cropfactor></camera>
      <lens><maker>Testcam</maker><model>Testcam 18-55mm f/3.5-5.6</model><mount>T</mount><cropfactor>1.5</cropfactor>
        <calibration>
          <distortion model="poly3" focal="18" k1="-0.03" />
          <vignetting model="pa" focal="18" aperture="3.5" distance="1000" k1="-0.4" k2="0" k3="0" />
        </calibration>
      </lens></lensdatabase>"#;

    fn meta() -> lightcraft_meta::Metadata {
        lightcraft_meta::Metadata {
            make: Some("Testcam".into()),
            model: Some("TC-1".into()),
            lens_model: Some("18-55mm F3.5-5.6".into()),
            focal_length: Some(18.0),
            f_number: Some(3.5),
            ..Default::default()
        }
    }

    #[test]
    fn exif_lens_is_found_and_converted() {
        let db = LensDb::parse(DB).unwrap();
        let lens = lookup(&db, &meta(), 6000.0, 4000.0, Orientation::Normal).unwrap();
        assert!(lens.warp.is_some() && lens.vignette.is_some());
        let rotated = lookup(&db, &meta(), 6000.0, 4000.0, Orientation::Rotate90).unwrap();
        assert_eq!(rotated.warp.unwrap().center, lightcraft_geom::Point::new(0.5, 0.5));
        // no lens name, no focal length, an unknown lens: nothing
        assert!(lookup(&db, &lightcraft_meta::Metadata { lens_model: None, ..meta() }, 6000.0, 4000.0, Orientation::Normal).is_none());
        assert!(lookup(&db, &lightcraft_meta::Metadata { focal_length: None, ..meta() }, 6000.0, 4000.0, Orientation::Normal).is_none());
        let other = lightcraft_meta::Metadata { lens_model: Some("70-200mm F4".into()), ..meta() };
        assert!(lookup(&db, &other, 6000.0, 4000.0, Orientation::Normal).is_none());
    }

    /// A Nikon DSLR raw names no lens, only its specification (maker note `0x0084`): it is looked up by the name
    /// built from it, and only a profile named exactly that is used, never another maker's lens that merely shares
    /// the focal range and aperture.
    #[test]
    fn lens_specification_needs_a_profile_named_by_it() {
        let nikon = lightcraft_meta::Metadata { lens_model: None, lens_spec: Some([18.0, 55.0, 3.5, 5.6]), ..meta() };
        let at = |db: &LensDb| lookup(db, &nikon, 6000.0, 4000.0, Orientation::Normal);
        // "Testcam 18-55mm f/3.5-5.6" fits the specification, but so would any maker's 18-55 f/3.5-5.6: refused
        assert!(at(&LensDb::parse(DB).unwrap()).is_none());
        let aliased = LensDb::parse(
            &DB.replace("<model>Testcam 18-55mm f/3.5-5.6</model>", "<model>Testcam 18-55mm f/3.5-5.6</model><model>18-55mm f/3.5-5.6</model>"),
        )
        .unwrap();
        let lens = at(&aliased).unwrap();
        assert!(lens.warp.is_some() && lens.vignette.is_some());
        assert_eq!(Some(lens), lookup(&aliased, &meta(), 6000.0, 4000.0, Orientation::Normal), "same profile as by its name");
        // a lens model, when the file has one, still decides
        let named_other = lightcraft_meta::Metadata { lens_model: Some("70-200mm F4".into()), ..nikon.clone() };
        assert!(lookup(&aliased, &named_other, 6000.0, 4000.0, Orientation::Normal).is_none());
    }

    /// A full-frame body in APS-C / Super 35 mode (an ILCE-1M2 at 70 mm records 105 mm equivalent) frames only the
    /// middle of the image circle: the profile is applied at the frame's crop factor, not the body's, so the crop's
    /// corners don't get the full frame's corner correction.
    #[test]
    fn in_camera_crop_mode_uses_the_frames_crop_factor() {
        let db = LensDb::parse(
            r#"<lensdatabase>
              <camera><maker>Sony</maker><model>ILCE-1M2</model><mount>Sony E</mount><cropfactor>1</cropfactor></camera>
              <lens><maker>Sigma</maker><model>24-70mm F2.8 DG DN | Art 019</model><mount>Sony E</mount><cropfactor>1.0</cropfactor>
                <calibration>
                  <distortion model="ptlens" focal="70" a="0.0019947" b="0.0035263" c="0.0005016" />
                  <vignetting model="pa" focal="70" aperture="2.8" distance="1000" k1="-0.4" k2="0" k3="0" />
                </calibration>
              </lens></lensdatabase>"#,
        )
        .unwrap();
        let m = |f35: Option<f64>| lightcraft_meta::Metadata {
            make: Some("SONY".into()),
            model: Some("ILCE-1M2".into()),
            lens_model: Some("24-70mm F2.8 DG DN | Art 019".into()),
            focal_length: Some(70.0),
            focal_length_35mm: f35,
            f_number: Some(2.8),
            ..Default::default()
        };
        let at = |crop: f64, w: f64, h: f64| {
            let shot = Shot { focal: 70.0, aperture: Some(2.8), distance: None, crop_factor: Some(crop), width: w, height: h };
            let mut corr = db.lenses[0].correction(&shot).unwrap();
            // (pincushion at 70 mm: zoomed to fill the frame)
            let raw = corr.warp.unwrap();
            corr.warp = Some(lightcraft_lensdb::fill_frame(raw, w, h));
            assert_ne!(corr.warp, Some(raw));
            corr
        };
        let aps_c = lookup(&db, &m(Some(105.0)), 5616.0, 3744.0, Orientation::Normal).unwrap();
        assert_eq!(aps_c, at(1.5, 5616.0, 3744.0));
        assert_ne!(aps_c, at(1.0, 5616.0, 3744.0), "the crop is not corrected as a full frame");
        // full frame, a 35 mm equivalent rounded off by a millimetre, or none at all: the body's crop factor
        for f35 in [Some(70.0), Some(71.0), None] {
            assert_eq!(lookup(&db, &m(f35), 8640.0, 5760.0, Orientation::Normal).unwrap(), at(1.0, 8640.0, 5760.0), "{f35:?}");
        }
        assert_eq!(frame_crop_factor(None, 70.0, Some(105.0)), Some(1.5), "no body in the database: the 35 mm equivalent's");
        assert_eq!(frame_crop_factor(Some(1.0), 0.0, Some(105.0)), Some(1.0), "a hostile focal length is ignored");
    }

    /// A synthetic grid recorded through the profile's barrel distortion comes out of the pipeline's geometry
    /// stage with straight lines.
    #[test]
    fn pipeline_straightens_a_distorted_grid() {
        let db = LensDb::parse(DB).unwrap();
        let profile = &db.lenses[0];
        let (w, h) = (600usize, 400usize);
        let lens = lookup(&db, &meta(), w as f64, h as f64, Orientation::Normal).unwrap();
        // record the grid: each sensor pixel sees the scene point whose distorted position it is
        let n = h as f64 / 2.0;
        let undistort = |x: f64, y: f64| {
            let (dx, dy) = ((x - w as f64 / 2.0) / n, (y - h as f64 / 2.0) / n);
            let rd = dx.hypot(dy);
            let (mut lo, mut hi) = (0.0, 3.0);
            for _ in 0..60 {
                let mid = (lo + hi) / 2.0;
                if mid * profile.distortion_ratio(18.0, mid) < rd {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let s = if rd > 0.0 { lo / rd } else { 1.0 };
            (w as f64 / 2.0 + dx * s * n, h as f64 / 2.0 + dy * s * n)
        };
        // horizontal scene lines every 50 px (2 px wide, dark on light)
        let line = |v: f64| if v.rem_euclid(50.0) < 2.0 { 0.02f32 } else { 0.8 };
        let src = lightcraft_raster::Rgb32f::from_fn(w, h, |x, y| {
            let (_, sy) = undistort(x as f64 + 0.5, y as f64 + 0.5);
            [line(sy); 3]
        });
        // the dark row nearest y = 51 in each column, sub-pixel (centroid of darkness), as rendered
        let line_rows = |img: &lightcraft_raster::Rgba8| -> Vec<f64> {
            (20..w - 20)
                .step_by(10)
                .map(|x| {
                    let (mut sw, mut swy) = (0.0, 0.0);
                    for y in 35..70 {
                        let d = 255.0 - img.get(x, y)[1] as f64;
                        if d > 60.0 {
                            sw += d;
                            swy += d * (y as f64 + 0.5);
                        }
                    }
                    swy / sw.max(1e-9)
                })
                .collect()
        };
        let spread = |rows: &[f64]| rows.iter().cloned().fold(f64::MIN, f64::max) - rows.iter().cloned().fold(f64::MAX, f64::min);
        let req = lightcraft_pipeline::RenderRequest::fit(w, h);
        let mut s = lightcraft_develop::DevelopSettings::default();
        let info = lightcraft_pipeline::SourceInfo { lens: Some(lens), ..Default::default() };
        let before = spread(&line_rows(&lightcraft_pipeline::render(&src, &info, &s, &req).image));
        s.optics.lens_profile = true;
        let after = spread(&line_rows(&lightcraft_pipeline::render(&src, &info, &s, &req).image));
        assert!(before > 2.0, "the recorded line is curved: {before} px");
        // 8-bit output and a 2 px line: the centroid measurement itself is good to about a pixel
        assert!(after < 1.0 && after < before / 8.0, "corrected line bent by {after} px (uncorrected {before})");
    }
}
