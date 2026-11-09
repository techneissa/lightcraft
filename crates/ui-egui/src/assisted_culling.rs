//! Fork feature (`fork/ai-culling.md`): Photo ▸ Assisted Culling in the app. The photos are read on worker threads
//! under a row in the activity stack that counts them and can stop (each photo's thumbnail, and for the eyes a render
//! and, without a people parse, the face detector and face mesh), so the window never waits on them. A stopped run
//! changes nothing.
//!
//! From a menu the command answers at once; when the run ends a toast says what it found, and photos whose eyes are
//! closed or out of focus are offered for rejection in a dialog (nothing is rejected unless the user says so). A
//! request from the control channel (an agent, `mcp --connect`) gets the command's answer when the run ends.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use lightcraft_engine::cmd::cull_run;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::control::{self, Outcome};
use crate::state::Dialog;

/// The activity stack's label (the menu item's name).
pub const LABEL: &str = "Assisted Culling";

/// How a run ended.
enum End {
    Done(Value),
    Dropped(&'static str),
    Failed(String),
}

/// Start a run for `p`: `end` runs on the UI thread when it is over. How many photos it reads, or why it can't start.
fn start(app: &mut LightcraftApp, p: &Value, end: impl FnOnce(&mut LightcraftApp, &egui::Context, End) + Send + 'static) -> Result<usize, String> {
    cull_run::no_run_going(&app.session).map_err(|why| format!("photo.analyze: {why}"))?;
    let plan = cull_run::plan(&mut app.session, p).map_err(|e| e.to_string())?;
    let count = plan.len();
    let library = app.session.library_generation();
    let cancel = Arc::new(AtomicBool::new(false));
    let (flag, stopped) = (cancel.clone(), cancel.clone());
    let work = move |task: lightcraft_engine::activity::TaskHandle| {
        let total = count as u64;
        task.progress(0, total);
        lightcraft_engine::guard::catch(LABEL, || cull_run::measure(plan, &flag, &|done| task.progress(done as u64, total)))
    };
    let done = move |app: &mut LightcraftApp, ctx: &egui::Context, measured: Result<cull_run::Measured, String>| {
        let ended = match measured {
            _ if app.session.library_generation() != library => End::Dropped("another library was opened before the run ended"),
            Err(e) => End::Failed(e),
            Ok(m) if m.stopped || stopped.load(Ordering::Relaxed) => End::Dropped("stopped: nothing was changed"),
            Ok(m) => match app.session.execute_fn("photo.analyze", |s| cull_run::apply(s, m)) {
                Ok(answer) => End::Done(answer),
                Err(e) => End::Failed(e.to_string()),
            },
        };
        end(app, ctx, ended);
    };
    crate::tasks::spawn_tracked(app, LABEL, cull_run::KIND, cancel, work, done)?;
    Ok(count)
}

/// The ids of a run's suggestions for one reason.
fn suggested(answer: &Value, why: &str) -> Vec<u64> {
    answer["suggestReject"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| v["why"].as_array().is_some_and(|w| w.iter().any(|x| x == why)))
        .filter_map(|v| v["id"].as_u64())
        .collect()
}

/// What a finished run says in its toast.
fn summary(answer: &Value) -> String {
    let n = answer["photos"].as_array().map_or(0, Vec::len);
    let mut text = crate::i18n::tr_format!("Assisted Culling scored {n} photos", n = n);
    let groups = answer["groups"].as_u64().unwrap_or(0);
    if groups > 0 {
        text.push_str(&crate::i18n::tr_format!(" · {groups} bursts", groups = groups));
    }
    if let Some(why) = answer["eyes"]["unavailable"].as_str()
        && answer["eyes"]["failed"].as_u64().unwrap_or(0) > 0
    {
        text.push_str(&format!(" · {}", crate::i18n::tr_format!("eyes not checked: {why}", why = why)));
    }
    text
}

/// The dialog offering a run's suggestions, when it has any.
pub fn suggestions(answer: &Value) -> Option<Dialog> {
    let (closed, soft) = (suggested(answer, "eyesClosed"), suggested(answer, "eyesOutOfFocus"));
    (!closed.is_empty() || !soft.is_empty()).then_some(Dialog::CullSuggest {
        reject_closed: !closed.is_empty(),
        reject_soft: !soft.is_empty(),
        closed,
        soft,
    })
}

/// The photos a suggestions dialog rejects with its boxes as they are (each once).
pub fn chosen(closed: &[u64], soft: &[u64], reject_closed: bool, reject_soft: bool) -> Vec<lightcraft_engine::catalog::PhotoId> {
    let mut ids: Vec<u64> = Vec::new();
    for (on, list) in [(reject_closed, closed), (reject_soft, soft)] {
        for id in list.iter().filter(|_| on) {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
    }
    ids.into_iter().map(lightcraft_engine::catalog::PhotoId).collect()
}

/// `photo.analyze` from a menu: start reading → `{background: true, count}`.
pub fn command(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let count = start(app, p, |app, ctx, end| match end {
        End::Done(answer) => {
            app.toast(ctx, summary(&answer));
            if app.ui.dialog.is_none() {
                app.ui.dialog = suggestions(&answer);
            }
        }
        End::Dropped(_) => {}
        End::Failed(e) => app.toast(ctx, e),
    })?;
    Ok(json!({"background": true, "count": count}))
}

/// `photo.analyze` from the control channel: `reply` gets the command's answer when the run ends.
pub fn ask(app: &mut LightcraftApp, p: &Value, reply: Sender<Value>) -> Outcome {
    let started = start(app, p, move |_, _, end| {
        let answer = match end {
            End::Done(answer) => control::success(answer),
            End::Dropped(why) => control::failure(format!("photo.analyze: {why}")),
            End::Failed(e) => control::failure(e),
        };
        let _ = reply.send(answer);
    });
    match started {
        Ok(_) => Outcome::Later,
        Err(e) => Outcome::Done(control::failure(e)),
    }
}

/// Another library was opened: stop a run of the one before.
pub fn library_changed(app: &LightcraftApp) {
    for row in app.session.activity.list().into_iter().filter(|r| r.kind == cull_run::KIND) {
        let _ = app.session.activity.cancel(row.id);
    }
}

#[cfg(test)]
mod tests;
