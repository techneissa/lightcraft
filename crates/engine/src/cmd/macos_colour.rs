//! Apply macOS Colour to All Photos from This Camera (the Profile menu): every photo of a camera moves to macOS Colour
//! in one undo step, never by itself. Photos this Mac can't make macOS Colour for are skipped and counted by reason
//! (`import::macos_gates`: not a Mac, a camera or format macOS doesn't read, another size).

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::CameraColour;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::Session;
use crate::import::MacosSkip;

const COUNT: &str = "profiles.macosForCameraCount";
const APPLY: &str = "profiles.macosForCamera";

/// The photos of `camera` that are not on macOS Colour yet: the ones that can move, and the skipped ones by reason.
pub(crate) struct Plan {
    pub camera: String,
    pub photos: Vec<PhotoId>,
    pub system: usize,
    pub unread: usize,
    pub size: usize,
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
    let mut plan = Plan { camera, photos: Vec::new(), system: 0, unread: 0, size: 0 };
    crate::import::with_macos_gate(|gate, verdicts| {
        for ph in s.catalog.photos() {
            let other = !ph.meta.camera.trim().eq_ignore_ascii_case(&plan.camera);
            if ph.deleted || !ph.fitted_look() || other || ph.develop.profile.camera_colour == CameraColour::Macos {
                continue;
            }
            match crate::import::macos_gates(ph, gate, verdicts) {
                Ok(()) => plan.photos.push(ph.id),
                Err(MacosSkip::System) => plan.system += 1,
                Err(MacosSkip::Camera) => plan.unread += 1,
                Err(MacosSkip::Size) => plan.size += 1,
            }
        }
    });
    Ok(plan)
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
        cmd!(query "profiles.macosForCameraCount", "macOS Colour for a Camera: Count", [], None, "{photo?: id (the active photo when absent)} — what `profiles.macosForCamera` would do for that photo's camera → {camera, photos: count that would move, skipped, notMac, cameraNotRead, otherSize}", always, |s, p| {
            plan(s, COUNT, p).map(|plan| plan.json())
        }),
        cmd!(
            "profiles.macosForCamera",
            "Apply macOS Colour to All Photos from This Camera",
            [],
            None,
            "{photo?: id (the active photo when absent)} — every photo of that photo's camera goes to macOS Colour in one undo step; photos this Mac can't make it for are skipped and counted → {camera, changed, skipped, notMac, cameraNotRead, otherSize}",
            always,
            |s, p| {
                let plan = plan(s, APPLY, p)?;
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
