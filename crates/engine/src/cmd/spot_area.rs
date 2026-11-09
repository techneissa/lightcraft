//! Fork feature (`fork/object-remove.md`): Remove spots in the shape of an object or a mask, each one undo step.
//!
//! - `spot.fromMask` (a mask's "Remove This Area"): the mask becomes one shaped spot.
//! - `spot.object`: a click on an object (SAM 3), ⇧ adds to and ⌥ takes from the selected Object spot.
//! - `spot.objectStroke`: a loose brush stroke snapped to the object under it; without SAM 3 a brush-shaped spot.
//!
//! Shaped spots fill with LaMa unless `fill` says Classic, and have no source.

use lightcraft_develop::{SegMask, Spot, SpotFill, SpotMode};
use lightcraft_geom::Point;
use serde_json::{Value, json};

use super::lama::{check_fill, describe};
use super::masks::{clicks_of, mask_id, segment};
use super::{CommandSpec, bad, bool_or, cmd, f64_or, has_active};
use crate::Session;
use crate::segment::MAX_CLICKS;
use crate::spot_area::{DEFAULT_EXPAND, EXPAND_RANGE, ObjectAsk, Reach, is_object_spot, stroke_clicks};

/// Why a click can't add to or take from an object spot that was moved.
pub(crate) const MOVED: &str = "This spot was moved, so clicks can't change it. Click the object again (without ⇧ or ⌥) to select it anew.";

/// The fill of a shaped spot: LaMa unless the command names Classic.
fn area_fill(c: &str, p: &Value) -> crate::Result<SpotFill> {
    let Some(v) = p.get("fill").filter(|v| !v.is_null()) else { return Ok(SpotFill::Lama) };
    let fill = v.as_str().and_then(SpotFill::from_name).ok_or_else(|| bad(c, "`fill` must be \"classic\" or \"lama\""))?;
    check_fill(c, fill, SpotMode::Remove)?;
    Ok(fill)
}

fn expand(p: &Value) -> f64 {
    let e = f64_or(p, "expand", DEFAULT_EXPAND);
    if e.is_finite() { e.clamp(EXPAND_RANGE.0, EXPAND_RANGE.1) } else { DEFAULT_EXPAND }
}

/// A selection the caller computed (`seg`, the model's photo coordinates), standing in for the model.
fn seg_given(c: &str, p: &Value) -> crate::Result<Option<SegMask>> {
    match p.get("seg").filter(|v| !v.is_null()) {
        Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|e| bad(c, format!("`seg`: {e}"))),
        None => Ok(None),
    }
}

/// A point in spot coordinates (the photo uncropped, 0..1) from `[x, y]` or `x`/`y`.
fn spot_point(c: &str, p: &Value) -> crate::Result<Point> {
    let at = match p.get("at").and_then(Value::as_array) {
        Some(a) => (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64)),
        None => (p.get("x").and_then(Value::as_f64), p.get("y").and_then(Value::as_f64)),
    };
    let (Some(x), Some(y)) = at else { return Err(bad(c, "missing the point (`x`, `y`)")) };
    if !((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)) {
        return Err(bad(c, "the point is outside the photo"));
    }
    Ok(Point::new(x, y))
}

/// Spot coordinates → the model's photo coordinates (before lens corrections, Upright and Transform).
fn to_photo(s: &Session, id: crate::PhotoId, at: Point) -> Point {
    let d = s.develop_of(id).unwrap_or_default();
    match s.mask_frame(id, &d) {
        Some(f) => {
            let n = f.for_mask(true).norm_to_mask(at);
            if n.x.is_finite() && n.y.is_finite() { Point::new(n.x.clamp(0.0, 1.0), n.y.clamp(0.0, 1.0)) } else { at }
        }
        None => at,
    }
}

fn from_mask(s: &mut Session, p: &Value) -> crate::Result<Value> {
    let c = "spot.fromMask";
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let mid = mask_id(p, s.active_mask, c)?;
    let fill = area_fill(c, p)?;
    let d = (*s.develop_of(id).unwrap_or_default()).clone();
    let m = d.masks.iter().find(|m| m.id == mid).ok_or_else(|| bad(c, format!("no mask {mid}")))?;
    if m.components.is_empty() {
        return Err(bad(c, "the mask is empty"));
    }
    let seg = s.mask_spot_area(id, &d, mid, None).map_err(|e| bad(c, e))?;
    let index = s.add_area_spot(id, lightcraft_develop::SpotArea::new("mask", seg), expand(p), fill, "Remove This Area")?;
    let mut out = json!({"index": index, "kind": "mask"});
    describe(s, fill, &mut out);
    Ok(out)
}

