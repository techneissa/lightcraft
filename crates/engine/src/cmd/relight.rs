//! `relight.*` (fork, issue #288, `fork/relight.md`): where a photo's Relight field stands, finding it again, and the
//! depth model's download; and (issue #299) adding, duplicating, deleting and selecting lights. A photo holds up to
//! eight lights. Each light's numbers are develop controls, `relight.<n>.amount`, `.softness`, `.warmth`, `.x`, `.y`,
//! `.depth`, `.wrap` and (issue #302) `.gelHue`, `.gelStrength`, `.pattern`, `.patternScale`, `.patternRotation`,
//! `.patternSoftness` and `.haze` (`n` from 0), and `relight.darken` is the photo's; `relight.amount` and the other shorthands act
//! on the selected light (see [`Session::relight_selected`]). They are set like every slider (`develop.set`,
//! `develop.merge`), so undo, presets, copy settings and sync reach them.

use serde_json::{Value, json};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::relight::{LightSettings, MAX_LIGHTS};
use lightcraft_develop::relight_effects::{GELS, LightEffects};

use super::{CommandSpec, always, bad, cmd, has_active, index_req, photo_id};
use crate::{Result, Session};

/// The depth model's licence.
const LICENSE: &str = "Apache-2.0";

fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.status";
    let id = photo_id(p, c)?.or_else(|| s.active()).ok_or_else(|| bad(c, "no photo"))?;
    s.catalog.photo(id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))?;
    let d = s.develop_of(id).unwrap_or_default();
    // (each light with every effect number, also the ones the saved form leaves out at their default)
    let lights: Vec<Value> = d
        .relight
        .lights
        .iter()
        .map(|l| {
            let mut v = serde_json::to_value(l).unwrap_or(Value::Null);
            if let (Some(o), Value::Object(e)) = (v.as_object_mut(), l.effects.to_full_json()) {
                o.extend(e);
            }
            v
        })
        .collect();
    let mut v = json!({"id": id.0, "on": lightcraft_pipeline::relight::scene_of(&d).is_some(), "lights": lights, "darken": d.relight.darken,
        "selected": s.relight_selected(id)});
    if let (Some(o), Value::Object(st)) = (v.as_object_mut(), s.relight_status(id)) {
        o.extend(st);
    }
    Ok(v)
}

fn find(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.findDepth";
    let id = photo_id(p, c)?.or_else(|| s.active()).ok_or_else(|| bad(c, "no photo"))?;
    let photo = s.catalog.photo(id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))?;
    if lightcraft_pipeline::relight::scene_of(&photo.develop).is_none() {
        return Err(bad(c, "Relight is off for this photo: raise Amount or Darken Surroundings first"));
    }
    s.relight_retry(id);
    Ok(s.relight_status(id))
}

fn download(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.downloadModel";
    if p.get("confirm").and_then(Value::as_bool) != Some(true) {
        let bytes = s.relight_model_status().get("bytes").and_then(Value::as_u64).unwrap_or(0);
        return Err(bad(
            c,
            format!(
                "the depth model ({}) is a {:.2} GB download ({LICENSE} licence): pass `confirm: true` once the user has agreed to download it",
                lightcraft_depth_label(),
                bytes as f64 / 1e9
            ),
        ));
    }
    let started = s.relight_download().map_err(|e| bad(c, e))?;
    Ok(json!({"started": started, "model": s.relight_model_status()}))
}

fn lightcraft_depth_label() -> &'static str {
    "Depth Anything 3 Mono Large"
}

impl Session {
    /// The index of the photo's selected light (UI state, never saved in the edit: the light a slider, a handle or
    /// the shorthand controls `relight.amount` … act on). The first light, when no light of this photo was picked,
    /// and never past the photo's last light.
    pub fn relight_selected(&self, id: PhotoId) -> usize {
        let last = self.develop_of(id).map_or(0, |d| d.relight.lights.len().saturating_sub(1));
        match self.relight_selection {
            (Some(p), i) if p == id => i.min(last),
            _ => 0,
        }
    }

