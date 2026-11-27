//! Fork (issue #460, `fork/color-swap.md`): the Color Swap section of the Edit panel. Each entry is a card: its name, its
//! on / off switch, move and remove buttons; the colour to replace and the colour it becomes (a swatch with the
//! colour picker, an eyedropper, a `#rrggbb` field each); Range and Intensity; and, folded under More, the range's hue,
//! saturation and lightness widths, Edge Softness and Keep Lightness. Every change goes through a `colorSwap.*` command
//! or `develop.set`, so an automation client does everything a person does here.
//!
//! Holding ⌥ over an entry's From colour or Range, or over the photo, draws what that entry reaches over the photo
//! (`Overlay::ColorSwapRange`); the Before button draws the photo without the entry (`Overlay::ColorSwapBefore`).

use egui::{Align, Color32, CornerRadius, Layout, Rect, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};
use lightcraft_develop::color_swap::{ColorSwap, MAX_SWAPS};
use lightcraft_develop::{DevelopSettings, controls};
use lightcraft_pipeline::recolor::{parse_hex, to_hex};
use serde_json::json;

use super::sliders::apply_slider_out;
use crate::LightcraftApp;
use crate::i18n::tr;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{icon_button, register, slider, text_button};

/// The on-canvas tool that adds an entry from a click on the photo (`colorSwap.pick` with no entry).
pub const ADD_TOOL: &str = "colorSwapAdd";

/// Which colour of an entry an eyedropper sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    From,
    To,
}

impl Side {
    fn key(self) -> &'static str {
        match self {
            Side::From => "from",
            Side::To => "to",
        }
    }
}

/// The on-canvas tool of entry `i`'s eyedropper (`colorSwapPick:<i>:from`).
pub fn pick_tool(i: usize, side: Side) -> String {
    format!("colorSwapPick:{i}:{}", side.key())
}

/// What an on-canvas tool name asks for: the entry (`None`: add one) and the side. `None` when it is not a Color Swap
/// eyedropper.
pub fn pick_target(tool: &str) -> Option<(Option<usize>, Side)> {
    if tool == ADD_TOOL {
        return Some((None, Side::From));
    }
    let mut it = tool.strip_prefix("colorSwapPick:")?.split(':');
    let i: usize = it.next()?.parse().ok()?;
    let side = match it.next()? {
        "from" => Side::From,
        "to" => Side::To,
        _ => return None,
    };
    Some((Some(i), side))
}

/// Width of a colour's swatch, and of its eyedropper.
const SWATCH: [f32; 2] = [46.0, 28.0];
const DROPPER: f32 = 28.0;

pub(super) fn color_swap(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    let t = Tokens::get(ui.ctx());
    let n = d.color_swaps.len();
    if app.ui.color_swap >= n && n > 0 {
        app.ui.color_swap = n - 1;
    }
    let full = n >= MAX_SWAPS;
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 8, bottom: 4 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if text_button(ui, "colorSwapAdd", "Add Color Swap", false).clicked()
                && !full
                && let Ok(r) = app.run("colorSwap.add", json!({}))
            {
                app.ui.color_swap = r["index"].as_u64().unwrap_or(0) as usize;
            }
            let active = app.ui.tool == ADD_TOOL;
            if icon_button(ui, "colorSwapAddPicker", Icon::Picker, vec2(26.0, 26.0), active, !full, "Click a color on the photo to add a swap for it")
                .clicked()
            {
                app.ui.tool = if active { String::new() } else { ADD_TOOL.into() };
            }
            if full {
                ui.label(
                    egui::RichText::new(crate::i18n::tr_format!("At most {n} swaps", n = MAX_SWAPS.to_string())).font(t.font(11.5)).color(t.text_dim),
                );
            }
        });
    });
    if n == 0 {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 2, bottom: 8 }).show(ui, |ui| {
            let r = ui.add(
                egui::Label::new(
                    egui::RichText::new(tr(
                        "Add a swap, then choose the color to replace and the color it becomes. Hold ⌥ over the photo to see what a swap reaches.",
                    ))
                    .font(t.font(12.0))
                    .color(t.text_dim),
                )
                .wrap(),
            );
            register(ui, "label:colorSwapEmpty", r.rect);
        });
        return;
    }
    for (i, e) in d.color_swaps.iter().enumerate() {
        card(app, ui, d, i, e);
    }
}

