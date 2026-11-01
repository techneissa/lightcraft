//! Starting Colour for Raw Photos (Settings ▸ Import ▸ Raw defaults): the colour source a new raw starts on. macOS
//! Colour by default, for every camera this Mac's raw decoder reads (the name gate) at LightCraft's size (the size
//! gate); a camera that fails them, or a computer that isn't a Mac, starts on Camera Matching. A default preset that
//! names a colour source wins. Changing the setting never touches photos already in the library
//! (`docs/camera-preview-colour.md`, "macOS Colour").

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use lightcraft_catalog::{Photo, Source};
use lightcraft_develop::{CameraColour, DevelopSettings};
use serde::{Deserialize, Serialize};

/// The setting's value, stored in the library's `prefs.json` (`import.startingColour`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StartingColour {
    /// macOS Colour where this computer can make it, else Camera Matching.
    #[default]
    Macos,
    /// Camera Matching (`auto`).
    CameraMatching,
    /// No Camera Matching (`colorimetric`).
    NoCameraMatching,
}

impl StartingColour {
    /// The command value (`library.preferences {import: {startingColour}}`).
    pub fn value(self) -> &'static str {
        match self {
            StartingColour::Macos => "macos",
            StartingColour::CameraMatching => "cameraMatching",
            StartingColour::NoCameraMatching => "noCameraMatching",
        }
    }

    /// The setting named by a command value.
    pub fn parse(v: &str) -> Option<StartingColour> {
        [StartingColour::Macos, StartingColour::CameraMatching, StartingColour::NoCameraMatching].into_iter().find(|c| c.value() == v)
    }
}

/// What the gates ask of the Mac's raw decoder, or a stand-in in tests.
pub(crate) trait Gate: Send + Sync {
    fn available(&self) -> bool;
    fn supports(&self, exif_model: &str) -> bool;
    /// The decoder's upright full size of the raw at `path`, without decoding it.
    fn native_size(&self, path: &Path) -> Option<(u32, u32)>;
}

/// [`lightcraft_macraw`] itself.
pub(crate) struct System;

impl Gate for System {
    fn available(&self) -> bool {
        lightcraft_macraw::available()
    }
    fn supports(&self, exif_model: &str) -> bool {
        lightcraft_macraw::supports(exif_model)
    }
    fn native_size(&self, path: &Path) -> Option<(u32, u32)> {
        lightcraft_macraw::probe(lightcraft_macraw::Input::Path(path)).ok().map(|p| p.native_size)
    }
}

/// Runs `f` with this computer's decoder and the session's verdicts (a test's stand-in on its own thread).
pub(crate) fn with_gate<R>(f: impl FnOnce(&dyn Gate, &Verdicts) -> R) -> R {
    current().with_gate(f)
}

/// The decoder and verdicts this thread checks with, to hand to a background check ([`check_later`]).
#[derive(Clone)]
pub(crate) struct GateRef(Option<std::sync::Arc<(Box<dyn Gate>, Verdicts)>>);

impl GateRef {
    pub(crate) fn with_gate<R>(&self, f: impl FnOnce(&dyn Gate, &Verdicts) -> R) -> R {
        match &self.0 {
            Some(t) => f(t.0.as_ref(), &t.1),
            None => f(&System, &VERDICTS),
        }
    }
}

/// This computer's decoder, or this test thread's stand-in.
pub(crate) fn current() -> GateRef {
    #[cfg(test)]
    if let Some(t) = GATE_FOR_TESTS.with(|g| g.borrow().clone()) {
        return GateRef(Some(t));
    }
    GateRef(None)
}

#[cfg(test)]
thread_local! {
    /// A stand-in for the decoder and fresh verdicts on this test's thread (shared with its background checks).
    pub(crate) static GATE_FOR_TESTS: std::cell::RefCell<Option<std::sync::Arc<(Box<dyn Gate>, Verdicts)>>> = const { std::cell::RefCell::new(None) };
}

