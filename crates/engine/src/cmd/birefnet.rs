//! Fork feature (fork/birefnet-masks.md, issue #48): the BiRefNet model download (`birefnet.model.*`) and the choice of
//! the model new Subject and Background parts use (`segment.selectionModel`, the Masking dropdown).

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::segment::Segmenter;
use crate::segment::birefnet::SelectionModel;
use crate::{Result, Session};

/// Where the model comes from, for the status and the consent text.
const LICENSE: &str = "MIT";
const LICENSE_URL: &str = "https://github.com/ZhengPeng7/BiRefNet/blob/main/LICENSE";

fn models(s: &Session) -> Value {
    let available = s.segmenter.available_models();
    json!(SelectionModel::ALL.iter().map(|m| json!({"id": m.id(), "label": m.label(), "installed": available.contains(m)})).collect::<Vec<_>>())
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    let g = &s.segmenter;
    #[cfg(feature = "sam")]
    let mirrors = lightcraft_birefnet::fetch::mirrors(
        lightcraft_birefnet::Variant::General,
        std::env::var(lightcraft_birefnet::fetch::MIRRORS_ENV).ok().as_deref(),
    )
    .len();
    #[cfg(not(feature = "sam"))]
    let mirrors = 0;
    Ok(json!({
        "available": Segmenter::AVAILABLE,
        "installed": g.birefnet_installed(),
        "dir": g.birefnet_dir().map(|d| d.display().to_string()),
        "sizeBytes": Segmenter::birefnet_bytes(),
        "license": LICENSE,
        "licenseUrl": LICENSE_URL,
        "mirrors": mirrors,
        "download": g.birefnet_download_status(),
        "selection": g.selection().id(),
        "models": models(s),
    }))
}

fn download(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "birefnet.model.download";
    if p.get("confirm").and_then(Value::as_bool) != Some(true) {
        return Err(bad(
            c,
            format!(
                "the BiRefNet model is a {:.0} MB download ({LICENSE} licence, {LICENSE_URL}): pass `confirm: true` once the user has agreed to download it",
                Segmenter::birefnet_bytes() as f64 / 1e6
            ),
        ));
    }
    let started = s.segmenter.start_birefnet_download(&s.activity).map_err(|e| bad(c, e))?;
    Ok(json!({"started": started, "installed": s.segmenter.birefnet_installed(), "downloading": s.segmenter.birefnet_download_status().running}))
}

fn selection(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "segment.selectionModel";
    if let Some(v) = p.get("model").filter(|v| !v.is_null()) {
        let id = v.as_str().ok_or_else(|| bad(c, "`model` must be a model id: sam3, birefnet or sam3+birefnet"))?;
        let model = SelectionModel::parse(id)
            .ok_or_else(|| bad(c, format!("unknown model `{}` (use sam3, birefnet or sam3+birefnet)", id.chars().take(40).collect::<String>())))?;
        s.segmenter.set_selection(model).map_err(|e| bad(c, e))?;
    }
    let m = s.segmenter.selection();
    Ok(json!({"model": m.id(), "label": m.label(), "models": models(s)}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "birefnet.model.status",
            "BiRefNet Model Status",
            [],
            None,
            "{} — whether the BiRefNet model (fine-edged Subject and Background masks) is installed, where, and the download's progress → {available, installed, dir, sizeBytes, license, licenseUrl, mirrors, download: {running, done, total, file, error, cancelled, finished}, selection, models: [{id, label, installed}]}",
            always,
            status
        ),
        cmd!(
            query "birefnet.model.download",
            "Download BiRefNet Model",
            [],
            None,
            "{confirm: true} — download the BiRefNet model (about 444 MB, MIT licence) in the background; only after the user agreed to it. Watch birefnet.model.status; birefnet.model.cancel stops it (it resumes later) → {started, installed, downloading}",
            always,
            download
        ),
        cmd!(
            query "birefnet.model.cancel",
            "Cancel BiRefNet Model Download",
            [],
            None,
            "{} — stop the BiRefNet download (what has arrived is kept, and a new download resumes from it) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": s.segmenter.cancel_birefnet_download()}))
        ),
        cmd!(
            query "segment.selectionModel",
            "Selection Model for New Subject Masks",
            [],
            None,
            "{model?: sam3|birefnet|sam3+birefnet} — read, or set (and save) the model new Subject and Background parts use; a part keeps the model that made it, and Sky, Object and Describe stay SAM 3. A model that is not installed is refused → {model, label, models: [{id, label, installed}]}",
            always,
            selection
        ),
    ]
}
