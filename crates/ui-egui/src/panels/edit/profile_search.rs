//! The Profile menu's search: with text typed, the menu shows every profile that matches (built-in,
//! imported LUT groups included) as one list instead of the Favorites, Recent and group submenus.
//! Resting on a match previews it in the loupe, as on the menu's own rows.

use lightcraft_develop::DevelopSettings;
use lightcraft_engine::presets::PROFILES;

use super::{LightcraftApp, json, register};
use crate::widgets::dropdown_search::Filter;

/// Every profile as (id, label as displayed), built-in ones first.
pub(super) fn items(app: &LightcraftApp) -> Vec<(String, String)> {
    PROFILES
        .iter()
        .map(|p| (p.id.to_string(), crate::i18n::profile_label(p.id, p.name).to_string()))
        .chain(app.session.lut_profiles.iter().map(|p| (p.id.clone(), p.name.clone())))
        .collect()
}

/// The matches of the typed text as rows; ↩ picks the highlighted one. Returns nothing: the profile is applied here.
pub(super) fn rows(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, find: &Filter, profiles: &[(String, String)]) {
    let cur = d.profile.id.as_str();
    let mut clicked = find.picked(ui).and_then(|i| profiles.get(i)).map(|(id, _)| id.clone());
    if clicked.is_some() {
        ui.close();
    }
    let mut hovered = None;
    let all: Vec<usize> = (0..profiles.len()).collect();
    for &i in find.options(&all).iter() {
        let Some((id, label)) = profiles.get(i) else { continue };
        let r = ui.selectable_label(id == cur, label);
        register(ui, format!("profileMenu:search:{id}"), r.rect);
        find.row(ui, &r);
        if r.clicked() {
            clicked = Some(id.clone());
            ui.close();
        }
        if r.hovered() {
            hovered = Some(id.clone());
        }
    }
    find.no_match(ui);
    // resting on a profile previews it in the loupe
    if let Some(id) = hovered.filter(|id| id != cur)
        && let Some((name, _)) = app.session.profile_info(&id)
    {
        let mut s = d.clone();
        s.profile.id = id.clone();
        s.profile.amount = 100.0;
        app.hover_preview =
            Some(crate::HoverPreview { label: crate::i18n::tr_format!("Profile: {}", crate::i18n::profile_label(&id, name)), settings: s });
    }
    if let Some(id) = clicked {
        let _ = app.run("develop.profile", json!({"id": id}));
    }
}