fn object(s: &mut Session, p: &Value) -> crate::Result<Value> {
    let c = "spot.object";
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let at = to_photo(s, id, spot_point(c, p)?);
    let (add, subtract) = (bool_or(p, "add", false), bool_or(p, "subtract", false));
    let fill = area_fill(c, p)?;
    // a click that adds or takes away works on the selected Object spot, or the one still on its way
    let target = if add || subtract {
        match super::masks::optional_index(p, "index", c)?.or(s.active_spot) {
            Some(i) if s.develop_of(id).is_some_and(|d| d.spots.get(i).is_some_and(is_object_spot)) => Some(i),
            _ if s.object_pending_new(id) => None,
            _ => return Err(bad(c, "select an object spot first (click an object to start one)")),
        }
    } else {
        None
    };
    let (mut hint, mut exclude) = match (add || subtract).then(|| s.object_pending(id, target)).flatten() {
        Some(pending) => pending,
        None => match target.and_then(|i| s.develop_of(id).and_then(|d| d.spots.get(i).and_then(|sp| sp.area.clone()))) {
            // a moved spot forgot its clicks (they no longer describe it): a click would replace its shape
            Some(a) if a.hint.is_empty() => return Err(bad(c, MOVED)),
            Some(a) => (a.hint, a.exclude),
            None => (Vec::new(), Vec::new()),
        },
    };
    if hint.len() + exclude.len() >= MAX_CLICKS {
        return Err(bad(c, format!("an object takes at most {MAX_CLICKS} clicks; start a new one")));
    }
    if subtract {
        exclude.push(at)
    } else {
        hint.push(at)
    }
    if hint.is_empty() {
        return Err(bad(c, "click the object to include it first"));
    }
    let size = target.and_then(|i| s.develop_of(id).and_then(|d| d.spots.get(i).map(|sp| sp.size)));
    let ask = ObjectAsk { target, hint, exclude, reach: None, expand: size.unwrap_or_else(|| expand(p)), fill };
    run_object(s, c, id, ask, p, add || subtract)
}

/// Make or change the Object spot `ask` describes: from `seg` when given, else from the model (queued in the app).
fn run_object(s: &mut Session, c: &str, id: crate::PhotoId, ask: ObjectAsk, p: &Value, replace: bool) -> crate::Result<Value> {
    let counts = json!({"include": ask.hint.len(), "exclude": ask.exclude.len()});
    let seg = match seg_given(c, p)? {
        Some(seg) => seg,
        None if s.segmenter.background && !bool_or(p, "wait", false) => {
            let clicks = clicks_of(&ask.hint, &ask.exclude);
            s.queue_object(id, ask, &clicks, replace)?;
            let mut out = counts;
            out["pending"] = json!(true);
            return Ok(out);
        }
        // (the model sees the photo without its spots, as in the app)
        None => segment(s, |s| {
            s.segment_clicks_request(id, &clicks_of(&ask.hint, &ask.exclude))?.wait()?.ok_or_else(|| "nothing was selected".to_string())
        })?,
    };
    let fill = ask.fill;
    let index = s.apply_object(id, seg, &ask)?;
    let mut out = counts;
    out["index"] = json!(index);
    describe(s, fill, &mut out);
    Ok(out)
}

fn object_stroke(s: &mut Session, p: &Value) -> crate::Result<Value> {
    let c = "spot.objectStroke";
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let pts: Vec<Point> = p
        .get("points")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|q| Some(Point::new(q.get(0)?.as_f64()?, q.get(1)?.as_f64()?))).collect())
        .unwrap_or_default();
    let pts: Vec<Point> = pts.into_iter().filter(|q| (0.0..=1.0).contains(&q.x) && (0.0..=1.0).contains(&q.y)).collect();
    if pts.is_empty() {
        return Err(bad(c, "missing points"));
    }
    let fill = area_fill(c, p)?;
    let radius = f64_or(p, "size", 0.02);
    let radius = if radius.is_finite() { radius.clamp(0.001, 0.25) } else { 0.02 };
    let given = seg_given(c, p)?;
    if given.is_none() && s.segmenter.require_model().is_err() {
        // no SAM 3: the stroke stays a brush-shaped spot, as without Object mode
        let mut add = json!({"mode": "remove", "points": pts.iter().map(|q| [q.x, q.y]).collect::<Vec<_>>(), "size": radius, "fill": fill.name()});
        for k in ["feather", "opacity"] {
            if let Some(v) = p.get(k) {
                add[k] = v.clone();
            }
        }
        let mut out = s.execute("spot.add", &add)?;
        out["snapped"] = json!(false);
        return Ok(out);
    }
    let hint: Vec<Point> = stroke_clicks(&pts).into_iter().map(|q| to_photo(s, id, q)).collect();
    let ask = ObjectAsk { target: None, hint, exclude: Vec::new(), reach: Some(Reach { points: pts, radius }), expand: expand(p), fill };
    let mut out = run_object(s, c, id, ask, p, false)?;
    out["snapped"] = json!(true);
    Ok(out)
}