/// One entry's card.
fn card(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, i: usize, e: &ColorSwap) {
    let t = Tokens::get(ui.ctx());
    let n = d.color_swaps.len();
    let selected = app.ui.color_swap == i;
    let frame_id = format!("colorSwapCard:{i}");
    let inner = egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 4, bottom: 4 }).show(ui, |ui| {
        egui::Frame::NONE
            .fill(t.chrome.gamma_multiply(1.12))
            .corner_radius(CornerRadius::same(10))
            .stroke(Stroke::new(1.0, if selected { t.pick.gamma_multiply(0.8) } else { t.divider }))
            .inner_margin(egui::Margin { left: 12, right: 12, top: 10, bottom: 10 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // (each card repeats the same sliders: its own scope, so their widget ids differ)
                crate::widgets::in_repeated_row(ui, i, |ui| {
                    header(app, ui, i, e);
                    ui.add_space(8.0);
                    colours(app, ui, i, e);
                    ui.add_space(6.0);
                    sliders(app, ui, d, i);
                    more(app, ui, d, i, n, e);
                });
            })
    });
    let rect = inner.response.rect;
    register(ui, frame_id, rect);
    // a press anywhere on the card makes it the entry in use (the one ⌥ over the photo shows)
    if ui.rect_contains_pointer(rect) && ui.input(|inp| inp.pointer.any_pressed()) {
        app.ui.color_swap = i;
    }
}

/// The name (typed over to rename) and the on / off switch.
fn header(app: &mut LightcraftApp, ui: &mut egui::Ui, i: usize, e: &ColorSwap) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let draft = egui::Id::unique("lc-colorswap-name").with(i);
        let shown = e.display_name(i);
        let mut text = ui.data(|d| d.get_temp::<String>(draft)).unwrap_or_else(|| e.name.clone());
        let widget = format!("field:colorSwapName:{i}");
        let field = crate::text_field::TextField::singleline(&widget, &mut text)
            .width((ui.available_width() - 38.0 - 6.0).max(60.0))
            .hint(shown)
            .font(t.semibold(13.0))
            .select_on_focus(true)
            .show(ui);
        if field.ending.is_some() || !field.editing {
            ui.data_mut(|d| d.remove::<String>(draft));
            if field.committed() && text.trim() != e.name.trim() {
                let _ = app.run("colorSwap.set", json!({"index": i, "name": text.trim()}));
            }
        } else {
            ui.data_mut(|d| d.insert_temp(draft, text.clone()));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if switch(ui, &format!("colorSwapOn:{i}"), e.on, &crate::i18n::tr_format!("{name} on", name = e.display_name(i))).clicked() {
                let _ = app.run("colorSwap.set", json!({"index": i, "on": !e.on}));
            }
        });
    });
}

/// A switch, blue and to the right while on.
fn switch(ui: &mut egui::Ui, id: &str, on: bool, label: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, true, on, label));
    register(ui, format!("switch:{id}"), r);
    let t_on = ui.ctx().animate_bool_with_time(resp.id, on, 0.12);
    let track = crate::widgets::lerp(t.button, t.accent, t_on);
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(11), track);
    let x = r.left() + 11.0 + t_on * (r.width() - 22.0);
    p.circle_filled(pos2(x, r.center().y), 8.5, Color32::from_gray(if on { 245 } else { 215 }));
    if resp.hovered() {
        p.rect_stroke(r, CornerRadius::same(11), Stroke::new(1.0, t.text_dim), StrokeKind::Inside);
    }
    resp.on_hover_text(tr(if on { "Turn Swap Off" } else { "Turn Swap On" }))
}

