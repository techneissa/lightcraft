//! Fork feature (`fork/hair-edges.md`, issue #103): the edge of Entire Person / Background and Subject masks
//! (`maskEdges.edge`) and the BiRefNet Matting download (`maskEdges.status`, `maskEdges.download`, `maskEdges.cancel`).

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::hair_edges::{Edge, EdgeKind, HairEdges, MODEL_BYTES};
use crate::{Result, Session};

const LICENSE: &str = "MIT";
const LICENSE_URL: &str = "https://huggingface.co/ZhengPeng7/BiRefNet-matting";

fn kinds(h: &HairEdges) -> Value {
    json!(
        EdgeKind::ALL
            .iter()
            .map(|k| {
                let available: Vec<&str> = Edge::ALL.iter().filter(|e| h.available(**e, *k)).map(|e| e.id()).collect();
                json!({"kind": k.id(), "label": k.label(), "chosen": h.chosen(*k).id(), "edge": h.edge(*k).id(), "available": available})
            })
            .collect::<Vec<_>>()
    )
}

fn edges() -> Value {
    json!(Edge::ALL.iter().map(|e| json!({"id": e.id(), "label": e.label()})).collect::<Vec<_>>())
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    let h = &s.hair_edges;
    Ok(json!({
        "available": HairEdges::AVAILABLE,
        "installed": h.installed(),
        "dir": h.model_dir().map(|d| d.display().to_string()),
        "sizeBytes": MODEL_BYTES,
        "license": LICENSE,
        "licenseUrl": LICENSE_URL,
        "download": h.download_status(),
        "kinds": kinds(h),
        "edges": edges(),
    }))
}

fn set_edge(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "maskEdges.edge";
    let kind = p.get("kind").and_then(Value::as_str).ok_or_else(|| bad(c, "`kind` must be people or subject"))?;
    let kind = EdgeKind::parse(kind)
        .ok_or_else(|| bad(c, format!("unknown kind `{}` (use people or subject)", kind.chars().take(40).collect::<String>())))?;
    let mut cancelled = 0;
    if let Some(v) = p.get("edge").filter(|v| !v.is_null()) {
        let id = v.as_str().ok_or_else(|| bad(c, "`edge` must be an edge id: current or birefnet-matting"))?;
        let edge = Edge::parse(id)
            .ok_or_else(|| bad(c, format!("unknown edge `{}` (use current or birefnet-matting)", id.chars().take(40).collect::<String>())))?;
        let before = s.hair_edges.edge(kind);
        s.hair_edges.set_edge(kind, edge).map_err(|e| bad(c, e))?;
        // Subject and Background requests still running were made for the old edge: stop them
        if kind == EdgeKind::Subject && s.hair_edges.edge(kind) != before {
            cancelled = s.segmenter.cancel_subject_jobs();
        }
    }
    let h = &s.hair_edges;
    Ok(json!({"kind": kind.id(), "chosen": h.chosen(kind).id(), "edge": h.edge(kind).id(), "kinds": kinds(h), "cancelled": cancelled}))
}

fn download(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "maskEdges.download";
    if p.get("confirm").and_then(Value::as_bool) != Some(true) {
        return Err(bad(
            c,
            format!(
                "BiRefNet Matting is a {} MB download ({LICENSE} licence, {LICENSE_URL}): pass `confirm: true` once the user has agreed to download it",
                MODEL_BYTES.div_ceil(1_000_000)
            ),
        ));
    }
    // (`kind`: choose BiRefNet Matting for it once the download has finished)
    let kind = match p.get("kind").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(v.as_str().and_then(EdgeKind::parse).ok_or_else(|| bad(c, "`kind` must be people or subject"))?),
    };
    let started = s.hair_edges.start_download(&s.activity).map_err(|e| bad(c, e))?;
    if let Some(kind) = kind {
        match started {
            true => s.hair_edges.choose_when_downloaded(kind),
            // (already installed: choose it now)
            false if s.hair_edges.installed() => s.hair_edges.set_edge(kind, Edge::Matting).map_err(|e| bad(c, e))?,
            false => {}
        }
    }
    Ok(json!({"started": started, "installed": s.hair_edges.installed(), "downloading": s.hair_edges.download_status().running}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "maskEdges.status",
            "Mask Edges Status",
            [],
            None,
            "{} — the edge of Entire Person / Background (people) and Subject masks, and whether BiRefNet Matting (hair-accurate edges) is installed → {available, installed, dir, sizeBytes, license, licenseUrl, download: {running, done, total, file, error, cancelled, finished}, kinds: [{kind, label, chosen, edge}], edges: [{id, label}]}",
            always,
            status
        ),
        cmd!(
            query "maskEdges.edge",
            "Mask Edge",
            [],
            None,
            "{kind: people|subject, edge?: current|birefnet-matting} — read, or set (and save) the edge of new masks of a kind: people is Entire Person and Background from people parsing, subject is Subject and the Background made from it. Current is the default; BiRefNet Matting keeps flyaway hair and needs the model (maskEdges.download). `edge` in the answer is what masks get now (Current while the chosen model is missing). Changing the Subject edge stops the Subject and Background requests still running (`cancelled`: how many) → {kind, chosen, edge, kinds, cancelled}",
            always,
            set_edge
        ),
        cmd!(
            query "maskEdges.download",
            "Download BiRefNet Matting Model",
            [],
            None,
            "{confirm: true, kind?: people|subject} — download BiRefNet Matting (about 885 MB, MIT licence) in the background; only after the user agreed to it; with `kind`, BiRefNet Matting is chosen for that kind once the download has finished. Size and SHA-256 are checked; watch maskEdges.status; maskEdges.cancel stops it (it resumes later) → {started, installed, downloading}",
            always,
            download
        ),
        cmd!(
            query "maskEdges.cancel",
            "Cancel BiRefNet Matting Download",
            [],
            None,
            "{} — stop the BiRefNet Matting download (what has arrived is kept, and a new download resumes from it) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": s.hair_edges.cancel_download()}))
        ),
    ]
}