/// Size-gate verdicts per (camera, size): one probe per camera model and size, not one per photo. Only a probe that
/// read the file gives a verdict: one that failed (a truncated or unreadable file) is not kept, so the next photo of
/// that camera and size is probed again instead of every one starting on Camera Matching for the session.
#[derive(Default)]
pub(crate) struct Verdicts {
    known: Mutex<HashMap<Key, bool>>,
    /// Keys a background check is probing now (one check per key at a time).
    checking: Mutex<std::collections::HashSet<Key>>,
    /// Keys a background check could not read: the cached path ([`macos_gates_known`]) counts them as another size
    /// rather than asking again; a probing path still tries them, and a reading clears the mark.
    unread: Mutex<std::collections::HashSet<Key>>,
}

/// A size verdict's key: the camera (make and model) and the raw's size.
type Key = (String, (u32, u32));

/// The verdicts of this session (a new OS build means a new session).
static VERDICTS: std::sync::LazyLock<Verdicts> = std::sync::LazyLock::new(Verdicts::default);

impl Verdicts {
    fn known(&self, key: &Key) -> Option<bool> {
        self.known.lock().unwrap_or_else(PoisonError::into_inner).get(key).copied()
    }

    /// The verdict for `key`, probing when there is none; `probe` answers `None` when it could not read the file
    /// (not kept: no verdict, the photo fails the gate this time).
    fn get_or(&self, key: Key, probe: impl FnOnce() -> Option<bool>) -> bool {
        if let Some(v) = self.known(&key) {
            return v;
        }
        let Some(v) = probe() else {
            self.unread.lock().unwrap_or_else(PoisonError::into_inner).insert(key);
            return false;
        };
        self.unread.lock().unwrap_or_else(PoisonError::into_inner).remove(&key);
        self.known.lock().unwrap_or_else(PoisonError::into_inner).insert(key, v);
        v
    }
}

/// Whether two sizes agree within 2 % on each edge, in either orientation (the catalog keeps the sensor's size, the
/// decoder reports it upright).
fn same_size(a: (u32, u32), b: (u32, u32)) -> bool {
    let near = |x: u32, y: u32| x > 0 && y > 0 && f64::from(x.abs_diff(y)) <= 0.02 * f64::from(x.max(y));
    (near(a.0, b.0) && near(a.1, b.1)) || (near(a.0, b.1) && near(a.1, b.0))
}

/// Why photo `p` can't start on (or be moved to) macOS Colour here; `None` when it can.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// Not a Mac, or its decoder lacks a setting the recipe needs.
    System,
    /// Not a raw macOS Colour applies to, or macOS doesn't list its camera, or LightCraft has no verified type for its
    /// format.
    Camera,
    /// macOS reads it at another size, or can't read it.
    Size,
}

/// What the gates say of a photo from what is known: decided, or waiting for its (camera, size) to be probed.
pub(crate) enum Known {
    Decided(Result<(), Skip>),
    /// The size gate needs a probe of this file (`check_later`).
    Unchecked(Key, String),
}

/// The gates for photo `p` from the catalog and the verdicts already found, never probing.
pub(crate) fn macos_gates_known(p: &Photo, gate: &dyn Gate, verdicts: &Verdicts) -> Known {
    gates(p, gate, verdicts, true)
}

/// [`macos_gates_known`]; `unread_fails`: a pair a background check could not read fails the size gate (else it is
/// unchecked, so a probing caller tries it again).
fn gates(p: &Photo, gate: &dyn Gate, verdicts: &Verdicts, unread_fails: bool) -> Known {
    if !p.fitted_look() || !p.format.eq_ignore_ascii_case("ARW") {
        return Known::Decided(Err(Skip::Camera));
    }
    if !gate.available() {
        return Known::Decided(Err(Skip::System));
    }
    // the catalog joins make and model (`SONY ILCE-7M5`); macOS lists the EXIF model
    let camera = p.meta.camera.trim();
    let model = camera.split_once(' ').map_or(camera, |(_, model)| model.trim());
    if camera.is_empty() || !(gate.supports(model) || gate.supports(camera)) {
        return Known::Decided(Err(Skip::Camera));
    }
    let Source::File { path } = &p.source else { return Known::Decided(Err(Skip::Camera)) };
    let key = (camera.to_owned(), (p.width, p.height));
    match verdicts.known(&key) {
        Some(fits) => Known::Decided(if fits { Ok(()) } else { Err(Skip::Size) }),
        None if unread_fails && verdicts.unread.lock().unwrap_or_else(PoisonError::into_inner).contains(&key) => Known::Decided(Err(Skip::Size)),
        None => Known::Unchecked(key, path.clone()),
    }
}

