//! Renaming a row of a list in place: albums and folders of albums, keywords, presets, versions.
//!
//! One behaviour for all of them: double-click the row or choose Rename… and the name turns into a
//! [`TextField`]; Return or a click away saves, Esc cancels. A name that is empty or already
//! used is refused with the reason under the field (Return keeps the field open; a click away
//! puts the old name back and says why in a toast). Renaming a keyword to one that exists asks
//! before merging. The shortcuts step aside while the field has the focus (the field's own rule).
//!
//! Which names are acceptable is [`lightcraft_engine::names::check_name`], the function the
//! commands use, so the field and the command agree. The change itself is the existing command
//! (`album.rename`, `keyword.rename`, `preset.rename`, `version.rename`).

use serde_json::json;

use crate::LightcraftApp;
use crate::text_field::{Ending, TextField};
use crate::theme::Tokens;

/// What is being renamed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// An album, smart album or folder of albums, by id.
    Album(u64),
    /// A keyword, by its full path (`Travel|Italy`); the field edits the last part.
    Keyword(String),
    /// A user preset, by id.
    Preset(String),
    /// A version of the active photo, by index.
    Version(usize),
}

impl Kind {
    /// The widget id suffix of the row's field.
    fn widget(&self) -> String {
        match self {
            Kind::Album(id) => format!("album:{id}"),
            Kind::Keyword(path) => format!("keyword:{path}"),
            Kind::Preset(id) => format!("preset:{id}"),
            Kind::Version(i) => format!("version:{i}"),
        }
    }

    /// Whether the sidebar row `row` (`album:3`, `folder:3`, `keyword:Travel|Italy`) is this one.
    pub(crate) fn is_row(&self, row: &str) -> bool {
        match self {
            Kind::Album(id) => row == format!("album:{id}") || row == format!("folder:{id}"),
            Kind::Keyword(path) => row.strip_prefix("keyword:") == Some(path.as_str()),
            _ => false,
        }
    }
}

/// An inline rename under way (the app has at most one).
#[derive(Clone, Debug)]
pub(crate) struct RowRename {
    pub kind: Kind,
    /// The name as typed.
    pub text: String,
}

/// Start renaming `kind` (the name it has now is selected).
pub(crate) fn begin(app: &mut LightcraftApp, kind: Kind) {
    if let Some(text) = current_name(app, &kind) {
        app.ui.rename_row = Some(RowRename { kind, text });
    }
}

/// The name `kind` has now (what the field starts with).
fn current_name(app: &LightcraftApp, kind: &Kind) -> Option<String> {
    match kind {
        Kind::Album(id) => app.session.catalog.album(lightcraft_catalog::AlbumId(*id)).map(|a| a.name.clone()),
        Kind::Keyword(path) => app.session.catalog.has_keyword(path).then(|| leaf(path).to_string()),
        Kind::Preset(id) => app.session.presets.iter().find(|p| p.id == *id && !p.builtin).map(|p| p.name.clone()),
        Kind::Version(i) => {
            app.session.active().and_then(|id| app.session.catalog.photo(id)).and_then(|p| p.versions.get(*i)).map(|v| v.name.clone())
        }
    }
}

/// The last part of a keyword path.
fn leaf(path: &str) -> &str {
    path.rsplit_once('|').map_or(path, |(_, last)| last)
}

/// `text`, trimmed, when `kind` may take it; else the reason (in words, translated).
pub(crate) fn check(app: &LightcraftApp, kind: &Kind, text: &str) -> Result<String, String> {
    let cat = &app.session.catalog;
    let others: Vec<String> = match kind {
        Kind::Album(id) => {
            let parent = cat.album(lightcraft_catalog::AlbumId(*id)).and_then(|a| a.parent);
            cat.albums().filter(|a| a.id.0 != *id && a.parent == parent).map(|a| a.name.clone()).collect()
        }
        // a keyword renamed onto another one merges with it, after asking
        Kind::Keyword(_) => Vec::new(),
        Kind::Preset(id) => {
            let group = app.session.presets.iter().find(|p| p.id == *id).map(|p| p.group.clone()).unwrap_or_default();
            app.session.presets.iter().filter(|p| p.id != *id && p.group == group).map(|p| p.name.clone()).collect()
        }
        Kind::Version(i) => app
            .session
            .active()
            .and_then(|id| cat.photo(id))
            .map(|p| p.versions.iter().enumerate().filter(|(k, _)| k != i).map(|(_, v)| v.name.clone()).collect())
            .unwrap_or_default(),
    };
    verdict(text, others.iter().map(String::as_str))
}

