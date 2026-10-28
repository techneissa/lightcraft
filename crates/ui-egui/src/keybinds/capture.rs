//! The Assign Shortcut box: "Press the keys…", Enter saves, Esc cancels, Remove takes the shortcut
//! away. Press one key for a single shortcut or two for a chord; a third press starts over. It
//! opens from a click in the Keyboard Shortcuts window, from a right-click on a button or menu item
//! ([`super::context`]) or from `app.assignShortcut`.
//!
//! While it is open [`LightcraftApp::recording_shortcut`] names the command, so no shortcut fires
//! and the native menu drops its accelerators: every key comes here. Keys are read at the start of
//! the frame ([`keys`], from `shortcuts::handle`) before a dialog under the box sees Esc.

use egui::{Align, Align2, Key, Modifiers, RichText};

use crate::LightcraftApp;
use crate::shortcuts::{self, find_bindable};
use crate::theme::Tokens;
use crate::widgets::register;

use super::chord::{self, Step};
use super::model;

/// The open box.
#[derive(Clone, Debug)]
pub struct Capture {
    pub id: String,
    /// 0 = Shortcut 1 (primary), 1 = Shortcut 2.
    pub slot: usize,
    /// The keys pressed so far.
    pub steps: Vec<Step>,
    /// Why the last key was refused.
    pub error: Option<String>,
}

impl Capture {
    /// The pressed keys as shortcut text (`Cmd+K C`), if any.
    pub fn text(&self) -> Option<String> {
        chord::spell(&self.steps)
    }
}

/// Open the box for `id`. `slot` `None` picks the first free one (Shortcut 1 when both are taken).
pub fn open(app: &mut LightcraftApp, id: &str, slot: Option<usize>) -> Result<(), String> {
    find_bindable(id).ok_or_else(|| format!("unknown command: {id}"))?;
    let slot = slot.unwrap_or_else(|| (0..model::SLOTS).find(|s| model::effective(&app.ui.settings.keymap, id, *s).is_none()).unwrap_or(0));
    if slot >= model::SLOTS {
        return Err(format!("slot must be 1 or {}", model::SLOTS));
    }
    app.keybinds.pending = None;
    app.keybinds.capture = Some(Capture { id: id.to_string(), slot, steps: Vec::new(), error: None });
    app.recording_shortcut = Some(id.to_string());
    Ok(())
}

/// Close the box without saving.
pub fn close(app: &mut LightcraftApp) {
    app.keybinds.capture = None;
    app.recording_shortcut = None;
}

/// Every key press of this frame, in order, as the box sees them. ⌘, ⇧, ⌥ and ⌃ on their own arrive
/// as keys too and are skipped: the box waits for the key they modify.
fn pressed(ctx: &egui::Context) -> Vec<(Modifiers, Key)> {
    ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } if !shortcuts::is_modifier(*key) => Some((*modifiers, *key)),
                // the windowing layer turns ⌘C / ⌘X / ⌘V (with or without other modifiers) into these
                egui::Event::Copy => Some((i.modifiers, Key::C)),
                egui::Event::Cut => Some((i.modifiers, Key::X)),
                egui::Event::Paste(_) => Some((i.modifiers, Key::V)),
                _ => None,
            })
            .collect()
    })
}

/// Read this frame's keys for the open box. Returns whether the box is open (the key pass stops).
pub fn keys(app: &mut LightcraftApp, ctx: &egui::Context) -> bool {
    let Some(mut cap) = app.keybinds.capture.take() else { return false };
    let presses = pressed(ctx);
    if !presses.is_empty() {
        // the key press is ours: nothing else (a focused button's Space, Tab navigation) acts on it
        super::runtime::consume_keys(ctx);
    }
    let (mut save, mut cancel) = (false, false);
    for (m, k) in presses {
        if k == Key::Escape && m.is_none() {
            cancel = true;
            break;
        }
        if k == Key::Enter && m.is_none() && !cap.steps.is_empty() {
            save = true;
            break;
        }
        let step = shortcuts::format(m, k).and_then(|s| shortcuts::parse(&s));
        match step {
            Some(step) => {
                if cap.steps.len() >= chord::MAX_STEPS {
                    cap.steps.clear();
                }
                cap.steps.push(step);
                cap.error = None;
            }
            None => cap.error = Some(crate::i18n::tr("That key can't be used as a shortcut").to_string()),
        }
    }
    if cancel {
        app.recording_shortcut = None;
        return true;
    }
    app.keybinds.capture = Some(cap);
    if save {
        save_open(app, ctx);
    }
    true
}

