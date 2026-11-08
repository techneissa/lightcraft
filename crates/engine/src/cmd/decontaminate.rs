//! Fork feature (`fork/decontaminate.md`, issue #115): a mask's Decontaminate amount (`mask.decontaminate`), and the
//! amount a new mask starts with: on (100) for Background and Entire Person, off for every other kind.

use lightcraft_develop::MaskShape;
use serde_json::Value;

use super::masks::{find, mask_id, masks_edit};
use super::{CommandSpec, cmd, f64_or, has_active};

/// What a new mask of `kind` (made from `shape`) starts with: Background and a whole person are decontaminated.
pub(crate) fn default_for(kind: &str, shape: &MaskShape) -> f64 {
    let whole = |parts: &[String]| {
        parts.is_empty() || parts.iter().any(|p| matches!(p.to_ascii_lowercase().as_str(), "person" | "entire person" | "entireperson"))
    };
    match shape {
        MaskShape::Background { .. } => 100.0,
        MaskShape::People { parts, .. } if whole(parts) => 100.0,
        _ if kind == "background" => 100.0,
        _ => 0.0,
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "mask.decontaminate",
        "Decontaminate Mask Edge",
        [],
        None,
        "{id?, value: 0..100} — at the mask's soft edge (strands of hair), edit each pixel's own colour and the colour behind it apart, so a background swap leaves no fringe of the old background; 0 = off (rendered on the CPU)",
        has_active,
        |s, p: &Value| {
            let v = f64_or(p, "value", 0.0);
            let v = if v.is_finite() { v.clamp(0.0, 100.0) } else { 0.0 };
            let mid = mask_id(p, s.active_mask, "mask.decontaminate")?;
            masks_edit(s, "mask.decontaminate", "Decontaminate", |masks, _| {
                let i = find(masks, mid, "mask.decontaminate")?;
                masks[i].decontaminate = v;
                Ok(())
            })
        }
    )]
}
