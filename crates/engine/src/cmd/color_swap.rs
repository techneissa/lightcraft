//! Color Swap (fork, issue #460, `fork/color-swap.md`): the entries that replace one colour with another across the
//! whole photo. `colorSwap.add`, `colorSwap.set`, `colorSwap.remove` and `colorSwap.move` change the list (each one
//! undo step), `colorSwap.pick` takes a colour from a point of a photo (and with no entry named, adds one from it:
//! click on the photo), `colorSwap.list` reads it. The entries' sliders are indexed controls too:
//! `develop.set {control: "colorSwap.0.range", value}`.

use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::color_swap::{ColorSwap, MAX_NAME, MAX_SWAPS};
use lightcraft_pipeline::recolor::{parse_hex, to_hex};
use serde_json::{Value, json};

use super::mask_recolor::color_at_with;
use super::{CommandSpec, bad, cmd, f64_req, has_active, index_req};
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "colorSwap.add",
            "Add Color Swap",
            [],
            None,
            "{from?: \"#rrggbb\", to?: \"#rrggbb\", name?} — adds a colour swap (at most 8) at the end of the list; it changes nothing until both colours are set; answers `{index}`",
            has_active,
            add
        ),
        cmd!(
            "colorSwap.set",
            "Change Color Swap",
            [],
            None,
            "{index, name?, on?, from?: \"#rrggbb\"|null, to?: \"#rrggbb\"|null, keepLightness?, range?, intensity?, hueRange?, satRange?, lumRange?, softness?: 0..100} — changes one swap, as one undo step; answers the swap (`colorSwap.list`'s entry)",
            has_active,
            set
        ),
        cmd!("colorSwap.remove", "Remove Color Swap", [], None, "{index}", has_active, remove),
        cmd!(
            "colorSwap.move",
            "Move Color Swap",
            [],
            None,
            "{index, to} — moves a swap to place `to` (0 = first); swaps run in order, each on what the ones before made",
            has_active,
            move_to
        ),
        cmd!(
            "colorSwap.pick",
            "Pick Color Swap Color",
            [],
            None,
            "{x, y, index?, target?: \"from\"|\"to\", photo?: photo id} — takes the colour at (x, y) (each 0..1 across the uncropped photo; outside is an error) of `photo` as developed (default: the active photo; a From colour is read without the photo's swaps, so it is the colour the swap will see). With `index` it sets that swap's `target` (default `from`); without, it adds a swap from this colour (at most 8); answers `{index, color}`",
            has_active,
            pick
        ),
        cmd!(
            query "colorSwap.list",
            "List Color Swaps",
            [],
            None,
            "{} — the active photo's swaps in order: `{index, name, on, from, to, active, keepLightness, range, intensity, hueRange, satRange, lumRange, softness}` (colours as \"#rrggbb\" or null)",
            has_active,
            list
        ),
    ]
}

/// The entry as the commands answer it.
fn entry_json(i: usize, e: &ColorSwap) -> Value {
    json!({
        "index": i,
        "name": e.display_name(i),
        "on": e.on,
        "from": e.from.map(to_hex),
        "to": e.to.map(to_hex),
        "active": e.is_active(),
        "keepLightness": e.keep_lightness,
        "range": e.range,
        "intensity": e.intensity,
        "hueRange": e.hue_range,
        "satRange": e.sat_range,
        "lumRange": e.lum_range,
        "softness": e.softness,
    })
}

fn photo_settings(s: &Session, c: &str) -> Result<(PhotoId, DevelopSettings)> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    Ok((id, (*s.develop_of(id).unwrap_or_default()).clone()))
}

/// An optional colour parameter `key` (`"#rrggbb"`); `Some(None)` for a null that clears it.
fn colour(p: &Value, key: &str, c: &str) -> Result<Option<Option<[f64; 3]>>> {
    match p.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(v) => {
            let text = v.as_str().ok_or_else(|| bad(c, format!("`{key}` must be a string like \"#7a3fb0\" or null")))?;
            Ok(Some(Some(parse_hex(text).ok_or_else(|| bad(c, format!("not a color: {text:?} (use \"#rrggbb\")")))?)))
        }
    }
}

/// An optional number parameter `key` (not a finite number is an error), clamped to 0..100.
fn percent(p: &Value, key: &str, c: &str) -> Result<Option<f64>> {
    match p.get(key).filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => {
            Ok(Some(v.as_f64().filter(|a| a.is_finite()).ok_or_else(|| bad(c, format!("`{key}` must be a number 0..100")))?.clamp(0.0, 100.0)))
        }
    }
}

fn flag(p: &Value, key: &str, c: &str) -> Result<Option<bool>> {
    match p.get(key).filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => Ok(Some(v.as_bool().ok_or_else(|| bad(c, format!("`{key}` must be true or false")))?)),
    }
}

