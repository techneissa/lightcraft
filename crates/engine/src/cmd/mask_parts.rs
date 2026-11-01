//! The parts (components) of a mask: which one is selected, which one a brush stroke goes into, and the rule that a
//! mask's first part is its base (fork feature, issue #26). `masks.rs` calls these from small hooks.

use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape};
use serde_json::Value;

use super::{bad, ok};
use crate::{Result, Session};

impl Session {
    /// The selected part (component index) of the selected mask, when it still exists.
    pub fn active_part(&self) -> Option<usize> {
        let (mask, k) = self.active_component?;
        if self.active_mask != Some(mask) {
            return None;
        }
        let d = self.develop_of(self.active()?)?;
        (k < d.masks.iter().find(|m| m.id == mask)?.components.len()).then_some(k)
    }
}

/// `key` of `p` as an index: omitted or null is `None`, anything but a non-negative integer is an error.
fn index_param(p: &Value, key: &str, c: &str) -> Result<Option<usize>> {
    p.get(key)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .ok_or_else(|| bad(c, format!("`{key}` must be a non-negative integer that fits in usize")))
        })
        .transpose()
}

/// `mask.select {id|null, component?}`: select a mask, and with `component` one of its parts.
pub(super) fn select(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.select";
    let id = match p.get("id").filter(|v| !v.is_null()) {
        Some(v) => Some(super::masks::parse_mask_id(v, c)?),
        None => None,
    };
    let part = index_param(p, "component", c)?;
    if let Some(k) = part {
        let mid = id.ok_or_else(|| bad(c, "`component` needs `id`"))?;
        let parts = s.active().and_then(|a| s.develop_of(a)).and_then(|d| d.masks.iter().find(|m| m.id == mid).map(|m| m.components.len()));
        match parts {
            None => return Err(bad(c, format!("no mask {mid}"))),
            Some(n) if k >= n => return Err(bad(c, format!("mask {mid} has no part {k}"))),
            Some(_) => {}
        }
    }
    s.active_mask = id;
    s.active_component = id.zip(part);
    ok()
}

/// The `want`ed part of mask `m` a brush stroke goes into (it must be a brush part), else the `selected` part when it
/// is one of this mask's brush parts, else the mask's last brush part; `None` when the mask has none yet.
pub(super) fn brush_target(m: &Mask, p: &Value, selected: Option<(u32, usize)>, c: &str) -> Result<Option<usize>> {
    let is_brush = |c: &MaskComponent| matches!(c.shape, MaskShape::Brush { .. });
    Ok(match index_param(p, "component", c)? {
        Some(k) if m.components.get(k).is_some_and(is_brush) => Some(k),
        Some(k) => return Err(bad(c, format!("part {k} is not a brush part"))),
        None => selected
            .filter(|(mid, k)| *mid == m.id && m.components.get(*k).is_some_and(is_brush))
            .map(|(_, k)| k)
            .or_else(|| m.components.iter().rposition(is_brush)),
    })
}

/// The mode part `k` gets when set to `op`: the first part is the base and takes no other mode than Add.
pub(super) fn mode_for(k: usize, op: MaskOp) -> MaskOp {
    if k == 0 { MaskOp::Add } else { op }
}

/// After part `k` was removed from `comps`: when it was the base, the next part is the base now, shown (and acting) as Add.
pub(super) fn rebase_after_delete(comps: &mut [MaskComponent], k: usize) {
    if k == 0
        && let Some(first) = comps.first_mut()
    {
        first.op = MaskOp::Add;
    }
}

/// The selected part (`selected`, an index in mask `mask`) keeps pointing at the same part after `action` on part `k`.
pub(super) fn follow(s: &mut Session, mask: u32, selected: Option<usize>, action: &str, k: usize) {
    if let Some(j) = selected {
        s.active_component = match action {
            "duplicate" if j > k => Some((mask, j + 1)),
            "delete" if j == k => None,
            "delete" if j > k => Some((mask, j - 1)),
            _ => s.active_component,
        };
    }
}
