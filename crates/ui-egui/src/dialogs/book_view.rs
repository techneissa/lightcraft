//! The Book dialog (File ▸ Book…): settings on the left, the pages on the right, as an overview of
//! every page or one open spread.
//!
//! Every change goes through a `book.*` command ([`lightcraft_engine::cmd`]); this file draws the
//! session's books ([`lightcraft_engine::Session::books`]) and turns input into commands: a template
//! button is `book.setTemplate`, dragging a photo onto a cell (or onto the tray of photos that are
//! on no page) is `book.movePhoto`. Widgets register under `button:book*`, `book:*` ids for the
//! control channel and the headless tests. The export is the dialog's OK button (`app.bookPdf`) and
//! Export JPEGs… (`app.bookJpeg`).
use egui::{Color32, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_engine::book::geometry::cells_for;
use lightcraft_engine::book::model::{Book, Slot, Template, size_names, size_of};
use lightcraft_engine::export::ExportOptions;
use lightcraft_engine::print::compose::place;
use lightcraft_engine::print::layout::Orientation;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Width of the settings column.
const SETTINGS_W: f32 = 290.0;
/// Height of the tray of photos that are on no page.
const TRAY_H: f32 = 64.0;
/// Pages per row in the overview.
const PER_ROW: usize = 4;

/// Where Auto Layout takes the photos from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    Selected,
    #[default]
    InView,
}

/// Auto Layout's choices: the preset's name for `book.layout` and its label.
const LAYOUTS: [(&str, &str); 6] =
    [("mixed", "Mixed"), ("one", "1 Photo"), ("two", "2 Photos"), ("three", "3 Photos"), ("grid4", "4 Grid"), ("fullBleed", "Full Bleed")];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// Every page, small.
    #[default]
    Pages,
    /// The spread of the page on show, large.
    Spread,
}

/// What the Book dialog remembers between frames (not saved).
#[derive(Clone, Debug, Default)]
pub struct BookUi {
    /// The page on show (selected), zero-based.
    pub page: usize,
    pub view: View,
    /// Auto Layout's template choice (an index into [`LAYOUTS`]).
    pub layout: usize,
    /// Where Auto Layout takes its photos from.
    pub source: Source,
    /// The name field, for the book `name_of`.
    name: String,
    name_of: Option<u32>,
    /// The photo being dragged: where it came from.
    drag: Option<Slot>,
    /// The width the top bar's group had last frame, to centre it this frame.
    group_w: Option<f32>,
    /// Where a dragged photo may land, as drawn this frame.
    hits: Vec<(Slot, Rect)>,
}

fn run(app: &mut LightcraftApp, ctx: &egui::Context, id: &str, params: Value) -> Option<Value> {
    match app.run(id, params) {
        Ok(v) => Some(v),
        Err(e) => {
            app.toast(ctx, e);
            None
        }
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

/// A row of buttons of which one is on. The one on has an accent outline and accent text on the
/// lighter fill, so it reads as selected at a glance; each is `button:{id}-{index}`.
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

fn size_label(name: &str) -> String {
    match name {
        "a4" | "a5" => name.to_uppercase(),
        "letter" => "Letter".into(),
        other => other.replace('x', " × ") + " in",
    }
}

fn template_label(t: Template) -> &'static str {
    match t {
        Template::One => "1 Photo",
        Template::Two => "2 Photos",
        Template::Three => "3 Photos",
        Template::Grid4 => "4 Grid",
        Template::FullBleed => "Full Bleed",
    }
}

/// The dialog's body: the settings column and the pages side by side.
pub fn body(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens) {
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    let width = (screen.width() - 80.0).clamp(640.0, 1180.0);
    let height = (screen.height() - 190.0).clamp(280.0, 780.0);
    let mut bu = std::mem::take(&mut app.ui.book);
    let book = app.session.books.find(None).ok().cloned();
    if let Some(b) = &book {
        bu.page = bu.page.min(b.pages.len().saturating_sub(1));
        if bu.name_of != Some(b.id) {
            bu.name_of = Some(b.id);
            bu.name = b.name.clone();
        }
    }
    bu.hits.clear();
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(vec2(SETTINGS_W, height), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_size(vec2(SETTINGS_W, height));
            egui::ScrollArea::vertical().id_salt("book-settings").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(SETTINGS_W - 14.0);
                settings(app, ui, t, &mut bu, book.as_ref());
            });
        });
        // (a `separator` would take the dialog's whole free height)
        let (line, _) = ui.allocate_exact_size(vec2(1.0, height), Sense::hover());
        ui.painter().line_segment([line.center_top(), line.center_bottom()], Stroke::new(1.0, t.divider));
        let w = (width - SETTINGS_W - 24.0).max(300.0);
        ui.allocate_ui_with_layout(vec2(w, height), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_size(vec2(w, height));
            match &book {
                Some(b) => preview(app, ui, t, &mut bu, b),
                None => {
                    ui.add_space(height / 3.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(crate::i18n::tr("Create a book from the selected photos or the photos in view.")).color(t.text_dim),
                        );
                    });
                }
            }
        });
    });
    drop_photo(app, &ctx, &mut bu, book.is_some());
    app.ui.book = bu;
}