/// How one frame of an inline rename field ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Done {
    /// Still being edited.
    Editing,
    /// Esc, or the edit was abandoned: the old name stays.
    Cancelled,
    /// Return, or a click away, with a name that can be used (trimmed).
    Save(String),
    /// A click away with a name that can't be used, and why: the old name stays.
    Refused(String),
}

/// What a field remembers between frames (temp data under the widget's name).
#[derive(Clone, Default)]
struct Memo {
    frames: u32,
    /// Why Return was refused, shown until the text changes.
    refused: Option<String>,
    text: String,
}

/// One inline rename field in `rect`, for any list: the shared behaviour. `text` is the name as
/// typed, `verdict` what [`check`] (or the caller's own rule) says of it, `unchanged` that it is
/// the name the item has now. The caller carries out `Save`.
pub(crate) fn edit(ui: &mut egui::Ui, widget: &str, text: &mut String, rect: egui::Rect, verdict: &Result<String, String>, unchanged: bool) -> Done {
    let memo_id = egui::Id::unique(("inline-rename", widget));
    let mut memo: Memo = ui.data(|d| d.get_temp(memo_id)).unwrap_or_default();
    let first = memo.frames == 0;
    if memo.text != *text {
        memo.refused = None;
    }
    let shown = ui
        .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            TextField::singleline(widget, text).width(rect.width()).select_on_focus(true).begin_edit(first).show(ui)
        })
        .inner;
    memo.frames = memo.frames.saturating_add(1);
    memo.text = text.clone();
    // the field lost the focus without ending the edit (the row scrolled out of sight and back)
    let lost = memo.frames > 2 && !shown.editing && shown.ending.is_none() && memo.refused.is_none();
    let done = match shown.ending {
        Some(Ending::Escape) => Done::Cancelled,
        _ if lost => Done::Cancelled,
        Some(_) if unchanged => Done::Cancelled,
        Some(Ending::Return) => match verdict {
            Ok(name) => Done::Save(name.clone()),
            Err(why) => {
                memo.refused = Some(why.clone());
                shown.response.request_focus();
                Done::Editing
            }
        },
        Some(Ending::Away) => match verdict {
            Ok(name) => Done::Save(name.clone()),
            Err(why) => Done::Refused(why.clone()),
        },
        None => Done::Editing,
    };
    if done == Done::Editing {
        // the reason, live as the name is typed, and after a refused Return
        let live = verdict.as_ref().err().filter(|_| !unchanged && shown.editing);
        if let Some(reason) = memo.refused.as_ref().or(live) {
            reason_note(ui, widget, rect, reason);
        }
        ui.data_mut(|d| d.insert_temp(memo_id, memo));
    } else {
        ui.data_mut(|d| d.remove::<Memo>(memo_id));
    }
    done
}

/// [`lightcraft_engine::names::check_name`] with the reason in the UI's words.
pub(crate) fn verdict<'a>(text: &str, others: impl IntoIterator<Item = &'a str>) -> Result<String, String> {
    lightcraft_engine::names::check_name(text, others).map_err(|problem| match problem {
        lightcraft_engine::names::NameProblem::Empty => crate::i18n::tr("Give it a name").to_string(),
        lightcraft_engine::names::NameProblem::Taken(other) => crate::i18n::tr_format!("“{name}” is already used", name = other),
    })
}

