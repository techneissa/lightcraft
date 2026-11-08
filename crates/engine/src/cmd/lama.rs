//! Fork feature (`fork/lama-fill.md`): the `fill` parameter of the Remove commands, and `lama.status` / `lama.download`.

use lightcraft_develop::{SpotFill, SpotMode};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::Session;
use crate::lama_fill::{DOWNLOAD_BYTES, Download};

/// The `fill` a command names for a spot of `mode` (`classic` when it names none). LaMa fills Remove and Content-Aware
/// spots; Heal and Clone copy a source, so naming LaMa for them is an error. LaMa for a computer without the model is
/// not: the spot is kept, and shows the Classic fill until the model is installed.
pub(super) fn fill_param(c: &str, p: &Value, mode: SpotMode) -> crate::Result<SpotFill> {
    let Some(v) = p.get("fill").filter(|v| !v.is_null()) else { return Ok(SpotFill::Classic) };
    let fill = v.as_str().and_then(SpotFill::from_name).ok_or_else(|| bad(c, "`fill` must be \"classic\" or \"lama\""))?;
    check_fill(c, fill, mode)?;
    Ok(fill)
}

/// LaMa only for the modes that take it.
pub(super) fn check_fill(c: &str, fill: SpotFill, mode: SpotMode) -> crate::Result<()> {
    if fill == SpotFill::Lama && !matches!(mode, SpotMode::Remove | SpotMode::Fill) {
        return Err(bad(c, "LaMa fills Remove and Content-Aware spots; Heal and Clone copy a source"));
    }
    Ok(())
}

/// What a command adds to its answer for a LaMa spot: which fill, and a note when the model is missing.
pub(super) fn describe(s: &Session, fill: SpotFill, out: &mut Value) {
    if fill == SpotFill::Lama {
        out["fill"] = json!(fill.name());
        if !s.lama.installed() {
            out["note"] =
                json!("LaMa is not installed: the spot shows the Classic fill until it is (lama.download, or put big-lama.pt in the model folder)");
        }
    }
}

fn status(s: &Session) -> Value {
    let (state, bytes, total, error) = match s.lama.download_state() {
        Download::Idle => ("idle", 0, 0, None),
        Download::Running { bytes, total } => ("running", bytes, total, None),
        Download::Done => ("done", DOWNLOAD_BYTES, DOWNLOAD_BYTES, None),
        Download::Cancelled => ("cancelled", 0, 0, None),
        Download::Failed(e) => ("failed", 0, 0, Some(e)),
    };
    json!({
        "installed": s.lama.installed(),
        "file": s.lama.model_path().map(|p| p.display().to_string()),
        "folder": s.lama.dir().map(|p| p.display().to_string()),
        "downloadBytes": DOWNLOAD_BYTES,
        "download": {"state": state, "bytes": bytes, "total": total, "error": error},
        "message": s.lama.message(),
    })
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "lama.status",
            "LaMa Fill Status",
            [],
            None,
            "{} — whether the LaMa model is installed → {installed, file, folder, downloadBytes, download: {state: idle|running|done|cancelled|failed, bytes, total, error}, message}",
            always,
            |s, _| Ok(status(s))
        ),
        cmd!(
            "lama.download",
            "Download LaMa",
            [],
            None,
            "{confirm: true} — downloads the LaMa model (big-lama.pt, 206 MB, from huggingface.co) into the folder shared with PhotoCraft, on a thread with a row in the activity stack (✕ stops it; asking again resumes). Without `confirm: true` it is refused: ask the user first (the app shows a dialog) → {started: true, bytes}",
            always,
            |s, p| {
                if p.get("confirm").and_then(Value::as_bool) != Some(true) {
                    return Err(bad(
                        "lama.download",
                        format!(
                            "this downloads {} MB from huggingface.co: ask the user first, then send {{\"confirm\": true}}",
                            (DOWNLOAD_BYTES + 500_000) / 1_000_000
                        ),
                    ));
                }
                s.lama.start_download().map_err(|e| bad("lama.download", e))?;
                Ok(json!({"started": true, "bytes": DOWNLOAD_BYTES}))
            }
        ),
    ]
}
