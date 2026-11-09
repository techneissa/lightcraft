//! The people picker's selection API, with a dummy parts list.

use egui::{Event, Modifiers, PointerButton, RawInput, Rect, pos2, vec2};

use super::{FaceChip, PartItem, Picked, Picker, select, show};

#[test]
fn a_click_selects_one_and_a_command_click_adds_or_removes() {
    assert_eq!(select(&[], 2, false), [2]);
    assert_eq!(select(&[0, 3], 2, false), [2], "a plain click selects that item alone");
    assert_eq!(select(&[0, 3], 2, true), [0, 2, 3], "⌘-click adds it");
    assert_eq!(select(&[0, 2, 3], 2, true), [0, 3], "⌘-click on a selected item removes it");
    assert_eq!(select(&[2], 2, false), [2], "a plain click on the only one keeps it");
    assert_eq!(select(&[3, 3, 1], 1, true), [3], "repeats and order in are not kept");
    assert!(select(&[4], 4, true).is_empty());
}

fn parts() -> Vec<PartItem> {
    ["Entire Person", "Entire Face", "Facial Skin", "Hair", "Background"]
        .iter()
        .map(|l| PartItem { key: l.to_lowercase().replace(' ', "-"), label: l.to_string(), group: None })
        .collect()
}

/// One frame of the picker over an 800 × 600 view; `events` and `modifiers` are this frame's input.
fn frame(ctx: &egui::Context, pk: &Picker<'_>, events: Vec<Event>, modifiers: Modifiers) -> Picked {
    let events = std::iter::once(Event::ModifiersChanged(modifiers)).chain(events).collect();
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))), events, ..Default::default() };
    let mut picked = Picked::default();
    let mut out = ctx.run_ui(raw, |ui| {
        picked = show(ui, Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)), pk);
    });
    out.textures_delta.clear();
    picked
}

fn rect_of(ctx: &egui::Context, id: &str) -> Option<Rect> {
    crate::widgets::take_registry(ctx).into_iter().find(|(k, _)| k == id).map(|(_, r)| r)
}

/// A click at `at` over two frames (press, release), with `modifiers` held.
fn click(ctx: &egui::Context, pk: &Picker<'_>, at: egui::Pos2, modifiers: Modifiers) -> Picked {
    let press = Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers };
    let release = Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers };
    let a = frame(ctx, pk, vec![Event::PointerMoved(at), press], modifiers);
    let b = frame(ctx, pk, vec![release], modifiers);
    if b == Picked::default() { a } else { b }
}

#[test]
fn the_parts_row_takes_several_with_command_click() {
    let ctx = egui::Context::default();
    let parts = parts();
    let faces = [FaceChip { texture: None, name: "Face 1".into() }];
    let selected = [0usize];
    let pk = Picker { id: "test", faces: &faces, face: None, parts: &parts, selected_parts: &selected, folded: false, note: None };
    frame(&ctx, &pk, Vec::new(), Modifiers::NONE);
    let _ = frame(&ctx, &pk, Vec::new(), Modifiers::NONE);
    assert!(rect_of(&ctx, "test:part:hair").is_none(), "the parts menu starts closed");
    let _ = frame(&ctx, &pk, Vec::new(), Modifiers::NONE);
    let button = rect_of(&ctx, "test:parts").expect("the parts dropdown");
    let _ = click(&ctx, &pk, button.center(), Modifiers::NONE);
    // (a new menu is laid out unseen first)
    let _ = frame(&ctx, &pk, Vec::new(), Modifiers::NONE);
    let _ = frame(&ctx, &pk, Vec::new(), Modifiers::NONE);
    let _ = crate::widgets::take_registry(&ctx);
    let _ = frame(&ctx, &pk, Vec::new(), Modifiers::NONE);
    let hair = rect_of(&ctx, "test:part:hair").expect("the open menu lists the parts");
    let _ = frame(&ctx, &pk, Vec::new(), Modifiers::NONE);
    let picked = click(&ctx, &pk, hair.center(), Modifiers::COMMAND);
    assert_eq!(picked.parts, Some(vec![0, 3]), "⌘-click adds Hair to Entire Person");
    let picked = click(&ctx, &pk, hair.center(), Modifiers::NONE);
    assert_eq!(picked.parts, Some(vec![3]), "a plain click selects Hair alone");
    assert!(!super::parts_menu::is_open(&ctx, &pk), "and closes the menu");
    // the face row is single-select
    let _ = frame(&ctx, &pk, Vec::new(), Modifiers::NONE);
    let face = rect_of(&ctx, "test:face:0").unwrap();
    let picked = click(&ctx, &pk, face.center(), Modifiers::COMMAND);
    assert_eq!((picked.face, picked.parts), (Some(Some(0)), None));
}