    /// Select light `i` of the photo (clamped to the photo's lights).
    pub fn relight_select(&mut self, id: PhotoId, i: usize) {
        let last = self.develop_of(id).map_or(0, |d| d.relight.lights.len().saturating_sub(1));
        self.relight_selection = (Some(id), i.min(last));
    }
}

/// Where a new light goes: the first of a few spread-out spots that no light is near, else the first.
fn free_spot(lights: &[LightSettings]) -> (f64, f64) {
    const SPOTS: [(f64, f64); 8] = [(0.7, 0.3), (0.3, 0.65), (0.75, 0.65), (0.5, 0.2), (0.2, 0.3), (0.85, 0.45), (0.5, 0.8), (0.15, 0.75)];
    SPOTS.iter().copied().find(|(x, y)| lights.iter().all(|l| (l.x - x).hypot(l.y - y) > 0.12)).unwrap_or(SPOTS[0])
}

/// Change the active photo's lights with `f` (which returns the light to select) as one undo step labelled `label`.
fn change(s: &mut Session, c: &str, label: &str, f: impl FnOnce(&mut DevelopSettings, usize) -> Result<usize>) -> Result<Value> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let sel = s.relight_selected(id);
    let mut d = (*s.develop_of(id).ok_or_else(|| bad(c, "no photo"))?).clone();
    let picked = f(&mut d, sel)?;
    let count = d.relight.lights.len();
    s.set_develop(id, d, label)?;
    s.relight_select(id, picked);
    Ok(json!({"index": s.relight_selected(id), "count": count}))
}

/// The light `p` names (`index`, else the selected one) in `d`.
fn light_param(p: &Value, c: &str, d: &DevelopSettings, sel: usize) -> Result<usize> {
    let i = if p.get("index").is_some_and(|v| !v.is_null()) { index_req(p, "index", c)? } else { sel };
    if i < d.relight.lights.len() { Ok(i) } else { Err(bad(c, format!("no light {i}: the photo has {}", d.relight.lights.len()))) }
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.addLight";
    change(s, c, "Add Light", |d, _| {
        if d.relight.lights.len() >= MAX_LIGHTS {
            return Err(bad(c, format!("Relight holds at most {MAX_LIGHTS} lights")));
        }
        let (x, y) = free_spot(&d.relight.lights);
        let at = |k: &str, v: f64| p.get(k).and_then(Value::as_f64).unwrap_or(v);
        let mut l = LightSettings { amount: at("amount", 50.0), x: at("x", x), y: at("y", y), ..LightSettings::default() };
        l.sanitize();
        d.relight.lights.push(l);
        Ok(d.relight.lights.len() - 1)
    })
}

fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.duplicateLight";
    change(s, c, "Duplicate Light", |d, sel| {
        let i = light_param(p, c, d, sel)?;
        if d.relight.lights.len() >= MAX_LIGHTS {
            return Err(bad(c, format!("Relight holds at most {MAX_LIGHTS} lights")));
        }
        let mut copy = d.relight.lights[i].clone();
        // (a little aside, so the copy shows next to the light it came from)
        copy.x = if copy.x + 0.08 <= 1.0 { copy.x + 0.08 } else { copy.x - 0.08 };
        copy.y = if copy.y + 0.06 <= 1.0 { copy.y + 0.06 } else { copy.y - 0.06 };
        copy.sanitize();
        d.relight.lights.insert(i + 1, copy);
        Ok(i + 1)
    })
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.deleteLight";
    change(s, c, "Delete Light", |d, sel| {
        let i = light_param(p, c, d, sel)?;
        if d.relight.lights.len() == 1 {
            // (a photo keeps one light: the last one goes back to its default, off)
            d.relight.lights[0] = LightSettings::default();
            return Ok(0);
        }
        d.relight.lights.remove(i);
        Ok(i.saturating_sub(1))
    })
}

