//! Apply macOS Colour to All Photos from This Camera, off the UI thread: the count (`profiles.macosForCameraCount`)
//! and the move (`profiles.macosForCamera`) ask the Mac's decoder about every photo of the camera, and the move builds
//! two ops per photo, which takes seconds in a large library. [`count`] and [`apply`] take what they need from the
//! session (a quick pass over the catalog), [`Job::run`] does the rest without it, and [`finish`] lands the result: the
//! same answer as the commands, the move as one undo step. A photo edited while the job ran keeps that edit.

use std::collections::HashMap;
use std::sync::Arc;

use lightcraft_catalog::{Op, Photo, PhotoId};
use lightcraft_develop::{CameraColour, DevelopSettings};
use serde_json::{Value, json};

use crate::Session;
use crate::cmd::macos_colour::{CHECKING, Plan, Unchecked, candidates, plan_of};

const LABEL: &str = "Profile: macOS Colour";

/// A count or a move set up from the session; [`Job::run`] needs none.
pub struct Job {
    apply: bool,
    camera: String,
    photos: Vec<Arc<Photo>>,
    gate: crate::import::MacosGate,
    /// The edit time the move stamps (the session's clock, read when the job was set up).
    edited: String,
}

/// What [`Job::run`] found, for [`finish`].
pub struct Done {
    apply: bool,
    plan: Plan,
    unchecked: Unchecked,
    gate: crate::import::MacosGate,
    /// Per photo to move: its settings when the job started, and the op that moves it.
    ops: Vec<(PhotoId, Arc<DevelopSettings>, Op)>,
}

/// Count what `profiles.macosForCameraCount {photo?}` would.
pub fn count(s: &Session, p: &Value) -> Result<Job, String> {
    job(s, p, false)
}

/// Move what `profiles.macosForCamera {photo?}` would.
pub fn apply(s: &Session, p: &Value) -> Result<Job, String> {
    job(s, p, true)
}

fn job(s: &Session, p: &Value, apply: bool) -> Result<Job, String> {
    let c = if apply { "profiles.macosForCamera" } else { "profiles.macosForCameraCount" };
    let (camera, photos) = candidates(s, c, p).map_err(|e| e.to_string())?;
    Ok(Job { apply, camera, photos, gate: crate::import::macos_gate(), edited: (s.clock)() })
}

impl Job {
    /// How many photos the job looks at.
    pub fn total(&self) -> usize {
        self.photos.len()
    }

    /// The gates for every photo and, for a move, its ops.
    pub fn run(self) -> Done {
        let (plan, unchecked) = plan_of(self.camera, &self.photos, &self.gate);
        let mut ops = Vec::new();
        if self.apply && plan.checking == 0 {
            let by_id: HashMap<PhotoId, &Arc<Photo>> = self.photos.iter().map(|p| (p.id, p)).collect();
            for id in &plan.photos {
                let Some(ph) = by_id.get(id) else { continue };
                let mut d = (*ph.develop).clone();
                d.profile.camera_colour = CameraColour::Macos;
                d.profile.camera_profile = None;
                let settings = Arc::new(d);
                let step = lightcraft_catalog::HistoryStep { label: LABEL.into(), settings: settings.clone() };
                let op = Op::Batch {
                    ops: vec![
                        Op::SetDevelop { id: *id, settings, label: LABEL.into(), edited: Some(self.edited.clone()) },
                        Op::PushHistory { id: *id, step },
                    ],
                };
                ops.push((*id, ph.develop.clone(), op));
            }
        }
        Done { apply: self.apply, plan, unchecked, gate: self.gate, ops }
    }
}