fn settings(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: Option<&Book>) {
    let ctx = ui.ctx().clone();
    ui.spacing_mut().item_spacing.y = 5.0;
    let selected = app.session.selected().len();
    let in_view = app.session.visible().len();

    heading(ui, t, "Book");
    let names: Vec<(u32, String)> = app.session.books.books.iter().map(|b| (b.id, b.name.clone())).collect();
    if let Some(b) = book {
        let c = crate::widgets::combo_box("bookPicker").selected_text(b.name.clone()).show_ui(ui, |ui| {
            for (id, name) in &names {
                if ui.selectable_label(*id == b.id, name).clicked() {
                    run(app, &ctx, "book.open", json!({"book": id}));
                    bu.page = 0;
                }
            }
        });
        crate::widgets::name_combo(&c.response, crate::i18n::tr("Book"));
        register(ui, "combo:bookPicker", c.response.rect);
        let r = crate::text_field::TextField::singleline("field:bookName", &mut bu.name).width(SETTINGS_W - 30.0).show(ui);
        if r.committed() && !bu.name.trim().is_empty() && bu.name.trim() != b.name {
            run(app, &ctx, "book.set", json!({"name": bu.name}));
        } else if r.cancelled() {
            bu.name = b.name.clone();
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
        ui.add_enabled_ui(selected > 0, |ui| {
            if text_button(ui, "bookNewSelected", "New from Selected", false).clicked() {
                run(app, &ctx, "book.create", json!({"source": "selection"}));
                bu.page = 0;
            }
        });
        ui.add_enabled_ui(in_view > 0, |ui| {
            if text_button(ui, "bookNewView", "New from View", false).clicked() {
                run(app, &ctx, "book.create", json!({"source": "view"}));
                bu.page = 0;
            }
        });
        if book.is_some() && text_button(ui, "bookDelete", "Delete Book", false).clicked() {
            run(app, &ctx, "book.delete", json!({}));
            bu.page = 0;
        }
    });
    ui.label(egui::RichText::new(crate::i18n::tr_format!("{n} selected · {m} in view", n = selected, m = in_view)).color(t.text_dim));
    let Some(b) = book else { return };
    let s = &b.settings;

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
    row(ui, t, "Photos", |ui| {
        let sources = [(Source::Selected, "Selected Photos"), (Source::InView, "Photos in View")];
        let shown = sources.iter().find(|(s, _)| *s == bu.source).map_or("", |(_, l)| *l);
        let (c, chosen) = crate::widgets::dropdown_search::pick_with(
            ui,
            "bookSource",
            |c| c.selected_text(crate::i18n::tr(shown)),
            &sources,
            |(s, _)| *s == bu.source,
            |(_, l)| crate::i18n::tr(l).to_string(),
            |ui, (s, _), row| register(ui, format!("menu:bookSource-{}", if *s == Source::Selected { "selected" } else { "view" }), row.rect),
        );
        crate::widgets::name_combo(&c, crate::i18n::tr("Photos"));
        register(ui, "combo:bookSource", c.rect);
        if let Some(i) = chosen {
            bu.source = sources[i].0;
        }
    });
    ui.horizontal(|ui| {
        if let Some(v) = check(ui, "bookCover", "Cover", b.cover) {
            run(app, &ctx, "book.setCover", json!({"cover": v}));
            // (the cover is page 0: the other pages move by one)
            bu.page = if v { bu.page + 1 } else { bu.page.saturating_sub(1) };
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if text_button(ui, "bookAutoLayout", "Auto Layout", false).clicked() {
                let source = if bu.source == Source::Selected { "selection" } else { "view" };
                let preset = LAYOUTS[bu.layout.min(LAYOUTS.len() - 1)].0;
                run(app, &ctx, "book.layout", json!({"cover": b.cover, "preset": preset, "source": source}));
                bu.page = 0;
            }
        });
    });

    heading(ui, t, "Page Template");
    let page = bu.page;
    let on = b.pages.get(page).map(|p| p.template);
    row(ui, t, "Template", |ui| {
        let all = Template::ALL;
        let (c, chosen) = crate::widgets::dropdown_search::pick_with(
            ui,
            "bookTemplate",
            |c| c.selected_text(on.map_or(String::new(), |tpl| crate::i18n::tr(template_label(tpl)).to_string())),
            &all,
            |tpl| on == Some(*tpl),
            |tpl| crate::i18n::tr(template_label(*tpl)).to_string(),
            |ui, tpl, row| register(ui, format!("menu:bookTemplate-{}", tpl.name()), row.rect),
        );
        crate::widgets::name_combo(&c, crate::i18n::tr("Template"));
        register(ui, "combo:bookTemplate", c.rect);
        if let Some(tpl) = chosen.and_then(|i| all.get(i)).filter(|tpl| on != Some(**tpl)) {
            run(app, &ctx, "book.setTemplate", json!({"page": page, "template": tpl.name()}));
        }
    });
    ui.label(egui::RichText::new(crate::i18n::tr_format!("Page {n} of {m}", n = page + 1, m = b.pages.len())).color(t.text_dim));

    heading(ui, t, "Page Setup");
    row(ui, t, "Size", |ui| {
        let c = crate::widgets::combo_box("bookSize").selected_text(size_label(&s.size)).show_ui(ui, |ui| {
            for name in size_names() {
                if ui.selectable_label(s.size == name, size_label(name)).clicked() {
                    run(app, &ctx, "book.set", json!({"size": name}));
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
                run(app, &ctx, "book.set", json!({"orientation": v}));
            }
        });
    });
    row(ui, t, "Margin", |ui| {
        if let Some(v) = inches(ui, "bookMargin", s.margin, 2.0) {
            run(app, &ctx, "book.set", json!({"margin": v}));
        }
    });
    row(ui, t, "Cell Padding", |ui| {
        if let Some(v) = inches(ui, "bookPadding", s.padding, 1.0) {
            run(app, &ctx, "book.set", json!({"padding": v}));
        }
    });
    row(ui, t, "Background", |ui| {
        let mut c = s.background;
        let r = ui.color_edit_button_srgb(&mut c);
        register(ui, "color:bookBackground", r.rect);
        if c != s.background {
            run(app, &ctx, "book.set", json!({"background": c}));
        }
    });
    if let Some(v) = check(ui, "bookFill", "Zoom to Fill Frame", s.fill) {
        run(app, &ctx, "book.set", json!({"fill": v}));
    }
    row(ui, t, "Resolution", |ui| {
        let mut ppi = s.ppi;
        let r = ui.add(egui::DragValue::new(&mut ppi).range(72.0..=720.0).speed(1.0).max_decimals(0).suffix(" ppi"));
        register(ui, "drag:bookPpi", r.rect);
        if r.changed() {
            run(app, &ctx, "book.set", json!({"ppi": ppi}));
        }
    });

    heading(ui, t, "Export");
    if text_button(ui, "bookExportJpeg", "Export JPEGs…", false).clicked() {
        run(app, &ctx, "app.bookJpeg", json!({}));
    }
}

