//! The photo menu's own items: what each is called, when it shows, and what it does. They are the
//! rows of the official menu ([`crate::panels::grid::context_menu`]), each on its own so the menu
//! can be reordered, plus Before/After Split and Customize Menu….

use lightcraft_catalog::PhotoId;
use serde_json::json;

use crate::LightcraftApp;
use crate::panels::grid::{albums_hint, label_menu, local_only, toast_skipped};
use crate::state::{BeforeAfter, ViewMode};
use crate::widgets::register;

/// One built-in item. `several`: it needs two or more selected photos, and is hidden with one.
/// `when`: the editor's note on when else it hides.
pub struct Builtin {
    pub id: &'static str,
    pub label: &'static str,
    pub several: bool,
    pub when: Option<&'static str>,
    /// It opens a submenu (the editor shows ›).
    pub submenu: bool,
}

const fn item(id: &'static str, label: &'static str) -> Builtin {
    Builtin { id, label, several: false, when: None, submenu: false }
}

const fn several_submenu(id: &'static str, label: &'static str) -> Builtin {
    Builtin { id, label, several: true, when: None, submenu: true }
}

const fn sub(id: &'static str, label: &'static str) -> Builtin {
    Builtin { id, label, several: false, when: None, submenu: true }
}

const fn only(id: &'static str, label: &'static str, when: &'static str) -> Builtin {
    Builtin { id, label, several: false, when: Some(when), submenu: false }
}

/// Every built-in item, in the default menu's order.
pub const BUILTINS: &[Builtin] = &[
    item("openInDetail", "Open in Detail"),
    item("beforeAfterSplit", "Before/After Split"),
    sub("editIn", "Edit In"),
    item("findSimilar", "Find Similar Photos"),
    item("setReference", "Set as Reference Photo"),
    sub("rating", "Set Rating"),
    sub("flag", "Set Flag"),
    sub("colorLabel", "Set Color Label"),
    sub("addToAlbum", "Add to Album"),
    only("removeFromAlbum", "Remove from Album", "in an album"),
    only("setAlbumCover", "Set as Album Cover", "in an album"),
    item("rename", "Rename…"),
    item("moveToFolder", "Move to Folder…"),
    item("reload", "Reload from Disk"),
    item("virtualCopy", "Create Virtual Copy"),
    item("createVersion", "Create Version"),
    sub("stack", "Stack"),
    item("copySettings", "Copy Edit Settings"),
    item("pasteSettings", "Paste Edit Settings"),
    item("pasteSelected", "Paste Selected Settings…"),
    item("resetEdits", "Reset Edits"),
    several_submenu("photoMerge", "Photo Merge"),
    item("rotateLeft", "Rotate Left"),
    item("rotateRight", "Rotate Right"),
    only("locateMissing", "Locate Missing File…", "when the file is missing"),
    item("showInFinder", "Show in Finder"),
    item("showInFolders", "Show in Folders"),
    item("export", "Export…"),
    sub("exportWithPreset", "Export with Preset"),
    Builtin { id: "exportAnimatedGif", label: "Export Animated GIF…", several: true, when: None, submenu: false },
    item("delete", "Delete Photo"),
    item("customize", "Customize Menu…"),
];

/// The built-in item `id`.
pub fn builtin(id: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.id == id)
}

/// What the item is called here: Show in Finder is Show in Explorer on Windows, and so on.
pub fn label(b: &Builtin) -> String {
    match b.id {
        "showInFinder" => crate::i18n::tr(crate::menus::reveal_label()).to_string(),
        _ => crate::i18n::tr(b.label).to_string(),
    }
}

/// The album a normal (not smart, not folder) album source is, for its two items.
fn normal_album(app: &LightcraftApp) -> Option<u64> {
    match app.session.source {
        lightcraft_engine::LibrarySource::Album(aid) if app.session.catalog.album(aid).is_some_and(|a| !a.folder && !a.is_smart()) => Some(aid.0),
        _ => None,
    }
}

fn file_missing(app: &LightcraftApp, id: PhotoId) -> bool {
    matches!(&app.session.catalog.photo(id).map(|p| p.source.clone()), Some(lightcraft_catalog::Source::File { path }) if app.session.media.availability.is_offline(path))
}

