//! Fork feature (`fork/adaptive-masks.md`, issue #239): Update AI Masks (`mask.updateAi`) finds each AI mask part of
//! the photos again on that photo, after a crop, a retouch or an edit that moved the subject.

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::MaskShape;
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_selection};
use crate::Session;
use crate::segment::adaptive_jobs::Ask;

/// Whether `shape` can be found again on photo `id` on this computer (a part no model can find keeps what it has).
fn findable(s: &Session, id: PhotoId, shape: &MaskShape) -> bool {
    match shape {
        MaskShape::People { .. } => s.parsing.ready(),
        _ if s.adaptive.has_finder() => true,
        _ => matches!(s.adaptive_ask(id, shape), Ask::Model { .. }),
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "mask.updateAi",
        "Update AI Masks",
        ["Photo"],
        None,
        "{ids?} → {photos, parts} — finds each AI mask part (Subject, Sky, Background, Object, Describe, Select People) of the photos (`ids`, else the selection) again on that photo, in the background in the app (one undo step); a part no model on this computer can find keeps what it has",
        has_selection,
        |s, p: &Value| {
            let c = "mask.updateAi";
            let ids = s.targets(p)?;
            let mut ops = Vec::new();
            let mut parts = 0;
            for id in ids {
                let Some(d) = s.develop_of(id) else { continue };
                let mut next = (*d).clone();
                let n = next.set_ai_waiting(|shape| findable(s, id, shape));
                if n > 0 {
                    parts += n;
                    ops.extend(s.develop_op(id, next, "Update AI Masks"));
                }
            }
            if ops.is_empty() {
                return Err(bad(c, "no AI mask on these photos can be found again on this computer"));
            }
            let photos = ops.len();
            s.commit("Update AI Masks", Op::Batch { ops })?;
            Ok(json!({"photos": photos, "parts": parts}))
        }
    )]
}