/// `relight.setGel`: a named gel, or any hue and strength, on a light (one undo step).
fn set_gel(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.setGel";
    let named = p.get("gel").filter(|v| !v.is_null());
    let (hue, strength) = match named {
        Some(Value::String(name)) => LightEffects::gel(name).ok_or_else(|| {
            let names: Vec<&str> = GELS.iter().map(|g| g.0).collect();
            bad(c, format!("no gel named {name:?}: the named gels are {}", names.join(", ")))
        })?,
        Some(_) => return Err(bad(c, "gel is a name")),
        None => {
            let num = |k: &str| p.get(k).filter(|v| !v.is_null()).map(|v| v.as_f64().ok_or_else(|| bad(c, format!("{k} is a number")))).transpose();
            match (num("hue")?, num("strength")?) {
                (None, None) => return Err(bad(c, "give a gel name, or a hue and a strength (strength 0 takes the gel off)")),
                (hue, strength) => (hue.unwrap_or(0.0), strength.unwrap_or(80.0)),
            }
        }
    };
    // (a named gel with its own strength: `strength` overrides it)
    let strength = p.get("strength").and_then(Value::as_f64).unwrap_or(strength);
    change(s, c, "Set Gel", |d, sel| {
        let i = light_param(p, c, d, sel)?;
        let e = &mut d.relight.lights[i].effects;
        (e.gel_hue, e.gel_strength) = (hue, strength);
        d.relight.lights[i].sanitize();
        // (the selection stays where it was)
        Ok(sel)
    })
}

