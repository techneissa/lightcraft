//! How a photo's History is stored (format 14, issue #77): the first step whole, each later one
//! as a [`Patch`] of the step before (`{"label": …, "patch": …}`). Each step holds the photo's
//! whole develop settings, and with brush masks those hold every stroke: 200 steps of a photo
//! painted with 300 strokes took about 100 MB of `catalog.snap`, written again at every
//! compaction and quit.
//!
//! Reading takes either form, step by step, so older catalogs (every step whole) still load. A
//! step that can't be rebuilt (a patch that doesn't fit, neither settings nor a patch) ends the
//! History there, with a warning: the photo and the rest of the catalog still load.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use serde::ser::{Error as _, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::HistoryStep;
use crate::delta::{self, Patch};

#[derive(Serialize)]
struct StepOut<'a> {
    label: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    settings: Option<&'a DevelopSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    patch: Option<Patch>,
}

#[derive(Deserialize)]
struct StepIn {
    label: String,
    #[serde(default)]
    settings: Option<Arc<DevelopSettings>>,
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
    // the step before as JSON, made when a patch first needs it
    let mut prev: Option<Value> = None;
    for (i, s) in steps.into_iter().enumerate() {
        let settings = match (s.settings, s.patch) {
            (Some(settings), _) => {
                prev = None;
                settings
            }
            (None, Some(Patch::Same)) if out.last().is_some() => out.last().map(|l| l.settings.clone()).unwrap_or_default(),
            (None, Some(patch)) => match rebuild(out.last(), &mut prev, &patch) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("History step {i} ({:?}) can't be rebuilt ({e}): the History ends before it", s.label);
                    break;
                }
            },
            (None, None) => {
                log::warn!("History step {i} ({:?}) holds no settings: the History ends before it", s.label);
                break;
            }
        };
        out.push(HistoryStep { label: s.label, settings });
    }
    Ok(out)
}

fn rebuild(last: Option<&HistoryStep>, prev: &mut Option<Value>, patch: &Patch) -> Result<Arc<DevelopSettings>, String> {
    let last = last.ok_or("a patch with no step before it")?;
    let mut v = match prev.take() {
        Some(v) => v,
        None => serde_json::to_value(&*last.settings).map_err(|e| e.to_string())?,
    };
    delta::apply(&mut v, patch)?;
    let s = Arc::new(DevelopSettings::deserialize(&v).map_err(|e| e.to_string())?);
    *prev = Some(v);
    Ok(s)
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
}
