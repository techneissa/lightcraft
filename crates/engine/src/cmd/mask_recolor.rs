//! Recolor on a mask (issue #83): `mask.recolor` sets the colour the mask's area takes (keeping its shading), its
//! amount, its Range and its Colour to Replace; `mask.recolorPick` takes either colour from a point of a photo, the
//! active one or another in the library.

use lightcraft_catalog::PhotoId;
use lightcraft_pipeline::recolor::{parse_hex, to_hex};
use lightcraft_pipeline::{RenderRequest, render};
use serde_json::{Value, json};

use super::masks::{find, mask_id, masks_edit};
use super::{CommandSpec, bad, cmd, f64_req, has_active};
use crate::media::SourceLevel;
use crate::{Result, Session};

/// Long edge of the render [`pick`] samples.
const PICK_EDGE: usize = 512;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mask.recolor",
            "Recolor Mask",
            [],
            None,
            "{id?, color?: \"#rrggbb\", amount?: 0..100, range?: -100..100, from?: \"#rrggbb\"|null} — the main colour of the mask's area takes `color` and keeps its shading; other colours inside the mask stay (amount 0 = off; a new color on an unset Recolor turns it to 100; `range` widens or narrows which colours count as the main one; `from`, the Colour to Replace, picks the main colour instead of finding it, null = automatic); answers `{color, amount, range, from}`",
            has_active,
            |s, p| recolor(s, p)
        ),
        cmd!(
            "mask.recolorPick",
            "Pick Recolor Color",
            [],
            None,
            "{id?, x, y, photo?: photo id, target?: \"to\"|\"from\"} — sets the mask's Recolor color (`to`, the default) or its Colour to Replace (`from`) to the color at (x, y) (each 0..1 across the uncropped photo; outside is an error) of `photo` as developed (default: the active photo); answers `{color, amount, range, from}`",
            has_active,
            |s, p| pick(s, p)
        ),
    ]
}

/// A change to a mask's Recolor.
#[derive(Default)]
struct Change {
    /// The colour the area takes (sRGB 0..1).
    to: Option<[f64; 3]>,
    amount: Option<f64>,
    range: Option<f64>,
    /// The Colour to Replace: `Some(None)` clears it (automatic).
    from: Option<Option<[f64; 3]>>,
}

/// Applies `ch` to mask `mid` as one undo step; answers the mask's Recolor after it.
fn set(s: &mut Session, c: &str, mid: u32, ch: Change) -> Result<Value> {
    let mut out = json!({});
    masks_edit(s, c, "Recolor", |masks, _| {
        let i = find(masks, mid, c)?;
        let a = &mut masks[i].adjust;
        if let Some([r, g, b]) = ch.to {
            (a.recolor_r, a.recolor_g, a.recolor_b) = (r, g, b);
            if ch.amount.is_none() && a.recolor <= 0.0 {
                a.recolor = 100.0;
            }
        }
        if let Some(v) = ch.amount {
            a.recolor = v;
        }
        if let Some(v) = ch.range {
            a.recolor_range = v;
        }
        if let Some(from) = ch.from {
            // (black is "automatic": a picked black is nudged so it stays a choice)
            let [r, g, b] = from.map_or([0.0; 3], |f| if f.iter().all(|v| *v <= 0.0) { [1.0 / 255.0; 3] } else { f });
            (a.recolor_from_r, a.recolor_from_g, a.recolor_from_b) = (r, g, b);
        }
        let from = lightcraft_pipeline::recolor::source_rgb(a).map(to_hex);
        out = json!({"color": to_hex([a.recolor_r, a.recolor_g, a.recolor_b]), "amount": a.recolor, "range": a.recolor_range, "from": from});
        Ok(())
    })?;
    Ok(out)
}

/// An optional number parameter `key`, clamped to `lo..hi` (not a finite number is an error).
fn number(p: &Value, key: &str, lo: f64, hi: f64, c: &str) -> Result<Option<f64>> {
    match p.get(key).filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => {
            Ok(Some(v.as_f64().filter(|a| a.is_finite()).ok_or_else(|| bad(c, format!("`{key}` must be a number {lo}..{hi}")))?.clamp(lo, hi)))
        }
    }
}

