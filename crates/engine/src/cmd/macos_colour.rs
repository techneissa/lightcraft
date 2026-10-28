//! Apply macOS Colour to All Photos from This Camera (the Profile menu): every photo of a camera moves to macOS Colour
//! in one undo step, never by itself. Photos this Mac can't make macOS Colour for are skipped and counted by reason
//! (`import::macos_gates`: not a Mac, a camera or format macOS doesn't read, another size). These commands run on the UI
//! thread, so they only use what is known: a camera and size not checked yet is checked on a thread of its own (an
//! activity row, `import::MACOS_CHECKING_LABEL`), and the count says how many photos wait for it (`checking`).

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::CameraColour;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::Session;
use crate::import::{MacosKnown, MacosSkip};

const COUNT: &str = "profiles.macosForCameraCount";
const APPLY: &str = "profiles.macosForCamera";
/// Why the move waits: some of the camera's photos are still being checked (the count's `checking`).
pub const CHECKING: &str = "Checking which photos macOS can read. Try again in a moment.";

/// The photos of `camera` that are not on macOS Colour yet: the ones that can move, and the skipped ones by reason.
pub(crate) struct Plan {
    pub camera: String,
    pub photos: Vec<PhotoId>,
    pub system: usize,
    pub unread: usize,
    pub size: usize,
    /// Photos whose camera and size are still being checked (`import::check_macos_later`).
    pub checking: usize,
}

impl Plan {
    fn json(&self) -> Value {
        json!({
            "camera": self.camera,
            "photos": self.photos.len(),
            "skipped": self.system + self.unread + self.size,
            "notMac": self.system,
            "cameraNotRead": self.unread,
            "otherSize": self.size,
            "checking": self.checking,
        })
    }
}

/// The camera of `photo` (the active photo when absent) and what moving its photos would do.
pub(crate) fn plan(s: &Session, c: &str, p: &Value) -> crate::Result<Plan> {
    let id = match p.get("photo").and_then(Value::as_u64) {
        Some(id) => PhotoId(id),
        None => s.active().ok_or_else(|| bad(c, "no photo"))?,
    };
    let camera = s.catalog.photo(id).map(|p| p.meta.camera.trim().to_owned()).filter(|c| !c.is_empty()).ok_or_else(|| bad(c, "no camera"))?;
    let mut plan = Plan { camera, photos: Vec::new(), system: 0, unread: 0, size: 0, checking: 0 };
    let gate = crate::import::macos_gate();
    let mut unchecked = Vec::new();
    gate.clone().with_gate(|gate, verdicts| {
        for ph in s.catalog.photos() {
            let other = !ph.meta.camera.trim().eq_ignore_ascii_case(&plan.camera);
            if ph.deleted || !ph.fitted_look() || other || ph.develop.profile.camera_colour == CameraColour::Macos {
                continue;
            }
            match crate::import::macos_gates_known(ph, gate, verdicts) {
                MacosKnown::Decided(Ok(())) => plan.photos.push(ph.id),
                MacosKnown::Decided(Err(MacosSkip::System)) => plan.system += 1,
                MacosKnown::Decided(Err(MacosSkip::Camera)) => plan.unread += 1,
                MacosKnown::Decided(Err(MacosSkip::Size)) => plan.size += 1,
                MacosKnown::Unchecked(key, path) => {
                    plan.checking += 1;
                    unchecked.push((key, path));
                }
            }
        }
    });
    crate::import::check_macos_later(&s.activity, gate, unchecked);
    Ok(plan)
}