// ---- preview -------------------------------------------------------------------------------

fn preview(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book) {
    let ctx = ui.ctx().clone();
    top_bar(ui, t, bu, book);
    let tray = if book.unplaced.is_empty() { 0.0 } else { TRAY_H + 8.0 };
    let size = vec2(ui.available_width(), (ui.available_height() - tray).max(120.0));
    let (area, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(area, 4.0, t.canvas);
    let mut shown = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink(8.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    match bu.view {
        View::Pages => pages_view(app, &mut shown, t, bu, book),
        View::Spread => spread_view(app, &mut shown, t, bu, book),
    }
    if !book.unplaced.is_empty() {
        tray_view(app, ui, t, bu, book);
    }
    let _ = &ctx;
}

/// The widest page counter, for the width that keeps the arrows still: "Page 50 / 50".
const WIDEST_COUNTER: &str = "Page 50 / 50";

/// The bar above the pages: the view choice, the page counter with its arrows and the book's size,
/// as one group centred in the bar.
fn top_bar(ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book) {
    let gap = 8.0;
    let (w, h) = book.settings.page_size().unwrap_or((0.0, 0.0));
    let info = format!("{w:.2} × {h:.2} in · {} {}", book.photos().len(), crate::i18n::tr("photos"));
    let font = t.font(13.0);
    let width_of = |text: &str| ui.painter().layout_no_wrap(text.to_string(), font.clone(), t.text).size().x;
    let page_word = crate::i18n::tr("Page");
    let counter_w = width_of(&WIDEST_COUNTER.replacen("Page", page_word, 1)) + 12.0;
    let (pages_w, spread_w) = (crate::widgets::text_button_size(ui, "Pages"), crate::widgets::text_button_size(ui, "Spread"));
    let arrow_w = crate::widgets::text_button_size(ui, "◀");
    let info_w = width_of(&info);
    // the group's width as drawn last frame; the first frame's is the sum of what it holds and the
    // spaces between, close enough for one frame
    let guess = pages_w + 3.0 + spread_w + 8.0 + 2.0 * arrow_w + counter_w + info_w + 5.0 * gap;
    let total = bu.group_w.unwrap_or(guess);
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    register(ui, "book:topBar", bar);
    let group = Rect::from_center_size(bar.center(), vec2(total.min(bar.width()), bar.height()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(group).layout(egui::Layout::left_to_right(egui::Align::Center)));
    if let Some(v) = choice(&mut child, "bookView", &[(View::Pages, "Pages"), (View::Spread, "Spread")], bu.view) {
        bu.view = v;
    }
    child.spacing_mut().item_spacing.x = gap;
    child.add_space(8.0);
    if text_button(&mut child, "bookPagePrev", "◀", false).clicked() && bu.page > 0 {
        bu.page -= 1;
    }
    let (counter, _) = child.allocate_exact_size(vec2(counter_w, 24.0), Sense::hover());
    let text = format!("{page_word} {} / {}", bu.page + 1, book.pages.len().max(1));
    child.painter().text(counter.center(), egui::Align2::CENTER_CENTER, text, font.clone(), t.text);
    register(&child, "book:pageLabel", counter);
    if text_button(&mut child, "bookPageNext", "▶", false).clicked() && bu.page + 1 < book.pages.len() {
        bu.page += 1;
    }
    let (info_rect, _) = child.allocate_exact_size(vec2(info_w, 24.0), Sense::hover());
    child.painter().text(info_rect.left_center(), egui::Align2::LEFT_CENTER, info, font, t.text_dim);
    let used = child.min_rect();
    register(&child, "book:topGroup", used);
    if (used.width() - total).abs() > 0.5 {
        bu.group_w = Some(used.width());
        child.ctx().request_repaint();
    }
}

/// Every page as a small sheet, [`PER_ROW`] to a row; a click selects the page.
fn pages_view(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book) {
    let Ok((pw, ph)) = book.settings.page_size() else { return };
    let gap = 10.0;
    let w = ((ui.available_width() - gap * (PER_ROW as f32 + 1.0)) / PER_ROW as f32).max(40.0);
    let h = (w as f64 * ph / pw) as f32;
    egui::ScrollArea::vertical().id_salt("book-pages").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        for (r, chunk) in (0..book.pages.len()).collect::<Vec<_>>().chunks(PER_ROW).enumerate() {
            ui.horizontal(|ui| {
                for &i in chunk {
                    ui.vertical(|ui| {
                        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
                        draw_page(app, ui, t, bu, book, i, rect);
                        if resp.clicked() {
                            bu.page = i;
                        }
                        let tag = if i == 0 && book.cover { crate::i18n::tr("Cover").to_string() } else { (i + 1).to_string() };
                        ui.label(egui::RichText::new(tag).color(if i == bu.page { t.accent } else { t.text_dim }));
                    });
                }
            });
            let _ = r;
        }
    });
}

/// The spread holding the selected page, as large as the room allows.
fn spread_view(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book) {
    let Ok((pw, ph)) = book.settings.page_size() else { return };
    let spreads = book.spreads();
    let Some(spread) = spreads.iter().find(|s| s.contains(&bu.page)).cloned() else { return };
    let room = ui.available_rect_before_wrap();
    let n = spread.len() as f64;
    let scale = ((f64::from(room.width()) - 12.0) / (pw * n)).min((f64::from(room.height()) - 8.0) / ph).max(1.0);
    let (w, h) = ((pw * scale) as f32, (ph * scale) as f32);
    let total = vec2(w * spread.len() as f32 + 2.0 * (spread.len() as f32 - 1.0), h);
    let origin = room.center() - total / 2.0;
    for (k, &i) in spread.iter().enumerate() {
        let rect = Rect::from_min_size(origin + vec2((w + 2.0) * k as f32, 0.0), vec2(w, h));
        let resp = ui.interact(rect, egui::Id::unique(("book-spread-page", i)), Sense::click());
        draw_page(app, ui, t, bu, book, i, rect);
        if resp.clicked() {
            bu.page = i;
        }
    }
}

/// One page on `rect`: its background, then each cell's photo as it will print (fit or fill).
fn draw_page(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book, index: usize, rect: Rect) {
    let Some(page) = book.pages.get(index) else { return };
    let Ok((pw, _)) = book.settings.page_size() else { return };
    let scale = rect.width() / pw as f32;
    let bg = book.settings.background;
    let painter = ui.painter_at(rect.expand(4.0));
    painter.rect_filled(rect.translate(vec2(1.5, 2.0)), 0.0, Color32::from_black_alpha(70));
    painter.rect_filled(rect, 0.0, Color32::from_rgb(bg[0], bg[1], bg[2]));
    register(ui, format!("book:page:{index}"), rect);
    let cells = cells_for(page.template, &book.settings).unwrap_or_default();
    let fill = book.settings.fill || page.template == Template::FullBleed;
    for (c, (cell, photo)) in cells.iter().zip(&page.slots).enumerate() {
        let r = Rect::from_min_size(rect.min + vec2(cell.x as f32, cell.y as f32) * scale, vec2(cell.w as f32, cell.h as f32) * scale);
        let id = photo.map(PhotoId);
        if let Some(id) = id {
            paint_photo(app, &painter, r, id, fill);
        } else {
            painter.rect_filled(r, 0.0, Color32::from_gray(205));
        }
        let slot = Slot::Cell { page: index, cell: c };
        bu.hits.push((slot, r));
        register(ui, format!("book:cell:{index}:{c}"), r);
        let resp = ui.interact(r, egui::Id::unique(("book-cell", index, c)), Sense::click_and_drag());
        if resp.clicked() {
            bu.page = index;
        }
        if resp.drag_started() && photo.is_some() {
            bu.drag = Some(slot);
            bu.page = index;
        }
    }
    if index == bu.page {
        painter.rect_stroke(rect.expand(1.0), 0.0, Stroke::new(2.0, t.accent), StrokeKind::Outside);
    }
}

/// A photo inside `cell`: its thumbnail, whole (centred) or cropped to the cell.
fn paint_photo(app: &mut LightcraftApp, painter: &egui::Painter, cell: Rect, id: PhotoId, fill: bool) {
    if cell.width() < 1.0 || cell.height() < 1.0 {
        return;
    }
    let Some(photo) = app.session.catalog.photo(id) else {
        painter.rect_filled(cell, 0.0, Color32::from_gray(90));
        return;
    };
    let (nw, nh) = lightcraft_engine::export::output_size(photo, &ExportOptions::default());
    let sub = 8.0; // sub-point resolution for the rounding in `place`
    let (aw, ah) = ((f64::from(cell.width()) * sub).round() as usize, (f64::from(cell.height()) * sub).round() as usize);
    let Some(p) = place(nw as f64, nh as f64, aw, ah, fill, false) else { return };
    let shown = Rect::from_center_size(cell.center(), vec2(p.w as f32, p.h as f32) / sub as f32);
    if app.renderer.thumb(id).is_none() {
        let px = (shown.width().max(shown.height()) * 2.0).clamp(64.0, 1024.0) as usize;
        crate::panels::grid::request_thumb(app, id, px, 10);
    }
    let clip = painter.with_clip_rect(cell.intersect(painter.clip_rect()));
    match app.renderer.thumb(id).map(|tex| tex.tex.id()) {
        Some(tex) => {
            let mut mesh = egui::Mesh::with_texture(tex);
            let corners = [shown.left_top(), shown.right_top(), shown.right_bottom(), shown.left_bottom()];
            for (pos, uv) in corners.into_iter().zip([pos2(0.0, 0.0), pos2(1.0, 0.0), pos2(1.0, 1.0), pos2(0.0, 1.0)]) {
                mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: Color32::WHITE });
            }
            mesh.indices.extend([0, 1, 2, 0, 2, 3]);
            clip.add(egui::Shape::mesh(mesh));
        }
        // (the page is paper in every theme: fixed greys while the thumbnail comes)
        None => {
            clip.rect_filled(shown, 0.0, Color32::from_gray(52));
        }
    }
}