/// Act on how a field kept in the caller's own state ended: run `command` with `params` and the
/// name under `key` when it is saved (a refusal of the command is a toast, the catalog unchanged),
/// say why when a click away was refused. `keep` gets `Some(text)` while the edit goes on and
/// `None` when it is over.
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_with(
    app: &mut LightcraftApp,
    ctx: &egui::Context,
    outcome: Done,
    command: &str,
    mut params: serde_json::Value,
    key: &str,
    text: String,
    keep: impl FnOnce(&mut LightcraftApp, Option<String>),
) {
    match outcome {
        Done::Editing => keep(app, Some(text)),
        Done::Cancelled => keep(app, None),
        Done::Save(name) => {
            keep(app, None);
            params[key] = serde_json::Value::String(name);
            if let Err(e) = app.run(command, params) {
                app.toast(ctx, e);
            }
        }
        Done::Refused(why) => {
            keep(app, None);
            app.toast(ctx, crate::i18n::tr_format!("Not renamed: {why}", why = why));
        }
    }
}

/// Draw the field of the rename of `kind` inside `rect`, and act on how the edit ends. Nothing is
/// drawn when `kind` is not the one being renamed.
pub(crate) fn field(app: &mut LightcraftApp, ui: &mut egui::Ui, kind: &Kind, rect: egui::Rect) {
    let Some(mut state) = app.ui.rename_row.take().filter(|r| &r.kind == kind) else { return };
    let widget = format!("field:rename:{}", kind.widget());
    let verdict = check(app, kind, &state.text);
    let unchanged = current_name(app, kind).is_some_and(|now| now == state.text.trim());
    match edit(ui, &widget, &mut state.text, rect, &verdict, unchanged) {
        Done::Editing => app.ui.rename_row = Some(state),
        Done::Cancelled => {}
        Done::Save(name) => finish(app, ui.ctx(), kind, name),
        Done::Refused(why) => app.toast(ui.ctx(), crate::i18n::tr_format!("Not renamed: {why}", why = why)),
    }
}

/// The reason a name is refused, in a small note under the field.
fn reason_note(ui: &egui::Ui, widget: &str, rect: egui::Rect, reason: &str) {
    let t = Tokens::get(ui.ctx());
    let area = egui::Area::new(egui::Id::unique(("rename-reason", widget)))
        .order(egui::Order::Tooltip)
        .fixed_pos(rect.left_bottom() + egui::vec2(0.0, 2.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(egui::RichText::new(reason).color(t.caution).size(12.0));
            });
        });
    crate::widgets::register(ui, format!("renameReason:{widget}"), area.response.rect);
}

/// Carry out the rename of `kind` to `name` (already checked): the existing command, and a toast
/// with the reason when it refuses (the catalog is then unchanged).
fn finish(app: &mut LightcraftApp, ctx: &egui::Context, kind: &Kind, name: String) {
    let result = match kind {
        Kind::Album(id) => app.run("album.rename", json!({"id": id, "name": name})),
        Kind::Keyword(path) => {
            let to = match path.rsplit_once('|') {
                Some((parent, _)) => format!("{parent}|{name}"),
                None => name,
            };
            if app.session.catalog.has_keyword(&to) && !lightcraft_catalog::keywords::same(path, &to) {
                // a keyword that exists: the merge is asked about, not done
                app.ui.dialog = Some(crate::state::Dialog::RenameKeyword { from: path.clone(), to });
                return;
            }
            app.run("keyword.rename", json!({"from": path, "to": to}))
        }
        Kind::Preset(id) => app.run("preset.rename", json!({"id": id, "name": name})),
        Kind::Version(i) => app.run("version.rename", json!({"index": i, "name": name})),
    };
    if let Err(e) = result {
        app.toast(ctx, e);
    }
}

/// The question the Rename Keyword window asks when the new name already exists: what merges into what.
pub(crate) fn merge_question(app: &LightcraftApp, from: &str, to: &str) -> Option<String> {
    (app.session.catalog.has_keyword(to) && !lightcraft_catalog::keywords::same(from, to)).then(|| {
        crate::i18n::tr_format!("“{to}” already exists. Merge “{from}” into it? Photos with “{from}” get “{to}” instead.", from = from, to = to)
    })
}
