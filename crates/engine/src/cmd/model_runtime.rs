//! Fork feature (`fork/coreml.md`, issue #105): `models.runtime`, which engine runs each AI model that has a Core ML
//! version, and `models.downloadCoreml`, the asked-for download of its Core ML file.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::Session;
use crate::lama_fill::Download;
use crate::model_runtime::{MODELS, Runtime};

const ID: &str = "models.runtime";
const DOWNLOAD_ID: &str = "models.downloadCoreml";

fn download_json(d: Download) -> Value {
    match d {
        Download::Idle => json!({"state": "idle"}),
        Download::Running { bytes, total } => json!({"state": "running", "bytes": bytes, "total": total}),
        Download::Done => json!({"state": "done"}),
        Download::Cancelled => json!({"state": "cancelled"}),
        Download::Failed(e) => json!({"state": "failed", "error": e}),
    }
}

/// The published Core ML file of `model`: (bytes, licence).
fn published(model: &str) -> Option<(u64, &'static str)> {
    #[cfg(not(target_arch = "wasm32"))]
    return crate::model_runtime::release(model).map(|r| (r.bytes, r.license));
    #[cfg(target_arch = "wasm32")]
    {
        let _ = model;
        None
    }
}

/// Whether `model`'s Core ML file is installed.
fn coreml_installed(s: &Session, model: &str) -> bool {
    if model == lightcraft_learn::ENCODER_ID { s.learn.coreml_installed() } else { s.parsing.coreml_installed(model) }
}

/// The engine `model` runs on, once loaded.
fn backend(s: &Session, model: &str) -> Option<&'static str> {
    if model == lightcraft_learn::ENCODER_ID { s.learn.backend() } else { s.parsing.backend(model) }
}

/// The models' rows: their runtime, whether Core ML can run here and its file is there, and the engine in use.
fn rows(s: &Session) -> Value {
    let models: Vec<Value> = MODELS
        .iter()
        .map(|(model, feature)| {
            json!({
                "model": model,
                "feature": feature,
                "runtime": s.model_runtimes.get(model).id(),
                "coremlAvailable": lightcraft_coreml::available(),
                "coremlInstalled": coreml_installed(s, model),
                "backend": backend(s, model),
                "coremlBytes": published(model).map(|(b, _)| b),
                "license": published(model).map(|(_, l)| l),
                "download": download_json(s.model_runtimes.download_state(model)),
            })
        })
        .collect();
    json!({"models": models})
}

fn run(s: &mut Session, p: &Value) -> crate::Result<Value> {
    if let Some(id) = str_param(p, "runtime") {
        let runtime = Runtime::from_id(id).ok_or_else(|| bad(ID, format!("unknown runtime `{id}`: automatic, coreml or candle")))?;
        let model = str_param(p, "model").ok_or_else(|| bad(ID, "missing `model`"))?;
        s.model_runtimes.set(model, runtime).map_err(|e| bad(ID, e))?;
        if model == lightcraft_learn::ENCODER_ID {
            s.learn.set_runtime(runtime);
        } else {
            s.parsing.set_runtime(model, runtime);
        }
    }
    Ok(rows(s))
}

fn download(s: &mut Session, p: &Value) -> crate::Result<Value> {
    let model = str_param(p, "model").ok_or_else(|| bad(DOWNLOAD_ID, "missing `model`"))?;
    let (bytes, _) = published(model).ok_or_else(|| bad(DOWNLOAD_ID, format!("no Core ML file is published for `{model}`")))?;
    if !lightcraft_coreml::available() {
        return Err(bad(DOWNLOAD_ID, "Core ML runs only on a Mac (macOS 13 or later)"));
    }
    if p.get("confirm").and_then(Value::as_bool) != Some(true) {
        let mb = (bytes + 500_000) / 1_000_000;
        return Err(bad(DOWNLOAD_ID, format!("this downloads {mb} MB from github.com: ask the user first, then send {{\"confirm\": true}}")));
    }
    if model != lightcraft_learn::ENCODER_ID {
        return Err(bad(DOWNLOAD_ID, format!("no model `{model}` has a Core ML version")));
    }
    if s.learn.coreml_installed() {
        return Err(bad(DOWNLOAD_ID, "the Core ML model is already installed"));
    }
    start(s, model)?;
    Ok(json!({"started": true, "bytes": bytes}))
}

#[cfg(not(target_arch = "wasm32"))]
fn start(s: &mut Session, model: &str) -> crate::Result<()> {
    let r = crate::model_runtime::release(model).ok_or_else(|| bad(DOWNLOAD_ID, "no Core ML file is published"))?;
    let dir = s.learn.model_dir().ok_or_else(|| bad(DOWNLOAD_ID, "Learn My Edits has no model folder on this computer"))?;
    let learn = s.learn.clone();
    // a loaded encoder is dropped, so the next job loads the new Core ML model
    let done = move || learn.set_runtime(learn.runtime());
    s.model_runtimes.start_download(&s.activity, r, dir.join(lightcraft_learn::image_encoder::COREML_DIR), done).map_err(|e| bad(DOWNLOAD_ID, e))
}

