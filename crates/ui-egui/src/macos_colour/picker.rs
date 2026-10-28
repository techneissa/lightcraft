//! macOS Colour in the Profile picker: its row (offered where this Mac's raw decoder reads the photo's camera; greyed
//! for a photo set to it on a computer that can't make it), the permanent note when the picture uses Camera Matching
//! instead, and Apply macOS Colour to All Photos from This Camera… with its question (`profiles.macosForCamera`).

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{CameraColour, DevelopSettings};
use lightcraft_pipeline::MacosUnavailable;

use crate::LightcraftApp;
use crate::panels::camera_colour::Row;
use crate::state::Dialog;
use crate::theme::Tokens;

/// The row's label (Title Case) and tooltips.
pub(crate) const LABEL: &str = "macOS Colour";
const TIP: &str = "Uses the Mac's own raw colours, the ones Photos and Quick Look show. Mac only.";
const TIP_OFF: &str = "This computer can't make macOS Colour for this camera, so the picture uses Camera Matching. Mac only.";

/// The menu item that moves every photo of the camera.
pub(crate) const ALL_FROM_CAMERA: &str = "Apply macOS Colour to All Photos from This Camera…";

/// Whether this computer's raw decoder reads photo `id`'s camera.
fn listed(app: &LightcraftApp, id: PhotoId) -> bool {
    app.session.catalog.photo(id).is_some_and(|p| lightcraft_engine::files::macos_lists(p))
}

/// The macOS Colour row of photo `id`'s picker: shown where the decoder reads its camera, and, greyed, for a photo set to
/// it that this computer can't make it for. `None`: not offered.
pub(crate) fn row(app: &LightcraftApp, id: PhotoId, d: &DevelopSettings) -> Option<Row> {
    let on = listed(app, id);
    (on || d.profile.camera_colour == CameraColour::Macos).then(|| Row {
        colour: CameraColour::Macos,
        profile: None,
        label: LABEL.into(),
        tip: crate::i18n::tr(if on { TIP } else { TIP_OFF }).to_string(),
        enabled: on,
    })
}

/// The line under the Profile picker for a photo set to macOS Colour whose picture uses Camera Matching, and why.
pub(crate) fn note(app: &LightcraftApp, id: PhotoId, d: &DevelopSettings) -> Option<String> {
    if d.profile.camera_colour != CameraColour::Macos {
        return None;
    }
    let why = match app.session.source_info(id).macos_note {
        Some(why) => why,
        // no decoded source yet: what the catalog says
        None if !listed(app, id) => MacosUnavailable::System,
        None => return None,
    };
    Some(note_text(why))
}

pub(crate) fn note_text(why: MacosUnavailable) -> String {
    crate::i18n::tr(match why {
        MacosUnavailable::System | MacosUnavailable::Camera => "Edited with macOS Colour. This picture uses Camera Matching, so colours differ.",
        MacosUnavailable::Size => "macOS reads this raw at a different size, so this picture uses Camera Matching.",
        MacosUnavailable::Unreadable => "macOS could not read this raw, so this picture uses Camera Matching.",
        MacosUnavailable::Transfer => "AI Denoise uses Camera Matching colours on this photo.",
    })
    .to_string()
}

/// Draws the note under the Profile picker (the Edit panel), when there is one.
pub(crate) fn show_note(app: &LightcraftApp, ui: &mut egui::Ui, t: &Tokens, id: PhotoId, d: &DevelopSettings) {
    if let Some(text) = note(app, id, d) {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 8 }).show(ui, |ui| {
            let r = ui.add(egui::Label::new(egui::RichText::new(text).font(t.font(12.0)).color(t.text_dim)).wrap());
            crate::widgets::register(ui, "label:profile.macosNote", r.rect);
        });
    }
}

/// The Profile menu's item, under the colour rows, where this Mac can make macOS Colour for the photo's camera.
pub(crate) fn menu_item(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    if !listed(app, id) {
        return;
    }
    let r = ui.button(crate::i18n::tr(ALL_FROM_CAMERA));
    crate::widgets::register(ui, "profileMenu:macosAllFromCamera", r.rect);
    if r.clicked() {
        ask(app, ui.ctx(), id);
    }
}

/// Counts the camera's photos first and asks; nothing to move says so instead, and so does a camera macOS is still
/// being asked about (the count never waits for it: an activity row checks it, and the next click asks). The count runs
/// on a worker ([`super::job`]); the question comes when it is done.
pub(crate) fn ask(app: &mut LightcraftApp, ctx: &egui::Context, id: PhotoId) {
    if let Err(e) = super::job::start(app, id.0, false) {
        app.toast(ctx, e);
    }
}

/// The count `plan` (`profiles.macosForCameraCount`'s answer) for photo `id` is in: ask, or say why not.
pub(crate) fn asked(app: &mut LightcraftApp, ctx: &egui::Context, id: u64, plan: &serde_json::Value) {
    if plan["checking"].as_u64().unwrap_or(0) > 0 {
        return app.toast(ctx, crate::i18n::tr(lightcraft_engine::cmd::macos_colour::CHECKING));
    }
    let count = plan["photos"].as_u64().unwrap_or(0) as usize;
    let skipped = plan["skipped"].as_u64().unwrap_or(0) as usize;
    let camera = plan["camera"].as_str().unwrap_or_default().to_string();
    if count == 0 {
        return app.toast(ctx, crate::i18n::tr_format!("Every photo from the {camera} that can use macOS Colour already does.", camera = camera));
    }
    app.ui.dialog = Some(Dialog::MacosForCamera { photo: id, camera, count, skipped });
}

/// The line for photos a preset put on macOS Colour that this computer makes with Camera Matching colours
/// (`preset.apply`'s `macosFallback`).
pub(crate) fn fallback_line(applied: &serde_json::Value) -> Option<String> {
    let n = applied.get("macosFallback").and_then(serde_json::Value::as_u64).filter(|n| *n > 0)?;
    Some(if n == 1 {
        crate::i18n::tr("1 photo can't use macOS Colour here and uses Camera Matching.").to_string()
    } else {
        crate::i18n::tr_format!("{n} photos can't use macOS Colour here and use Camera Matching.", n = n)
    })
}

/// Says a preset was applied, and how many of its photos can't use macOS Colour here (longer, so it can be read).
pub(crate) fn preset_applied(app: &mut LightcraftApp, ctx: &egui::Context, text: String, applied: &serde_json::Value) {
    match fallback_line(applied) {
        Some(line) => app.toast_for(ctx, format!("{text}. {line}"), 8.0),
        None => app.toast(ctx, text),
    }
}

/// The question's text.
pub(crate) fn body(ui: &mut egui::Ui, t: &Tokens, camera: &str, count: usize, skipped: usize) {
    let question = if count == 1 {
        crate::i18n::tr_format!("Put 1 photo from the {camera} on macOS Colour?", camera = camera)
    } else {
        crate::i18n::tr_format!("Put {count} photos from the {camera} on macOS Colour?", count = count, camera = camera)
    };
    ui.label(question);
    if skipped > 0 {
        let line = crate::i18n::tr_format!("{n} photos can't use macOS Colour on this Mac and stay as they are.", n = skipped);
        let r = ui.label(egui::RichText::new(line).color(t.text_dim));
        crate::widgets::register(ui, "label:macosForCameraSkipped", r.rect);
    }
    ui.label(egui::RichText::new(crate::i18n::tr("One step: Undo in the Edit menu takes it back.")).color(t.text_dim));
}