/// Whether item `b` is in the menu for `photo` right now. `several`: two or more photos are
/// selected.
pub fn shown(app: &LightcraftApp, b: &Builtin, photo: PhotoId, several: bool) -> bool {
    if b.several && !several {
        return false;
    }
    match b.id {
        "removeFromAlbum" | "setAlbumCover" => normal_album(app).is_some(),
        "moveToFolder" => app.services.pick_folder.is_some() || app.services.picker.is_some(),
        "locateMissing" => file_missing(app, photo),
        _ => true,
    }
}

/// Whether Before/After Split is what the photo shows (its check mark).
pub fn split_on(app: &LightcraftApp) -> bool {
    matches!(app.ui.view, ViewMode::Detail | ViewMode::Compare) && app.ui.before_after == BeforeAfter::Split
}

/// Draw item `b` for `id` (one row, or a submenu) and run it when clicked.
pub fn draw(app: &mut LightcraftApp, ui: &mut egui::Ui, b: &Builtin, id: PhotoId) {
    let text = label(b);
    match b.id {
        "openInDetail" | "beforeAfterSplit" | "editIn" | "findSimilar" | "setReference" => draw_open_items(app, ui, b, id, &text),
        "rating" | "flag" | "colorLabel" | "addToAlbum" | "removeFromAlbum" | "setAlbumCover" => draw_meta_items(app, ui, b, id, &text),
        "rename" | "moveToFolder" | "reload" | "virtualCopy" | "createVersion" | "stack" => draw_file_items(app, ui, b, id, &text),
        "copySettings" | "pasteSettings" | "pasteSelected" | "resetEdits" | "photoMerge" | "rotateLeft" | "rotateRight" => {
            draw_edit_items(app, ui, b, &text)
        }
        "locateMissing" | "showInFinder" | "showInFolders" | "export" | "exportWithPreset" | "exportAnimatedGif" | "delete" | "customize" => {
            draw_export_items(app, ui, b, id, &text)
        }
        _ => {}
    }
}

fn draw_open_items(app: &mut LightcraftApp, ui: &mut egui::Ui, b: &Builtin, id: PhotoId, text: &str) {
    match b.id {
        "openInDetail" => {
            if ui.button(text).clicked() {
                let _ = app.run("view.detail", json!({}));
            }
        }
        "beforeAfterSplit" => {
            let r = ui.button(text);
            if split_on(app) {
                super::check_mark(ui, r.rect);
            }
            register(ui, "button:contextBeforeAfterSplit", r.rect);
            if r.clicked() {
                // from a grid it opens the photo with the split on; in Detail it switches it
                if matches!(app.ui.view, ViewMode::Detail | ViewMode::Compare) {
                    let _ = app.run("view.beforeAfterSplit", json!({}));
                } else {
                    match app.run("view.detail", json!({})) {
                        Ok(_) => app.ui.before_after = BeforeAfter::Split,
                        Err(e) => app.toast(ui.ctx(), e),
                    }
                }
            }
        }
        // Photo ▸ Edit In: a copy with this photo's edits, opened in PhotoCraft or another editor
        // (desktop only: the web can't start programs)
        "editIn" => {
            ui.add_enabled_ui(crate::menus::ui_enabled(app, "photo.editInPhotoCraft"), |ui| {
                crate::widgets::menu_button(ui, text, |ui| {
                    if ui.button(crate::i18n::tr("Edit in PhotoCraft")).clicked() {
                        let _ = app.run("photo.editInPhotoCraft", json!({}));
                    }
                    if ui.button(crate::i18n::tr("Edit in Other Editor")).clicked() {
                        let _ = app.run("photo.editInExternal", json!({}));
                    }
                    if ui
                        .add_enabled(
                            crate::menus::ui_enabled(app, "photo.openAsLayersInPhotocraft"),
                            egui::Button::new(crate::i18n::tr("Open as Layers in PhotoCraft…")),
                        )
                        .clicked()
                    {
                        let _ = app.run("photo.openAsLayersInPhotocraft", json!({}));
                    }
                    // the raw itself, unrendered: for a raw developer set as the other editor
                    if ui
                        .add_enabled(
                            crate::menus::ui_enabled(app, "photo.openOriginalInExternal"),
                            egui::Button::new(crate::i18n::tr("Open Original in Other Editor (RAW, no copy)")),
                        )
                        .clicked()
                    {
                        let _ = app.run("photo.openOriginalInExternal", json!({}));
                    }
                });
            });
        }
        "findSimilar" => {
            if ui.button(text).clicked() {
                match app.run("library.findSimilar", json!({"id": id.0})) {
                    Ok(r) => {
                        let n = r["photos"].as_array().map_or(0, Vec::len);
                        app.ui.view = ViewMode::PhotoGrid;
                        app.toast(
                            ui.ctx(),
                            crate::i18n::tr_format!("{n} similar photo{} · View ▸ Clear Filters to see all", if n == 1 { "" } else { "s" }, n = n),
                        );
                    }
                    Err(e) => app.toast(ui.ctx(), e),
                }
            }
        }
        "setReference" => {
            let live = crate::panels::compare::live(app, id);
            let r =
                ui.add_enabled(live, egui::Button::new(text)).on_disabled_hover_text(crate::i18n::tr("Restore the photo to use it as the reference"));
            if r.clicked()
                && let Err(e) = app.run("photo.setReference", json!({"id": id.0}))
            {
                app.toast(ui.ctx(), e);
            }
        }
        _ => {}
    }
}

