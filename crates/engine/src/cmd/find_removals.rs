//! `spot.findPeople`, `spot.findWires` and the commands that act on what they find: select a candidate, remove the
//! selected (or all) with content-aware spots in one undo step, forget them. See [`crate::find_removals`].

use lightcraft_develop::{Spot, SpotMode};
use lightcraft_geom::Point;
use lightcraft_pipeline::find::{Kind, SPOT_FEATHER};
use serde_json::{Value, json};

use super::masks::SPOT_SIZE;
use super::{CommandSpec, bad, bool_or, cmd, f64_or, has_active};
use crate::Session;
use crate::find_removals::{Entry, Job};

fn candidate_json(e: &Entry) -> Value {
    let c = &e.candidate;
    json!({
        "id": e.id,
        "kind": c.kind.name(),
        "source": c.source,
        "main": c.main,
        "selected": e.selected,
        "area": c.area,
        "score": c.score,
        "bbox": c.bbox,
        "closed": c.closed,
        "outline": c.outline.iter().map(|p| json!([p.0, p.1])).collect::<Vec<_>>(),
        "spots": c.spots.len(),
        "dabs": c.spots.iter().map(|s| s.points.len()).sum::<usize>(),
    })
}

/// The candidates of the active photo, and what the searches are doing.
fn state_json(s: &Session) -> Value {
    let v = s.finds.view(s.active());
    json!({
        "running": v.running.iter().map(|k| k.name()).collect::<Vec<_>>(),
        "candidates": v.entries.iter().map(candidate_json).collect::<Vec<_>>(),
        "text": v.last.as_ref().map(|o| o.text.clone()),
        "error": v.last.as_ref().is_some_and(|o| o.error),
    })
}

/// Spots for `entries` appended to the active photo's, in one undo step. Returns (spots added, index of the first).
fn add_spots(s: &mut Session, c: &str, entries: &[Entry]) -> crate::Result<(usize, usize)> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
    let first = d.spots.len();
    for e in entries {
        for plan in &e.candidate.spots {
            if plan.points.is_empty() {
                continue;
            }
            d.spots.push(Spot {
                mode: SpotMode::Fill,
                points: plan.points.iter().map(|p| Point::new(p.0, p.1)).collect(),
                size: plan.size.clamp(SPOT_SIZE.0, SPOT_SIZE.1),
                feather: SPOT_FEATHER,
                opacity: 100.0,
                source_offset: None,
            });
        }
    }
    let added = d.spots.len() - first;
    if added == 0 {
        return Err(bad(c, "nothing to remove"));
    }
    let label =
        match (entries.iter().any(|e| e.candidate.kind == Kind::Person), entries.iter().any(|e| e.candidate.kind == Kind::Wire), entries.len()) {
            (true, false, 1) => "Remove Person",
            (true, false, _) => "Remove People",
            (false, true, 1) => "Remove Wire",
            (false, true, _) => "Remove Wires",
            _ => "Remove People and Wires",
        };
    s.set_develop(id, d, label)?;
    s.active_spot = Some(first);
    Ok((added, first))
}

