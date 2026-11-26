//! Fork (issue #407, `fork/mask-models-kept.md`): an AI mask's models, and its stop button.
//!
//! - `mask.aiModels {id?}`: the models the mask is made with, which of them are in memory, and whether a request for
//!   this mask is under way (the stop button can be clicked while either is).
//! - `mask.stopAi {id?}`: cancels the requests on the AI mask worker and lets the mask's models go now; the mask keeps
//!   its stored selection.

use serde_json::{Value, json};

use super::masks::mask_id;
use super::{CommandSpec, cmd, has_active};
use crate::{EngineError, Result, Session};

fn state(s: &Session, p: &Value, c: &str) -> Result<Value> {
    let id = mask_id(p, s.active_mask, c)?;
    let photo = s.active().ok_or_else(|| EngineError::Other("no photo is open".into()))?;
    let d = s.develop_of(photo).ok_or_else(|| EngineError::Other("no such photo".into()))?;
    let m = d.masks.iter().find(|m| m.id == id).ok_or_else(|| EngineError::Other(format!("no mask {id}")))?;
    let uses = s.mask_models_of_mask(photo, m);
    let loaded = uses.both(s.mask_models_loaded());
    let working = s.segmenter.mask_busy(photo, id) || s.adaptive.asking_for(photo, id);
    Ok(json!({"id": id, "uses": uses.ids(), "loaded": loaded.ids(), "working": working, "canStop": s.mask_ai_stoppable(photo, id, uses)}))
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "mask.aiModels",
            "AI Mask Models",
            [],
            None,
            "{id?: mask id (default: the selected mask)} → {id, uses: [\"sam3\"|\"birefnet\"|\"matting\"] (the models the mask's AI parts are made with; brush, gradient and range parts: none), loaded: the ones in memory now, working: a request for this mask is under way, canStop: whether Stop AI does something now (one of them loaded, or working)}",
            has_active,
            |s, p| state(s, p, "mask.aiModels")
        ),
        cmd!(
            query "mask.stopAi",
            "Stop AI",
            [],
            None,
            "{id?: mask id (default: the selected mask)} — cancels this mask's AI requests under way (other masks' go on) and lets the models the mask is made with go once the step under way ends (up to a few seconds), even while the Masking panel keeps them; the mask keeps its stored selection → {id, stopped: [the models that were in memory], uses}. An error for a mask that uses none of these models",
            has_active,
            |s, p| {
                let before = state(s, p, "mask.stopAi")?;
                let id = mask_id(p, s.active_mask, "mask.stopAi")?;
                s.stop_mask_ai(id).map_err(EngineError::Other)?;
                Ok(json!({"id": id, "stopped": before["loaded"], "uses": before["uses"]}))
            }
        ),
    ]
}
