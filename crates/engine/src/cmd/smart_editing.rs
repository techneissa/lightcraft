//! Use Smart Previews instead of Originals for editing (`library.smartPreviewsForEditing`), and the smart previews it
//! needs built: for the photos a new import adds, and for the library when the setting is switched on
//! ([`crate::media::smart_editing`] has the rule for when a proxy is used).

use serde_json::{Value, json};

use super::{CommandSpec, always, cmd};
use crate::{Result, Session};

/// Photos whose originals are files (the ones that have smart previews).
fn file_photos(s: &Session) -> Vec<lightcraft_catalog::PhotoId> {
    s.catalog.photos().filter(|p| !p.deleted && matches!(p.source, lightcraft_catalog::Source::File { .. })).map(|p| p.id).collect()
}

/// Build the smart previews of `ids` that are missing or out of date, on a worker thread with a row in the activity
/// stack (the existing Build Smart Previews job, [`super::previews::smart_run`]). Apart from `library.previewProgress`:
/// it does not wait for a preview build that is running, nor take its place. → how many photos it looks at.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn build_missing(s: &mut Session, ids: &[lightcraft_catalog::PhotoId]) -> std::result::Result<usize, String> {
    use super::previews::{PreviewBuild, SmartJob, smart_local, smart_run, task_for};
    use std::sync::atomic::Ordering;
    let dir = s.media.smart_dir.clone().ok_or("smart previews need a library on disk")?;
    let mut jobs: Vec<SmartJob> = Vec::new();
    for id in ids {
        let Some(ph) = s.catalog.photo(*id).cloned() else { continue };
        if ph.deleted || !matches!(ph.source, lightcraft_catalog::Source::File { .. }) {
            continue;
        }
        // (as Build Smart Previews: a smart preview keeps the camera matching, except for a local profile or macOS Colour)
        let (options, local) = smart_local(&ph);
        let source = s.media.source_ref_for(&ph, crate::media::SourceLevel::Preview, options.unwrap_or_default());
        jobs.push((*id, crate::smart::sharded_file_path(&dir, &ph), Some(source), super::previews::preview_fitted_look(&ph), local));
    }
    if jobs.is_empty() {
        return Ok(0);
    }
    let n = jobs.len();
    let custom = s.smart_previews_dir.is_some();
    let state = std::sync::Arc::new(PreviewBuild { total: n, what: "smart previews", ..Default::default() });
    let task = task_for(s, &state);
    std::thread::Builder::new()
        .name("lc-smart-editing".into())
        .spawn(move || {
            if let Err(e) = smart_run(&dir, custom, false, jobs, Some((&state, &task))) {
                log::warn!("smart previews for editing: {e}");
                *state.error.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(e);
            }
            state.finished.store(true, Ordering::Relaxed);
        })
        .map_err(|e| format!("could not start: {e}"))?;
    Ok(n)
}

/// A new import's photos get their smart previews when the setting is on (a folder only browsed in Local adds none).
pub(crate) fn after_import(s: &mut Session, imported: &[u64]) {
    #[cfg(not(target_arch = "wasm32"))]
    if s.media.smart_editing() && s.media.smart_dir.is_some() && !imported.is_empty() {
        let ids: Vec<_> = imported.iter().map(|i| lightcraft_catalog::PhotoId(*i)).collect();
        if let Err(e) = build_missing(s, &ids) {
            log::warn!("smart previews for the new photos: {e}");
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (s, imported);
}

fn for_editing(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.smartPreviewsForEditing";
    if let Some(on) = p.get("enabled").and_then(Value::as_bool) {
        s.media.set_smart_editing(on);
    }
    let photos = file_photos(s).len();
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut out = json!({
        "enabled": s.media.smart_editing(),
        "available": s.media.smart_dir.is_some(),
        "photos": photos,
        "smartPreviews": s.media.smart_dir.as_deref().map_or(0, |d| crate::smart::stats(d).0),
    });
    if p.get("build").and_then(Value::as_bool).unwrap_or(false) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let ids = file_photos(s);
            out["building"] = json!(build_missing(s, &ids).map_err(|e| super::bad(C, e))?);
        }
        #[cfg(target_arch = "wasm32")]
        return Err(super::bad(C, "smart previews are not available here"));
    }
    Ok(out)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "library.smartPreviewsForEditing",
        "Use Smart Previews for Viewing and Editing",
        [],
        None,
        "{enabled?: bool, build?: bool} — Use Smart Previews instead of Originals for image editing (Lightroom's option; off by default). On: the grid, the loupe at fit and fill, Compare, Survey and every slider edit render from a photo's current smart preview, which is read instead of the original; the original is read for zoom to 100 % and more, export, Edit In, Open as Layers, DNG export, Photo Merge, AI Denoise and the photos with no current smart preview. A smart preview that is out of date, or made for another colour than the photo's Profile asks for, counts as missing. With build, the smart previews the library lacks are built in the background (an activity row) → {enabled, available, photos, smartPreviews, building?}",
        always,
        for_editing
    )]
}
