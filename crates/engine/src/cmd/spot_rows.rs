//! Fork feature (`fork/spot-rows.md`): `spot.visible` hides or shows one spot's effect, `spot.circle` sets when its
//! circle shows on the photo. Each is one undo step. `spot.select` and `spot.delete` (in `masks.rs`) are the rest of
//! a spot's row in the Remove panel.

use lightcraft_develop::SpotCircle;
use serde_json::{Value, json};

use super::masks::{spot_index, spots_edit};
use super::{CommandSpec, bad, cmd, has_active};
use crate::Session;

fn visible(s: &mut Session, p: &Value) -> crate::Result<Value> {
    let c = "spot.visible";
    let i = spot_index(s, p, c)?;
    let was = s.active().and_then(|id| s.develop_of(id)).and_then(|d| d.spots.get(i).map(|sp| !sp.hidden)).ok_or_else(|| bad(c, "no such spot"))?;
    let want = match p.get("visible").filter(|v| !v.is_null()) {
        Some(v) => v.as_bool().ok_or_else(|| bad(c, "`visible` must be true or false"))?,
        None => !was,
    };
    if want != was {
        spots_edit(s, c, if want { "Show Spot" } else { "Hide Spot" }, |spots| {
            let sp = spots.get_mut(i).ok_or_else(|| bad(c, "no such spot"))?;
            sp.hidden = !want;
            Ok(())
        })?;
    }
    Ok(json!({"index": i, "visible": want}))
}

fn circle(s: &mut Session, p: &Value) -> crate::Result<Value> {
    let c = "spot.circle";
    let i = spot_index(s, p, c)?;
    let want = p
        .get("mode")
        .and_then(Value::as_str)
        .and_then(SpotCircle::from_name)
        .ok_or_else(|| bad(c, "`mode` must be \"global\", \"auto\", \"always\", \"selected\" or \"never\""))?;
    let was = s.active().and_then(|id| s.develop_of(id)).and_then(|d| d.spots.get(i).map(|sp| sp.circle)).ok_or_else(|| bad(c, "no such spot"))?;
    if want != was {
        spots_edit(s, c, "Spot Circle", |spots| {
            let sp = spots.get_mut(i).ok_or_else(|| bad(c, "no such spot"))?;
            sp.circle = want;
            Ok(())
        })?;
    }
    Ok(json!({"index": i, "mode": want.map_or("global", SpotCircle::name)}))
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "spot.visible",
            "Show or Hide Spot",
            [],
            None,
            "{index? (the selected spot), visible? (default: the other way round)} — a hidden spot has no effect in the preview or an export; its circle follows `spot.circle`",
            has_active,
            visible
        ),
        cmd!(
            "spot.circle",
            "Spot Circle",
            [],
            None,
            "{index? (the selected spot), mode: global|auto|always|selected|never} — when the spot's circle shows on the photo; `global` follows view.toolOverlay. The effect is `spot.visible`",
            has_active,
            circle
        ),
    ]
}
