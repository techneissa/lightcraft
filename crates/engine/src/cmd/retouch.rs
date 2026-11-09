//! `retouch.*`: portrait retouching (fork feature, `fork/retouch-foundation.md`).
//!
//! Every value is also a develop control (`retouch.<feature>.<field>` for every face, `retouchFace.<i>.…` for the
//! `i`-th face with its own settings), so `develop.set` and `develop.controls` reach them too; these commands add
//! the faces: which were found, and setting a value for one of them. `retouch.refine.*` paints and clears the Refine
//! brush's strokes (fork `fork/refine-brush.md`, issue #99).

use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_active, index_req, photo_id, str_param};
use crate::{Result, Session};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::input::{Searched, Strands};
use lightcraft_develop::retouch::{FEATURES, Face, KeyMap, Refine, RefineMode, RefineStroke};
use lightcraft_pipeline::retouch_step::FrameKeyMap;

/// `keys` as the matcher takes it.
pub(super) fn as_map(keys: &Option<FrameKeyMap>) -> Option<&dyn KeyMap> {
    keys.as_ref().map(|k| k as &dyn KeyMap)
}

pub(super) fn target(s: &Session, p: &Value, c: &str) -> Result<PhotoId> {
    let id = photo_id(p, c)?.or_else(|| s.active()).ok_or_else(|| bad(c, "no photo"))?;
    s.catalog.photo(id).map(|_| id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))
}

/// `face`: a face index (of `retouch.faces`), or absent / null for every face.
pub(super) fn face_param(p: &Value, c: &str) -> Result<Option<usize>> {
    match p.get("face") {
        None | Some(Value::Null) => Ok(None),
        Some(_) => index_req(p, "face", c).map(Some),
    }
}

/// The faces of `id` as found, for a face index a command was given.
pub(super) fn found_faces(s: &Session, id: PhotoId, c: &str) -> Result<std::sync::Arc<lightcraft_develop::retouch::Input>> {
    let st = s.retouch_faces_of(id);
    match st.found {
        Some(f) => Ok(f.input),
        None if st.busy => Err(bad(c, "the photo's faces are still being found: try again in a moment")),
        None => Err(bad(c, "the photo's faces are not known yet: run retouch.detect first")),
    }
}

/// Face `i` of `input`, or a clear error: with no face in the photo (a landscape) it says so, else how many there are.
pub(super) fn check_face(input: &lightcraft_develop::retouch::Input, i: usize, c: &str) -> Result<()> {
    match input.faces.len() {
        n if i < n => Ok(()),
        0 => Err(bad(c, format!("no face {i}: the photo has no people (retouch.faces lists no face)"))),
        n => Err(bad(c, format!("no face {i}: the photo has {n}"))),
    }
}

/// How a face's Stray Hair search went, for `retouch.faces`: `status` is `searched` (`count` strands found, in `ms`
/// milliseconds, on a face `facePx` pixels tall in the searched picture), `tooSmall`, `noParts` (no Hair or Facial
/// Skin mask) or `notSearched` (the face came without the people parse, so there is nothing to search with).
fn strands_json(s: Option<&Strands>) -> Value {
    let Some(s) = s else { return json!({"status": "notSearched"}) };
    match s.searched {
        Searched::Yes => json!({"status": "searched", "count": s.count, "ms": s.ms, "cropMs": s.crop_ms, "facePx": s.face_px}),
        Searched::TooSmall => json!({"status": "tooSmall", "facePx": s.face_px}),
        Searched::NoParts => json!({"status": "noParts", "facePx": s.face_px}),
    }
}

fn faces_json(s: &Session, id: PhotoId) -> Value {
    let st = s.retouch_faces_of(id);
    let d = s.develop_of(id).unwrap_or_default();
    let input = st.found.as_ref().map(|f| f.input.clone()).unwrap_or_default();
    let faces: &[Face] = &input.faces;
    let keys = s.retouch_key_map(id);
    let own = lightcraft_develop::retouch::overrides::resolve_in(&d.retouch, faces, as_map(&keys));
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
                "strands": strands_json(f.strands.as_ref()),
            })
        })
        .collect();
    json!({"id": id.0, "found": st.found.is_some(), "noPeople": st.no_people(), "nobodyHint": st.hint, "busy": st.busy, "error": st.error, "faces": list})
}