/// Save the pressed keys into the open box's slot and close it.
pub fn save_open(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(cap) = app.keybinds.capture.clone() else { return };
    let Some(text) = cap.text() else { return };
    if apply(app, ctx, &cap.id, cap.slot, Some(&text)) {
        close(app);
    } else if let Some(c) = app.keybinds.capture.as_mut() {
        c.steps.clear();
    }
}

/// Run `app.setShortcut` and say what changed (a key taken from another command, or why not).
/// Returns whether it was saved.
pub fn apply(app: &mut LightcraftApp, ctx: &egui::Context, id: &str, slot: usize, shortcut: Option<&str>) -> bool {
    let params = serde_json::json!({"id": id, "slot": slot + 1, "shortcut": shortcut});
    match app.run("app.setShortcut", params) {
        Ok(r) => {
            let lost: Vec<String> = r["removedFrom"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .map(|id| find_bindable(id).map_or(id, |b| crate::i18n::tr(b.label)).to_string())
                .collect();
            if !lost.is_empty() {
                app.toast_for(ctx, crate::i18n::tr_format!("Removed from {}", lost.join(", ")), 3.0);
            }
            true
        }
        Err(e) => {
            app.toast_for(ctx, e, 3.0);
            false
        }
    }
}

/// Draw the box (and open it for a right-click's request).
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    if let Some(id) = super::context::take_request(ctx)
        && let Err(e) = open(app, &id, None)
    {
        app.toast(ctx, e);
    }
    let Some(mut cap) = app.keybinds.capture.clone() else { return };
    // another window owns the screen (a quit prompt, the library problem list): the box waits
    if crate::panels::notices::blocks_input(app) || crate::panels::library_problem::blocks_dialog_input(app) {
        return;
    }
    let Some(command) = find_bindable(&cap.id) else {
        close(app);
        return;
    };
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let t = Tokens::get(ctx);
    let keymap = app.ui.settings.keymap.clone();
    let current = model::effective(&keymap, &cap.id, cap.slot).map(str::to_string);
    let conflicts: Vec<_> =
        cap.text().and_then(|s| chord::parse_chord(&s)).map(|steps| model::conflicts(&keymap, &cap.id, &steps)).unwrap_or_default();

    let (mut do_save, mut do_remove, mut do_cancel) = (false, false, false);
    let width = 440.0_f32.min(ctx.content_rect().width() * 0.92);
    let frame = egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::symmetric(18, 16));
    let area = egui::Modal::default_area(egui::Id::unique("lightcraft-assign-shortcut")).anchor(Align2::CENTER_CENTER, [0.0, -40.0]);
    let modal = egui::Modal::new(egui::Id::unique("lightcraft-assign-shortcut"))
        .area(area)
        .backdrop_color(egui::Color32::from_black_alpha(120))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new(crate::i18n::tr("Assign Shortcut")).font(t.semibold(15.0)).color(t.text));
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::i18n::tr(command.label)).font(t.semibold(13.0)).color(t.text));
                ui.label(RichText::new(command.id).size(11.0).color(t.text_dim));
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for slot in 0..model::SLOTS {
                    let now = model::effective(&keymap, &cap.id, slot).map(|sc| chord::text(sc, mac));
                    let title = crate::i18n::tr_format!("Shortcut {}", slot + 1);
                    let text = match now {
                        Some(sc) => format!("{title}: {sc}"),
                        None => format!("{title}: —"),
                    };
                    let r = ui.selectable_label(cap.slot == slot, text);
                    register(ui, format!("button:assignShortcut.slot{}", slot + 1), r.rect);
                    if r.clicked() {
                        cap.slot = slot;
                    }
                }
            });
            ui.add_space(2.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 52.0), egui::Sense::hover());
            register(ui, "assignShortcut:keys", rect);
            ui.painter().rect(
                rect,
                6.0,
                t.field,
                egui::Stroke::new(1.5, if cap.steps.is_empty() { t.field_border } else { t.accent }),
                egui::StrokeKind::Inside,
            );
            keycaps(ui, rect, &t, &cap, mac);
            if let Some(e) = &cap.error {
                ui.label(RichText::new(e).size(12.0).color(t.caution));
            }
            for (other, slot) in &conflicts {
                let name = find_bindable(other).map_or(*other, |b| crate::i18n::tr(b.label));
                ui.label(
                    RichText::new(crate::i18n::tr_format!("Also used by {} (Shortcut {}); saving takes it away from there.", name, slot + 1))
                        .size(12.0)
                        .color(t.caution),
                );
            }
            ui.label(
                RichText::new(crate::i18n::tr("Press the keys, then Enter to save. Press two keys one after the other for a chord. Esc cancels."))
                    .size(11.0)
                    .color(t.text_dim),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let r = ui.add_enabled(current.is_some(), egui::Button::new(crate::i18n::tr("Remove")));
                register(ui, "button:assignShortcut.remove", r.rect);
                do_remove = r.clicked();
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    let r = ui.add_enabled(
                        cap.text().is_some(),
                        egui::Button::new(RichText::new(crate::i18n::tr("Save")).color(egui::Color32::WHITE)).fill(t.accent),
                    );
                    register(ui, "button:assignShortcut.save", r.rect);
                    do_save = r.clicked();
                    let r = ui.button(crate::i18n::tr("Cancel"));
                    register(ui, "button:assignShortcut.cancel", r.rect);
                    do_cancel = r.clicked();
                });
            });
        });
    if modal.backdrop_response.clicked() {
        do_cancel = true;
    }
    // the slot picked here, and keys pressed while the box was drawn, persist for the next frame
    if let Some(open) = app.keybinds.capture.as_mut() {
        open.slot = cap.slot;
    }
    if do_remove {
        apply(app, ctx, &cap.id, cap.slot, None);
        close(app);
    } else if do_save {
        save_open(app, ctx);
    } else if do_cancel {
        close(app);
    }
}

