//! Settings ▸ Import ▸ Lens profiles: the folder of a lensfun-format lens database
//! ([`lightcraft_engine::lens_profiles`]). `LIGHTCRAFT_LENSFUN_DIR` still wins when it is set.
//!
//! The folder is an app setting ([`crate::state::AppSettings::lens_database_dir`]) and is handed to
//! the engine when it changes. Photos pick up a changed database when they are imported or reloaded
//! (`photo.reload`): each keeps the profile it was last read with.
//!
//! Agents use `lensDatabase.get`, `lensDatabase.set {dir}` (null or "" clears it) and
//! `lensDatabase.reload`.

use lightcraft_engine::lens_profiles::{self, FolderSource, Problem, Status};
use serde_json::{Value, json};

use super::widgets::{button_width, hint, path_label, row};
use super::{LightcraftApp, RichText, Tokens, heading, register};

/// The folder set in Settings, for the engine.
fn folder(app: &LightcraftApp) -> Option<std::path::PathBuf> {
    let dir = app.ui.settings.lens_database_dir.trim();
    (!dir.is_empty()).then(|| dir.into())
}

/// Per frame (with the other settings that act outside the UI state): hand a changed folder to the
/// engine. Does nothing while the app has never had one, so other windows' state is left alone.
pub(crate) fn sync(ctx: &egui::Context, app: &LightcraftApp) {
    let id = egui::Id::unique("lens-database-applied");
    let applied: Option<Option<std::path::PathBuf>> = ctx.data(|d| d.get_temp(id));
    let wanted = folder(app);
    if applied.unwrap_or(None) != wanted {
        lens_profiles::set_folder(wanted.as_deref());
    }
    ctx.data_mut(|d| d.insert_temp(id, wanted));
}

/// One line about the database in use: how many profiles, or why none.
pub(crate) fn status_line(s: &Status) -> String {
    let mut line = match (&s.problem, s.dir.as_ref()) {
        (_, None) => crate::i18n::tr("No folder chosen").to_string(),
        (None, Some(_)) if s.skipped > 0 => {
            crate::i18n::tr_format!("{n} lens profiles found, {k} files skipped", n = s.profiles, k = s.skipped)
        }
        (None, Some(_)) if s.profiles == 1 => crate::i18n::tr("1 lens profile found").to_string(),
        (None, Some(_)) => crate::i18n::tr_format!("{n} lens profiles found", n = s.profiles),
        (Some(Problem::Missing), _) => crate::i18n::tr("No profiles: the folder doesn't exist").to_string(),
        (Some(Problem::Unreadable(why)), _) => crate::i18n::tr_format!("No profiles: the folder can't be read ({why})", why = why),
        (Some(Problem::NoXml), _) => crate::i18n::tr("No profiles: the folder has no XML files").to_string(),
        (Some(Problem::NoLenses), _) => crate::i18n::tr("No profiles: the XML files hold no lens profiles").to_string(),
    };
    if s.source == FolderSource::Env {
        line += &crate::i18n::tr_format!(" (from {env}, which wins over the setting)", env = lens_profiles::ENV);
    }
    line
}

fn status_json(app: &LightcraftApp) -> Value {
    let s = lens_profiles::status();
    let source = match s.source {
        FolderSource::None => "none",
        FolderSource::Env => "env",
        FolderSource::Setting => "setting",
    };
    let problem = s.problem.as_ref().map(|p| match p {
        Problem::Missing => "missing",
        Problem::Unreadable(_) => "unreadable",
        Problem::NoXml => "noXml",
        Problem::NoLenses => "noLenses",
    });
    json!({
        "dir": s.dir, "setting": folder(app), "source": source, "env": lens_profiles::ENV,
        "profiles": s.profiles, "skipped": s.skipped, "problem": problem, "message": status_line(&s),
    })
}

/// `lensDatabase.get` / `lensDatabase.set {dir}` / `lensDatabase.reload`.
pub(crate) fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    match id {
        "lensDatabase.get" => {}
        "lensDatabase.set" => {
            let dir = match p.get("dir") {
                Some(Value::String(s)) => s.trim().to_string(),
                Some(Value::Null) => String::new(),
                _ => return Err("lensDatabase.set: `dir` must be a folder path, or null to clear it".into()),
            };
            if dir.contains('\0') {
                return Err("lensDatabase.set: `dir` can't contain a NUL character".into());
            }
            app.ui.settings.lens_database_dir = dir;
            lens_profiles::set_folder(folder(app).as_deref());
        }
        "lensDatabase.reload" => lens_profiles::reload(),
        _ => return Err(format!("unknown command `{id}`")),
    }
    Ok(status_json(app))
}

pub(super) fn lens_database_rows(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, t, "Lens profiles");
    let dir = app.ui.settings.lens_database_dir.clone();
    row(ui, t, crate::i18n::tr("Lens database"), |ui| {
        let choose = crate::i18n::tr("Choose…");
        if dir.is_empty() {
            ui.label(RichText::new(crate::i18n::tr("None")).color(t.text));
        } else {
            let after = button_width(ui, choose) + button_width(ui, crate::i18n::tr("Clear"));
            path_label(ui, "settingsLensDatabase", &dir, egui::TextStyle::Body.resolve(ui.style()), t.text, after);
        }
        let can = app.services.pick_folder.is_some();
        let r = ui.add_enabled(can, egui::Button::new(choose));
        register(ui, "button:settingsLensDatabaseChoose", r.rect);
        if r.clicked()
            && let Some(f) = app.services.pick_folder.as_mut().and_then(|f| f())
        {
            let _ = app.run("lensDatabase.set", json!({"dir": f}));
        }
        let r = ui.add_enabled(!dir.is_empty(), egui::Button::new(crate::i18n::tr("Clear")));
        register(ui, "button:settingsLensDatabaseClear", r.rect);
        if r.clicked() {
            let _ = app.run("lensDatabase.set", json!({"dir": null}));
        }
    });
    // (read once per folder: a big lensfun folder takes a moment)
    hint(ui, t, &status_line(&lens_profiles::status()));
}
