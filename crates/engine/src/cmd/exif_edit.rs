//! Edited camera / lens / exposure values (`photo.setMeta` keys `camera`, `lens`, `focal`,
//! `aperture`, `shutter`, `iso`, and `photo.resetExif`). The file's values and the file itself
//! are never touched: an edit is an override stored in the catalog (`Meta::exif`), a reset removes
//! it. Rendering keeps using the file's camera and lens; see `lightcraft_catalog::exif_override`.

use lightcraft_catalog::{ExifField, Meta, Op};
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_selection};
use crate::{Result, Session};

/// The text of one `photo.setMeta` value: a string, or a number for the numeric fields; `None`
/// for null (reset).
fn value_text(cmd: &str, field: ExifField, v: &Value) -> Result<Option<String>> {
    match v {
        Value::Null => Ok(None),
        Value::String(t) => Ok(Some(t.clone())),
        Value::Number(n) if !matches!(field, ExifField::Camera | ExifField::Lens) => Ok(Some(n.to_string())),
        _ => Err(bad(cmd, format!("`{}` is text or null (null resets it to the file's value)", field.key()))),
    }
}

/// Applies the edit keys present in `p` to `m`; returns whether any was present. Nothing is
/// half-applied: on `Err`, `m` is as it was.
pub(crate) fn apply_params(cmd: &str, m: &mut Meta, p: &Value) -> Result<bool> {
    let mut edited = m.clone();
    let mut any = false;
    for field in ExifField::ALL {
        let Some(v) = p.get(field.key()) else { continue };
        any = true;
        let text = value_text(cmd, field, v)?;
        edited.set_exif(field, text.as_deref()).map_err(|e| bad(cmd, format!("{}: {e}", field.key())))?;
    }
    if any {
        *m = edited;
    }
    Ok(any)
}

/// The fields named by `fields` (a list of keys), or all of them when it is absent.
fn fields_param(p: &Value) -> Result<Vec<ExifField>> {
    const ID: &str = "photo.resetExif";
    match p.get("fields") {
        None | Some(Value::Null) => Ok(ExifField::ALL.to_vec()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                let key = v.as_str().ok_or_else(|| bad(ID, "`fields` is a list of names"))?;
                ExifField::from_key(key).ok_or_else(|| bad(ID, format!("unknown field `{key}`: camera, lens, focal, aperture, shutter, iso")))
            })
            .collect(),
        Some(_) => Err(bad(ID, "`fields` is a list of names")),
    }
}

fn reset_exif(s: &mut Session, p: &Value) -> Result<Value> {
    let fields = fields_param(p)?;
    let targets = s.targets(p)?;
    let mut ops = Vec::new();
    for id in targets {
        let Some(ph) = s.catalog.photo(id) else { continue };
        let mut m = ph.meta.clone();
        for f in &fields {
            m.reset_exif(*f);
        }
        if m != ph.meta {
            ops.push(Op::SetMeta { id, meta: Box::new(m) });
        }
    }
    let n = ops.len();
    if n > 0 {
        s.commit("Reset to File Values", Op::Batch { ops })?;
    }
    Ok(json!({"changed": n}))
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "photo.resetExif",
        "Reset Camera Info to File Values",
        [],
        None,
        "{ids?, fields?: [\"camera\"|\"lens\"|\"focal\"|\"aperture\"|\"shutter\"|\"iso\"]} → {changed} — removes the edited camera / lens / exposure values (all of them, or the fields named) so the file's show again; one undo step; `photo.setMeta` with the field set to null resets one field the same way",
        has_selection,
        reset_exif
    )]
}
