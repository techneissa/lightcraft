//! Fork feature (issue #462): Copy Portrait Settings and Paste Portrait Settings take what the Portrait panel holds
//! (the retouch block: every feature's sliders, the faces' own settings and the Refine strokes) to other photos, with
//! a clipboard of their own. A paste is Paste Settings of the Portrait group alone.
//!
//! The Relight group is not copied: the Portrait panel does not hold it. The block stores no people masks (they are
//! found on each photo by the people parser, and a face's own settings find their face on the target by its name or
//! place), so a paste never brings one photo's mask pixels.

use lightcraft_catalog::Op;
use lightcraft_develop::SettingsGroup;
use serde_json::json;

use super::develop::pasted;
use super::{CommandSpec, bad, cmd, has_active, has_selection};
use crate::Session;

/// Whether Paste Portrait Settings can run: photos are selected and portrait settings were copied.
pub fn can_paste(s: &Session) -> std::result::Result<(), String> {
    has_selection(s)?;
    if s.portrait_clipboard.is_none() { Err("no portrait settings copied".into()) } else { Ok(()) }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "portrait.copy",
            "Copy Portrait Settings",
            ["Edit"],
            None,
            "{} — keep the shown photo's Portrait settings in the session, apart from the settings develop.copy keeps",
            has_active,
            |s, _| {
                let c = "portrait.copy";
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                let d = s.develop_of(id).unwrap_or_default();
                s.portrait_clipboard = Some(lightcraft_develop::extract_groups(&d, &[SettingsGroup::Portrait]));
                Ok(json!({"groups": [SettingsGroup::Portrait]}))
            }
        ),
        cmd!(
            "portrait.paste",
            "Paste Portrait Settings",
            ["Edit"],
            None,
            "{ids?} → {changed} — replace the Portrait settings of the photos (`ids`, else the selection) with the copied ones, nothing else changes, one undo step; an error when nothing is copied",
            can_paste,
            |s, p| {
                let c = "portrait.paste";
                let partial = s.portrait_clipboard.clone().ok_or_else(|| bad(c, "no portrait settings copied (use portrait.copy first)"))?;
                let ids = s.targets(p)?;
                let ops = pasted(s, &ids, &partial, "Paste Portrait Settings");
                let changed = ops.len();
                s.commit("Paste Portrait Settings", Op::Batch { ops })?;
                Ok(json!({"changed": changed}))
            }
        ),
    ]
}