/// `feature`: a feature's key, required.
fn feature_param(p: &Value, c: &str) -> Result<&'static lightcraft_develop::retouch::FeatureInfo> {
    let keys = || FEATURES.iter().map(|x| x.key).collect::<Vec<_>>().join(", ");
    let f = str_param(p, "feature").ok_or_else(|| bad(c, format!("missing `feature` (one of {})", keys())))?;
    FEATURES.iter().find(|x| x.key == f).ok_or_else(|| bad(c, format!("unknown feature `{f}` (one of {})", keys())))
}

/// A stroke from `retouch.refine.stroke`'s parameters.
fn stroke_param(p: &Value, c: &str, add_ok: bool) -> Result<RefineStroke> {
    let mode = match str_param(p, "mode") {
        None => RefineMode::Reduce,
        Some(m) => RefineMode::from_name(m).ok_or_else(|| bad(c, format!("`mode` is reduce, restore or add, not `{m}`")))?,
    };
    if mode == RefineMode::Add && !add_ok {
        return Err(bad(c, "this feature reads no mask, so there is nothing for Add to widen"));
    }
    let points: Vec<(f64, f64)> = p
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(c, "missing `points` ([[x, y]…], fractions of the photo before its crop)"))?
        .iter()
        .map(|q| match (q.get(0).and_then(Value::as_f64), q.get(1).and_then(Value::as_f64)) {
            (Some(x), Some(y)) => Ok((x, y)),
            _ => Err(bad(c, format!("a point is [x, y], not {q}"))),
        })
        .collect::<Result<_>>()?;
    let num = |k: &str, d: f64| -> Result<f64> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(d),
            Some(v) => v.as_f64().filter(|v| v.is_finite()).ok_or_else(|| bad(c, format!("`{k}` must be a number"))),
        }
    };
    let d = RefineStroke::default();
    let pressure =
        p.get("pressure").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).map(|v| v as f32).collect()).unwrap_or_default();
    let s = RefineStroke {
        mode,
        points,
        size: num("size", d.size)?,
        feather: num("feather", d.feather)?,
        flow: num("flow", d.flow)?,
        strength: num("strength", d.strength)?,
        pressure,
    };
    s.sanitized().ok_or_else(|| bad(c, "the stroke has no point on the photo"))
}

/// An Add stroke works on or near a face: refused with no face in the photo, or when the face it is for (`face`, else
/// the nearest) is further than [`lightcraft_develop::retouch::refine::ADD_REACH`] face sizes away.
fn check_add_near_face(s: &Session, id: PhotoId, face: Option<usize>, stroke: &RefineStroke, c: &str) -> Result<()> {
    use lightcraft_develop::retouch::refine::{ADD_REACH, add_distance, stroke_centre};
    const SAY: &str = "Add works on or near a face";
    let input = found_faces(s, id, c)?;
    if input.faces.is_empty() {
        return Err(bad(c, format!("{SAY}: the photo has no face")));
    }
    let Some((cx, cy)) = stroke_centre(stroke) else { return Err(bad(c, "the stroke has no point on the photo")) };
    // (the faces' boxes are in the frame before lens corrections and transform when they come from a people parse)
    let keys = s.retouch_key_map(id);
    let centre = match (input.on_photo, as_map(&keys)) {
        (true, Some(m)) => m.edit_to_photo(cx, cy).unwrap_or((cx, cy)),
        _ => (cx, cy),
    };
    let distance = |i: usize| input.faces.get(i).map_or(f64::INFINITY, |f| add_distance(f, centre));
    let d = match face {
        Some(i) => distance(i),
        None => (0..input.faces.len()).map(distance).fold(f64::INFINITY, f64::min),
    };
    if d > ADD_REACH {
        return Err(bad(c, format!("{SAY}: the stroke is {d:.1} face sizes from {}", if face.is_some() { "that face" } else { "the nearest face" })));
    }
    Ok(())
}

