//! Fork feature (fork/birefnet-masks.md, issue #48): the selection models behind Subject and Background parts —
//! SAM 3, BiRefNet (fine edges) and "SAM 3 + BiRefNet Edges" — which of them are installed, which one new parts use
//! (the Masking dropdown, kept across restarts), and the BiRefNet download.
//!
//! A part keeps the model that made it (`SegMask::model`); changing the choice never touches existing parts.

use std::path::{Path, PathBuf};

use super::{DownloadStatus, Segmenter};

/// A model that can make a Subject or Background selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionModel {
    /// SAM 3: finds the subject; coarse edges (the only model before BiRefNet).
    #[default]
    Sam3,
    /// BiRefNet: cuts out the main subject with very fine edges.
    BiRefNet,
    /// SAM 3 decides what the subject is, BiRefNet draws its edge.
    Combined,
}

impl SelectionModel {
    pub const ALL: [SelectionModel; 3] = [SelectionModel::Sam3, SelectionModel::BiRefNet, SelectionModel::Combined];

    /// The id stored with the choice and with each selection (`SegMask::model`; SAM 3 stores none).
    pub fn id(self) -> &'static str {
        match self {
            SelectionModel::Sam3 => "sam3",
            SelectionModel::BiRefNet => "birefnet",
            SelectionModel::Combined => "sam3+birefnet",
        }
    }

    /// What the user reads (Title Case).
    pub fn label(self) -> &'static str {
        match self {
            SelectionModel::Sam3 => "SAM 3",
            SelectionModel::BiRefNet => "BiRefNet",
            SelectionModel::Combined => "SAM 3 + BiRefNet Edges",
        }
    }

    pub fn parse(id: &str) -> Option<SelectionModel> {
        SelectionModel::ALL.into_iter().find(|m| m.id() == id.trim())
    }

    /// The id a selection made by this model stores (SAM 3: none, so its documents stay as before).
    pub fn stored_id(self) -> Option<&'static str> {
        (self != SelectionModel::Sam3).then(|| self.id())
    }

    pub fn uses_sam3(self) -> bool {
        self != SelectionModel::BiRefNet
    }

    pub fn uses_birefnet(self) -> bool {
        self != SelectionModel::Sam3
    }
}

/// What a stored selection's model id reads as: `None` is SAM 3; an id this build does not know is shown as it is.
pub fn label_of(model: Option<&str>) -> String {
    // (an id from a document: one level only, and an unknown one shown shortened)
    let plain = |id: &str| SelectionModel::parse(id).map_or_else(|| id.chars().take(60).collect(), |m| m.label().to_string());
    match model {
        None => SelectionModel::Sam3.label().to_string(),
        // (an edge redrawn by BiRefNet-matting, or another edge model: `fork/hair-edges.md`)
        Some(id) => {
            let redrawn =
                crate::hair_edges::Edge::ALL.into_iter().filter(|e| !e.suffix().is_empty()).find_map(|e| Some((id.strip_suffix(e.suffix())?, e)));
            match redrawn {
                Some((base, e)) => format!("{}, {} Edges", plain(base), e.label()),
                None => plain(id),
            }
        }
    }
}

/// The dropdown's state and the BiRefNet files, kept by the [`Segmenter`].
#[derive(Default)]
pub struct Settings {
    /// The folder holding `birefnet/` (`ArtCraft/models` on the Mac); `None`: no BiRefNet.
    pub root: Option<PathBuf>,
    /// The model new Subject and Background parts use (when installed); `None`: never chosen, so BiRefNet when it is
    /// installed (its edges are finer on every photo of the comparison), else SAM 3.
    pub selection: Option<SelectionModel>,
    /// Where the choice is saved.
    pub file: Option<PathBuf>,
    #[cfg(feature = "sam")]
    pub(super) download: super::download::Downloader,
}

/// Where the choice is kept, inside the config folder.
pub const SELECTION_FILE: &str = "selection-model.json";

