//! `retouch.glare*`: Portrait ▸ Glasses Glare's Repair Glare (fork feature, `fork/glare-repair.md`, issue #74).

use serde_json::{Value, json};

use super::retouch::{as_map, face_param, found_faces, target};
use super::{CommandSpec, bad, cmd, has_active, str_param};
use crate::glare_repair::store;
use crate::glare_repair::{Finished, Job, Method, lens};
use crate::{Result, Session};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::{Face, Input, KeyMap, Part, Retouch};

/// The long edge the photo is rendered at for a repair: enough that the crop around the glasses of the face with
/// the eyes closest together is at least [`lens::SIDE`] pixels, within 1536…8192.
fn render_edge(faces: &[(usize, Face)]) -> usize {
    let eyes = faces
        .iter()
        .filter_map(|(_, f)| {
            let (r, l) = (f.landmark(0)?, f.landmark(1)?);
            Some((r.0 - l.0).abs().max((r.1 - l.1).abs()))
        })
        .filter(|d| d.is_finite() && *d > 0.0)
        .fold(f64::INFINITY, f64::min);
    let want = lens::SIDE as f64 / (2.6 * eyes);
    if eyes.is_finite() && want.is_finite() { want.ceil().clamp(1536.0, 8192.0) as usize } else { 4096 }
}

fn method_param(s: &Session, p: &Value, c: &str) -> Result<Method> {
    match str_param(p, "method") {
        Some(m) => Method::from_name(m).ok_or_else(|| bad(c, "`method` must be \"flux\" or \"lama\"")),
        // (FLUX when its files are in place and the Mac has the memory for it; else the glints)
        None => Ok(if s.glare.weights().is_ok() && s.glare.memory_check().is_none() { Method::Flux } else { Method::Lama }),
    }
}

/// The faces to repair: face `i`, or every face with a glasses (Others) mask.
fn targets(input: &Input, face: Option<usize>, c: &str) -> Result<Vec<(usize, Face)>> {
    if !input.on_photo {
        return Err(bad(c, "Repair Glare needs the people parts (the glasses mask): download a people parts set in Settings ▸ People"));
    }
    let with_glasses = |f: &Face| f.mask(Part::Others).is_some() && f.landmark(0).is_some() && f.landmark(1).is_some();
    let list: Vec<(usize, Face)> = match face {
        Some(i) => {
            let f = input.faces.get(i).ok_or_else(|| bad(c, format!("no face {i}: the photo has {}", input.faces.len())))?;
            if !with_glasses(f) {
                return Err(bad(c, format!("face {} has no glasses the people parts found", i + 1)));
            }
            vec![(i, f.clone())]
        }
        None => input.faces.iter().enumerate().filter(|(_, f)| with_glasses(f)).map(|(i, f)| (i, f.clone())).collect(),
    };
    if list.is_empty() {
        return Err(bad(c, "no face with glasses found"));
    }
    Ok(list)
}

const AMOUNT: &str = "retouch.glare.amount";

/// Turns Glasses Glare's Amount up to 100 for the faces `targets` (indices of `faces`) that don't show a repair
/// (Amount 0, or none). For every face (`all`) the global Amount is the one turned up when it is 0; a face with its
/// own override at 0 gets its own Amount turned up too. Whether anything changed.
fn show_amounts(retouch: &mut Retouch, faces: &[Face], targets: &[usize], all: bool, map: Option<&dyn KeyMap>) -> bool {
    let shown = |r: &Retouch, i: usize| r.settings_for_in(faces, i, map).get(AMOUNT).is_some_and(|v| v > 0.0);
    let mut changed = false;
    if all && targets.iter().any(|i| !shown(retouch, *i)) && !retouch.global.get(AMOUNT).is_some_and(|v| v > 0.0) {
        retouch.global.set(AMOUNT, 100.0);
        changed = true;
    }
    for &i in targets {
        if !shown(retouch, i)
            && let Some(own) = retouch.override_mut_in(faces, i, map)
        {
            own.set(AMOUNT, 100.0);
            changed = true;
        }
    }
    changed
}