/// The Refine strokes of `face` (its own, made when it has none yet) or, without one, those for every face.
fn refine_mut<'a>(s: &Session, d: &'a mut lightcraft_develop::DevelopSettings, id: PhotoId, face: Option<usize>, c: &str) -> Result<&'a mut Refine> {
    match face {
        None => Ok(&mut d.retouch.refine),
        Some(i) => {
            let input = found_faces(s, id, c)?;
            check_face(&input, i, c)?;
            let keys = s.retouch_key_map(id);
            let own = d.retouch.own_mut_in(&input.faces, i, as_map(&keys)).ok_or_else(|| bad(c, "too many faces have their own settings"))?;
            Ok(&mut own.refine)
        }
    }
}

/// The strokes `face` renders with (its own, else those for every face), read only.
fn refine_of(s: &Session, d: &lightcraft_develop::DevelopSettings, id: PhotoId, face: Option<usize>, c: &str) -> Result<Refine> {
    let Some(i) = face else { return Ok(d.retouch.refine.clone()) };
    let input = found_faces(s, id, c)?;
    // (a face past the last one reads the strokes for every face, as it always has; but a photo whose search found
    // nobody has no face to ask about)
    if input.faces.is_empty() {
        check_face(&input, i, c)?;
    }
    let keys = s.retouch_key_map(id);
    let own = lightcraft_develop::retouch::overrides::resolve_in(&d.retouch, &input.faces, as_map(&keys)).get(i).copied().flatten();
    Ok(own.and_then(|o| d.retouch.faces.get(o)).map_or_else(|| d.retouch.refine.clone(), |o| o.refine.clone()))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "retouch.faces",
            "List Portrait Faces",
            [],
            None,
            "{id?} → {id, found, noPeople, nobodyHint, busy, error?, faces: [{index, bounds: [x0, y0, x1, y1], landmarks: [[x, y]…], score, person?, masks: [part…], own?, strands: {status: searched|tooSmall|noParts|notSearched, count?, ms?, cropMs?, facePx?}}]} — the faces portrait retouching found on the photo (fractions of the photo before its crop); `own` is the index of the face's own settings (`retouchFace.<own>.…`); `strands` is how the Stray Hair search went for it (`count` strands found, in `ms` milliseconds after `cropMs` to cut the head's full-size crop, on a face `facePx` pixels tall; `notSearched`: the faces came without the people parse). `noPeople` is true once the search found no face, or the early check saw none when it has not searched (`nobodyHint`: a hint only, renders and exports ignore it; the Portrait icon and tab are then dimmed, and a command that needs a face, such as `face` on retouch.set, answers with an error once a search found none). See retouch.search. Starts nothing: see retouch.detect",
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
            "retouch.search",
            "Search Portrait Faces",
            [],
            None,
            "{id?, again?: bool} → as retouch.faces — start the real face search of the photo in the background (the Portrait tab's \"Search Again\": `again` searches anew, ignoring what was found or hinted before), with one \"Finding Faces\" row in the activity stack; read the result with retouch.faces. Answers with the reason when no face model is installed",
            has_active,
            |s, p| {
                let c = "retouch.search";
                let id = target(s, p, c)?;
                let again = p.get("again").and_then(Value::as_bool).unwrap_or(false);
                if let Some(why) = s.retouch_search(id, again) {
                    return Err(bad(c, why));
                }
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
                        let input = found_faces(s, id, c)?;
                        check_face(&input, i, c)?;
                        let keys = s.retouch_key_map(id);
                        let own = d
                            .retouch
                            .override_mut_in(&input.faces, i, as_map(&keys))
                            .ok_or_else(|| bad(c, "too many faces have their own settings"))?;
                        own.set(control, value);
                    }
                }
                s.set_develop(id, d, spec.label)?;
                Ok(Value::Null)
            }
        ),
        cmd!(
            "retouch.refine.stroke",
            "Refine Portrait Feature",
            [],
            None,
            "{feature: key, mode?: reduce|restore|add, points: [[x, y]…], size?: radius (fraction of the long edge), feather?: 0–100, flow?: 1–100, strength?: 0–100, pressure?: [0–1…], face?: index, id?} → {strokes} — paint a Refine stroke for one portrait feature (points are fractions of the photo before its crop, as Remove spots): Reduce weakens the feature there (none at strength 100), Restore brings back its automatic result, Add applies it where its mask missed (features that read a mask), on or near a face (within one face size of its box; with no face in the photo it errors). For every face, or (with `face`) that face alone, which then gets its own settings. One undo step",
            has_active,
            |s, p| {
                let c = "retouch.refine.stroke";
                let id = target(s, p, c)?;
                let f = feature_param(p, c)?;
                if f.stage == lightcraft_develop::retouch::Stage::Geometry {
                    // (it moves pixels in the geometry step, which the Refine brush does not wrap yet)
                    return Err(bad(c, format!("{} has no Refine brush yet", f.label)));
                }
                let stroke = stroke_param(p, c, !f.add_parts.is_empty())?;
                let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
                let face = face_param(p, c)?;
                if stroke.mode == RefineMode::Add {
                    check_add_near_face(s, id, face, &stroke, c)?;
                }
                let refine = refine_mut(s, &mut d, id, face, c)?;
                if !refine.push(f.key, stroke) {
                    return Err(bad(c, format!("{} has the most strokes it keeps ({})", f.label, lightcraft_develop::retouch::refine::MAX_STROKES)));
                }
                let n = refine.count(Some(f.key));
                lightcraft_develop::retouch::refine::check_budget(&d.retouch).map_err(|e| bad(c, e))?;
                s.set_develop(id, d, &format!("Refine {}", f.label))?;
                Ok(json!({"strokes": n}))
            }
        ),
        cmd!(
            "retouch.refine.clear",
            "Clear Refine Strokes",
            [],
            None,
            "{feature?: key, face?: index, id?} → {removed: bool} — remove the Refine strokes of one feature (every feature without `feature`), for every face or (with `face`) that face's own. One undo step",
            has_active,
            |s, p| {
                let c = "retouch.refine.clear";
                let id = target(s, p, c)?;
                let f = match p.get("feature") {
                    None | Some(Value::Null) => None,
                    Some(_) => Some(feature_param(p, c)?),
                };
                let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
                let face = face_param(p, c)?;
                if refine_of(s, &d, id, face, c)?.count(f.map(|f| f.key)) == 0 {
                    return Ok(json!({"removed": false}));
                }
                refine_mut(s, &mut d, id, face, c)?.clear(f.map(|f| f.key));
                let label = f.map_or_else(|| "Clear Refine Strokes".to_string(), |f| format!("Clear Refine {}", f.label));
                s.set_develop(id, d, &label)?;
                Ok(json!({"removed": true}))
            }
        ),
        cmd!(
            query "retouch.refine.list",
            "List Refine Strokes",
            [],
            None,
            "{face?: index, id?} → {features: [{feature, strokes: [{mode, points, size, feather, flow, strength, pressure?}]}]} — the Refine strokes the photo renders with, for every face or (with `face`) for that face",
            has_active,
            |s, p| {
                let c = "retouch.refine.list";
                let id = target(s, p, c)?;
                let d = s.develop_of(id).unwrap_or_default();
                let r = refine_of(s, &d, id, face_param(p, c)?, c)?;
                Ok(json!({"features": r.0}))
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
                let input = found_faces(s, id, c)?;
                check_face(&input, i, c)?;
                let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
                let keys = s.retouch_key_map(id);
                if !d.retouch.remove_override_in(&input.faces, i, as_map(&keys)) {
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
            "{feature?: key, face?: index, id?} — reset portrait retouching: everything (every face's own settings too), or one feature (`skin`, `eyes`…) for every face or for one face; its Refine strokes go too",
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
                        d.retouch.refine.clear(Some(f));
                    }
                    (f, Some(i)) => {
                        let input = found_faces(s, id, c)?;
                        check_face(&input, i, c)?;
                        let keys = s.retouch_key_map(id);
                        let own =
                            lightcraft_develop::retouch::overrides::resolve_in(&d.retouch, &input.faces, as_map(&keys)).get(i).copied().flatten();
                        let Some(own) = own.and_then(|o| d.retouch.faces.get_mut(o)) else { return Ok(Value::Null) };
                        match f {
                            Some(f) => {
                                own.settings.reset_feature(f);
                                own.refine.clear(Some(f));
                            }
                            None => {
                                own.settings = Default::default();
                                own.refine = Default::default();
                            }
                        }
                    }
                }
                s.set_develop(id, d, "Reset Portrait")?;
                Ok(Value::Null)
            }
        ),
    ]
}