/// The pressed keys as keycaps in the middle of `rect`, or the prompt.
fn keycaps(ui: &egui::Ui, rect: egui::Rect, t: &Tokens, cap: &Capture, mac: bool) {
    let p = ui.painter();
    if cap.steps.is_empty() {
        p.text(rect.center(), Align2::CENTER_CENTER, crate::i18n::tr("Press the keys…"), t.font(14.0), t.text_dim);
        return;
    }
    let caps: Vec<String> = cap.steps.iter().filter_map(|(m, k)| shortcuts::format(*m, *k)).map(|s| crate::menubar::shortcut_text(&s, mac)).collect();
    let font = t.semibold(17.0);
    let gap = 10.0;
    let galleys: Vec<_> = caps.iter().map(|c| p.layout_no_wrap(c.clone(), font.clone(), t.text)).collect();
    let total: f32 = galleys.iter().map(|g| g.size().x + 20.0).sum::<f32>() + gap * galleys.len().saturating_sub(1) as f32;
    let mut x = rect.center().x - total / 2.0;
    for g in galleys {
        let w = g.size().x + 20.0;
        let r = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - 14.0), egui::vec2(w, 28.0));
        p.rect(r, 5.0, t.button, egui::Stroke::new(1.0, t.button_border), egui::StrokeKind::Inside);
        p.galley(egui::pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0), g, t.text);
        x += w + gap;
    }
}
