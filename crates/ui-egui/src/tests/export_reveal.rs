//! Export ▸ Show in Finder: the option reveals the first file written, only where the host gave
//! the app an opener, only for an export started in the app (the Export dialog, Export with Previous),
//! never for one an agent runs through the control channel, and a failing opener is a message, never a panic.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{Value, json};

use crate::{LightcraftApp, Services};

/// A demo app whose writer keeps the files in memory and whose opener records its calls.
fn setup(opener: Option<Result<(), String>>) -> (LightcraftApp, Rc<RefCell<Vec<String>>>) {
    let revealed = Rc::new(RefCell::new(Vec::new()));
    let mut services = Services { png: None, ..Default::default() };
    services.write = Some(Box::new(|_, _| Ok(())));
    if let Some(result) = opener {
        let seen = revealed.clone();
        services.reveal = Some(Box::new(move |path: &str| {
            seen.borrow_mut().push(path.to_string());
            result.clone()
        }));
    }
    (LightcraftApp::new(lightcraft_engine::Session::with_demo(), services), revealed)
}

fn export(app: &mut LightcraftApp, show: bool) -> Value {
    let params = json!({"dir": "out", "longEdge": 32, "showInFinder": show});
    crate::control::export_active(app, &params).unwrap()
}

#[test]
fn the_first_file_written_is_revealed_when_the_option_is_on() {
    let (mut app, revealed) = setup(Some(Ok(())));
    let out = export(&mut app, true);
    let first = out["files"][0]["path"].as_str().unwrap().to_string();
    assert_eq!(*revealed.borrow(), vec![first]);
    assert_eq!(out["shownInFinder"], true);
}

/// A control-channel request (an agent, the CLI, MCP into the running app) for `command`.
fn control(app: &mut LightcraftApp, command: &str, params: Value) -> Value {
    let (req, _rx) = crate::control::ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    match crate::control::handle(app, &egui::Context::default(), &req) {
        crate::control::Outcome::Done(reply) => reply,
        _ => panic!("expected a reply"),
    }
}

#[test]
fn an_agent_export_never_reveals_even_with_the_option_on() {
    let (mut app, revealed) = setup(Some(Ok(())));
    let r = control(&mut app, "app.export", json!({"dir": "out", "longEdge": 32, "showInFinder": true}));
    let out = &r["result"];
    assert!(out["files"][0]["path"].is_string(), "{r}");
    assert!(revealed.borrow().is_empty(), "an agent export opened the file manager");
    assert!(out.get("shownInFinder").is_none() && out.get("showInFinderError").is_none(), "{r}");
    assert!(!app.from_control, "the flag is only set while the command runs");
}

#[test]
fn export_with_previous_reveals_for_the_user_but_not_for_an_agent() {
    // the previous export carries the option (as the dialog saved it)
    let (mut app, revealed) = setup(Some(Ok(())));
    export(&mut app, true);
    assert_eq!(revealed.borrow().len(), 1);
    control(&mut app, "app.exportPrevious", json!({}));
    assert_eq!(revealed.borrow().len(), 1, "an agent's Export with Previous revealed the file");
    app.run("app.exportPrevious", json!({})).unwrap();
    assert_eq!(revealed.borrow().len(), 2, "the menu's Export with Previous still reveals");
}

#[test]
fn nothing_is_revealed_with_the_option_off() {
    let (mut app, revealed) = setup(Some(Ok(())));
    let out = export(&mut app, false);
    assert!(revealed.borrow().is_empty());
    assert!(out.get("shownInFinder").is_none());
}

#[test]
fn a_run_without_an_opener_exports_and_shows_nothing() {
    // headless runs, the CLI and the web have no opener
    let (mut app, _) = setup(None);
    let out = export(&mut app, true);
    assert!(out["files"][0]["path"].is_string(), "{out}");
    assert!(out.get("shownInFinder").is_none() && out.get("showInFinderError").is_none(), "{out}");
}

#[test]
fn a_failing_opener_is_reported_and_the_export_still_succeeds() {
    let (mut app, revealed) = setup(Some(Err("no file manager".into())));
    let out = export(&mut app, true);
    assert_eq!(revealed.borrow().len(), 1);
    assert_eq!(out["showInFinderError"], "no file manager");
    assert!(out["files"][0]["path"].is_string());
    let files = out["files"].as_array().unwrap();
    let note = crate::export_reveal::reveal_note(&mut app, true, files).unwrap();
    assert!(note.contains("no file manager"), "{note}");
}

#[test]
fn skipped_and_failed_photos_have_nothing_to_reveal() {
    let (mut app, revealed) = setup(Some(Ok(())));
    let files = vec![json!({"skipped": "a.jpg"}), json!({"photo": 1, "file": "b", "error": "x"})];
    assert_eq!(crate::export_reveal::reveal_exported(&mut app, true, &files), Ok(false));
    assert!(revealed.borrow().is_empty());
}

#[test]
fn the_label_names_the_platforms_file_manager() {
    let label = crate::export_reveal::label();
    assert!(label.starts_with("Show in "));
    if cfg!(target_os = "macos") {
        assert_eq!(label, "Show in Finder");
    }
}
