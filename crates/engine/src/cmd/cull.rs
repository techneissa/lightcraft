//! Assisted culling: score photos for focus and clipping, group similar shots taken close
//! together (bursts) and mark the sharpest of each group; optionally reject blurry ones and pick
//! each group's best. Scores are kept on the photos (rules / smart albums: `sharpness`,
//! `bestOfGroup`). Fork (`fork/ai-culling.md`): the faces' eyes too (`eyesClosed`, `eyesOutOfFocus`), measured on
//! worker threads in the app (`cull_run`).

use serde_json::Value;

use super::{CommandSpec, cmd};
use crate::Result;

/// The command's id.
pub(super) const C: &str = "photo.analyze";

/// Seconds between shots of the same burst at most (default of `burstGap`).
const BURST_GAP: f64 = 10.0;
/// Signature similarity for "the same scene" (default of `similarity`).
const SAME: f64 = 0.93;
/// The longest `burstGap` accepted: a day.
const MAX_GAP: f64 = 86_400.0;

/// `burstGap` (seconds, 0..=a day) and `similarity` (0..=1) of `photo.analyze`, refused before
/// anything is measured when they are not numbers in range.
pub(super) fn burst_params(p: &Value) -> Result<(f64, f32)> {
    let num = |key: &str, d: f64, max: f64| -> Result<f64> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(d),
            Some(v) => match v.as_f64() {
                Some(x) if x.is_finite() && (0.0..=max).contains(&x) => Ok(x),
                _ => Err(super::bad(C, format!("`{key}` must be a number from 0 to {max}"))),
            },
        }
    };
    Ok((num("burstGap", BURST_GAP, MAX_GAP)?, num("similarity", SAME, 1.0)? as f32))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "library.findSimilar",
            "Find Similar Photos",
            ["Photo"],
            None,
            "{id?, similarity?: 0..1 (0.8), filter?: bool (true)} — photos that look like the active one (composition and tones), most similar first; filters the view to them (Clear Filters to go back) → {photos: [{id, similarity}]}. In the app the photos are read on a worker thread under an activity row: a control request is answered with this when the search ends, and from a menu the answer is {background: true, count}",
            super::similar::no_search_running,
            super::similar::run
        ),
        cmd!(
            "photo.analyze",
            "Assisted Culling",
            [],
            None,
            "{ids?, rejectBelow?: sharpness 0..100, pickBest?: bool, burstGap?: seconds 0..86400 (10), similarity?: 0..1 (0.93), eyes?: true | false | \"again\" (true)} — score the selected photos (else all in view) for focus (0..100) and clipping, group look-alike shots (signature similarity at least `similarity`) taken at most `burstGap` seconds apart and mark the best of each (the sharpest whose eyes are open and in focus); read the main faces' eyes (closed; eye focus 0..100, out of focus below 35) from the people parse when the photo has one, else from the face detector and face mesh (eyes measured before are kept unless `eyes` is \"again\"); optionally reject blurry photos and pick each group's best; photos with closed or soft eyes are only suggested for rejection; one undo step → {photos: [{id, sharpness, clipped, group, best, eyes?: {faces, main, closed, focus, outOfFocus}, faces?: [{rect, main, closed, focus, mesh, eyes: [{open}]}], eyesError?}], groups, rejected, picked, failed, suggestReject: [{id, why: [eyesClosed | eyesOutOfFocus]}], eyes: {measured, fromParse, kept, failed, unavailable, meshEngine}}. In the app the photos are read on worker threads under an activity row: a control request is answered when the run ends, and from a menu the answer is {background: true, count}",
            super::cull_run::no_run_going,
            super::cull_run::run
        ),
    ]
}