fn draw_meta_items(app: &mut LightcraftApp, ui: &mut egui::Ui, b: &Builtin, id: PhotoId, text: &str) {
    match b.id {
        "rating" => {
            crate::widgets::menu_button(ui, text, |ui| {
                for r in 0..=5 {
                    if ui.button(if r == 0 { crate::i18n::tr("No Stars").to_string() } else { "★".repeat(r) }).clicked() {
                        let _ = app.run("photo.rate", json!({"rating": r}));
                    }
                }
            });
        }
        "flag" => {
            crate::widgets::menu_button(ui, text, |ui| {
                for (l, f) in [("Pick", "pick"), ("Reject", "reject"), ("Unflagged", "none")] {
                    if ui.button(crate::i18n::tr(l)).clicked() {
                        let _ = app.run("photo.flag", json!({"flag": f}));
                    }
                }
            });
        }
        "colorLabel" => {
            crate::widgets::menu_button(ui, text, |ui| label_menu(app, ui));
        }
        "addToAlbum" => add_to_album(app, ui, text.to_string()),
        "removeFromAlbum" => {
            if let Some(aid) = normal_album(app)
                && ui.button(text).clicked()
            {
                let _ = app.run("album.removePhotos", json!({"id": aid}));
            }
        }
        "setAlbumCover" => {
            if let Some(aid) = normal_album(app)
                && ui.button(text).clicked()
            {
                let _ = app.run("album.setCover", json!({"id": aid, "photo": id.0}));
            }
        }
        _ => {}
    }
}

fn draw_file_items(app: &mut LightcraftApp, ui: &mut egui::Ui, b: &Builtin, id: PhotoId, text: &str) {
    match b.id {
        "rename" => {
            if ui.button(text).clicked() {
                let _ = app.run("dialog.rename", json!({}));
            }
        }
        "moveToFolder" => {
            if ui.button(text).clicked() {
                move_to_folder(app, ui);
            }
        }
        "reload" => {
            // the files are read in the background (activity row `reload`)
            if ui.button(text).clicked()
                && let Err(e) = app.run("photo.reload", json!({}))
            {
                app.toast_error(ui.ctx(), e);
            }
        }
        "virtualCopy" => {
            if ui.button(text).clicked() {
                let _ = app.run("photo.virtualCopy", json!({}));
            }
        }
        "createVersion" => {
            if ui.button(text).clicked() {
                let _ = app.run("version.create", json!({}));
            }
        }
        "stack" => stack_menu(app, ui, text.to_string(), id),
        _ => {}
    }
}

