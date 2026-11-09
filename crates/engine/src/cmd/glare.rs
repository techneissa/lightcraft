//! `retouch.glare*`: Portrait ▸ Glasses Glare's Repair Glare (fork feature, `fork/glare-repair.md`, issue #74).

use serde_json::{Value, json};

use super::retouch::{as_map, face_param, found_faces, target};
use super::{CommandSpec, bad, cmd, has_active, str_param};
use crate::glare_repair::{Job, Method, lens};
use crate::{Result, Session};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::{Face, Input, Part};

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
        None => Ok(if s.glare.weights().is_ok() { Method::Flux } else { Method::Lama }),
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

/// Turn the Amount of the faces being repaired up to 100 where it is 0, so the repair shows when it lands.
fn show_repair(s: &mut Session, id: PhotoId, input: &Input, face: Option<usize>) -> Result<()> {
    const AMOUNT: &str = "retouch.glare.amount";
    let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
    let keys = s.retouch_key_map(id);
    let now = match face {
        None => d.retouch.global.get(AMOUNT),
        Some(i) => d.retouch.settings_for_in(&input.faces, i, as_map(&keys)).get(AMOUNT),
    };
    if now.is_some_and(|v| v > 0.0) {
        return Ok(());
    }
    match face {
        None => {
            d.retouch.global.set(AMOUNT, 100.0);
        }
        Some(i) => {
            let Some(own) = d.retouch.override_mut_in(&input.faces, i, as_map(&keys)) else { return Ok(()) };
            own.set(AMOUNT, 100.0);
        }
    }
    s.set_develop(id, d, "Glasses Glare")
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
            "{face?: index, method?: \"flux\"|\"lama\", lenses?: \"auto\"|\"whole\"|\"glints\", id?} — repaint the glasses of one face (an index of retouch.faces) or of every face with glasses, on a worker with a row in the activity stack (✕ stops it). `flux` (FLUX.1 Fill, minutes a face) repaints whole lenses where glare hides the eyes; `lama` (seconds) fills the glints only. `lenses` (flux): `auto` repaints a whole lens where glare covers enough of it to hide the eye, `whole` every lens, `glints` the glints only. Default: flux when its files are in place. Glasses Glare's Amount shows the result (set to 100 when it was 0) → {started: true, faces, method}",
            has_active,
            |s, p| {
                let c = "retouch.glareRepair";
                let id = target(s, p, c)?;
                let method = method_param(s, p, c)?;
                let lenses = match str_param(p, "lenses") {
                    Some(l) => lens::Lenses::from_name(l).ok_or_else(|| bad(c, "`lenses` must be \"auto\", \"whole\" or \"glints\""))?,
                    None => lens::Lenses::Auto,
                };
                match method {
                    Method::Flux => {
                        s.glare.weights().map_err(|e| bad(c, format!("FLUX.1 Fill is not installed: {e}")))?;
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
                show_repair(s, id, &input, face)?;
                let indices: Vec<usize> = faces.iter().map(|(i, _)| *i).collect();
                let store = s.retouch_faces.clone();
                let done_key = key.clone();
                let job = Job {
                    photo: id,
                    key,
                    render,
                    orient: photo.develop.orientation,
                    faces,
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
            "{id?} → {flux: {installed, folder, error?}, lama: {installed}, running?: {photo, faces, method}, message?, repaired: [{face, method}]} — what Repair Glare can use, what runs, and which faces of the photo have a repair",
            has_active,
            |s, p| {
                let id = target(s, p, "retouch.glareStatus")?;
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
}