impl Settings {
    /// The defaults of the desktop app and the headless hosts: the models folder (`LIGHTCRAFT_BIREFNET_DIR` for a
    /// scratch run, else `ArtCraft/models` on the Mac, shared with PhotoCraft, else `<config>/models`) and the saved
    /// choice.
    pub fn with_defaults() -> Settings {
        let config = crate::config::config_dir();
        let root = std::env::var_os("LIGHTCRAFT_BIREFNET_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(crate::shared_models::default_shared_models_dir)
            .or_else(|| config.as_ref().map(|c| c.join("models")));
        let file = config.map(|c| c.join(SELECTION_FILE));
        let selection = file.as_deref().and_then(read_selection);
        // (the download belongs to the build with AI masks)
        #[allow(clippy::needless_update)]
        Settings { root, selection, file, ..Default::default() }
    }
}

/// The saved choice; a missing or damaged file is no choice.
fn read_selection(file: &Path) -> Option<SelectionModel> {
    let Ok(text) = std::fs::read_to_string(file) else { return None };
    match serde_json::from_str::<serde_json::Value>(&text).ok().as_ref().and_then(|v| v.get("model")).and_then(serde_json::Value::as_str) {
        Some(id) => SelectionModel::parse(id).or_else(|| {
            log::warn!("{}: unknown selection model `{id}`; ignored", file.display());
            None
        }),
        None => None,
    }
}

/// Write `model` to `file` (replacing it whole, so a crash never leaves half of it).
fn write_selection(file: &Path, model: SelectionModel) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not save the choice of model: {e}"))?;
    }
    let tmp = file.with_extension("json.part");
    let body = serde_json::json!({ "model": model.id() }).to_string();
    std::fs::write(&tmp, body).and_then(|()| std::fs::rename(&tmp, file)).map_err(|e| format!("could not save the choice of model: {e}"))
}

#[cfg(feature = "sam")]
use lightcraft_birefnet::Variant;

impl Segmenter {
    /// Whether the BiRefNet model file is in place.
    pub fn birefnet_installed(&self) -> bool {
        #[cfg(feature = "sam")]
        if let Some(dir) = self.birefnet_dir() {
            return lightcraft_birefnet::is_model_dir(&dir);
        }
        false
    }

    /// The BiRefNet model folder (`<models>/birefnet`).
    pub fn birefnet_dir(&self) -> Option<PathBuf> {
        #[cfg(feature = "sam")]
        return self.birefnet.root.as_deref().map(|r| Variant::General.dir_in(r));
        #[cfg(not(feature = "sam"))]
        None
    }

    /// The selection models that can be used now, in the dropdown's order.
    pub fn available_models(&self) -> Vec<SelectionModel> {
        if !Self::AVAILABLE {
            return Vec::new();
        }
        let sam = self.has_model();
        let bire = self.birefnet_installed();
        // (the mix needs the local SAM 3: a remote worker returns masks, not the model)
        let mix = self.installed() && bire && self.remote_endpoint().is_none();
        SelectionModel::ALL
            .into_iter()
            .filter(|m| match m {
                SelectionModel::Sam3 => sam,
                SelectionModel::BiRefNet => bire,
                SelectionModel::Combined => mix,
            })
            .collect()
    }

    /// The model new Subject and Background parts use: the saved choice when it is installed; with none, BiRefNet
    /// when it is installed, else SAM 3 (whose request then says how to install it).
    pub fn selection(&self) -> SelectionModel {
        let available = self.available_models();
        if let Some(m) = self.birefnet.selection.filter(|m| available.contains(m)) {
            return m;
        }
        if available.contains(&SelectionModel::BiRefNet) {
            return SelectionModel::BiRefNet;
        }
        available.first().copied().unwrap_or_default()
    }

    /// Choose the model for new Subject and Background parts, and save the choice. An uninstalled model is refused.
    pub fn set_selection(&mut self, model: SelectionModel) -> Result<(), String> {
        if !self.available_models().contains(&model) {
            return Err(match model {
                SelectionModel::Sam3 => "The SAM 3 model is not installed.".to_string(),
                SelectionModel::BiRefNet => "The BiRefNet model is not installed. Download it first (birefnet.model.download).".to_string(),
                SelectionModel::Combined => "SAM 3 + BiRefNet Edges needs both models installed on this Mac.".to_string(),
            });
        }
        self.birefnet.selection = Some(model);
        if let Some(file) = &self.birefnet.file {
            write_selection(file, model)?;
        }
        Ok(())
    }

    /// The model a new `kind` ("subject", "sky" or "background") part is made with: Sky is always SAM 3.
    pub fn concept_model(&self, kind: &str) -> SelectionModel {
        if kind == "sky" { SelectionModel::Sam3 } else { self.selection() }
    }