fn draw_edit_items(app: &mut LightcraftApp, ui: &mut egui::Ui, b: &Builtin, text: &str) {
    match b.id {
        "copySettings" => {
            if ui.button(text).clicked() {
                let _ = app.run("develop.copy", json!({}));
            }
        }
        "pasteSettings" => {
            if ui.add_enabled(app.session.clipboard.is_some(), egui::Button::new(text)).clicked() {
                let _ = app.run("develop.paste", json!({}));
            }
        }
        "pasteSelected" => {
            if ui.add_enabled(app.session.clipboard.is_some(), egui::Button::new(text)).clicked() {
                let _ = app.run("dialog.pasteSettings", json!({}));
            }
        }
        "resetEdits" => {
            if ui.button(text).clicked() {
                let _ = app.run("develop.reset", json!({}));
            }
        }
        "photoMerge" => {
            crate::widgets::menu_button(ui, text, |ui| {
                let n = app.session.selected().len();
                for (cmd, l) in [("dialog.mergeHdr", "HDR…"), ("dialog.mergePanorama", "Panorama…"), ("dialog.mergeHdrPanorama", "HDR Panorama…")]
                {
                    if ui.add_enabled(n >= 2, egui::Button::new(crate::i18n::tr(l))).clicked() {
                        let _ = app.run(cmd, json!({}));
                    }
                }
            });
        }
        "rotateLeft" if ui.button(text).clicked() => {
            let _ = app.run("photo.rotateLeft", json!({}));
        }
        "rotateRight" if ui.button(text).clicked() => {
            let _ = app.run("photo.rotateRight", json!({}));
        }
        _ => {}
    }
}

fn draw_export_items(app: &mut LightcraftApp, ui: &mut egui::Ui, b: &Builtin, id: PhotoId, text: &str) {
    match b.id {
        // the original moved or its drive is gone: point the photo at the file again
        "locateMissing" => {
            if ui.button(text).clicked() {
                let _ = app.run("photo.locate", json!({}));
            }
        }
        "showInFinder" => {
            if ui.add_enabled(crate::menus::ui_enabled(app, "app.showInFinder"), egui::Button::new(text)).clicked() {
                let _ = app.run("app.showInFinder", json!({}));
            }
        }
        // the photo's folder in the sidebar's Folders section (the photo right-clicked, which a
        // multi-selection may leave other than the active one)
        "showInFolders" => {
            if ui.add_enabled(crate::menus::show_in_folders_enabled(app, Some(id)), egui::Button::new(text)).clicked() {
                let _ = app.run("app.showInFolders", json!({"id": id.0}));
            }
        }
        "export" => {
            if ui.button(text).clicked() {
                let _ = app.run("dialog.export", json!({}));
            }
        }
        "exportWithPreset" => export_with_preset(app, ui, text.to_string()),
        "exportAnimatedGif" => crate::animated_gif::context_menu_item(app, ui),
        "delete" => delete(app, ui, text.to_string()),
        "customize" => {
            let r = ui.button(text);
            register(ui, "button:contextCustomizeMenu", r.rect);
            if r.clicked() {
                let _ = app.run("app.settings", json!({"tab": super::SETTINGS_TAB}));
            }
        }
        _ => {}
    }
}

fn add_to_album(app: &mut LightcraftApp, ui: &mut egui::Ui, text: String) {
    if local_only(app, &app.session.selected()) {
        // albums hold library photos (issue #684): nothing here to add until they join My Photos
        let add = ui.add_enabled(false, egui::Button::new(text)).on_disabled_hover_text(albums_hint());
        register(ui, "button:contextAddToAlbum", add.rect);
        return;
    }
    let add = crate::widgets::menu_button(ui, text, |ui| {
        let albums: Vec<_> = app.session.catalog.albums().filter(|a| !a.folder && !a.is_smart()).map(|a| (a.id.0, a.name.clone())).collect();
        let any = !albums.is_empty();
        for (aid, name) in albums {
            let b = ui.button(name);
            register(ui, format!("button:contextAlbum:{aid}"), b.rect);
            if b.clicked() {
                match app.run("album.addPhotos", json!({"id": aid})) {
                    Ok(r) => toast_skipped(app, ui.ctx(), &r),
                    Err(e) => app.toast(ui.ctx(), e),
                }
            }
        }
        if any {
            ui.separator();
        }
        // as in the Photo menu: a new album holding the selection, also when there is none yet
        // to pick (issue #676: the submenu came up empty)
        let new = ui.button(crate::i18n::tr("New Album…"));
        register(ui, "button:contextNewAlbum", new.rect);
        if new.clicked() {
            let _ = app.run("dialog.newAlbum", json!({}));
        }
    });
    register(ui, "button:contextAddToAlbum", add.response.rect);
}