/// A search command: start it, and report the candidates when it has finished by the time it returns.
fn find(s: &mut Session, c: &str, job: Job, p: &Value) -> crate::Result<Value> {
    let wait = bool_or(p, "wait", false);
    let add = bool_or(p, "add", false);
    if add && !wait && s.segmenter.background {
        return Err(bad(c, "`add` needs `wait: true` here (the app finds in the background); remove with spot.removeCandidates once found"));
    }
    let started = s.start_find(c, job, wait)?;
    if !started.finished {
        return Ok(json!({"started": true, "kind": job.kind().name()}));
    }
    let mut out = state_json(s);
    if add {
        // only this search's own kind: the other kind's candidates (found earlier) stay listed
        let kind = job.kind();
        let ids: Vec<u32> = s.finds.view(s.active()).entries.iter().filter(|e| e.candidate.kind == kind).map(|e| e.id).collect();
        let mine = s.finds.take(s.active(), Some(&ids), false);
        if !mine.is_empty() {
            let (added, _) = add_spots(s, c, &mine)?;
            out["added"] = json!(mine.len());
            out["spots"] = json!(added);
        } else {
            out["added"] = json!(0);
        }
    }
    Ok(out)
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "spot.findPeople",
            "Find People",
            [],
            None,
            "{includeMain?: bool (false: the largest, most central person, however small, is the main subject and not proposed), wait?: bool, add?: bool (remove everything found at once; needs wait)} — finds people with SAM 3's \"person\" mask (rough shapes from faces when only the YuNet detector is installed), on a thread with a row in the activity stack; nothing is added to the photo. Without `wait` the app returns {started} and spot.candidates has the result → {candidates: [{id, kind, source, main, selected, area, score, bbox, outline, spots}], text}",
            has_active,
            |s, p| {
                let include_main = bool_or(p, "includeMain", false);
                find(s, "spot.findPeople", Job::People { include_main }, p)
            }
        ),
        cmd!(
            "spot.findWires",
            "Find Wires",
            [],
            None,
            "{sensitivity?: 0..100 (50), wait?: bool, add?: bool (remove everything found at once; needs wait)} — finds power lines and cables (long, thin, dark, gently curved lines on smooth bright areas such as sky) with a pure-Rust thin-line detector; nothing is added to the photo → as spot.findPeople",
            has_active,
            |s, p| {
                let sensitivity = f64_or(p, "sensitivity", 50.0).clamp(0.0, 100.0) as f32;
                find(s, "spot.findWires", Job::Wires { sensitivity }, p)
            }
        ),
        cmd!(
            query "spot.candidates",
            "Show Found People and Wires",
            [],
            None,
            "{} — the candidates Find People / Find Wires found on the active photo → {running: [kind], candidates: [...], text, error}",
            has_active,
            |s, _| Ok(state_json(s))
        ),
        cmd!(
            "spot.selectCandidate",
            "Select Found Item",
            [],
            None,
            "{id?: n|null, at?: [x,y] (a click, 0..1; empty ground clears), tolerance?: 0..1 of the long edge (0.01, for wires), toggle?: bool (keep the others selected)} → {selected: [ids]}",
            has_active,
            |s, p| {
                let c = "spot.selectCandidate";
                let photo = s.active();
                let view = s.finds.view(photo);
                let toggle = bool_or(p, "toggle", false);
                let at = match p.get("at") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(
                        v.as_array()
                            .and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)))
                            .filter(|(x, y)| x.is_finite() && y.is_finite())
                            .ok_or_else(|| bad(c, "`at` must be a click [x, y], 0..1 of the photo"))?,
                    ),
                };
                let id = if let Some((x, y)) = at {
                    let tol = f64_or(p, "tolerance", 0.01).clamp(0.0, 0.2);
                    // the smallest thing under the click, a wire before a person
                    let mut hits: Vec<&Entry> = view.entries.iter().filter(|e| e.candidate.hit(x, y, tol)).collect();
                    hits.sort_by(|a, b| {
                        (a.candidate.kind == Kind::Person, a.candidate.area)
                            .partial_cmp(&(b.candidate.kind == Kind::Person, b.candidate.area))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    hits.first().map(|e| e.id)
                } else if p.get("id").is_some_and(|v| v.is_null()) || p.get("id").is_none() {
                    None
                } else {
                    Some(p["id"].as_u64().and_then(|v| u32::try_from(v).ok()).ok_or_else(|| bad(c, "`id` must be a candidate id"))?)
                };
                let selected = match id {
                    Some(id) => !(toggle && view.entries.iter().any(|e| e.id == id && e.selected)),
                    None => false,
                };
                if let Some(id) = id
                    && !s.finds.select(photo, Some(id), selected, !toggle)
                {
                    return Err(bad(c, "no such candidate (it may have been removed)"));
                }
                if id.is_none() && !toggle {
                    s.finds.select(photo, None, false, true);
                }
                let v = s.finds.view(photo);
                Ok(json!({"selected": v.entries.iter().filter(|e| e.selected).map(|e| e.id).collect::<Vec<_>>()}))
            }
        ),
        cmd!(
            "spot.removeCandidates",
            "Remove Found Items",
            [],
            None,
            "{ids?: [n], all?: bool} — remove the selected candidates (or `ids`, or `all`) with content-aware spots, grown a little, one undo step; each spot is then an ordinary editable spot → {removed, spots, index}",
            has_active,
            |s, p| {
                let c = "spot.removeCandidates";
                let ids: Option<Vec<u32>> = match p.get("ids") {
                    Some(v) => Some(
                        v.as_array()
                            .ok_or_else(|| bad(c, "`ids` must be a list of candidate ids"))?
                            .iter()
                            .map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok()))
                            .collect::<Option<Vec<u32>>>()
                            .ok_or_else(|| bad(c, "`ids` must be a list of candidate ids"))?,
                    ),
                    None => None,
                };
                let all = bool_or(p, "all", false);
                if ids.is_none() && !all && !s.finds.view(s.active()).entries.iter().any(|e| e.selected) {
                    return Err(bad(c, "nothing is selected: select a found item, or pass `all: true`"));
                }
                let taken = s.finds.take(s.active(), ids.as_deref(), ids.is_none() && !all);
                if taken.is_empty() {
                    return Err(bad(c, "no such candidate (it may have been removed)"));
                }
                let (spots, index) = match add_spots(s, c, &taken) {
                    Ok(r) => r,
                    Err(e) => {
                        // nothing was added: keep the candidates
                        s.finds.restore(s.active(), taken);
                        return Err(e);
                    }
                };
                Ok(json!({"removed": taken.len(), "spots": spots, "index": index}))
            }
        ),
        cmd!(
            "spot.clearCandidates",
            "Clear Found Items",
            [],
            None,
            "{} — forget the candidates and cancel searches in progress",
            has_active,
            |s, _| {
                s.finds.cancel();
                s.finds.clear();
                Ok(json!({}))
            }
        ),
    ]
}