/// An optional colour parameter `key` (`"#rrggbb"`).
fn colour(p: &Value, key: &str, c: &str) -> Result<Option<[f64; 3]>> {
    match p.get(key).filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => {
            let text = v.as_str().ok_or_else(|| bad(c, format!("`{key}` must be a string like \"#7a3fb0\"")))?;
            Ok(Some(parse_hex(text).ok_or_else(|| bad(c, format!("not a color: {text:?} (use \"#rrggbb\")")))?))
        }
    }
}

fn recolor(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.recolor";
    let mid = mask_id(p, s.active_mask, c)?;
    let ch = Change {
        to: colour(p, "color", c)?,
        amount: number(p, "amount", 0.0, 100.0, c)?,
        range: number(p, "range", -100.0, 100.0, c)?,
        from: match p.get("from") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(_) => Some(colour(p, "from", c)?),
        },
    };
    if ch.to.is_none() && ch.amount.is_none() && ch.range.is_none() && ch.from.is_none() {
        return Err(bad(c, "give `color`, `amount`, `range` or `from`"));
    }
    set(s, c, mid, ch)
}

/// The colour (sRGB 0..1) at normalized point `(x, y)` of photo `id` as developed: a 5 × 5 pixel average of a small
/// uncropped render.
pub(crate) fn color_at(s: &mut Session, id: PhotoId, x: f64, y: f64, c: &str) -> Result<[f64; 3]> {
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Err(bad(c, "`x` and `y` must be numbers 0..1 (a point on the photo)"));
    }
    let src = s.source_now(id, SourceLevel::Thumb).map_err(|e| bad(c, e))?;
    let info = s.source_info(id);
    let d = s.develop_of(id).unwrap_or_default();
    let req = RenderRequest { apply_crop: false, ..RenderRequest::fit(PICK_EDGE, PICK_EDGE) };
    let img = render(&src, &info, &d, &req).image;
    if img.width == 0 || img.height == 0 {
        return Err(bad(c, "the photo has no pixels to pick from"));
    }
    let (w, h) = (img.width as isize, img.height as isize);
    let (cx, cy) = ((x * img.width as f64) as isize, (y * img.height as f64) as isize);
    let mut acc = [0.0f64; 3];
    let mut n = 0.0;
    for dy in -2..=2isize {
        for dx in -2..=2isize {
            let (px, py) = ((cx + dx).clamp(0, w - 1) as usize, (cy + dy).clamp(0, h - 1) as usize);
            if let Some(p) = img.data.get(py * img.width + px) {
                for (a, v) in acc.iter_mut().zip(p) {
                    *a += f64::from(*v) / 255.0;
                }
                n += 1.0;
            }
        }
    }
    if n == 0.0 {
        return Err(bad(c, "no pixel at that point"));
    }
    Ok(acc.map(|a| a / n))
}

fn pick(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.recolorPick";
    let mid = mask_id(p, s.active_mask, c)?;
    let (x, y) = (f64_req(p, "x", c)?, f64_req(p, "y", c)?);
    let photo = match p.get("photo").filter(|v| !v.is_null()) {
        None => s.active().ok_or_else(|| bad(c, "no active photo"))?,
        Some(v) => PhotoId(v.as_u64().ok_or_else(|| bad(c, "`photo` must be a photo id (a whole number)"))?),
    };
    let rgb = color_at(s, photo, x, y, c)?;
    match p.get("target").and_then(Value::as_str).unwrap_or("to") {
        "to" => set(s, c, mid, Change { to: Some(rgb), ..Change::default() }),
        "from" => set(s, c, mid, Change { from: Some(Some(rgb)), ..Change::default() }),
        other => Err(bad(c, format!("unknown target {other:?} (to|from)"))),
    }
}

#[cfg(test)]
#[path = "mask_recolor_tests.rs"]
mod tests;