fn move_to_folder(app: &mut LightcraftApp, ui: &egui::Ui) {
    // the folder dialog goes through the shared picker (off the UI thread where the host has
    // one: then photo.moveToFolder runs when it closes)
    let req = crate::pick::PickRequest::folder(crate::i18n::tr("Move to Folder…"));
    let picked =
        crate::pick::ask(app, "photo.moveToFolder", &json!({}), "folder", req, |s| s.pick_folder.as_mut().and_then(|f| f()).map(|x| vec![x]));
    if let crate::pick::Picked::Now(v) = picked
        && let Some(into) = v.into_iter().next()
    {
        match app.run("photo.moveToFolder", json!({"folder": into})) {
            Ok(r) => app.toast(ui.ctx(), crate::i18n::tr_format!("Moved {} photo(s)", r["moved"])),
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
}

/// The Stack submenu's rows above Auto-Stack (label, command, enabled): as the official menu's,
/// but Group into Stack needs two photos, so with one selected it is not offered at all.
pub(super) fn stack_rows(several: bool, stacked: bool) -> Vec<(&'static str, &'static str, bool)> {
    [
        ("Group into Stack", "stack.group", several),
        ("Ungroup Stack", "stack.ungroup", stacked),
        ("Remove from Stack", "stack.remove", stacked),
        ("Set as Top of Stack", "stack.setTop", stacked),
        ("Move Up in Stack", "stack.moveUp", stacked),
        ("Move Down in Stack", "stack.moveDown", stacked),
        ("Split Stack", "stack.split", stacked),
        ("Expand/Collapse Stack", "stack.toggle", stacked),
    ]
    .into_iter()
    .filter(|(_, cmd, _)| several || *cmd != "stack.group")
    .collect()
}

fn stack_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, text: String, id: PhotoId) {
    crate::widgets::menu_button(ui, text, |ui| {
        let stacked = app.session.catalog.stack_of(id).is_some();
        let several = app.session.selection.ids.len() > 1;
        for (label, cmd, on) in stack_rows(several, stacked) {
            if ui.add_enabled(on, egui::Button::new(crate::i18n::tr(label))).clicked() {
                let _ = app.run(cmd, json!({}));
            }
        }
        ui.separator();
        if ui.button(crate::i18n::tr("Auto-Stack by Capture Time…")).clicked() {
            let _ = app.run("dialog.autoStack", json!({}));
        }
    });
}

fn export_with_preset(app: &mut LightcraftApp, ui: &mut egui::Ui, text: String) {
    crate::widgets::menu_button(ui, text, |ui| {
        for (p, _) in app.session.all_export_presets() {
            if ui.button(&p.name).clicked() {
                match app.run("app.export", json!({"preset": p.name, "background": true})) {
                    Ok(r) if r.get("background").is_some() => {}
                    Ok(r) => {
                        let n = r["files"].as_array().map_or(0, Vec::len);
                        app.toast(ui.ctx(), crate::i18n::tr_format!("Exported {n} photo{}", if n == 1 { "" } else { "s" }, n = n));
                    }
                    Err(e) => app.toast(ui.ctx(), e),
                }
            }
        }
    });
}

fn delete(app: &mut LightcraftApp, ui: &mut egui::Ui, text: String) {
    if crate::menubar::selection_deleted(app) {
        if ui.button(crate::i18n::tr("Restore")).clicked() {
            let _ = app.run("photo.restore", json!({}));
        }
        if ui.button(crate::i18n::tr("Delete Permanently")).clicked() {
            let _ = crate::menubar::run_item(app, "photo.deletePermanently", json!({}));
        }
    } else {
        let delete = ui.button(text);
        register(ui, "button:contextDeletePhoto", delete.rect);
        if delete.clicked() {
            let _ = crate::menubar::run_item(app, "photo.delete", json!({}));
        }
    }
}
