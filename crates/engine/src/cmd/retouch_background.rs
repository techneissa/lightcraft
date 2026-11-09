//! `retouch.background`: what Background Swap puts behind the person (fork, `fork/retouch-background.md`). The
//! sliders (Blur, Swap, Clean Backdrop) are develop controls (`retouch.background.*`), set like every portrait value.
//! A picture file is copied into the library ([`crate::retouch_background::FOLDER`]) and kept by its hash: the
//! photo's settings never hold its path.

use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_active, photo_id};
use crate::{Result, Session};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::background::Swap;

/// `with` as a [`Swap`] for photo `photo`: `{colour: "#RRGGBB"}`, `{file: path}` (copied into the library),
/// `{picture: hash}` (one there already) or `{photo: id}` (another photo).
fn swap_param(s: &mut Session, photo: PhotoId, w: &Value, c: &str) -> Result<Swap> {
    let field = |k: &str| w.get(k).filter(|v| !v.is_null()).cloned();
    match (field("colour").or_else(|| field("color")), field("file"), field("picture"), field("photo")) {
        (Some(v), None, None, None) => {
            let hex = v.as_str().ok_or_else(|| bad(c, "`colour` is a string, #RRGGBB"))?;
            Swap::from_hex(hex).ok_or_else(|| bad(c, format!("`{hex}` is not a colour (#RRGGBB)")))
        }
        (None, Some(v), None, None) => {
            let path = v.as_str().map(str::trim).filter(|p| !p.is_empty()).ok_or_else(|| bad(c, "`file` is a path"))?;
            let path = std::path::Path::new(path);
            if !path.is_file() {
                return Err(bad(c, format!("no picture at {}", path.display())));
            }
            let hash = s.backdrop_import(path).map_err(|e| bad(c, e))?;
            Ok(Swap::Picture { hash })
        }
        (None, None, Some(v), None) => {
            let hash = v.as_str().map(str::to_ascii_lowercase).ok_or_else(|| bad(c, "`picture` is a picture's hash"))?;
            Ok(Swap::Picture { hash })
        }
        (None, None, None, Some(v)) => {
            let id = v.as_u64().ok_or_else(|| bad(c, "`photo` is a photo id"))?;
            s.catalog.photo(PhotoId(id)).ok_or_else(|| bad(c, format!("no photo {id}")))?;
            if id == photo.0 {
                return Err(bad(c, "a photo can't be its own background"));
            }
            Ok(Swap::Photo { id })
        }
        _ => Err(bad(c, "`with` takes one of `colour` (#RRGGBB), `file` (a path), `picture` (a hash) or `photo` (an id)")),
    }
}

fn status(s: &Session, id: PhotoId) -> Value {
    let d = s.develop_of(id).unwrap_or_default();
    let b = &d.retouch.global.background;
    let with = match &b.with {
        Swap::Colour { .. } => json!({"colour": b.with.hex()}),
        Swap::Picture { hash } => json!({"picture": hash}),
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
        "{with?: {colour: \"#RRGGBB\"} | {file: path} | {picture: hash} | {photo: id}, id?} → {id, blur, swap, clean, with, picture?, name?, loaded, loading, missing, error?} — what Background Swap puts behind the person: a colour, a picture file (copied into the library's Backgrounds folder and kept by its SHA-256) or a library photo as it is developed (loaded in the background; an export waits for it and fails when it is missing). Without `with`: the state only. The sliders are develop controls (retouch.background.blur, .swap, .clean)",
        has_active,
        |s, p| {
            let c = "retouch.background";
            let id = photo_id(p, c)?.or_else(|| s.active()).ok_or_else(|| bad(c, "no photo"))?;
            s.catalog.photo(id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))?;
            if let Some(w) = p.get("with").filter(|v| !v.is_null()) {
                let with = swap_param(s, id, w, c)?;
                let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
                if d.retouch.global.background.with != with {
                    d.retouch.global.background.with = with.clone();
                    s.set_develop(id, d, "Background Swap")?;
                }
                // (loaded now, on a worker; choosing a picture again retries one that failed)
                if let Some(key) = with.picture() {
                    s.retouch_faces.backdrops.forget(&key);
                    let d = s.develop_of(id).unwrap_or_default();
                    s.backdrop_prefetch(&d);
                }
            }
            Ok(status(s, id))
        }
    )]
}
