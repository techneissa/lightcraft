//! The Settings tab of the right column: Auto Layout and the page setup.
use egui::{Sense, Stroke, StrokeKind, Ui, vec2};
use lightcraft_engine::book::model::{Book, Template, size_names, size_of};
use lightcraft_engine::print::layout::Orientation;
use serde_json::json;

use super::{BookUi, run};
use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Auto Layout's choices: the preset's name for `book.layout` and its label.
pub(super) const LAYOUTS: [(&str, &str); 6] =
    [("mixed", "Mixed"), ("one", "1 Photo"), ("two", "2 Photos"), ("three", "3 Photos"), ("grid4", "4 Grid"), ("fullBleed", "Full Bleed")];

pub(super) fn template_label(t: Template) -> &'static str {
    match t {
        Template::One => "1 Photo",
        Template::Two => "2 Photos",
        Template::Three => "3 Photos",
        Template::Grid4 => "4 Grid",
        Template::FullBleed => "Full Bleed",
    }
}

fn size_label(name: &str) -> String {
    match name {
        "a4" | "a5" => name.to_uppercase(),
        "letter" => "Letter".into(),
        other => other.replace('x', " × ") + " in",
    }
}

fn heading(ui: &mut Ui, t: &Tokens, text: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(crate::i18n::tr(text)).font(t.semibold(12.0)).color(t.text));
    ui.add_space(2.0);
}

fn row<R>(ui: &mut Ui, t: &Tokens, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(86.0, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(crate::i18n::tr(label)).color(t.text_label));
        });
        add(ui)
    })
    .inner
}

fn inches(ui: &mut Ui, id: &str, value: f64, max: f64) -> Option<f64> {
    let mut v = value;
    let r = ui.add(egui::DragValue::new(&mut v).range(0.0..=max).speed(0.01).max_decimals(2).suffix(" in"));
    register(ui, format!("drag:{id}"), r.rect);
    r.changed().then_some(v)
}

fn check(ui: &mut Ui, id: &str, label: &str, value: bool) -> Option<bool> {
    let mut v = value;
    let r = ui.checkbox(&mut v, crate::i18n::tr(label));
    register(ui, format!("check:{id}"), r.rect);
    r.changed().then_some(v)
}

/// A row of buttons of which one is on, each `button:{id}-{index}`.
fn choice<V: PartialEq + Copy>(ui: &mut Ui, id: &str, options: &[(V, &str)], current: V) -> Option<V> {
    let t = Tokens::get(ui.ctx());
    let mut out = None;
    ui.spacing_mut().item_spacing.x = 3.0;
    for (i, (v, l)) in options.iter().enumerate() {
        let on = current == *v;
        let label = crate::i18n::tr(l);
        let (r, resp) = ui.allocate_exact_size(vec2(crate::widgets::text_button_size(ui, l), 24.0), Sense::click());
        crate::widgets::announce_selected(&resp, label.to_string(), on);
        register(ui, format!("button:{id}-{i}"), r);
        let fill = if on {
            t.pressed
        } else if resp.hovered() {
            t.hover
        } else {
            t.button
        };
        let stroke = if on { Stroke::new(1.5, t.accent) } else { Stroke::new(1.0, t.button_border) };
        let p = ui.painter();
        p.rect(r, egui::CornerRadius::same(4), fill, stroke, StrokeKind::Inside);
        p.text(r.center(), egui::Align2::CENTER_CENTER, label, t.semibold(11.5), if on { t.accent } else { t.text });
        if resp.clicked() && !on {
            out = Some(*v);
        }
    }
    out
}