/// Photos on no page, in a strip below the pages; drag one onto a cell to place it, or drag a page
/// photo onto the strip to take it off its page.
fn tray_view(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book) {
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), TRAY_H + 8.0), Sense::hover());
    ui.painter().rect_filled(area, 4.0, t.canvas);
    // dropped on the strip, a photo goes to the end of it
    bu.hits.push((Slot::Unplaced { unplaced: book.unplaced.len() }, area));
    register(ui, "book:tray", area);
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink2(vec2(8.0, 3.0))).layout(egui::Layout::top_down(egui::Align::Min)));
    inner.spacing_mut().item_spacing.y = 1.0;
    inner.label(egui::RichText::new(format!("{} ({})", crate::i18n::tr("Not on a Page"), book.unplaced.len())).font(t.font(10.0)).color(t.text_dim));
    let size = TRAY_H - 22.0;
    egui::ScrollArea::horizontal().id_salt("book-tray").auto_shrink([false, false]).show(&mut inner, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (i, photo) in book.unplaced.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::drag());
                let painter = ui.painter_at(r);
                paint_photo(app, &painter, r, PhotoId(*photo), false);
                register(ui, format!("book:unplaced:{i}"), r);
                if resp.drag_started() {
                    bu.drag = Some(Slot::Unplaced { unplaced: i });
                }
            }
        });
    });
}

