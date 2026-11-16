//! `relight.*` (fork, issue #288, `fork/relight.md`): where a photo's Relight field stands, and the depth model's
//! download. The light itself is develop controls (`relight.amount`, `.softness`, `.warmth`, `.x`, `.y`, `.depth`,
//! `.darken`, `.wrap`), set like every slider (`develop.set`, `develop.merge`), so undo, presets, copy settings and
//! sync reach it.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, photo_id};
use crate::{Result, Session};

/// The depth model's licence.
const LICENSE: &str = "Apache-2.0";

fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.status";
    let id = photo_id(p, c)?.or_else(|| s.active()).ok_or_else(|| bad(c, "no photo"))?;
    s.catalog.photo(id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))?;
    let d = s.develop_of(id).unwrap_or_default();
    let mut v = json!({"id": id.0, "on": lightcraft_pipeline::relight::light_of(&d).is_some(), "light": d.relight});
    if let (Some(o), Value::Object(st)) = (v.as_object_mut(), s.relight_status(id)) {
        o.extend(st);
    }
    Ok(v)
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

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "relight.status",
            "Relight Status",
            [],
            None,
            "{id?} → {id, on, light: {amount, softness, warmth, x, y, depth, darken, wrap}, ready, finding, source?: file|model, error?, model: {installed, bytes, downloading, done, total, error?, available}} — whether the photo's depth is found (from its own depth map, an iPhone Portrait photo's, or the depth model's), being found, or why it can't be. The light is develop controls (relight.amount, .softness, .warmth, .x, .y, .depth, .darken, .wrap): set them with develop.set or develop.merge",
            always,
            status
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
    ]
}