#[cfg(target_arch = "wasm32")]
fn start(_: &mut Session, _: &str) -> crate::Result<()> {
    Err(bad(DOWNLOAD_ID, "the web build downloads no models"))
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "models.downloadCoreml",
            "Download Core ML Model",
            [],
            None,
            "{model: \"dinov2-small\", confirm: true} — downloads the model's Core ML file (a zip from the models-coreml-1 release of techneissa/lightcraft, size and SHA-256 checked) into the shared models folder, with a row in the activity stack. Without `confirm: true` it is refused: ask the user first → {started: true, bytes}",
            always,
            download
        ),
        cmd!(
            "models.runtime",
            "AI Model Runtime",
            [],
            None,
            "{model?: \"dinov2-small\"|\"face-parsing-segformer-b0\"|\"human-parsing-segformer-b3\", runtime?: \"automatic\"|\"coreml\"|\"candle\"} — sets the engine a model runs on (Automatic: Core ML on a Mac when its model is installed, else candle), or with no `runtime` lists them → {models: [{model, feature, runtime, coremlAvailable, coremlInstalled, backend: \"coreml\"|\"candle\"|null, coremlBytes, license, download: {state, bytes?, total?, error?}}]}",
            always,
            run
        ),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;
    use crate::model_runtime::Runtime;

    #[test]
    fn the_runtime_choice_is_kept_and_reaches_learn_my_edits() {
        let dir = std::env::temp_dir().join(format!("lightcraft-model-runtimes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join(crate::model_runtime::FILE);
        let mut s = Session::default();
        s.model_runtimes.open(Some(file.clone()));
        let listed = s.execute("models.runtime", &json!({})).unwrap();
        assert_eq!(listed["models"][0]["model"], "dinov2-small");
        assert_eq!(listed["models"][0]["runtime"], "automatic");

        let set = s.execute("models.runtime", &json!({"model": "dinov2-small", "runtime": "candle"})).unwrap();
        assert_eq!(set["models"][0]["runtime"], "candle");
        assert_eq!(s.learn.runtime(), Runtime::Candle);
        let reopened = crate::model_runtime::ModelRuntimes::default();
        reopened.open(Some(file.clone()));
        assert_eq!(reopened.get("dinov2-small"), Runtime::Candle);

        assert!(s.execute("models.runtime", &json!({"model": "dinov2-small", "runtime": "fast"})).is_err());
        assert!(s.execute("models.runtime", &json!({"model": "sam3", "runtime": "coreml"})).is_err());
        assert!(s.execute("models.runtime", &json!({"runtime": "coreml"})).is_err());

        // the people-parsing SegFormers have a row each, and their choice reaches the parser
        assert_eq!(listed["models"][1]["model"], "face-parsing-segformer-b0");
        assert_eq!(listed["models"][2]["model"], "human-parsing-segformer-b3");
        assert_eq!(listed["models"][2]["coremlInstalled"], false);
        s.execute("models.runtime", &json!({"model": "human-parsing-segformer-b3", "runtime": "candle"})).unwrap();
        s.execute("models.runtime", &json!({"model": "face-parsing-segformer-b0", "runtime": "coreml"})).unwrap();
        assert_eq!(s.parsing.runtimes(), lightcraft_parsing::Runtimes { face: Runtime::CoreMl, body: Runtime::Candle });
        assert_eq!(s.learn.runtime(), Runtime::Candle);
        reopened.open(Some(file.clone()));
        assert_eq!(reopened.get("human-parsing-segformer-b3"), Runtime::Candle);
        std::fs::write(&file, b"{ damaged").unwrap();
        reopened.open(Some(file));
        assert_eq!(reopened.get("dinov2-small"), Runtime::Automatic);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A choice that can't be saved is not applied.
    #[test]
    fn a_choice_that_cannot_be_saved_changes_nothing() {
        let dir = std::env::temp_dir().join(format!("lightcraft-model-runtimes-ro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // the settings file's folder is a plain file, so the save fails
        std::fs::write(dir.join("not-a-folder"), b"").unwrap();
        let mut s = Session::default();
        s.model_runtimes.open(Some(dir.join("not-a-folder").join(crate::model_runtime::FILE)));
        assert!(s.execute("models.runtime", &json!({"model": "dinov2-small", "runtime": "candle"})).is_err());
        assert_eq!(s.model_runtimes.get("dinov2-small"), Runtime::Automatic);
        assert_eq!(s.learn.runtime(), Runtime::Automatic);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The download is refused until the user has been asked, and starts nothing.
    #[test]
    fn the_core_ml_download_asks_first() {
        let mut s = Session::default();
        let listed = s.execute("models.runtime", &json!({})).unwrap();
        assert_eq!(listed["models"][0]["license"], "Apache-2.0");
        assert_eq!(listed["models"][0]["download"]["state"], "idle");
        assert!(s.execute("models.downloadCoreml", &json!({"model": "dinov2-small"})).is_err());
        assert!(s.execute("models.downloadCoreml", &json!({"model": "sam3", "confirm": true})).is_err());
        assert_eq!(s.model_runtimes.download_state("dinov2-small"), crate::lama_fill::Download::Idle);
    }
}
