//! Hover help (fork/hover-help.md): every icon-only clickable control has hover text, the Solo Mode button on the
//! Develop section headers, and the hover text drawn on screen.
//!
//! The scan reads the accessibility tree the headless view builds: `on_hover_text` also sets a widget's
//! description, so a clickable node whose label is not painted text (an icon, a swatch, a star) and whose
//! description is empty has no hover text.

use std::time::Duration;

use serde_json::json;

use crate::headless::{A11yNode, Headless};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn app_at(size: [f32; 2]) -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let h = Headless::new(app, size, 1.0);
    h.view.ctx.enable_accesskit();
    // hover text appears at once, so a test needs no waiting for the pointer to rest
    h.view.ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 0.0);
    h
}

fn req(h: &mut Headless, method: &str, params: serde_json::Value) -> serde_json::Value {
    h.request(method, params, T)
}

fn settle(h: &mut Headless) {
    for _ in 0..4 {
        h.step();
    }
}

/// Clickable nodes that a sighted user sees as an icon, with no hover text.
fn missing(h: &Headless) -> Vec<String> {
    missing_in(&h.view.a11y, &h.painted_text())
}

fn missing_in(nodes: &[A11yNode], painted: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for n in nodes {
        let a_button = matches!(n.role.as_str(), "Button" | "DisclosureTriangle" | "ComboBox" | "Link" | "Tab");
        if !a_button || !n.clickable || !n.description.is_empty() {
            continue;
        }
        if painted_label(painted, n) {
            continue;
        }
        out.push(format!("{} {:?}", n.role, n.label));
    }
    out.sort();
    out.dedup();
    out
}

/// The node's name is text the user can read on screen: a text button, a row ("Garden, 3 photos" is the row
/// "Garden" with its count), a menu entry.
fn painted_label(painted: &[String], n: &A11yNode) -> bool {
    let name = n.label.split([',', ':', '·']).next().unwrap_or_default().trim();
    !name.is_empty() && painted.iter().any(|t| t.contains(name))
}

/// Clickable things that are not icon-only controls, by name: photo cells (a file name), the dock handles belong to craft-ui (the docking
/// library names them and draws no hover text of its own), the top bar's background is the bar itself, and the
/// histogram is a large drag area with its own readout.
fn exempt(label: &str) -> bool {
    // a photo's cell is named by its file name
    if label
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && (2..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        return true;
    }
    ["Collapse column", "Expand column", "Move panel column", "Move tab group", "Move panel window", "Top Bar", "Histogram"].contains(&label)
}

