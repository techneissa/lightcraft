//! A profile file: its name, kind of photography, examples (features and targets, never pixels), the groups it
//! learns, and the trained model with its accuracy. JSON, versioned, at most [`MAX_FILE`] bytes; everything read
//! is checked (lengths, finite numbers, known ids), so a damaged or hostile file is an error, never a panic or a
//! wrong edit.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ridge::Layout;
use crate::sliders::{Group, SLIDERS};
use crate::train::{self, Example, MAX_EXAMPLES, Model, Report};
use crate::{Error, Result};

/// What the file says it is.
pub const FORMAT: &str = "lightcraft-learn-profile";
/// The file version this build writes and reads.
pub const VERSION: u32 = 1;
/// Largest profile file read.
pub const MAX_FILE: u64 = 16 << 20;
/// File name extension.
pub const EXTENSION: &str = "lclearn";
/// Longest name and kind kept.
const MAX_NAME: usize = 100;

/// Which features the examples carry: the photo statistics, and the encoder that made the rest (`None`: none).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Features {
    pub encoder: Option<String>,
    pub layout: Layout,
}

/// A Learn My Edits profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub format: String,
    pub version: u32,
    pub name: String,
    /// The kind of photography ("Portraits", "Landscapes", …), free text.
    #[serde(default)]
    pub kind: String,
    pub features: Features,
    /// Which groups it learns.
    pub groups: Vec<Group>,
    /// Slider ids, in the order of each example's targets.
    pub sliders: Vec<String>,
    pub examples: Vec<Example>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<Report>,
    /// Why the last training made no model, when it didn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

fn clean_name(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect::<String>().trim().to_string()
}

impl Profile {
    /// An empty profile.
    pub fn new(name: &str, kind: &str, features: Features, groups: &[Group]) -> Profile {
        let mut groups = groups.to_vec();
        groups.sort();
        groups.dedup();
        Profile {
            format: FORMAT.into(),
            version: VERSION,
            name: clean_name(name),
            kind: clean_name(kind),
            features,
            groups,
            sliders: SLIDERS.iter().map(|s| s.id.to_string()).collect(),
            examples: Vec::new(),
            model: None,
            report: None,
            problem: None,
        }
    }

    /// Add an example, or replace the one of the same photo. The model is out of date until [`Profile::train`].
    pub fn add_example(&mut self, example: Example) -> Result<()> {
        if example.features.len() != self.features.layout.dims() || example.targets.len() != SLIDERS.len() {
            return Err(Error::Input("the example does not match the profile's features".into()));
        }
        if example.features.iter().chain(&example.targets).any(|v| !v.is_finite()) {
            return Err(Error::Input("the example has numbers that are not finite".into()));
        }
        if let Some(e) = self.examples.iter_mut().find(|e| e.photo == example.photo) {
            *e = example;
        } else if self.examples.len() >= MAX_EXAMPLES {
            return Err(Error::Input(format!("a profile holds at most {MAX_EXAMPLES} examples")));
        } else {
            self.examples.push(example);
        }
        self.model = None;
        self.report = None;
        Ok(())
    }

    /// Remove the example of `photo`; true when there was one.
    pub fn remove_example(&mut self, photo: &str) -> bool {
        let before = self.examples.len();
        self.examples.retain(|e| e.photo != photo);
        let removed = self.examples.len() != before;
        if removed {
            self.model = None;
            self.report = None;
        }
        removed
    }

    /// Train on the examples. A failure (too few, nothing in common, cancelled) leaves no model and says why in
    /// [`Profile::problem`], except a cancel, which keeps the profile as it was.
    pub fn train(&mut self, cancel: &dyn Fn() -> bool, progress: &mut dyn FnMut(usize, usize)) -> Result<&Report> {
        match train::train(&self.examples, self.features.layout, &self.groups, cancel, progress) {
            Ok((model, report)) => {
                self.model = Some(model);
                self.problem = None;
                Ok(self.report.insert(report))
            }
            Err(Error::Cancelled) => Err(Error::Cancelled),
            Err(e) => {
                self.model = None;
                self.report = None;
                self.problem = Some(e.to_string());
                Err(e)
            }
        }
    }

    /// The learned changes for a photo with features `f`: `(slider index into SLIDERS, target)`.
    pub fn predict(&self, f: &[f32]) -> Result<Vec<(usize, f32)>> {
        let Some(model) = &self.model else {
            return Err(Error::Input(match &self.problem {
                Some(p) => format!("profile \"{}\" has no model: {p}", self.name),
                None => format!("profile \"{}\" is not trained yet", self.name),
            }));
        };
        if f.len() != self.features.layout.dims() || f.iter().any(|v| !v.is_finite()) {
            return Err(Error::Input("the photo's features do not match the profile".into()));
        }
        let x: Vec<Vec<f32>> = self.examples.iter().take(model.examples).map(|e| model.standardizer.apply(&e.features)).collect();
        Ok(train::predict(model, &x, f))
    }