/// Shows a finished repair: its faces' Amount goes up to 100 where it hides it, as one undo step ("Glasses Glare").
fn show_repair(s: &mut Session, done: &Finished) -> Result<()> {
    let Some(found) = s.retouch_faces_of(done.photo).found else { return Ok(()) };
    let input = found.input;
    // the faces repaired, found again by their key boxes (their indices may have changed since the run began)
    let targets: Vec<usize> = done
        .key_boxes
        .iter()
        .filter_map(|b| {
            let best = input.faces.iter().enumerate().map(|(i, f)| (store::iou(&f.key_bounds(), b), i)).filter(|(o, _)| *o >= 0.5);
            best.max_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, i)| i)
        })
        .collect();
    if targets.is_empty() {
        return Ok(());
    }
    let keys = s.retouch_key_map(done.photo);
    let mut d = (*s.develop_of(done.photo).unwrap_or_default()).clone();
    if !show_amounts(&mut d.retouch, &input.faces, &targets, done.all_faces, as_map(&keys)) {
        return Ok(());
    }
    s.set_develop(done.photo, d, "Glasses Glare")
}

impl Session {
    /// Shows the repairs finished since the last call (the Amount of their faces goes up to 100 where it was 0, one
    /// undo step each). Called every frame by the window and by every `retouch.glare*` command; the number shown.
    /// A run that stopped before it kept anything, found no glare or failed to start changes nothing.
    pub fn poll_glare(&mut self) -> usize {
        let finished = self.glare.take_finished();
        let n = finished.len();
        for f in &finished {
            if let Err(e) = show_repair(self, f) {
                log::warn!("glare repair: the Amount could not be turned up: {e}");
            }
        }
        n
    }
}

