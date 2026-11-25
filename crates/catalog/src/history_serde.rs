//! How a photo's History is stored (fork format 2, issue #77): the first step whole, each later one
//! as a [`Patch`] of the step before (`{"label": …, "patch": …}`). Each step holds the photo's
//! whole develop settings, and with brush masks those hold every stroke: 200 steps of a photo
//! painted with 300 strokes took about 100 MB of `catalog.snap`, written again at every
//! compaction and quit.
//!
//! Reading takes either form, step by step, so older catalogs (every step whole) still load. A
//! step that can't be rebuilt (a patch that doesn't fit, neither settings nor a patch) ends the
//! History there, with a warning: the photo and the rest of the catalog still load, and
//! [`take_cut`] tells `Journal::open` to keep a copy of the file before it is written again.

use std::cell::Cell;
use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use serde::ser::{Error as _, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::HistoryStep;
use crate::delta::{self, Patch};

thread_local! {
    /// Histories cut short on this thread since the last [`take_cut`].
    static CUT: Cell<usize> = const { Cell::new(0) };
}

/// How many histories were cut short on this thread since the last call (and start again at 0).
pub(crate) fn take_cut() -> usize {
    CUT.with(|c| c.replace(0))
}

fn cut() {
    CUT.with(|c| c.set(c.get().saturating_add(1)));
}

#[derive(Serialize)]
struct StepOut<'a> {
    label: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    settings: Option<&'a DevelopSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    patch: Option<Patch>,
}

/// A step as read: its settings as JSON, so a patch of the next step applies to exactly what was
/// written (inside a snapshot a mask's grid is a reference into the snapshot's table, which the
/// settings resolve only later, in `Catalog::repair`).
#[derive(Deserialize)]
struct StepIn {
    label: String,
    #[serde(default)]
    settings: Option<Value>,
    #[serde(default)]
    patch: Option<Patch>,
}