fn select(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.selectLight";
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let d = s.develop_of(id).ok_or_else(|| bad(c, "no photo"))?;
    if p.get("index").is_none_or(Value::is_null) {
        return Err(bad(c, "missing `index`"));
    }
    let i = light_param(p, c, &d, 0)?;
    s.relight_select(id, i);
    Ok(json!({"index": i, "count": d.relight.lights.len()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "relight.status",
            "Relight Status",
            [],
            None,
            "{id?} → {id, on, lights: [{amount, softness, warmth, x, y, depth, wrap, gelHue, gelStrength, pattern, patternScale, patternRotation, patternSoftness, haze}], darken, selected: index of the selected light, ready, finding, cancelled, source?: file|model, error?, model: {installed, bytes, downloading, done, total, error?, available}} — whether the photo's depth is found (from its own depth map, an iPhone Portrait photo's, or the depth model's), being found, or why it can't be. The lights are develop controls (relight.<n>.amount, .softness, .warmth, .x, .y, .depth, .wrap, .gelHue, .gelStrength, .pattern (0 none, 1 blinds, 2 window panes, 3 leaves, 4 dapple), .patternScale, .patternRotation, .patternSoftness and .haze for light n from 0, and relight.darken; relight.amount … act on the selected light): set them with develop.set or develop.merge",
            always,
            status
        ),
        cmd!(
            query "relight.findDepth",
            "Find Depth",
            [],
            None,
            "{id?} → relight.status's fields — find the photo's depth again after it was cancelled (renders don't start a cancelled one) or failed; an error while the photo's light is off",
            always,
            find
        ),
        cmd!(
            query "relight.downloadModel",
            "Download Depth Model",
            [],
            None,
            "{confirm: true} — download Relight's depth model (Depth Anything 3 Mono Large, about 1.34 GB, Apache-2.0) into the shared models folder in the background; only after the user agreed to it. Watch relight.status → {started, model}",
            always,
            download
        ),
        cmd!(
            "relight.addLight",
            "Add Light",
            [],
            None,
            "{amount?: 0..100 (default 50), x?, y?: normalized place} → {index, count} — add a light (at most 8) on the active photo, in a free spot, and select it; one undo step",
            has_active,
            add
        ),
        cmd!(
            "relight.duplicateLight",
            "Duplicate Light",
            [],
            None,
            "{index?: light (default the selected one)} → {index, count} — copy a light, a little aside, after it, and select the copy; an error at 8 lights; one undo step",
            has_active,
            duplicate
        ),
        cmd!(
            "relight.deleteLight",
            "Delete Light",
            [],
            None,
            "{index?: light (default the selected one)} → {index, count} — remove a light and select the one before it (deleting the first selects the one that follows); a photo's only light goes back to its defaults (off) instead; one undo step",
            has_active,
            delete
        ),
        cmd!(
            "relight.setGel",
            "Set Gel",
            [],
            None,
            "{gel?: Warm Amber|Teal|Magenta|Deep Blue|Red, hue?: 0..360 degrees, strength?: 0..100, index?: light (default the selected one)} → {index, count} — put a gel on a light: a named one (hue and strength set together; strength overrides its own), or any hue (0 red, 120 green, 240 blue) with a strength; strength 0 takes the gel off. Warmth stays. The gel is also the controls relight.<n>.gelHue and .gelStrength; one undo step",
            has_active,
            set_gel
        ),
        cmd!(
            query "relight.selectLight",
            "Select Light",
            [],
            None,
            "{index: light, from 0; required} → {index, count} — select a light: relight.amount, .softness, .warmth, .x, .y, .depth and .wrap act on it, and its handle is the one marked on the photo. Not part of the edit: no undo step",
            has_active,
            select
        ),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    fn session() -> (Session, lightcraft_catalog::PhotoId) {
        let mut s = Session::with_demo();
        let id = s.catalog.photos().next().unwrap().id;
        s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
        (s, id)
    }

    fn lights(s: &Session, id: lightcraft_catalog::PhotoId) -> Vec<lightcraft_develop::relight::LightSettings> {
        s.develop_of(id).unwrap().relight.lights.clone()
    }

    #[test]
    fn lights_are_added_duplicated_deleted_and_selected_and_each_is_one_undo_step() {
        let (mut s, id) = session();
        assert_eq!((lights(&s, id).len(), s.relight_selected(id)), (1, 0));
        // the shorthand acts on the selected light, the indexed ids on theirs
        s.execute("develop.set", &json!({"control": "relight.amount", "value": 40})).unwrap();
        let r = s.execute("relight.addLight", &json!({})).unwrap();
        assert_eq!((r["index"].as_u64(), r["count"].as_u64()), (Some(1), Some(2)));
        assert_eq!(s.relight_selected(id), 1);
        s.execute("develop.set", &json!({"control": "relight.warmth", "value": -60})).unwrap();
        s.execute("develop.set", &json!({"values": {"relight.0.softness": 10, "relight.1.depth": 70}})).unwrap();
        let l = lights(&s, id);
        assert_eq!((l[0].amount, l[0].warmth, l[0].softness), (40.0, 0.0, 10.0));
        assert_eq!((l[1].amount, l[1].warmth, l[1].depth), (50.0, -60.0, 70.0));
        s.execute("relight.selectLight", &json!({"index": 0})).unwrap();
        s.execute("develop.adjust", &json!({"control": "relight.amount", "delta": 5})).unwrap();
        assert_eq!(lights(&s, id)[0].amount, 45.0);
        // the controls list shows the selected light under the shorthand and every light's own
        let controls = s.execute("develop.controls", &json!({"section": "relight"})).unwrap();
        let value = |id: &str| controls.as_array().unwrap().iter().find(|c| c["id"] == id).map(|c| c["value"].clone());
        assert_eq!((value("relight.amount"), value("relight.1.warmth")), (Some(json!(45.0)), Some(json!(-60.0))));
        // a copy comes after its light and is selected; deleting goes back to the one before
        s.execute("relight.duplicateLight", &json!({})).unwrap();
        assert_eq!((lights(&s, id).len(), s.relight_selected(id), lights(&s, id)[1].amount), (3, 1, 45.0));
        s.execute("relight.deleteLight", &json!({})).unwrap();
        assert_eq!((lights(&s, id).len(), s.relight_selected(id)), (2, 0));
        // undo brings the deleted light back, and an undone add leaves the selection inside the lights
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(lights(&s, id).len(), 3);
        s.execute("relight.selectLight", &json!({"index": 1})).unwrap();
        // (the copy, the nudge, the two sets and the add)
        for _ in 0..5 {
            s.execute("edit.undo", &json!({})).unwrap();
        }
        assert_eq!((lights(&s, id).len(), s.relight_selected(id)), (1, 0));
        // errors, not panics: no such light, past the cap, the only light
        assert!(s.execute("relight.selectLight", &json!({"index": 5})).is_err());
        assert!(s.execute("relight.selectLight", &json!({})).is_err(), "no index is an error, not light 0");
        // a control of a light the photo doesn't have is an error and makes no undo step
        let before = s.develop_of(id).unwrap();
        assert!(s.execute("develop.set", &json!({"control": "relight.6.amount", "value": 5})).is_err());
        assert!(s.execute("develop.adjust", &json!({"control": "relight.6.amount", "delta": 5})).is_err());
        assert!(std::sync::Arc::ptr_eq(&before, &s.develop_of(id).unwrap()));
        // Quick Develop's shorthand acts on the selected light
        s.execute("relight.addLight", &json!({})).unwrap();
        s.execute("develop.quickAdjust", &json!({"control": "relight.softness", "delta": 10})).unwrap();
        assert_eq!((lights(&s, id)[0].softness, lights(&s, id)[1].softness), (50.0, 60.0));
        s.execute("edit.undo", &json!({})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(s.execute("relight.deleteLight", &json!({"index": 9})).is_err());
        for _ in 0..7 {
            s.execute("relight.addLight", &json!({})).unwrap();
        }
        assert!(s.execute("relight.addLight", &json!({})).is_err() && s.execute("relight.duplicateLight", &json!({})).is_err());
        assert_eq!(lights(&s, id).len(), 8);
        for _ in 0..9 {
            s.execute("relight.deleteLight", &json!({})).unwrap();
        }
        assert!(s.develop_of(id).unwrap().relight.is_default(), "the last light goes back to its defaults");
    }

    #[test]
    fn a_gel_is_set_by_name_or_by_hue_and_one_undo_step_each_and_the_controls_reach_the_effects() {
        let (mut s, id) = session();
        s.execute("relight.addLight", &json!({})).unwrap();
        let effects = |s: &Session, i: usize| lights(s, id)[i].effects;
        // a named gel on the selected light (the new one), its own strength overridden by `strength`
        s.execute("relight.setGel", &json!({"gel": "teal"})).unwrap();
        assert_eq!((effects(&s, 1).gel_hue, effects(&s, 1).gel_strength, effects(&s, 0).gel_strength), (178.0, 65.0, 0.0));
        s.execute("relight.setGel", &json!({"gel": "Deep Blue", "strength": 40, "index": 0})).unwrap();
        assert_eq!((effects(&s, 0).gel_hue, effects(&s, 0).gel_strength), (232.0, 40.0));
        // any hue, and strength 0 takes it off; each is one undo step
        s.execute("relight.setGel", &json!({"hue": 90, "strength": 55})).unwrap();
        assert_eq!((effects(&s, 1).gel_hue, effects(&s, 1).gel_strength), (90.0, 55.0));
        s.execute("relight.setGel", &json!({"hue": 90, "strength": 0})).unwrap();
        assert_eq!(effects(&s, 1).gel_strength, 0.0);
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(effects(&s, 1).gel_strength, 55.0);
        // nothing named, a gel that does not exist, a light that is not there: errors
        assert!(s.execute("relight.setGel", &json!({})).is_err());
        assert!(s.execute("relight.setGel", &json!({"gel": "chartreuse"})).is_err());
        assert!(s.execute("relight.setGel", &json!({"gel": "red", "index": 7})).is_err());
        // the pattern, scale and haze are controls by id (the shorthand acts on the selected light) and the status shows them all
        s.execute("develop.set", &json!({"values": {"relight.0.pattern": 3, "relight.0.patternScale": 20, "relight.haze": 40}})).unwrap();
        assert_eq!((effects(&s, 0).pattern, effects(&s, 0).pattern_scale, effects(&s, 1).haze), (3.0, 20.0, 40.0));
        let st = s.execute("relight.status", &json!({})).unwrap();
        assert_eq!((st["lights"][0]["pattern"].as_f64(), st["lights"][1]["patternSoftness"].as_f64()), (Some(3.0), Some(30.0)));
    }
}