/// Of `ids`, the photos on macOS Colour that this computer makes with Camera Matching colours (not a Mac, a camera or
/// format macOS doesn't read, another size): what a preset applied to several photos reports. A photo whose camera and
/// size are not checked yet is not counted; its check starts on a thread of its own.
pub(crate) fn fallbacks(s: &Session, ids: &[PhotoId]) -> usize {
    let gate = crate::import::macos_gate();
    let mut unchecked = Vec::new();
    let n = gate.clone().with_gate(|gate, verdicts| {
        ids.iter()
            .filter_map(|id| s.catalog.photo(*id))
            .filter(|p| p.develop.profile.camera_colour == CameraColour::Macos)
            .filter(|p| match crate::import::macos_gates_known(p, gate, verdicts) {
                MacosKnown::Decided(r) => r.is_err(),
                MacosKnown::Unchecked(key, path) => {
                    unchecked.push((key, path));
                    false
                }
            })
            .count()
    });
    crate::import::check_macos_later(&s.activity, gate, unchecked);
    n
}

/// Leaving macOS Colour for `colour` on a relative-scale source: the as-shot white a white set in Kelvin is carried
/// over from (`pipeline::local::leave_kelvin`), for the active photo; `None` when nothing is carried.
pub(crate) fn leaving_as_shot(s: &Session, colour: Option<CameraColour>) -> Option<(f64, f64)> {
    let photo = s.active()?;
    let p = s.catalog.photo(photo)?;
    let from_macos = p.develop.profile.camera_colour == CameraColour::Macos;
    let to_relative = colour.is_some_and(|c| c != CameraColour::Macos) && p.relative_wb();
    (from_macos && to_relative).then(|| crate::files::macos_as_shot(s, photo)).flatten()
}

/// The history's name for a colour-source change (`develop.profile` with `camera_colour` alone): the picker's row.
pub(crate) fn colour_label(colour: Option<CameraColour>) -> &'static str {
    match colour {
        Some(CameraColour::Macos) => "macOS Colour",
        Some(CameraColour::Colorimetric) => "No Camera Matching",
        _ => "Camera Matching",
    }
}

/// Moves the planned photos to macOS Colour in one undo step; returns how many moved.
pub(crate) fn apply(s: &mut Session, plan: &Plan) -> crate::Result<usize> {
    let label = "Profile: macOS Colour";
    let ops: Vec<Op> = plan
        .photos
        .iter()
        .filter_map(|photo| s.develop_of(*photo).map(|d| (*photo, d)))
        .filter_map(|(photo, d)| {
            let mut d = (*d).clone();
            d.profile.camera_colour = CameraColour::Macos;
            d.profile.camera_profile = None;
            s.develop_op(photo, d, label)
        })
        .collect();
    let changed = ops.len();
    if changed > 0 {
        s.commit(label, Op::Batch { ops })?;
    }
    Ok(changed)
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "profiles.macosForCameraCount", "macOS Colour for a Camera: Count", [], None, "{photo?: id (the active photo when absent)} — what `profiles.macosForCamera` would do for that photo's camera, from what is known (never waits for macOS) → {camera, photos: count that would move, skipped, notMac, cameraNotRead, otherSize, checking: photos whose camera and size are being checked in the background}", always, |s, p| {
            plan(s, COUNT, p).map(|plan| plan.json())
        }),
        cmd!(
            "profiles.macosForCamera",
            "Apply macOS Colour to All Photos from This Camera",
            [],
            None,
            "{photo?: id (the active photo when absent)} — every photo of that photo's camera goes to macOS Colour in one undo step; photos this Mac can't make it for are skipped and counted; an error while some are still being checked (`checking` in the count) → {camera, changed, skipped, notMac, cameraNotRead, otherSize}",
            always,
            |s, p| {
                let plan = plan(s, APPLY, p)?;
                if plan.checking > 0 {
                    return Err(bad(APPLY, CHECKING));
                }
                let changed = apply(s, &plan)?;
                let mut out = plan.json();
                if let Some(o) = out.as_object_mut() {
                    o.remove("photos");
                    o.insert("changed".into(), json!(changed));
                }
                Ok(out)
            }
        ),
    ]
}

#[cfg(test)]
#[path = "macos_colour_tests.rs"]
mod tests;