    /// Whether a new `kind` part can be computed by a model at all (Sky needs SAM 3; Subject and Background any).
    pub fn can_select(&self, kind: &str) -> bool {
        if kind == "sky" { self.has_model() } else { !self.available_models().is_empty() }
    }

    /// Download size of the BiRefNet model, in bytes.
    pub fn birefnet_bytes() -> u64 {
        #[cfg(feature = "sam")]
        return Variant::General.size();
        #[cfg(not(feature = "sam"))]
        0
    }

    /// The BiRefNet download's state.
    pub fn birefnet_download_status(&self) -> DownloadStatus {
        #[cfg(feature = "sam")]
        {
            let s = self.birefnet.download.status();
            DownloadStatus {
                running: s.running,
                done: s.done,
                total: s.total,
                file: s.file,
                error: s.error,
                cancelled: s.cancelled,
                finished: s.finished,
            }
        }
        #[cfg(not(feature = "sam"))]
        DownloadStatus::default()
    }

    /// Start downloading BiRefNet in the background (activity row, resume, SHA-256). `Ok(false)`: it is installed or
    /// downloading already.
    pub fn start_birefnet_download(&self, activity: &crate::activity::Activity) -> Result<bool, String> {
        if !Self::AVAILABLE {
            return Err("AI masks are not available in this build".into());
        }
        #[cfg(feature = "sam")]
        {
            let dir = self.birefnet_dir().ok_or("no folder is set for the BiRefNet model")?;
            if self.birefnet_installed() {
                return Ok(false);
            }
            let env = std::env::var(lightcraft_birefnet::fetch::MIRRORS_ENV).ok();
            let mirrors = lightcraft_birefnet::fetch::mirrors(Variant::General, env.as_deref());
            self.birefnet.download.start(Variant::General.files(), mirrors, dir, lightcraft_birefnet::fetch::Options::default(), activity, "BiRefNet")
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = activity;
            Ok(false)
        }
    }

    /// Stop a running BiRefNet download (its partial file stays, to resume). False when none runs.
    pub fn cancel_birefnet_download(&self) -> bool {
        #[cfg(feature = "sam")]
        return self.birefnet.download.cancel();
        #[cfg(not(feature = "sam"))]
        false
    }
}

#[cfg(feature = "sam")]
impl crate::Session {
    /// The worker request for a Subject, Sky or Background selection, and the model that makes it: the dropdown's
    /// choice for Subject and Background (SAM 3 when the chosen model is not installed), SAM 3 for Sky. Errors say
    /// which model is missing.
    pub(super) fn concept_kind(&self, concept: &crate::cmd::concepts::Concept) -> Result<(super::worker::Kind, SelectionModel), String> {
        let model = self.segmenter.concept_model(concept.kind);
        let text = concept.text.trim().to_string();
        let matting = self.subject_matting(concept.kind, model);
        let kind = if model.uses_birefnet() || matting.is_some() {
            let dir = match model.uses_birefnet() {
                true => {
                    self.segmenter.birefnet_dir().filter(|d| lightcraft_birefnet::is_model_dir(d)).ok_or("The BiRefNet model is not installed")?
                }
                false => PathBuf::new(),
            };
            if model.uses_sam3() {
                self.segmenter.model_dir()?;
            }
            super::worker::Kind::BiRefNet { model, text, main: concept.main_only, dir, matting }
        } else {
            self.segmenter.model_dir()?;
            super::worker::Kind::Concept(text, concept.main_only)
        };
        Ok((kind, model))
    }
}

#[cfg(feature = "sam")]
impl crate::Session {
    /// The hair edges a new `kind` part made by `model` gets (`fork/hair-edges.md`): BiRefNet Matting when it is the
    /// Subject edge and installed, for Subject and Background only. SAM 3 must run on this Mac for it (a remote worker
    /// returns masks, not the model).
    pub(crate) fn subject_matting(&self, kind: &str, model: SelectionModel) -> Option<crate::hair_edges::HairEdges> {
        use crate::hair_edges::{Edge, EdgeKind};
        let local = !model.uses_sam3() || (self.segmenter.installed() && self.segmenter.remote_endpoint().is_none());
        (kind != "sky" && local && self.hair_edges.edge(EdgeKind::Subject) != Edge::Current).then(|| self.hair_edges.clone())
    }
}