    /// The groups the trained model writes.
    pub fn learned_groups(&self) -> Vec<Group> {
        self.model.as_ref().map(|m| m.groups.iter().map(|g| g.group).collect()).unwrap_or_default()
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| Error::Profile(e.to_string()))
    }

    /// Parse and check a profile file's text.
    pub fn from_json(text: &str) -> Result<Profile> {
        if text.len() as u64 > MAX_FILE {
            return Err(Error::Profile(format!("larger than {} MB", MAX_FILE >> 20)));
        }
        let head: serde_json::Value = serde_json::from_str(text).map_err(|e| Error::Profile(format!("not a profile file ({e})")))?;
        if head.get("format").and_then(serde_json::Value::as_str) != Some(FORMAT) {
            return Err(Error::Profile("not a Learn My Edits profile".into()));
        }
        match head.get("version").and_then(serde_json::Value::as_u64) {
            Some(v) if v == VERSION as u64 => {}
            Some(v) if v > VERSION as u64 => return Err(Error::Profile(format!("made by a newer LightCraft (version {v}); update LightCraft"))),
            _ => return Err(Error::Profile("unknown version".into())),
        }
        let p: Profile = serde_json::from_value(head).map_err(|e| Error::Profile(format!("damaged ({e})")))?;
        p.validate()?;
        Ok(p)
    }

    /// Read a profile file.
    pub fn read(path: &Path) -> Result<Profile> {
        let len = std::fs::metadata(path).map_err(|e| Error::Profile(format!("{}: {e}", path.display())))?.len();
        if len > MAX_FILE {
            return Err(Error::Profile(format!("{}: larger than {} MB", path.display(), MAX_FILE >> 20)));
        }
        let text = std::fs::read_to_string(path).map_err(|e| Error::Profile(format!("{}: {e}", path.display())))?;
        Profile::from_json(&text).map_err(|e| Error::Profile(format!("{}: {e}", path.display())))
    }

    /// Write the profile atomically (a temporary file renamed over the old one).
    pub fn write(&self, path: &Path) -> Result<()> {
        let text = self.to_json()?;
        let tmp = path.with_extension(format!("{EXTENSION}.tmp"));
        std::fs::write(&tmp, text).map_err(|e| Error::Profile(format!("{}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            Error::Profile(format!("{}: {e}", path.display()))
        })
    }

    fn validate(&self) -> Result<()> {
        let bad = |what: &str| Err(Error::Profile(format!("damaged ({what})")));
        if self.name.trim().is_empty() || self.name.chars().count() > MAX_NAME || self.kind.chars().count() > MAX_NAME {
            return bad("name");
        }
        let layout = self.features.layout;
        if layout.photo != crate::PHOTO_DIMS {
            return Err(Error::Profile("made with other features; train it again from its photos".into()));
        }
        match (self.features.encoder.as_deref(), layout.encoder) {
            (None, 0) => {}
            (Some(crate::ENCODER_ID), 768) => {}
            (Some(_), _) => return Err(Error::Profile("made with another image model; train it again from its photos".into())),
            (None, _) => return bad("features"),
        }
        if self.sliders.len() != SLIDERS.len() || self.sliders.iter().zip(SLIDERS).any(|(a, b)| a != b.id) {
            return Err(Error::Profile("made by another version of LightCraft; train it again from its photos".into()));
        }
        if self.examples.len() > MAX_EXAMPLES || self.groups.len() > Group::ALL.len() {
            return bad("too many examples");
        }
        for e in &self.examples {
            if e.photo.len() > 4096 || e.features.len() != layout.dims() || e.targets.len() != SLIDERS.len() {
                return bad("an example's size");
            }
            if e.features.iter().chain(&e.targets).any(|v| !v.is_finite() || v.abs() > 1e6) {
                return bad("an example's numbers");
            }
        }
        if let Some(m) = &self.model {
            let n = m.examples;
            if m.layout != layout || n > self.examples.len() || n == 0 {
                return bad("model size");
            }
            if m.standardizer.mean.len() != layout.dims() || m.standardizer.scale.len() != layout.dims() {
                return bad("model features");
            }
            if m.standardizer.mean.iter().chain(&m.standardizer.scale).any(|v| !v.is_finite()) || m.standardizer.scale.iter().any(|s| *s <= 0.0) {
                return bad("model features");
            }
            for g in &m.groups {
                let t = g.sliders.len();
                if t == 0 || g.mean.len() != t || Some(g.alpha.len()) != n.checked_mul(t) {
                    return bad("model weights");
                }
                if g.sliders.iter().any(|id| crate::sliders::find(id).is_none_or(|s| s.group != g.group)) {
                    return bad("model sliders");
                }
                if g.mean.iter().chain(&g.alpha).any(|v| !v.is_finite()) {
                    return bad("model weights");
                }
                if !g.kernel.encoder_weight.is_finite() || g.kernel.encoder_weight < 0.0 || g.kernel.rbf.is_some_and(|r| !r.is_finite() || r <= 0.0) {
                    return bad("model kernel");
                }
            }
        }
        Ok(())
    }
}