/// The two colours: swatch and eyedropper, with a `#rrggbb` field under each swatch.
fn colours(app: &mut LightcraftApp, ui: &mut egui::Ui, i: usize, e: &ColorSwap) {
    let t = Tokens::get(ui.ctx());
    let group_w = SWATCH[0] + 4.0 + DROPPER;
    let width = ui.available_width();
    let h = SWATCH[1] + 4.0 + 24.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let left = Rect::from_min_size(rect.min, vec2(group_w, h));
    let right = Rect::from_min_size(pos2(rect.right() - group_w, rect.top()), vec2(group_w, h));
    ui.painter().text(pos2(rect.center().x, rect.top() + SWATCH[1] / 2.0), egui::Align2::CENTER_CENTER, tr("to"), t.font(13.0), t.text_dim);
    for (r, side, colour) in [(left, Side::From, e.from), (right, Side::To, e.to)] {
        ui.scope_builder(UiBuilder::new().max_rect(r).layout(Layout::top_down(Align::Min)), |ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                swatch(app, ui, i, side, colour);
                let tool = pick_tool(i, side);
                let active = app.ui.tool == tool;
                let tip = if side == Side::From { "Pick the Color to Replace from the Photo" } else { "Pick the New Color from the Photo" };
                if icon_button(ui, &format!("colorSwapPicker:{i}:{}", side.key()), Icon::Picker, vec2(DROPPER, DROPPER), active, true, tip).clicked()
                {
                    app.ui.tool = if active { String::new() } else { tool };
                }
            });
            hex_field(app, ui, i, side, colour);
        });
    }
    // ⌥ over the From colour shows what the entry reaches
    peek_over(app, ui, i, left);
}

/// The colour swatch with egui's colour picker; a drag in the picker is one undo step. An unset colour is a white
/// swatch with a red slash.
fn swatch(app: &mut LightcraftApp, ui: &mut egui::Ui, i: usize, side: Side, colour: Option<[f64; 3]>) {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(SWATCH[0], SWATCH[1]), Sense::click());
    let name = format!("{} {}", tr(if side == Side::From { "Color to Replace" } else { "New Color" }), i + 1);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ColorWell, true, &name));
    register(ui, format!("colorSwapSwatch:{i}:{}", side.key()), r);
    let rounding = CornerRadius::same(5);
    let p = ui.painter();
    match colour {
        Some(c) => {
            let [r8, g8, b8] = c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            p.rect_filled(r, rounding, Color32::from_rgb(r8, g8, b8));
        }
        None => {
            p.rect_filled(r, rounding, Color32::WHITE);
            p.line_segment([r.left_bottom() + vec2(3.0, -3.0), r.right_top() + vec2(-3.0, 3.0)], Stroke::new(1.5, Color32::from_rgb(220, 60, 50)));
        }
    }
    p.rect_stroke(r, rounding, Stroke::new(1.0, if resp.hovered() { t.text_dim } else { t.button_border }), StrokeKind::Inside);
    let resp = resp.on_hover_text(tr(if side == Side::From { "Color to Replace" } else { "New Color" }));
    let before = colour.map_or([255u8; 3], |c| c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
    let mut picked = Color32::from_rgb(before[0], before[1], before[2]);
    let mut changed = false;
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.spacing_mut().slider_width = 240.0;
        changed = egui::color_picker::color_picker_color32(ui, &mut picked, egui::color_picker::Alpha::Opaque);
    });
    let held = egui::Id::unique("lc-colorswap-picking").with((i, side.key()));
    let picking = ui.data(|d| d.get_temp::<bool>(held)).unwrap_or(false);
    if changed {
        if !picking {
            let _ = app.run("develop.beginInteraction", json!({"label": "Color Swap"}));
            ui.data_mut(|d| d.insert_temp(held, true));
        }
        let hex = to_hex([picked.r(), picked.g(), picked.b()].map(|v| f64::from(v) / 255.0));
        let _ = app.run("colorSwap.set", json!({"index": i, side.key(): hex}));
    } else if picking && !ui.input(|inp| inp.pointer.any_down()) {
        let _ = app.run("develop.endInteraction", json!({}));
        ui.data_mut(|d| d.remove::<bool>(held));
    }
}