impl Segmenter {
    /// Stop the Subject and Background requests in flight (their edge setting changed: `fork/hair-edges.md`), so
    /// none of them adds a part with the old edge. The number stopped.
    pub fn cancel_subject_jobs(&mut self) -> usize {
        let before = self.in_flight.len();
        self.in_flight.retain(|job| {
            let subject = matches!(&job.tag, super::Tag::NewConcept { kind, .. } if kind == "subject" || kind == "background");
            if subject {
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            !subject
        });
        before - self.in_flight.len()
    }
}

#[cfg(all(test, feature = "sam"))]
impl Segmenter {
    /// The tags of the requests in flight (tests answer them by hand).
    pub(crate) fn in_flight_tags(&self) -> Vec<super::Tag> {
        self.in_flight.iter().map(|j| j.tag.clone()).collect()
    }

    /// Answer a request as the worker would (the tag decides what the result is applied to).
    pub(crate) fn deliver(&self, tag: super::Tag, result: Result<Option<lightcraft_develop::SegMask>, String>) {
        let _ = self.results.0.send(super::worker::Outcome { id: 0, tag, result, superseded: false, notice: None });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-selmodel-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn ids_labels_and_the_stored_id() {
        for m in SelectionModel::ALL {
            assert_eq!(SelectionModel::parse(m.id()), Some(m));
            assert!(!m.label().is_empty());
        }
        assert_eq!(SelectionModel::Sam3.stored_id(), None, "SAM 3 selections are stored as before");
        assert_eq!(SelectionModel::BiRefNet.stored_id(), Some("birefnet"));
        assert_eq!(SelectionModel::Combined.stored_id(), Some("sam3+birefnet"));
        assert_eq!(SelectionModel::parse("nonsense"), None);
        assert_eq!(SelectionModel::parse(""), None);
        assert_eq!(label_of(None), "SAM 3");
        assert_eq!(label_of(Some("birefnet")), "BiRefNet");
        assert_eq!(label_of(Some("something-newer")), "something-newer", "an id this build does not know is shown as it is");
    }

    #[test]
    fn the_saved_choice_survives_and_a_damaged_file_is_the_default() {
        let dir = scratch("file");
        let file = dir.join(SELECTION_FILE);
        assert_eq!(read_selection(&file), None, "no file");
        write_selection(&file, SelectionModel::Combined).unwrap();
        assert_eq!(read_selection(&file), Some(SelectionModel::Combined));
        for damaged in ["", "{", "[]", r#"{"model": 7}"#, r#"{"model": "gpt"}"#, "\u{0}\u{0}"] {
            std::fs::write(&file, damaged).unwrap();
            assert_eq!(read_selection(&file), None, "{damaged:?}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn nothing_is_available_without_models_and_sky_never_uses_birefnet() {
        let s = Segmenter::default();
        assert!(s.available_models().is_empty());
        assert_eq!(s.selection(), SelectionModel::Sam3);
        assert_eq!(s.concept_model("sky"), SelectionModel::Sam3);
        assert!(!s.can_select("subject") && !s.can_select("sky"));
    }

    /// Changing the Subject edge stops the Subject and Background requests in flight (one queued with BiRefNet
    /// Matting still added a matted part after the edge was set back to Current); Sky and clicks go on.
    #[test]
    fn subject_jobs_stop_and_others_go_on() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let activity = crate::activity::Activity::default();
        let mut s = Segmenter::default();
        let photo = lightcraft_catalog::PhotoId(1);
        let concept = |kind: &str| super::super::Tag::NewConcept {
            photo,
            mask: None,
            op: "add".into(),
            name: None,
            kind: kind.into(),
            invert: kind == "background",
            model: SelectionModel::BiRefNet,
        };
        let mut flags = Vec::new();
        for (i, tag) in [concept("subject"), concept("background"), concept("sky"), super::super::Tag::Prepare].into_iter().enumerate() {
            let cancel = Arc::new(AtomicBool::new(false));
            flags.push(cancel.clone());
            s.in_flight.push(super::super::InFlight {
                id: i as u64 + 1,
                tag,
                cancel,
                detail: false,
                _task: activity.start("segment", "x", crate::activity::Cancel::No),
            });
        }
        assert_eq!(s.cancel_subject_jobs(), 2);
        assert_eq!(s.in_flight.len(), 2, "Sky and the analysis go on");
        let stopped: Vec<bool> = flags.iter().map(|f| f.load(Ordering::Relaxed)).collect();
        assert_eq!(stopped, [true, true, false, false]);
        assert_eq!(s.cancel_subject_jobs(), 0);
    }
}
