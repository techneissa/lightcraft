//! Fork feature (`fork/copy-masks.md`, issue #365): Copy Masks and Paste Masks take all of a photo's masks to other
//! photos, with a clipboard of their own. A paste is Paste Settings of the Masking group alone.

use lightcraft_catalog::Op;
use lightcraft_develop::SettingsGroup;
use serde_json::json;

use super::develop::pasted;
use super::{CommandSpec, bad, cmd, has_active, has_selection};
use crate::Session;

/// Whether Copy Masks can run: the shown photo has a mask.
pub fn can_copy(s: &Session) -> std::result::Result<(), String> {
    has_active(s)?;
    if s.active().and_then(|id| s.develop_of(id)).is_none_or(|d| d.masks.is_empty()) { Err("the photo has no masks".into()) } else { Ok(()) }
}

/// Whether Paste Masks can run: photos are selected and masks were copied.
pub fn can_paste(s: &Session) -> std::result::Result<(), String> {
    has_selection(s)?;
    if s.mask_clipboard.is_none() { Err("no masks copied".into()) } else { Ok(()) }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mask.copyAll",
            "Copy Masks",
            ["Edit"],
            None,
            "{} → {masks} — keep all of the shown photo's masks in the session, apart from the settings develop.copy keeps",
            can_copy,
            |s, _| {
                let c = "mask.copyAll";
                let d = s.active().and_then(|id| s.develop_of(id)).ok_or_else(|| bad(c, "no active photo"))?;
                s.mask_clipboard = Some(lightcraft_develop::extract_groups(&d, &[SettingsGroup::Masks]));
                Ok(json!({"masks": d.masks.len()}))
            }
        ),
        cmd!(
            "mask.pasteAll",
            "Paste Masks",
            ["Edit"],
            None,
            "{ids?} → {changed} — replace the masks of the photos (`ids`, else the selection) with the copied ones, one undo step; AI parts are found again on each photo; an error when nothing is copied",
            can_paste,
            |s, p| {
                let c = "mask.pasteAll";
                let masks = s.mask_clipboard.clone().ok_or_else(|| bad(c, "no masks copied (use mask.copyAll first)"))?;
                let ids = s.targets(p)?;
                let ops = pasted(s, &ids, &masks, "Paste Masks");
                let changed = ops.len();
                s.commit("Paste Masks", Op::Batch { ops })?;
                Ok(json!({"changed": changed}))
            }
        ),
    ]
}