fn status(s: &Session, id: PhotoId) -> Value {
    let weights = s.glare.weights();
    let repaired = s
        .catalog
        .photo(id)
        .map(|p| s.retouch_key(p, &p.develop))
        .zip(s.retouch_faces_of(id).found)
        .map(|(key, f)| {
            s.glare
                .repairs()
                .methods(&key, &f.input)
                .into_iter()
                .enumerate()
                .filter_map(|(i, m)| m.map(|m| json!({"face": i, "method": m.name()})))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "flux": {
            "installed": weights.is_ok(),
            "folder": s.glare.dir().map(|d| d.display().to_string()),
            "error": weights.err(),
            "memoryError": s.glare.memory_check(),
        },
        "lama": {"installed": s.lama.installed()},
        "running": s.glare.running().map(|r| json!({"photo": r.photo.0, "faces": r.faces, "method": r.method.name()})),
        "message": s.glare.message(),
        "repaired": repaired,
    })
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "retouch.glareRepair",
            "Repair Glare",
            [],
            None,
            "{face?: index, method?: \"flux\"|\"lama\", lenses?: \"auto\"|\"whole\"|\"glints\", id?} — repaint the glasses of one face (an index of retouch.faces) or of every face with glasses, on a worker with a row in the activity stack (✕ stops it). `flux` (FLUX.1 Fill, minutes a face) repaints whole lenses where glare hides the eyes; `lama` (seconds) fills the glints only. `lenses` (flux): `auto` repaints a whole lens where glare covers enough of it to hide the eye, `whole` every lens, `glints` the glints only. Default: flux when its files are in place. Glasses Glare's Amount shows the result (turned up to 100 once a repair is kept, when it was 0) → {started: true, faces, method}",
            has_active,
            |s, p| {
                let c = "retouch.glareRepair";
                s.poll_glare();
                let id = target(s, p, c)?;
                let method = method_param(s, p, c)?;
                let lenses = match str_param(p, "lenses") {
                    Some(l) => lens::Lenses::from_name(l).ok_or_else(|| bad(c, "`lenses` must be \"auto\", \"whole\" or \"glints\""))?,
                    None => lens::Lenses::Auto,
                };
                match method {
                    Method::Flux => {
                        s.glare.weights().map_err(|e| bad(c, format!("FLUX.1 Fill is not installed: {e}")))?;
                        if let Some(m) = s.glare.memory_check() {
                            return Err(bad(c, m));
                        }
                    }
                    Method::Lama if !s.lama.installed() => return Err(bad(c, "LaMa is not installed (lama.download)")),
                    Method::Lama => {}
                }
                if s.glare.running().is_some() {
                    return Err(bad(c, "a glare repair is already running"));
                }
                let input = found_faces(s, id, c)?;
                let face = face_param(p, c)?;
                let faces = targets(&input, face, c)?;
                let photo = s.catalog.photo(id).cloned().ok_or_else(|| bad(c, "no such photo"))?;
                let key = s.retouch_key(&photo, &photo.develop);
                let edge = render_edge(&faces);
                let render =
                    s.preview_job(id, edge, edge, false, &lightcraft_develop::DevelopSettings::default()).ok_or_else(|| bad(c, "no such photo"))?;
                let indices: Vec<usize> = faces.iter().map(|(i, _)| *i).collect();
                let store = s.retouch_faces.clone();
                let done_key = key.clone();
                let job = Job {
                    photo: id,
                    key,
                    render,
                    orient: photo.develop.orientation,
                    faces,
                    all_faces: face.is_none(),
                    method,
                    lenses,
                    lama: s.lama.clone(),
                    done: Box::new(move || store.reattach(&done_key)),
                };
                s.glare.start(job).map_err(|e| bad(c, e))?;
                Ok(json!({"started": true, "faces": indices, "method": method.name()}))
            }
        ),
        cmd!(
            query "retouch.glareStatus",
            "Glare Repair Status",
            [],
            None,
            "{id?} → {flux: {installed, folder, error?, memoryError?}, lama: {installed}, running?: {photo, faces, method}, message?, repaired: [{face, method}]} — what Repair Glare can use, what runs, and which faces of the photo have a repair",
            has_active,
            |s, p| {
                let id = target(s, p, "retouch.glareStatus")?;
                s.poll_glare();
                Ok(status(s, id))
            }
        ),
        cmd!(
            "retouch.glareClear",
            "Remove Glare Repair",
            [],
            None,
            "{face?: index, id?} — drop the glare repair of one face, or of every face of the photo",
            has_active,
            |s, p| {
                let c = "retouch.glareClear";
                if s.glare.running().is_some() {
                    return Err(bad(c, "Repair Running: wait for the glare repair to finish, or stop it, then remove it"));
                }
                let id = target(s, p, c)?;
                let photo = s.catalog.photo(id).cloned().ok_or_else(|| bad(c, "no such photo"))?;
                let key = s.retouch_key(&photo, &photo.develop);
                let key_box = match face_param(p, c)? {
                    Some(i) => {
                        let input = found_faces(s, id, c)?;
                        Some(input.faces.get(i).ok_or_else(|| bad(c, format!("no face {i}")))?.key_bounds())
                    }
                    None => None,
                };
                s.glare.repairs().clear(&key, key_box.as_ref()).map_err(|e| bad(c, e))?;
                s.retouch_faces.reattach(&key);
                Ok(Value::Null)
            }
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::retouch::NormRect;

    #[test]
    fn the_render_is_large_enough_for_a_768_crop_and_bounded() {
        let face = |d: f64| Face { landmarks: vec![(0.5, 0.5), (0.5 + d, 0.5)], ..Face::boxed(NormRect::new(0.4, 0.4, 0.7, 0.7)) };
        assert_eq!(render_edge(&[(0, face(0.1))]), 2954);
        assert_eq!(render_edge(&[(0, face(0.5))]), 1536);
        assert_eq!(render_edge(&[(0, face(0.001))]), 8192);
        assert_eq!(render_edge(&[(0, Face::default())]), 4096);
        assert_eq!(render_edge(&[(0, face(0.1)), (1, face(0.05))]), 5908);
    }

    #[test]
    fn faces_without_glasses_or_parts_are_refused() {
        let c = "t";
        let plain = Input::new(vec![Face::boxed(NormRect::new(0.1, 0.1, 0.2, 0.2))]);
        assert!(targets(&plain, None, c).is_err());
        let no_glasses = Input::new_on_photo(vec![Face::boxed(NormRect::new(0.1, 0.1, 0.2, 0.2))]);
        assert!(targets(&no_glasses, None, c).is_err());
        assert!(targets(&no_glasses, Some(3), c).is_err());
        let mask =
            lightcraft_develop::retouch::PartMask { plane: lightcraft_raster::Plane::filled(2, 2, 1.0), area: NormRect::new(0.1, 0.1, 0.2, 0.2) };
        let glasses =
            Face { landmarks: vec![(0.12, 0.15), (0.18, 0.15)], masks: vec![(Part::Others, mask)], ..Face::boxed(NormRect::new(0.1, 0.1, 0.2, 0.2)) };
        let two = Input::new_on_photo(vec![Face::boxed(NormRect::new(0.5, 0.1, 0.6, 0.2)), glasses]);
        assert_eq!(targets(&two, None, c).map(|v| v.iter().map(|(i, _)| *i).collect::<Vec<_>>()).ok(), Some(vec![1]));
        assert!(targets(&two, Some(0), c).is_err());
    }

    fn faces() -> Vec<Face> {
        vec![Face::boxed(NormRect::new(0.1, 0.1, 0.3, 0.4)), Face::boxed(NormRect::new(0.6, 0.1, 0.8, 0.4))]
    }

    fn amount(r: &Retouch, faces: &[Face], i: usize) -> Option<f64> {
        r.settings_for_in(faces, i, None).get(AMOUNT)
    }

    #[test]
    fn all_faces_turns_up_a_face_whose_own_amount_is_zero_too() {
        let faces = faces();
        let mut r = Retouch::default();
        // the global Amount is 0, and face 2 has its own override, also at 0
        assert!(r.override_mut_in(&faces, 1, None).is_some_and(|o| o.set(AMOUNT, 0.0)));
        assert_eq!(amount(&r, &faces, 0), Some(0.0));
        assert_eq!(amount(&r, &faces, 1), Some(0.0));
        assert!(show_amounts(&mut r, &faces, &[0, 1], true, None));
        assert_eq!(amount(&r, &faces, 0), Some(100.0), "the global Amount shows face 1");
        assert_eq!(amount(&r, &faces, 1), Some(100.0), "face 2's own Amount is turned up as well");
        // nothing to do now
        assert!(!show_amounts(&mut r, &faces, &[0, 1], true, None));
    }

    #[test]
    fn a_face_already_showing_the_repair_keeps_its_amount() {
        let faces = faces();
        let mut r = Retouch::default();
        assert!(r.global.set(AMOUNT, 40.0));
        assert!(!show_amounts(&mut r, &faces, &[0, 1], true, None));
        assert_eq!(amount(&r, &faces, 0), Some(40.0));
        // one face asked for: only that face gets an override, the other follows the global Amount
        let mut r = Retouch::default();
        assert!(show_amounts(&mut r, &faces, &[1], false, None));
        assert_eq!(amount(&r, &faces, 1), Some(100.0));
        assert_eq!(amount(&r, &faces, 0), Some(0.0));
    }

    #[test]
    fn nothing_kept_leaves_the_amount_and_the_history_alone() {
        let mut s = Session::with_demo();
        let id = s.active().unwrap();
        let before = (s.develop_of(id).unwrap(), s.undo.len());
        // a start that fails (no faces known), a poll with nothing finished, and a run noted for faces not found
        assert!(s.execute("retouch.glareRepair", &json!({"method": "lama"})).is_err());
        assert_eq!(s.poll_glare(), 0);
        s.glare.note_finished(Finished { photo: id, key_boxes: vec![NormRect::new(0.1, 0.1, 0.3, 0.4)], all_faces: true });
        assert_eq!(s.poll_glare(), 1);
        let after = (s.develop_of(id).unwrap(), s.undo.len());
        assert_eq!(before.1, after.1, "no undo step");
        assert_eq!(before.0, after.0, "no change of settings");
    }

    #[test]
    fn remove_repair_is_refused_while_a_repair_runs() {
        let mut s = Session::with_demo();
        s.glare.mark_running_for_test();
        let e = s.execute("retouch.glareClear", &json!({})).unwrap_err().to_string();
        assert!(e.contains("Repair Running"), "{e}");
    }
}