/// The message for a lasso (or a shape) that would cover most of the photo; the same as Remove This Area's.
pub(crate) const TOO_BIG: &str = "the area covers most of the photo; Remove rebuilds smaller areas from what is around them";

/// A brushed Remove or Content-Aware stroke that closes on itself becomes a shaped spot (kind `lasso`) covering the
/// loop's inside as well as the stroke's ribbon, so drawing around an object removes the object. What counts as a
/// loop is `lightcraft_pipeline::spot_lasso`. The shape is the loop's inside (clipped to the photo) and the spot's
/// `size` (its Expand) is the brush radius, so it reaches as far past the loop, and feathers as a stroke's ribbon does.
/// The stroke is kept in the shape, so Make Stroke gives it back. Heal and Clone need a source and keep their stroke.
/// `Ok(true)` when `spot` was changed; a lasso that would cover most of the photo is an error.
pub(crate) fn lasso(s: &Session, id: crate::PhotoId, d: &lightcraft_develop::DevelopSettings, spot: &mut Spot) -> crate::Result<bool> {
    use lightcraft_pipeline::spot_lasso::{Lasso, lasso_area};
    if !matches!(spot.mode, SpotMode::Remove | SpotMode::Fill) || spot.area.is_some() {
        return Ok(false);
    }
    let Some(f) = s.mask_frame(id, d) else { return Ok(false) };
    let seg = match lasso_area(&spot.points, f.ow, f.oh, spot.size) {
        Lasso::No => return Ok(false),
        Lasso::TooBig => return Err(bad("spot.add", TOO_BIG)),
        Lasso::Area(seg) => seg,
    };
    let mut area = lightcraft_develop::SpotArea::new("lasso", seg);
    area.stroke = std::mem::take(&mut spot.points);
    area.brush = spot.size;
    spot.points = vec![crate::spot_area::centre_of(&area).unwrap_or(area.stroke.first().copied().unwrap_or(Point::new(0.5, 0.5)))];
    spot.source_offset = None;
    spot.area = Some(area);
    Ok(true)
}

/// `spot.makeStroke`: a lasso becomes the plain stroke it came from (its points and brush size come back from the shape,
/// so nothing is lost; a Classic Remove spot picks its source again).
fn make_stroke(s: &mut Session, p: &Value) -> crate::Result<Value> {
    let c = "spot.makeStroke";
    let i = super::masks::spot_index(s, p, c)?;
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let d = (*s.develop_of(id).unwrap_or_default()).clone();
    let spot = d.spots.get(i).ok_or_else(|| bad(c, "no such spot"))?;
    let Some(area) = spot.area.as_ref().filter(|a| a.kind == "lasso" && !a.stroke.is_empty()) else {
        return Err(bad(c, "this spot is not a lasso made from a stroke"));
    };
    let mut after = spot.clone();
    after.points = area.stroke.clone();
    after.size = if area.brush > 0.0 { area.brush.clamp(super::masks::SPOT_SIZE.0, super::masks::SPOT_SIZE.1) } else { spot.size };
    after.area = None;
    after.source_offset = None;
    if !sourceless(&after) && after.mode == SpotMode::Remove {
        after.source_offset = super::masks::pick_source(s, id, &d, &after, None);
    }
    super::masks::spots_edit(s, c, "Make Stroke", |spots| {
        if let Some(sp) = spots.get_mut(i) {
            *sp = after;
        }
        Ok(())
    })?;
    s.active_spot = Some(i);
    Ok(json!({"index": i}))
}