pub(super) fn show(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book) {
    let ctx = ui.ctx().clone();
    ui.spacing_mut().item_spacing.y = 5.0;
    let s = &book.settings;
    egui::ScrollArea::vertical().id_salt("book-settings").auto_shrink([false, false]).show(ui, |ui| {
        heading(ui, t, "Auto Layout");
        row(ui, t, "Layout", |ui| {
            let (c, chosen) = crate::widgets::dropdown_search::pick_with(
                ui,
                "bookLayout",
                |c| c.selected_text(crate::i18n::tr(LAYOUTS[bu.layout.min(LAYOUTS.len() - 1)].1)),
                &LAYOUTS,
                |l| LAYOUTS.get(bu.layout).is_some_and(|cur| cur.0 == l.0),
                |l| crate::i18n::tr(l.1).to_string(),
                |ui, l, row| register(ui, format!("menu:bookLayout-{}", l.0), row.rect),
            );
            crate::widgets::name_combo(&c, crate::i18n::tr("Layout"));
            register(ui, "combo:bookLayout", c.rect);
            if let Some(i) = chosen {
                bu.layout = i;
            }
        });
        ui.horizontal(|ui| {
            if let Some(v) = check(ui, "bookCover", "Cover", book.cover) {
                run(app, &ctx, bu, "book.setCover", json!({"cover": v}));
                // (the cover is page 0: the other pages move by one)
                bu.page = if v { bu.page + 1 } else { bu.page.saturating_sub(1) };
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if text_button(ui, "bookAutoLayout", "Auto Layout", false).on_hover_text(crate::i18n::tr("Apply Layout to Book")).clicked() {
                    let preset = LAYOUTS[bu.layout.min(LAYOUTS.len() - 1)].0;
                    run(app, &ctx, bu, "book.layout", json!({"cover": book.cover, "preset": preset}));
                    bu.page = 0;
                }
            });
        });
        ui.label(egui::RichText::new(crate::i18n::tr("Locked pages stay as they are.")).font(t.font(11.0)).color(t.text_dim));

        heading(ui, t, "Page Setup");
        row(ui, t, "Size", |ui| {
            let c = crate::widgets::combo_box("bookSize").selected_text(size_label(&s.size)).show_ui(ui, |ui| {
                for name in size_names() {
                    if ui.selectable_label(s.size == name, size_label(name)).clicked() {
                        run(app, &ctx, bu, "book.set", json!({"size": name}));
                    }
                }
            });
            crate::widgets::name_combo(&c.response, crate::i18n::tr("Size"));
            register(ui, "combo:bookSize", c.response.rect);
        });
        let square = size_of(&s.size).is_some_and(|(w, h)| w == h);
        ui.add_enabled_ui(!square, |ui| {
            row(ui, t, "Orientation", |ui| {
                let o = [(Orientation::Portrait, "Portrait"), (Orientation::Landscape, "Landscape")];
                if let Some(v) = choice(ui, "bookOrientation", &o, s.orientation) {
                    run(app, &ctx, bu, "book.set", json!({"orientation": v}));
                }
            });
        });
        row(ui, t, "Margin", |ui| {
            if let Some(v) = inches(ui, "bookMargin", s.margin, 2.0) {
                run(app, &ctx, bu, "book.set", json!({"margin": v}));
            }
        });
        row(ui, t, "Cell Padding", |ui| {
            if let Some(v) = inches(ui, "bookPadding", s.padding, 1.0) {
                run(app, &ctx, bu, "book.set", json!({"padding": v}));
            }
        });
        row(ui, t, "Background", |ui| {
            let mut c = s.background;
            let r = ui.color_edit_button_srgb(&mut c);
            register(ui, "color:bookBackground", r.rect);
            if c != s.background {
                run(app, &ctx, bu, "book.set", json!({"background": c}));
            }
        });
        if let Some(v) = check(ui, "bookFill", "Zoom to Fill Frame", s.fill) {
            run(app, &ctx, bu, "book.set", json!({"fill": v}));
        }
        row(ui, t, "Resolution", |ui| {
            let mut ppi = s.ppi;
            let r = ui.add(egui::DragValue::new(&mut ppi).range(72.0..=720.0).speed(1.0).max_decimals(0).suffix(" ppi"));
            register(ui, "drag:bookPpi", r.rect);
            if r.changed() {
                run(app, &ctx, bu, "book.set", json!({"ppi": ppi}));
            }
        });
    });
}