/// The changes of `p` applied to `e` (every parameter checked first, so a bad one changes nothing).
fn change(e: &mut ColorSwap, p: &Value, c: &str) -> Result<()> {
    let name = match p.get("name") {
        None => None,
        Some(v) => Some(v.as_str().ok_or_else(|| bad(c, "`name` must be text"))?.trim().chars().take(MAX_NAME).collect::<String>()),
    };
    let (from, to) = (colour(p, "from", c)?, colour(p, "to", c)?);
    let (on, keep) = (flag(p, "on", c)?, flag(p, "keepLightness", c)?);
    let numbers = [
        ("range", percent(p, "range", c)?),
        ("intensity", percent(p, "intensity", c)?),
        ("hueRange", percent(p, "hueRange", c)?),
        ("satRange", percent(p, "satRange", c)?),
        ("lumRange", percent(p, "lumRange", c)?),
        ("softness", percent(p, "softness", c)?),
    ];
    if let Some(n) = name {
        e.name = n;
    }
    if let Some(f) = from {
        e.from = f;
    }
    if let Some(t) = to {
        e.to = t;
    }
    if let Some(v) = on {
        e.on = v;
    }
    if let Some(v) = keep {
        e.keep_lightness = v;
    }
    for (field, v) in numbers {
        if let (Some(v), Some(slot)) = (v, lightcraft_develop::color_swap::field_mut(e, field)) {
            *slot = v;
        }
    }
    Ok(())
}

fn entry_at(d: &mut DevelopSettings, p: &Value, c: &str) -> Result<usize> {
    let i = index_req(p, "index", c)?;
    if i >= d.color_swaps.len() {
        return Err(bad(c, format!("no swap {i} (the photo has {})", d.color_swaps.len())));
    }
    Ok(i)
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "colorSwap.add";
    let (id, mut d) = photo_settings(s, c)?;
    if d.color_swaps.len() >= MAX_SWAPS {
        return Err(bad(c, format!("at most {MAX_SWAPS} color swaps")));
    }
    let mut e = ColorSwap::default();
    change(&mut e, p, c)?;
    d.color_swaps.push(e);
    let i = d.color_swaps.len() - 1;
    s.set_develop(id, d, "Add Color Swap")?;
    Ok(json!({"index": i}))
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "colorSwap.set";
    let (id, mut d) = photo_settings(s, c)?;
    let i = entry_at(&mut d, p, c)?;
    let before = d.color_swaps[i].clone();
    change(&mut d.color_swaps[i], p, c)?;
    let e = d.color_swaps[i].clone();
    if e != before {
        s.set_develop(id, d, "Color Swap")?;
    }
    Ok(entry_json(i, &e))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "colorSwap.remove";
    let (id, mut d) = photo_settings(s, c)?;
    let i = entry_at(&mut d, p, c)?;
    d.color_swaps.remove(i);
    s.set_develop(id, d, "Remove Color Swap")?;
    Ok(Value::Null)
}

fn move_to(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "colorSwap.move";
    let (id, mut d) = photo_settings(s, c)?;
    let i = entry_at(&mut d, p, c)?;
    let to = index_req(p, "to", c)?.min(d.color_swaps.len() - 1);
    if to != i {
        let e = d.color_swaps.remove(i);
        d.color_swaps.insert(to, e);
        s.set_develop(id, d, "Move Color Swap")?;
    }
    Ok(json!({"index": to}))
}

fn pick(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "colorSwap.pick";
    let (x, y) = (f64_req(p, "x", c)?, f64_req(p, "y", c)?);
    let active = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let photo = match p.get("photo").filter(|v| !v.is_null()) {
        None => active,
        Some(v) => PhotoId(v.as_u64().ok_or_else(|| bad(c, "`photo` must be a photo id (a whole number)"))?),
    };
    let target = p.get("target").and_then(Value::as_str).unwrap_or("from");
    if !matches!(target, "from" | "to") {
        return Err(bad(c, format!("unknown target {target:?} (from|to)")));
    }
    let (_, mut d) = photo_settings(s, c)?;
    let index = match p.get("index").filter(|v| !v.is_null()) {
        Some(_) => Some(entry_at(&mut d, p, c)?),
        None if d.color_swaps.len() >= MAX_SWAPS => return Err(bad(c, format!("at most {MAX_SWAPS} color swaps"))),
        None if target == "to" => return Err(bad(c, "name the swap (`index`) to give a To colour")),
        None => None,
    };
    // a From colour is the one the swap will see: the photo as developed without its swaps
    let rgb = if target == "from" { color_at_with(s, photo, x, y, c, |d| d.color_swaps.clear())? } else { color_at_with(s, photo, x, y, c, |_| {})? };
    let (_, mut d) = photo_settings(s, c)?;
    let i = match index {
        Some(i) => i,
        None => {
            d.color_swaps.push(ColorSwap::default());
            d.color_swaps.len() - 1
        }
    };
    let Some(e) = d.color_swaps.get_mut(i) else { return Err(bad(c, "the swap is gone")) };
    if target == "from" {
        e.from = Some(rgb);
    } else {
        e.to = Some(rgb);
    }
    s.set_develop(active, d, "Pick Color Swap Color")?;
    Ok(json!({"index": i, "color": to_hex(rgb)}))
}

fn list(s: &mut Session, _p: &Value) -> Result<Value> {
    let (_, d) = photo_settings(s, "colorSwap.list")?;
    Ok(json!(d.color_swaps.iter().enumerate().map(|(i, e)| entry_json(i, e)).collect::<Vec<_>>()))
}

#[cfg(test)]
#[path = "color_swap_tests.rs"]
mod tests;