/// The colour as `#rrggbb`, editable; Return or leaving the field sets it, a bad value is refused, an empty one clears it.
fn hex_field(app: &mut LightcraftApp, ui: &mut egui::Ui, i: usize, side: Side, colour: Option<[f64; 3]>) {
    let draft = egui::Id::unique("lc-colorswap-hex").with((i, side.key()));
    let current = colour.map(to_hex).unwrap_or_default();
    let mut text = ui.data(|d| d.get_temp::<String>(draft)).unwrap_or_else(|| current.clone());
    let widget = format!("field:colorSwapHex:{i}:{}", side.key());
    let t = Tokens::get(ui.ctx());
    let field = crate::text_field::TextField::singleline(&widget, &mut text)
        .width(SWATCH[0] + 4.0 + DROPPER)
        .hint("#rrggbb")
        .font(t.font(12.0))
        .select_on_focus(true)
        .show(ui);
    if field.ending.is_some() || !field.editing {
        ui.data_mut(|d| d.remove::<String>(draft));
        if field.committed() && text.trim() != current {
            if text.trim().is_empty() {
                let _ = app.run("colorSwap.set", json!({"index": i, side.key(): null}));
            } else if let Some(c) = parse_hex(&text) {
                let _ = app.run("colorSwap.set", json!({"index": i, side.key(): to_hex(c)}));
            } else {
                app.toast(ui.ctx(), tr("Enter a color as #rrggbb"));
            }
        }
    } else {
        ui.data_mut(|d| d.insert_temp(draft, text.clone()));
    }
}

/// Range and Intensity.
fn sliders(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, i: usize) {
    // ⌥ over Range, or dragging it, shows what the entry reaches
    let top = ui.cursor().top();
    control(app, ui, d, i, "range");
    let rect = Rect::from_min_max(pos2(ui.max_rect().left(), top), pos2(ui.max_rect().right(), ui.cursor().top()));
    peek_over(app, ui, i, rect);
    if range_dragging(ui, i) {
        peek(app, ui, i);
    }
    control(app, ui, d, i, "intensity");
}

/// An entry's slider `field`; Range's drag is noted for the ⌥ view.
fn control(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, i: usize, field: &str) {
    let id = format!("colorSwap.{i}.{field}");
    let Some(spec) = controls::find(&id) else { return };
    let v = controls::get(d, &id).unwrap_or(spec.default);
    let out = slider(ui, spec, v, true, None);
    if field == "range" {
        let key = egui::Id::unique("lc-colorswap-range-drag").with(i);
        if out.drag_started && !out.reset {
            ui.data_mut(|data| data.insert_temp(key, true));
        }
        if out.drag_stopped || out.reset {
            ui.data_mut(|data| data.remove::<bool>(key));
        }
    }
    apply_slider_out(app, spec, out, |app, v| app.run("develop.set", json!({"control": id, "value": v})));
}

fn range_dragging(ui: &egui::Ui, i: usize) -> bool {
    ui.data(|d| d.get_temp::<bool>(egui::Id::unique("lc-colorswap-range-drag").with(i))).unwrap_or(false)
}

/// ⌥ is held and the pointer is on `rect`: entry `i`'s range is asked for.
fn peek_over(app: &mut LightcraftApp, ui: &egui::Ui, i: usize, rect: Rect) {
    if ui.rect_contains_pointer(rect) {
        peek(app, ui, i);
    }
}

fn peek(app: &mut LightcraftApp, ui: &egui::Ui, i: usize) {
    if app.ui.alt_held {
        app.ui.color_swap_peek = Some((i, ui.ctx().cumulative_frame_nr()));
    }
}