/// One screen to scan: `ui.set` params, commands to run, widgets to hover (so hover-only controls show).
struct Scene {
    name: &'static str,
    set: serde_json::Value,
    commands: Vec<(&'static str, serde_json::Value)>,
    hover: Vec<&'static str>,
}

fn scene(name: &'static str, set: serde_json::Value) -> Scene {
    Scene { name, set, commands: vec![], hover: vec![] }
}

fn scenes() -> Vec<Scene> {
    let sections = json!(["light", "color", "effects", "detail", "optics", "calibration"]);
    let edit = |name, extra: serde_json::Value| {
        let mut set = json!({"view": "detail", "leftPanel": false, "right": "edit", "openSections": sections});
        if let (Some(set), Some(extra)) = (set.as_object_mut(), extra.as_object()) {
            set.extend(extra.clone());
        }
        scene(name, set)
    };
    let mut over_header = edit("develop with a header under the pointer", json!({}));
    over_header.hover = vec!["section:light"];
    vec![
        scene("grid with sidebar", json!({"view": "photoGrid", "leftPanel": true, "right": "none"})),
        scene("square grid", json!({"view": "squareGrid", "leftPanel": false})),
        over_header,
        scene("crop", json!({"view": "detail", "leftPanel": false, "right": "crop"})),
        scene("remove", json!({"view": "detail", "leftPanel": false, "right": "remove"})),
        scene("red-eye", json!({"view": "detail", "leftPanel": false, "right": "redEye"})),
        scene("masking", json!({"view": "detail", "leftPanel": false, "right": "masking"})),
        scene("presets", json!({"view": "detail", "leftPanel": false, "right": "edit", "presets": true})),
        scene("versions", json!({"view": "detail", "leftPanel": false, "right": "versions"})),
        scene("activity", json!({"view": "detail", "leftPanel": false, "right": "activity"})),
        scene("keywords", json!({"view": "detail", "leftPanel": false, "right": "keywords"})),
        scene("info", json!({"view": "detail", "leftPanel": false, "right": "info"})),
        scene("map", json!({"view": "map", "leftPanel": false})),
        scene("book", json!({"view": "book", "leftPanel": false})),
        scene("people", json!({"view": "people", "leftPanel": false})),
        scene("compare", json!({"view": "compare", "leftPanel": false})),
        scene("survey", json!({"view": "survey", "leftPanel": false})),
    ]
}

/// Dialogs and windows opened over the grid, by command.
const DIALOGS: &[&str] = &[
    "dialog.export",
    "dialog.copySettings",
    "app.shortcuts",
    "dialog.print",
    "app.settings",
    "app.about",
    "app.whatsNew",
    "app.systemInfo",
    "dialog.mergeHdr",
    "dialog.mergePanorama",
    "dialog.contactSheet",
    "dialog.animatedGif",
    "dialog.slideshowExport",
    "dialog.book",
    "dialog.allMetadata",
    "dialog.createPreset",
    "dialog.cull",
    "dialog.autoStack",
    "dialog.manageMaskEffects",
    "dialog.faceModel",
    "dialog.newAlbum",
    "dialog.smartAlbum",
];

fn scan(h: &mut Headless, what: &str, report: &mut Vec<String>) {
    settle(h);
    for item in missing(h) {
        if !exempt(item.split('"').nth(1).unwrap_or("").split(", ").next().unwrap_or("")) {
            report.push(format!("{what}: {item}"));
        }
    }
}

#[test]
fn every_icon_only_control_has_hover_text() {
    let mut report = Vec::new();
    let mut h = app_at([1600.0, 1000.0]);
    for s in scenes() {
        let r = req(&mut h, "ui.set", s.set.clone());
        assert_eq!(r["ok"], true, "{}: {r}", s.name);
        for (command, params) in &s.commands {
            let _ = req(&mut h, "engine.execute", json!({"command": command, "params": params}));
        }
        settle(&mut h);
        for id in &s.hover {
            let r = req(&mut h, "ui.hoverWidget", json!({"id": id}));
            assert_eq!(r["ok"], true, "{}: hover {id}: {r}", s.name);
        }
        scan(&mut h, s.name, &mut report);
    }
    for command in DIALOGS {
        let _ = req(&mut h, "ui.set", json!({"view": "photoGrid", "leftPanel": true}));
        let r = req(&mut h, "engine.execute", json!({"command": command}));
        if r["ok"] != true {
            continue;
        }
        scan(&mut h, command, &mut report);
        let _ = req(&mut h, "ui.key", json!({"key": "Escape"}));
        settle(&mut h);
    }
    report.sort();
    report.dedup();
    assert!(report.is_empty(), "icon-only controls without hover text:\n{}", report.join("\n"));
}

/// The scan itself: an icon-only button without hover text is reported, one with it (or with a visible name) is not.
#[test]
fn the_scan_reports_an_icon_without_hover_text() {
    use crate::headless::HeadlessView;
    let mut view = HeadlessView::new();
    view.ctx.enable_accesskit();
    for _ in 0..3 {
        let raw = HeadlessView::raw_input(egui::vec2(400.0, 200.0), 1.0, 0.0, vec![]);
        view.run(raw, |ui| {
            let (_, bare) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
            crate::widgets::announce(&bare, "Mystery Icon");
            let (_, tipped) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
            crate::widgets::announce(&tipped, "Known Icon");
            tipped.on_hover_text("Do the Known Thing");
            // a text button: its name is on screen
            crate::widgets::text_button(ui, "plain", "Plain", false);
        });
    }
    let nodes = view.a11y.clone();
    let found = missing_in(&nodes, &view.painted_text());
    assert_eq!(found, vec![r#"Button "Mystery Icon""#.to_string()], "{nodes:?}");
}

fn solo_nodes(h: &Headless) -> Vec<String> {
    h.view.a11y.iter().filter(|n| n.label == "Solo Mode").map(|n| n.description.clone()).collect()
}

fn tooltip_shown(h: &Headless) -> bool {
    h.view.ctx.memory(|m| m.areas().visible_layer_ids().into_iter().any(|l| l.order == egui::Order::Tooltip))
}

#[test]
fn the_solo_button_turns_solo_mode_on_and_off_from_a_section_header() {
    let mut h = app_at([1200.0, 1400.0]);
    let r = req(&mut h, "ui.set", json!({"view": "detail", "leftPanel": false, "right": "edit", "openSections": ["light"]}));
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    // off: the button shows on the header under the pointer, and only there
    assert_eq!(req(&mut h, "ui.hoverWidget", json!({"id": "section:light"}))["ok"], true);
    settle(&mut h);
    assert_eq!(solo_nodes(&h), ["Turn Solo Mode On"]);
    assert!(!h.app.ui.single_panel);
    let r = req(&mut h, "ui.clickWidget", json!({"id": "sectionSolo:light"}));
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    assert!(h.app.ui.single_panel, "one click turns it on");
    assert!(crate::menubar::checked(&h.app, "view.soloSections") == Some(true));
    // on: it is lit on every header, the pointer on one of them or not
    assert_eq!(req(&mut h, "ui.hoverWidget", json!({"id": "section:color"}))["ok"], true);
    settle(&mut h);
    let all = solo_nodes(&h);
    assert!(all.len() >= 6 && all.iter().all(|d| d == "Turn Solo Mode Off"), "{all:?}");
    let r = req(&mut h, "ui.clickWidget", json!({"id": "sectionSolo:color"}));
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    assert!(!h.app.ui.single_panel, "one click turns it off");
}

#[test]
fn hover_text_is_drawn_for_the_eye_and_the_solo_button() {
    let mut h = app_at([1200.0, 1400.0]);
    req(&mut h, "ui.set", json!({"view": "detail", "leftPanel": false, "right": "edit", "openSections": ["light"]}));
    settle(&mut h);
    for (id, text) in [("sectionEye:light", "Turn Section Off"), ("sectionSolo:light", "Turn Solo Mode On")] {
        req(&mut h, "ui.hoverWidget", json!({"id": "section:light"}));
        req(&mut h, "ui.hoverWidget", json!({"id": id}));
        let shown = h.step_until(Duration::from_secs(5), |h| tooltip_shown(h) && h.painted_text().iter().any(|t| t == text));
        let painted = h.painted_text();
        assert!(
            shown,
            "{id}: {text:?} not shown, last painted {:?}, pointer {:?}",
            &painted[painted.len().saturating_sub(5)..],
            h.view.ctx.input(|i| i.pointer.hover_pos())
        );
    }
}