fn key(k: egui::Key) -> Event {
    Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
}

/// The open menu takes ↓ / ↑ and ↩; Esc closes it.
#[test]
fn the_parts_menu_takes_the_arrow_keys_and_return() {
    let ctx = egui::Context::default();
    let parts = parts();
    let selected = [0usize];
    let pk = Picker { id: "test", faces: &[], face: None, parts: &parts, selected_parts: &selected, folded: false, note: None };
    frame(&ctx, &pk, Vec::new(), Modifiers::NONE);
    let button = rect_of(&ctx, "test:parts").expect("the parts dropdown");
    let _ = click(&ctx, &pk, button.center(), Modifiers::NONE);
    let _ = frame(&ctx, &pk, vec![key(egui::Key::ArrowDown)], Modifiers::NONE);
    let _ = frame(&ctx, &pk, vec![key(egui::Key::ArrowDown)], Modifiers::NONE);
    let _ = frame(&ctx, &pk, vec![key(egui::Key::ArrowDown)], Modifiers::NONE);
    let _ = frame(&ctx, &pk, vec![key(egui::Key::ArrowUp)], Modifiers::NONE);
    let picked = frame(&ctx, &pk, vec![key(egui::Key::Enter)], Modifiers::NONE);
    assert_eq!(picked.parts, Some(vec![1]), "↓↓↓↑ ↩ picks the second part");
    let _ = frame(&ctx, &pk, Vec::new(), Modifiers::NONE);
    assert!(!super::parts_menu::is_open(&ctx, &pk), "↩ closes it");
    let _ = click(&ctx, &pk, button.center(), Modifiers::NONE);
    assert!(super::parts_menu::is_open(&ctx, &pk));
    let _ = frame(&ctx, &pk, vec![key(egui::Key::Escape)], Modifiers::NONE);
    assert!(!super::parts_menu::is_open(&ctx, &pk), "Esc closes it");
}

#[test]
fn without_parts_there_is_no_parts_row() {
    let ctx = egui::Context::default();
    let pk = Picker { id: "test", faces: &[], face: None, parts: &[], selected_parts: &[], folded: false, note: None };
    frame(&ctx, &pk, Vec::new(), Modifiers::NONE);
    let _ = frame(&ctx, &pk, Vec::new(), Modifiers::NONE);
    let ids: Vec<String> = crate::widgets::take_registry(&ctx).into_iter().map(|(k, _)| k).collect();
    assert!(ids.iter().any(|k| k == "test:face:all") && !ids.iter().any(|k| k.starts_with("test:part")), "{ids:?}");
}

/// The camera-preview badge (top-right of the photo) is drawn first; when it is at the view's corner the card goes
/// below it.
#[test]
fn the_card_moves_below_the_camera_preview_badge() {
    let ctx = egui::Context::default();
    let pk = Picker { id: "test", faces: &[], face: None, parts: &[], selected_parts: &[], folded: false, note: None };
    let view = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
    let badge = Rect::from_min_size(pos2(680.0, 8.0), vec2(112.0, 18.0));
    for with_badge in [false, true] {
        let raw = RawInput { screen_rect: Some(view), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            if with_badge {
                crate::widgets::register(ui, "loupe:cameraPreview", badge);
            }
            let _ = show(ui, view, &pk);
        });
        out.textures_delta.clear();
        let card = rect_of(&ctx, "test:card").unwrap();
        assert_eq!(card.right(), 800.0 - super::MARGIN);
        if with_badge {
            assert!(card.top() > badge.bottom(), "{card:?} below {badge:?}");
        } else {
            assert_eq!(card.top(), super::MARGIN);
        }
    }
}