/// The footer (More, Before, move earlier / later, remove) and the folded part: the range's three widths, Edge Softness
/// and Keep Lightness.
fn more(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, i: usize, n: usize, e: &ColorSwap) {
    let t = Tokens::get(ui.ctx());
    let key = format!("colorSwapMore:{i}");
    let open = app.ui.flyout_open(&key);
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (r, resp) = ui.allocate_exact_size(vec2(60.0, 24.0), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr("More")));
        register(ui, format!("button:{key}"), r);
        let p = ui.painter();
        let chevron = if open { Icon::ChevronDown } else { Icon::ChevronRight };
        paint(p, Rect::from_center_size(pos2(r.left() + 8.0, r.center().y), vec2(14.0, 14.0)), chevron, t.text_label);
        let colour = if resp.hovered() { t.text } else { t.text_label };
        p.text(pos2(r.left() + 20.0, r.center().y), egui::Align2::LEFT_CENTER, tr("More"), t.font(12.0), colour);
        if resp.clicked() {
            app.ui.toggle_flyout(&key);
        }
        // hold to see the photo without this swap
        let before = icon_button(
            ui,
            &format!("colorSwapBefore:{i}"),
            Icon::BeforeAfter,
            vec2(24.0, 24.0),
            false,
            true,
            "Hold to see the photo without this swap",
        );
        if before.is_pointer_button_down_on() {
            app.ui.color_swap_before = Some(i);
        } else if app.ui.color_swap_before == Some(i) {
            app.ui.color_swap_before = None;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let small = vec2(24.0, 24.0);
            if icon_button(ui, &format!("colorSwapRemove:{i}"), Icon::Close, small, false, true, "Remove Color Swap").clicked() {
                let _ = app.run("colorSwap.remove", json!({"index": i}));
            }
            if icon_button(ui, &format!("colorSwapDown:{i}"), Icon::ChevronDown, small, false, i + 1 < n, "Run Later").clicked() {
                let _ = app.run("colorSwap.move", json!({"index": i, "to": i + 1}));
            }
            if icon_button(ui, &format!("colorSwapUp:{i}"), Icon::ChevronUp, small, false, i > 0, "Run Earlier").clicked() {
                let _ = app.run("colorSwap.move", json!({"index": i, "to": i.saturating_sub(1)}));
            }
        });
    });
    if !open {
        return;
    }
    for field in ["hueRange", "satRange", "lumRange", "softness"] {
        let top = ui.cursor().top();
        control(app, ui, d, i, field);
        if field != "softness" {
            // (the width sliders change the range: ⌥ over them shows it too)
            let rect = Rect::from_min_max(pos2(ui.max_rect().left(), top), pos2(ui.max_rect().right(), ui.cursor().top()));
            peek_over(app, ui, i, rect);
        }
    }
    let mut keep = e.keep_lightness;
    if crate::dialogs::settings::check(ui, &format!("colorSwap.keepLightness.{i}"), &mut keep, "Keep Lightness") {
        let _ = app.run("colorSwap.set", json!({"index": i, "keepLightness": keep}));
    }
}

/// ⌥ over the photo: the entry in use is asked for (the photo's own hover; the panel sets it for its controls).
pub(crate) fn hover(app: &mut LightcraftApp, ui: &egui::Ui, resp: &egui::Response) {
    if app.ui.right == crate::state::RightPanel::Edit && app.ui.alt_held && resp.hovered() {
        app.ui.color_swap_peek = Some((app.ui.color_swap, ui.ctx().cumulative_frame_nr()));
    }
}

/// The overlay Color Swap asks the loupe for: the Before button's, or the range of the entry ⌥ is over this frame (or
/// the last: the panel and the photo are drawn in either order). Only an entry with a From colour has a range.
pub(crate) fn overlay(app: &LightcraftApp, d: &DevelopSettings, frame: u64) -> Option<lightcraft_pipeline::Overlay> {
    use lightcraft_pipeline::Overlay;
    if d.color_swaps.is_empty() || !d.section_enabled(lightcraft_develop::color_swap::SECTION) {
        return None;
    }
    if let Some(i) = app.ui.color_swap_before.filter(|i| *i < d.color_swaps.len()) {
        return Some(Overlay::ColorSwapBefore(i as u8));
    }
    let (i, at) = app.ui.color_swap_peek?;
    let fresh = app.ui.alt_held && frame.saturating_sub(at) <= 1;
    (fresh && d.color_swaps.get(i).is_some_and(|e| e.from.is_some())).then_some(Overlay::ColorSwapRange(i as u8))
}