pub fn serialize<S: Serializer>(history: &[HistoryStep], s: S) -> Result<S::Ok, S::Error> {
    let mut seq = s.serialize_seq(Some(history.len()))?;
    let mut prev: Option<(&Arc<DevelopSettings>, Value)> = None;
    for step in history {
        let out = match prev.take() {
            Some((p, v)) if Arc::ptr_eq(p, &step.settings) => {
                prev = Some((p, v));
                StepOut { label: &step.label, settings: None, patch: Some(Patch::Same) }
            }
            Some((_, v)) => {
                let now = serde_json::to_value(&*step.settings).map_err(S::Error::custom)?;
                let patch = delta::diff(&v, &now);
                prev = Some((&step.settings, now));
                StepOut { label: &step.label, settings: None, patch: Some(patch) }
            }
            None => {
                prev = Some((&step.settings, serde_json::to_value(&*step.settings).map_err(S::Error::custom)?));
                StepOut { label: &step.label, settings: Some(&step.settings), patch: None }
            }
        };
        seq.serialize_element(&out)?;
    }
    seq.end()
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<HistoryStep>, D::Error> {
    let steps = Vec::<StepIn>::deserialize(d)?;
    let mut out: Vec<HistoryStep> = Vec::with_capacity(steps.len().min(crate::HISTORY_LIMIT));
    // the step before as JSON, as written
    let mut prev: Option<Value> = None;
    for (i, s) in steps.into_iter().enumerate() {
        let earlier = out.last().map(|l| &l.settings);
        let settings = match (s.settings, s.patch) {
            (None, Some(Patch::Same)) if prev.is_some() => earlier.cloned(),
            (Some(v), _) => typed(v, &mut prev, earlier).ok(),
            (None, Some(patch)) => prev.take().and_then(|mut v| delta::apply(&mut v, &patch).ok().and_then(|()| typed(v, &mut prev, earlier).ok())),
            (None, None) => None,
        };
        let Some(settings) = settings else {
            log::warn!("History step {i} ({:?}) can't be rebuilt: the History ends before it", s.label);
            cut();
            break;
        };
        out.push(HistoryStep { label: s.label, settings });
    }
    Ok(out)
}

/// The settings `v` holds, its brush strokes sharing the points of the same strokes in the step before (`earlier`):
/// a History of a photo painted with many strokes holds each stroke once, as it does while painting (issue #370).
/// `v` becomes the step before.
fn typed(v: Value, prev: &mut Option<Value>, earlier: Option<&Arc<DevelopSettings>>) -> Result<Arc<DevelopSettings>, serde_json::Error> {
    let mut s = DevelopSettings::deserialize(&v)?;
    if let Some(e) = earlier {
        s.share_strokes_with(e);
    }
    *prev = Some(v);
    Ok(Arc::new(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Photo;

    fn step(label: &str, exposure: f64) -> HistoryStep {
        let mut d = DevelopSettings::default();
        d.light.exposure = exposure;
        HistoryStep { label: label.into(), settings: Arc::new(d) }
    }

    fn photo(history: Vec<HistoryStep>) -> Photo {
        let mut p = Photo::new(crate::PhotoId(1), crate::Source::File { path: "/p/a.jpg".into() }, "a.jpg", "JPEG", 4, 3, "2026-01-01");
        p.history = history;
        p
    }

    #[test]
    fn history_round_trips_and_later_steps_are_patches() {
        let a = step("Import", 0.0);
        let same = HistoryStep { label: "Again".into(), settings: a.settings.clone() };
        let p = photo(vec![a, same, step("Exposure", 1.0), step("Exposure", 1.5)]);
        let text = serde_json::to_string(&p).unwrap();
        assert_eq!(text.matches("\"settings\"").count(), 1, "{text}");
        let back: Photo = serde_json::from_str(&text).unwrap();
        assert_eq!(back, p);
        assert!(Arc::ptr_eq(&back.history[0].settings, &back.history[1].settings), "an unchanged step shares its settings");
    }

    /// A painted History saved and read again is the same History, and its steps share the strokes they have in
    /// common, as they did before it was saved (issue #370).
    #[test]
    fn a_painted_history_round_trips_and_shares_its_strokes() {
        use lightcraft_develop::{BrushStroke, Mask, MaskComponent, MaskShape};
        use lightcraft_geom::Point;
        let mut strokes = Vec::new();
        let mut steps = Vec::new();
        for k in 0..4 {
            let y = 0.1 * f64::from(k);
            strokes.push(BrushStroke {
                points: vec![Point::new(0.1, y), Point::new(0.9, -y)].into(),
                pressure: vec![0.5, 1.0].into(),
                ..Default::default()
            });
            let part = MaskComponent { name: None, op: Default::default(), invert: false, shape: MaskShape::Brush { strokes: strokes.clone() } };
            let d = DevelopSettings { masks: vec![Mask { id: 1, components: vec![part], ..Default::default() }], ..Default::default() };
            steps.push(HistoryStep { label: "Brush".into(), settings: Arc::new(d) });
        }
        let p = photo(steps);
        let back: Photo = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
        let first = |s: &HistoryStep| match &s.settings.masks[0].components[0].shape {
            MaskShape::Brush { strokes } => Some((strokes[0].points.clone(), strokes[0].pressure.clone())),
            _ => None,
        };
        let (points, pressure) = first(&back.history[0]).unwrap();
        for step in &back.history[1..] {
            let (p2, q2) = first(step).unwrap();
            assert!(Arc::ptr_eq(&points, &p2) && Arc::ptr_eq(&pressure, &q2), "the first stroke is held once");
        }
    }

    #[test]
    fn whole_steps_of_older_catalogs_load() {
        let p = photo(vec![step("Import", 0.0), step("Exposure", 1.0)]);
        let mut v = serde_json::to_value(&p).unwrap();
        v["history"] = serde_json::json!([
            {"label": "Import", "settings": serde_json::to_value(&*p.history[0].settings).unwrap()},
            {"label": "Exposure", "settings": serde_json::to_value(&*p.history[1].settings).unwrap()},
        ]);
        let back: Photo = serde_json::from_value(v).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn a_step_that_cannot_be_rebuilt_ends_the_history_there() {
        let p = photo(vec![step("Import", 0.0), step("Exposure", 1.0), step("Exposure", 2.0)]);
        let good = serde_json::to_value(&p).unwrap();
        let bad_patch = serde_json::json!({"a": {"n": 9, "h": 0, "t": 0, "v": []}});
        for bad in [
            serde_json::json!({"label": "X", "patch": bad_patch}),
            serde_json::json!({"label": "X"}),
            serde_json::json!({"label": "X", "patch": {"o": {"s": {"light": {"v": "not settings"}}}}}),
        ] {
            let mut v = good.clone();
            v["history"][1] = bad;
            let back: Photo = serde_json::from_value(v).unwrap();
            assert_eq!(back.history, p.history[..1].to_vec());
        }
        // a patch first, with no step before it
        let mut v = good.clone();
        v["history"][0] = serde_json::json!({"label": "X", "patch": "="});
        let back: Photo = serde_json::from_value(v).unwrap();
        assert!(back.history.is_empty());
    }

    /// A History written before several lights (#299): its first step holds Relight's block in the first form and
    /// the next steps are patches of it, which read as they were written.
    #[test]
    fn a_history_of_the_first_form_of_relight_reads_as_one_light() {
        let first = serde_json::json!({"label": "Relight", "settings": {"fork_relight": {"amount": 60.0, "darken": 10.0}}});
        let next = serde_json::json!({"label": "Relight", "patch": {"o": {"s": {"fork_relight": {"o": {"s": {"amount": {"v": 70.0}}}}}}}});
        let steps: Vec<HistoryStep> = deserialize(serde_json::json!([first, next])).unwrap();
        let amounts: Vec<_> = steps.iter().map(|s| (s.settings.relight.lights[0].amount, s.settings.relight.darken)).collect();
        assert_eq!(amounts, vec![(60.0, 10.0), (70.0, 10.0)]);
    }
}
