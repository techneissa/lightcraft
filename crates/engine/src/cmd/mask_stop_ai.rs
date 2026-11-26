//! Fork (issue #407, `fork/mask-models-kept.md`): an AI mask's models, and its stop button.
//!
//! - `mask.aiModels {id?}`: the models the mask is made with, and which of them are in memory or working now (the stop
//!   button can be clicked while one is).
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
    let uses = s.mask_models_of_mask(m);
    let active = uses.both(s.mask_models_in_use());
    Ok(json!({"id": id, "uses": uses.ids(), "active": active.ids(), "canStop": active.any()}))
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "mask.aiModels",
            "AI Mask Models",
            [],
            None,
            "{id?: mask id (default: the selected mask)} → {id, uses: [\"sam3\"|\"birefnet\"|\"matting\"] (the models the mask's AI parts are made with; brush, gradient and range parts: none), active: the ones in memory or working now, canStop: whether Stop AI does something now}",
            has_active,
            |s, p| state(s, p, "mask.aiModels")
        ),
        cmd!(
            query "mask.stopAi",
            "Stop AI",
            [],
            None,
            "{id?: mask id (default: the selected mask)} — cancels the AI mask requests under way and lets the models the mask is made with go now, even while the Masking panel keeps them; the mask keeps its stored selection. Refused for a mask no AI model makes → {id, stopped: [the models that were in memory or working], uses}",
            has_active,
            |s, p| {
                let before = state(s, p, "mask.stopAi")?;
                let id = mask_id(p, s.active_mask, "mask.stopAi")?;
                s.stop_mask_ai(id).map_err(EngineError::Other)?;
                Ok(json!({"id": id, "stopped": before["active"], "uses": before["uses"]}))
            }
        ),
    ]
}