/// Whether `spot` reads no source: a shaped spot, or a Remove or Content-Aware spot LaMa fills (both rebuild the area
/// from its surroundings). Heal, Clone and a Classic Remove spot copy from their source.
pub(super) fn sourceless(spot: &Spot) -> bool {
    spot.area.is_some() || lightcraft_pipeline::spots_learned::is_learned(spot)
}

/// What `spot.update` does to spot `i`'s source when the call changes the spot's `mode` or `fill` (otherwise nothing): `Some(None)` takes it
/// away (the spot becomes sourceless), `Some(Some(_))` gives a Classic Remove spot back the source it needs, `None`
/// leaves it. A shaped spot never heals or clones and has no source to move.
pub(super) fn update_source(
    s: &mut Session,
    c: &str,
    i: usize,
    p: &Value,
    mode: Option<SpotMode>,
    fill: Option<SpotFill>,
) -> crate::Result<Option<Option<Point>>> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let d = (*s.develop_of(id).unwrap_or_default()).clone();
    let spot = d.spots.get(i).ok_or_else(|| bad(c, "no such spot"))?.clone();
    if spot.area.is_some() {
        if mode.is_some_and(|m| !matches!(m, SpotMode::Remove | SpotMode::Fill)) {
            return Err(bad(c, "a spot in an object's shape is rebuilt from its surroundings; it can't heal or clone"));
        }
        if ["source", "moveSource"].iter().any(|k| p.get(k).is_some_and(|v| !v.is_null())) {
            return Err(bad(c, "this spot has no source: it is rebuilt from its surroundings"));
        }
        return Ok(None);
    }
    // only a change of mode or fill decides the source again: a size, a move or an opacity keeps the one it has (an old
    // LaMa spot saved with a source keeps it, for where LaMa can't run)
    if mode.is_none() && fill.is_none() {
        return Ok(None);
    }
    let mut after = spot.clone();
    if let Some(m) = mode {
        after.mode = m;
        if !matches!(m, SpotMode::Remove | SpotMode::Fill) {
            after.fill = SpotFill::Classic;
        }
    }
    if let Some(f) = fill {
        after.fill = f;
    }
    if sourceless(&after) {
        return Ok(spot.source_offset.is_some().then_some(None));
    }
    let gave_source = p.get("source").is_some_and(|v| !v.is_null());
    let had_none = sourceless(&spot) || spot.mode == SpotMode::Fill;
    if after.mode == SpotMode::Remove && after.source_offset.is_none() && had_none && !gave_source {
        return Ok(Some(super::masks::pick_source(s, id, &d, &after, None)));
    }
    Ok(None)
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "spot.fromMask",
            "Remove This Area",
            [],
            None,
            "{id?: maskId (the selected mask), expand?: 0.001..0.05 of the long edge past its outline (0.005), fill?: classic|lama (default lama)} — the mask (any kind: Object, Describe, Subject, Brush, a gradient…) becomes one Remove spot in its shape, rebuilt from its surroundings (no source); the mask stays; one undo step → {index, kind}",
            has_active,
            from_mask
        ),
        cmd!(
            "spot.makeStroke",
            "Make Stroke",
            [],
            None,
            "{index? (the selected spot)} — a lasso (a stroke that closed on itself and covers its inside) becomes the plain stroke it came from, without its inside; one undo step → {index}",
            has_active,
            make_stroke
        ),
        cmd!(
            "spot.object",
            "Remove Object",
            [],
            None,
            "{x, y (or at: [x, y]): a point on the photo (the spots' coordinates), add?: bool (⇧: add to the selected Object spot), subtract?: bool (⌥: leave this part out of it), index?: the Object spot to change, expand?: 0.001..0.05 (0.005), fill?: classic|lama (default lama), seg?: a selection in the model's photo coordinates (stands in for SAM 3), wait?: bool} — SAM 3 selects the object under the click and one Remove spot in its shape takes it out (one undo step). Needs SAM 3; in the app it returns {pending} and the spot appears when the model has answered → {index, include, exclude}",
            has_active,
            object
        ),
        cmd!(
            "spot.objectStroke",
            "Remove Object (Brush)",
            [],
            None,
            "{points: [[x, y], …] (the spots' coordinates), size?: the brush radius (long edge), feather?, opacity?, expand?, fill?: classic|lama (default lama), seg?, wait?} — a loose stroke over an object: SAM 3 snaps it to the object's outline (within a few brush radii of the stroke) and one Remove spot in that shape takes it out. Without SAM 3 the stroke is a brush-shaped spot (spot.add) → {index, snapped}",
            has_active,
            object_stroke
        ),
    ]
}