/// Land a finished job: the count's answer, or the move committed as one undo step (`changed` counts the photos that
/// moved; one edited meanwhile, or gone, is left as it is). Sizes still unchecked start their check, and a move then
/// fails with [`CHECKING`] as the command does.
pub fn finish(s: &mut Session, d: Done) -> Result<Value, String> {
    crate::import::check_macos_later(&s.activity, d.gate, d.unchecked);
    let mut out = d.plan.json();
    if !d.apply {
        return Ok(out);
    }
    if d.plan.checking > 0 {
        return Err(CHECKING.into());
    }
    let ops: Vec<Op> = d
        .ops
        .into_iter()
        .filter(|(id, before, _)| s.catalog.photo(*id).is_some_and(|p| Arc::ptr_eq(&p.develop, before)))
        .map(|(_, _, op)| op)
        .collect();
    let changed = ops.len();
    if changed > 0 {
        s.commit(LABEL, Op::Batch { ops }).map_err(|e| e.to_string())?;
    }
    if let Some(o) = out.as_object_mut() {
        o.remove("photos");
        o.insert("changed".into(), json!(changed));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use lightcraft_catalog::{Op, PhotoId};
    use lightcraft_develop::CameraColour;
    use serde_json::json;

    use crate::Session;
    use crate::import::starting_colour_tests::{checked, mac, raw};

    fn colour(s: &Session, id: u64) -> CameraColour {
        s.catalog.photo(PhotoId(id)).unwrap().develop.profile.camera_colour
    }

    /// The count and the move run on another thread with this thread's decoder, give the commands' answers, and the
    /// move lands as one undo step; a photo edited while the move ran keeps its edit.
    #[test]
    fn the_count_and_the_move_run_off_the_ui_thread() {
        let (_, _g) = mac(true, true, Some((7008, 4672)));
        let mut s = Session::new();
        for i in 1..=3 {
            s.catalog.apply(Op::AddPhoto { photo: Box::new(raw(i, "SONY ILCE-7M5")) }).unwrap();
        }
        s.catalog.apply(Op::AddPhoto { photo: Box::new(raw(4, "SONY ILCE-7M4")) }).unwrap();
        let on_worker = |job: super::Job| std::thread::spawn(move || job.run()).join().unwrap();

        let done = on_worker(super::count(&s, &json!({"photo": 1})).unwrap());
        assert_eq!(super::finish(&mut s, done).unwrap()["checking"].as_u64(), Some(3), "the sizes are checked first");
        let done = on_worker(super::apply(&s, &json!({"photo": 1})).unwrap());
        assert!(super::finish(&mut s, done).is_err_and(|e| e == crate::cmd::macos_colour::CHECKING), "a move waits as the command does");
        checked(&s);
        let done = on_worker(super::count(&s, &json!({"photo": 1})).unwrap());
        let n = super::finish(&mut s, done).unwrap();
        assert_eq!(n, s.execute("profiles.macosForCameraCount", &json!({"photo": 1})).unwrap(), "the command's answer");
        assert_eq!(n["photos"].as_u64(), Some(3));

        let job = super::apply(&s, &json!({"photo": 1})).unwrap();
        // photo 3 is edited while the move runs
        let mut d = (*s.develop_of(PhotoId(3)).unwrap()).clone();
        d.light.exposure = 1.0;
        s.set_develop(PhotoId(3), d, "Exposure").unwrap();
        let undo = s.undo.len();
        let r = super::finish(&mut s, on_worker(job)).unwrap();
        assert_eq!(r["changed"].as_u64(), Some(2), "{r}");
        assert_eq!(s.undo.len(), undo + 1, "one undo step");
        assert_eq!(
            (colour(&s, 1), colour(&s, 2), colour(&s, 3), colour(&s, 4)),
            (CameraColour::Macos, CameraColour::Macos, CameraColour::Auto, CameraColour::Auto)
        );
        assert_eq!(s.develop_of(PhotoId(3)).unwrap().light.exposure, 1.0, "the edit made meanwhile stays");
        s.execute("edit.undo", &json!({})).unwrap();
        assert!((1..=2).all(|i| colour(&s, i) == CameraColour::Auto), "undone in one step");
    }
}
