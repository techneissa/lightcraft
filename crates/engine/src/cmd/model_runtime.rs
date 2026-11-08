//! Fork feature (`fork/coreml.md`, issue #105): `models.runtime`, which engine runs each AI model that has a Core ML
//! version.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::Session;
use crate::model_runtime::{MODELS, Runtime};

const ID: &str = "models.runtime";

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
                "coremlInstalled": s.learn.coreml_installed(),
                "backend": s.learn.backend(),
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
        }
    }
    Ok(rows(s))
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "models.runtime",
        "AI Model Runtime",
        [],
        None,
        "{model?: \"dinov2-small\", runtime?: \"automatic\"|\"coreml\"|\"candle\"} — sets the engine a model runs on (Automatic: Core ML on a Mac when its model is installed, else candle), or with no `runtime` lists them → {models: [{model, feature, runtime, coremlAvailable, coremlInstalled, backend: \"coreml\"|\"candle\"|null}]}",
        always,
        run
    )]
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
        std::fs::write(&file, b"{ damaged").unwrap();
        reopened.open(Some(file));
        assert_eq!(reopened.get("dinov2-small"), Runtime::Automatic);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
