//! `retouch.*`: portrait retouching (fork feature, `fork/retouch-foundation.md`).
//!
//! Every value is also a develop control (`retouch.<feature>.<field>` for every face, `retouchFace.<i>.…` for the
//! `i`-th face with its own settings), so `develop.set` and `develop.controls` reach them too; these commands add
//! the faces: which were found, and setting a value for one of them.

use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_active, index_req, photo_id, str_param};
use crate::{Result, Session};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::{FEATURES, Face};

fn target(s: &Session, p: &Value, c: &str) -> Result<PhotoId> {
    let id = photo_id(p, c)?.or_else(|| s.active()).ok_or_else(|| bad(c, "no photo"))?;
    s.catalog.photo(id).map(|_| id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))
}

/// `face`: a face index (of `retouch.faces`), or absent / null for every face.
fn face_param(p: &Value, c: &str) -> Result<Option<usize>> {
    match p.get("face") {
        None | Some(Value::Null) => Ok(None),
        Some(_) => index_req(p, "face", c).map(Some),
    }
}

/// The faces of `id` as found, for a face index a command was given.
fn found_faces(s: &Session, id: PhotoId, c: &str) -> Result<Vec<Face>> {
    let st = s.retouch_faces_of(id);
    match st.found {
        Some(f) => Ok(f.input.faces.clone()),
        None if st.busy => Err(bad(c, "the photo's faces are still being found: try again in a moment")),
        None => Err(bad(c, "the photo's faces are not known yet: run retouch.detect first")),
    }
}

fn faces_json(s: &Session, id: PhotoId) -> Value {
    let st = s.retouch_faces_of(id);
    let d = s.develop_of(id).unwrap_or_default();
    let faces: Vec<Face> = st.found.as_ref().map(|f| f.input.faces.clone()).unwrap_or_default();
    let own = lightcraft_develop::retouch::overrides::resolve(&d.retouch, &faces);
    let list: Vec<Value> = faces
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let b = f.bounds;
            json!({
                "index": i,
                "bounds": [b.x0, b.y0, b.x1, b.y1],
                "landmarks": f.landmarks.iter().map(|(x, y)| json!([x, y])).collect::<Vec<_>>(),
                "score": f.score,
                "person": f.person,
                "masks": f.masks.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
                // the index of its own settings in `retouchFace.<i>.…`, when it has them
                "own": own.get(i).copied().flatten(),
            })
        })
        .collect();
    json!({"id": id.0, "found": st.found.is_some(), "busy": st.busy, "error": st.error, "faces": list})
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "retouch.faces",
            "List Portrait Faces",
            [],
            None,
            "{id?} → {id, found, busy, error?, faces: [{index, bounds: [x0, y0, x1, y1], landmarks: [[x, y]…], score, person?, masks: [part…], own?}]} — the faces portrait retouching found on the photo (fractions of the photo before its crop); `own` is the index of the face's own settings (`retouchFace.<own>.…`). Starts nothing: see retouch.detect",
            has_active,
            |s, p| {
                let id = target(s, p, "retouch.faces")?;
                Ok(faces_json(s, id))
            }
        ),
        cmd!(
            query "retouch.detect",
            "Find Portrait Faces",
            [],
            None,
            "{id?, again?: bool} → as retouch.faces — find the photo's faces now (YuNet from Settings ▸ Faces, or the installed people parser); `again` searches anew. Renders of a retouched photo find them by themselves, in the background",
            has_active,
            |s, p| {
                let c = "retouch.detect";
                let id = target(s, p, c)?;
                let again = p.get("again").and_then(Value::as_bool).unwrap_or(false);
                s.retouch_detect_now(id, again)?;
                Ok(faces_json(s, id))
            }
        ),
        cmd!(
            "retouch.set",
            "Set Portrait Value",
            [],
            None,
            "{control: \"retouch.<feature>.<field>\", value, face?: index, id?} — set a portrait value for every face, or (with `face`, an index of retouch.faces) for that face alone: it then gets its own settings, starting from those of every face, kept by its person (named faces) or its place",
            has_active,
            |s, p| {
                let c = "retouch.set";
                let id = target(s, p, c)?;
                let control = str_param(p, "control").ok_or_else(|| bad(c, "missing `control`"))?;
                let value = p.get("value").and_then(Value::as_f64).ok_or_else(|| bad(c, "missing number `value`"))?;
                let spec = lightcraft_develop::retouch::find(control)
                    .filter(|_| control.starts_with("retouch."))
                    .ok_or_else(|| bad(c, format!("unknown portrait control `{control}` (see develop.controls, section portrait)")))?;
                let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
                match face_param(p, c)? {
                    None => {
                        d.retouch.global.set(control, value);
                    }
                    Some(i) => {
                        let faces = found_faces(s, id, c)?;
                        if i >= faces.len() {
                            return Err(bad(c, format!("no face {i}: the photo has {}", faces.len())));
                        }
                        let own = d.retouch.override_mut(&faces, i).ok_or_else(|| bad(c, "too many faces have their own settings"))?;
                        own.set(control, value);
                    }
                }
                s.set_develop(id, d, spec.label)?;
                Ok(Value::Null)
            }
        ),
        cmd!(
            "retouch.resetFace",
            "Reset Face to All Faces",
            [],
            None,
            "{face: index, id?} — the face follows the settings for every face again (its own are removed)",
            has_active,
            |s, p| {
                let c = "retouch.resetFace";
                let id = target(s, p, c)?;
                let i = index_req(p, "face", c)?;
                let faces = found_faces(s, id, c)?;
                let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
                if !d.retouch.remove_override(&faces, i) {
                    return Ok(json!({"removed": false}));
                }
                s.set_develop(id, d, "Reset Face")?;
                Ok(json!({"removed": true}))
            }
        ),
        cmd!(
            "retouch.reset",
            "Reset Portrait",
            [],
            None,
            "{feature?: key, face?: index, id?} — reset portrait retouching: everything (every face's own settings too), or one feature (`skin`, `eyes`…) for every face or for one face",
            has_active,
            |s, p| {
                let c = "retouch.reset";
                let id = target(s, p, c)?;
                let feature = str_param(p, "feature");
                if let Some(f) = feature
                    && !FEATURES.iter().any(|x| x.key == f)
                {
                    let keys: Vec<&str> = FEATURES.iter().map(|x| x.key).collect();
                    return Err(bad(c, format!("unknown feature `{f}` (one of {})", keys.join(", "))));
                }
                let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
                match (feature, face_param(p, c)?) {
                    (None, None) => d.retouch = Default::default(),
                    (Some(f), None) => {
                        d.retouch.global.reset_feature(f);
                    }
                    (f, Some(i)) => {
                        let faces = found_faces(s, id, c)?;
                        let own = lightcraft_develop::retouch::overrides::resolve(&d.retouch, &faces).get(i).copied().flatten();
                        let Some(own) = own.and_then(|o| d.retouch.faces.get_mut(o)) else { return Ok(Value::Null) };
                        match f {
                            Some(f) => {
                                own.settings.reset_feature(f);
                            }
                            None => own.settings = Default::default(),
                        }
                    }
                }
                s.set_develop(id, d, "Reset Portrait")?;
                Ok(Value::Null)
            }
        ),
    ]
}
