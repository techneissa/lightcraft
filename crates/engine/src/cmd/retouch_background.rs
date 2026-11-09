//! `retouch.background`: what Background Swap puts behind the person (fork, `fork/retouch-background.md`). The
//! sliders (Blur, Swap, Clean Backdrop) are develop controls (`retouch.background.*`), set like every portrait value.

use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_active, photo_id};
use crate::{Result, Session};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::background::Swap;

/// `with` as a [`Swap`]: `{colour: "#RRGGBB"}`, `{file: path}` or `{photo: id}`.
fn swap_param(s: &Session, w: &Value, c: &str) -> Result<Swap> {
    let field = |k: &str| w.get(k).filter(|v| !v.is_null());
    match (field("colour").or_else(|| field("color")), field("file"), field("photo")) {
        (Some(v), None, None) => {
            let hex = v.as_str().ok_or_else(|| bad(c, "`colour` is a string, #RRGGBB"))?;
            Swap::from_hex(hex).ok_or_else(|| bad(c, format!("`{hex}` is not a colour (#RRGGBB)")))
        }
        (None, Some(v), None) => {
            let path = v.as_str().filter(|p| !p.trim().is_empty()).ok_or_else(|| bad(c, "`file` is a path"))?;
            if !std::path::Path::new(path).is_file() {
                return Err(bad(c, format!("no picture at {path}")));
            }
            Ok(Swap::File { path: path.to_string() })
        }
        (None, None, Some(v)) => {
            let id = v.as_u64().ok_or_else(|| bad(c, "`photo` is a photo id"))?;
            s.catalog.photo(PhotoId(id)).ok_or_else(|| bad(c, format!("no photo {id}")))?;
            Ok(Swap::Photo { id })
        }
        _ => Err(bad(c, "`with` takes one of `colour` (#RRGGBB), `file` (a path) or `photo` (an id)")),
    }
}

fn status(s: &Session, id: PhotoId) -> Value {
    let d = s.develop_of(id).unwrap_or_default();
    let b = &d.retouch.global.background;
    let with = match &b.with {
        Swap::Colour { .. } => json!({"colour": b.with.hex()}),
        Swap::File { path } => json!({"file": path}),
        Swap::Photo { id } => json!({"photo": id}),
    };
    let mut v = json!({"id": id.0, "blur": b.blur, "swap": b.swap, "clean": b.clean, "with": with});
    if let (Some(o), Value::Object(st)) = (v.as_object_mut(), s.backdrop_status(&d)) {
        o.extend(st);
    }
    v
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "retouch.background",
        "Background Swap With",
        [],
        None,
        "{with?: {colour: \"#RRGGBB\"} | {file: path} | {photo: id}, id?} → {id, blur, swap, clean, with, picture?, loaded, loading, error?} — what Background Swap puts behind the person: a colour, a picture file or a library photo as it is developed (loaded in the background; an export waits for it). Without `with`: the state only. The sliders are develop controls (retouch.background.blur, .swap, .clean)",
        has_active,
        |s, p| {
            let c = "retouch.background";
            let id = photo_id(p, c)?.or_else(|| s.active()).ok_or_else(|| bad(c, "no photo"))?;
            s.catalog.photo(id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))?;
            if let Some(w) = p.get("with").filter(|v| !v.is_null()) {
                let with = swap_param(s, w, c)?;
                let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
                if d.retouch.global.background.with != with {
                    d.retouch.global.background.with = with.clone();
                    s.set_develop(id, d, "Background Swap")?;
                }
                // (choosing a picture again retries one that failed, and reloads a file changed since)
                if let Some(key) = with.picture() {
                    s.retouch_faces.backdrops.forget(&key);
                    s.backdrop_load(&key);
                }
            }
            Ok(status(s, id))
        }
    )]
}