/// While a photo is dragged, a ghost of it follows the pointer; released over a cell it moves there
/// (`book.movePhoto`), over the tray it comes off its page.
fn drop_photo(app: &mut LightcraftApp, ctx: &egui::Context, bu: &mut BookUi, has_book: bool) {
    let Some(from) = bu.drag else { return };
    let pos = ctx.input(|i| i.pointer.latest_pos());
    let book = app.session.books.find(None).ok();
    let photo = book.and_then(|b| match from {
        Slot::Cell { page, cell } => b.pages.get(page).and_then(|p| p.slots.get(cell).copied().flatten()),
        Slot::Unplaced { unplaced } => b.unplaced.get(unplaced).copied(),
    });
    if let (Some(pos), Some(photo)) = (pos, photo) {
        let ghost = Rect::from_center_size(pos, Vec2::splat(48.0));
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("book-ghost")));
        painter.rect_filled(ghost, 2.0, Color32::from_black_alpha(120));
        paint_photo(app, &painter, ghost.shrink(2.0), PhotoId(photo), false);
    }
    ctx.request_repaint();
    if !ctx.input(|i| i.pointer.any_released()) {
        return;
    }
    bu.drag = None;
    if !has_book {
        return;
    }
    let target = pos.and_then(|p| bu.hits.iter().rev().find(|(_, r)| r.contains(p)).map(|(s, _)| *s));
    if let Some(to) = target
        && to != from
        // (a tray photo dropped on the strip stays where it is)
        && !matches!((from, to), (Slot::Unplaced { .. }, Slot::Unplaced { .. }))
    {
        let slot = |s: Slot| match s {
            Slot::Cell { page, cell } => json!({"page": page, "cell": cell}),
            Slot::Unplaced { unplaced } => json!({"unplaced": unplaced}),
        };
        run(app, ctx, "book.movePhoto", json!({"from": slot(from), "to": slot(to)}));
    }
}