/// The gates for photo `p`, from the catalog and one probe per (camera, size) (`verdicts`). Probes (up to seconds a
/// camera): never on the UI thread, which uses [`macos_gates_known`] and [`check_later`].
pub(crate) fn macos_gates(p: &Photo, gate: &dyn Gate, verdicts: &Verdicts) -> Result<(), Skip> {
    match gates(p, gate, verdicts, false) {
        Known::Decided(r) => r,
        Known::Unchecked(key, path) => {
            let size = key.1;
            if verdicts.get_or(key, || gate.native_size(Path::new(&path)).map(|n| same_size(n, size))) { Ok(()) } else { Err(Skip::Size) }
        }
    }
}

/// The activity row of a background size check.
pub const CHECKING_LABEL: &str = "Checking macOS Raw Support";

/// Probes the unchecked (camera, size) pairs on a thread of its own, under an activity row ([`CHECKING_LABEL`]), so the
/// next count or summary knows them; a pair already being checked is not checked twice. Returns at once.
pub(crate) fn check_later(activity: &crate::activity::Activity, gate: GateRef, unchecked: Vec<(Key, String)>) {
    let todo: Vec<(Key, String)> = gate.with_gate(|_, verdicts| {
        let mut checking = verdicts.checking.lock().unwrap_or_else(PoisonError::into_inner);
        let mut todo = Vec::new();
        for (key, path) in unchecked {
            if checking.insert(key.clone()) {
                todo.push((key, path));
            }
        }
        todo
    });
    if todo.is_empty() {
        return;
    }
    let task = activity.start("macosCheck", CHECKING_LABEL, crate::activity::Cancel::No);
    let keys: Vec<Key> = todo.iter().map(|(k, _)| k.clone()).collect();
    let worker = gate.clone();
    let spawned = std::thread::Builder::new().name("lc-macos-check".into()).spawn(move || {
        // the row stays while this runs (it is dropped with the closure, however that ends)
        let row = task;
        let total = todo.len() as u64;
        for (i, (key, path)) in todo.into_iter().enumerate() {
            worker.with_gate(|gate, verdicts| {
                let size = key.1;
                verdicts.get_or(key.clone(), || gate.native_size(Path::new(&path)).map(|n| same_size(n, size)));
                verdicts.checking.lock().unwrap_or_else(PoisonError::into_inner).remove(&key);
            });
            row.progress(i as u64 + 1, total);
        }
    });
    // (not started: forget the keys, so the next count tries again)
    if let Err(e) = spawned {
        log::warn!("macOS Colour: could not check raw support: {e}");
        gate.with_gate(|_, verdicts| {
            let mut checking = verdicts.checking.lock().unwrap_or_else(PoisonError::into_inner);
            keys.iter().for_each(|k| {
                checking.remove(k);
            });
        });
    }
}

/// The colour source a new photo starts on, from the setting and the gates.
pub(crate) fn effective(setting: StartingColour, p: &Photo, gate: &dyn Gate, verdicts: &Verdicts) -> CameraColour {
    match setting {
        StartingColour::CameraMatching => CameraColour::Auto,
        StartingColour::NoCameraMatching => CameraColour::Colorimetric,
        StartingColour::Macos if macos_gates(p, gate, verdicts).is_ok() => CameraColour::Macos,
        StartingColour::Macos => CameraColour::Auto,
    }
}

/// Puts a freshly imported photo on its starting colour, as part of its import look (so it is not "edited"). Nothing
/// changes for a photo the choice doesn't apply to, or when the default preset names a colour source (`preset_colour`).
pub(crate) fn apply(setting: StartingColour, preset_colour: bool, p: &mut Photo, gate: &dyn Gate, verdicts: &Verdicts) {
    if preset_colour || !p.fitted_look() {
        return;
    }
    let colour = effective(setting, p, gate, verdicts);
    if colour == p.develop.profile.camera_colour {
        return;
    }
    let mut d: DevelopSettings = (*p.develop).clone();
    d.profile.camera_colour = colour;
    d.profile.camera_profile = None;
    let look = std::sync::Arc::new(d);
    p.develop = look.clone();
    p.import_look = Some(look);
}

#[cfg(test)]
#[path = "starting_colour_tests.rs"]
pub(crate) mod tests;
