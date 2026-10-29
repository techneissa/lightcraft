//! Quick filters in the bottom bar, beside the thumbnail slider: a Stars dropdown (several ratings
//! can be ticked, the menu stays open) and Picked / Rejected / Unflagged toggle buttons. They drive
//! the same `library.filter` as the filter bar (`ratings` / `flags`, see
//! `lightcraft_catalog::quick_filter`), so both show one state. Choices within a group are OR, the
//! groups AND; nothing chosen filters nothing.
//!
//! Automation ids: `quickfilter:stars` (the dropdown), `quickfilter:star:0` … `quickfilter:star:5`
//! (its rows, while it is open), `quickfilter:flag:pick|reject|none`, `quickfilter:clear` (the ✕), `quickfilter:menuClear` (Clear Filters in the Stars menu),
//! `icon:quickFilters` (the one button standing in for all of them in a narrow bar).

use egui::{Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::{Filter, Flag};
use serde_json::Value;

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::register;

/// Bar width (inside its padding) from which every control shows, and from which one button stands in for them.
pub const FULL_FROM: f32 = 900.0;
pub const COMPACT_FROM: f32 = 700.0;
const STARS_W: f32 = 104.0;
const BUTTON: f32 = 26.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Full,
    Compact,
    Hidden,
}

/// How much of the quick filters a bottom bar `width` points wide has room for. `crowded`: the right
/// side already holds many buttons (Detail, Compare), which only leaves room for the single button.
pub fn mode(width: f32, crowded: bool) -> Mode {
    match (width >= FULL_FROM && !crowded, width >= if crowded { FULL_FROM + 20.0 } else { COMPACT_FROM }) {
        (true, _) => Mode::Full,
        (false, true) => Mode::Compact,
        _ => Mode::Hidden,
    }
}

fn rating_name(stars: u8) -> String {
    if stars == 0 { crate::i18n::tr("Unrated").to_string() } else { format!("{stars}★") }
}

fn flag_name(flag: Flag) -> &'static str {
    match flag {
        Flag::Pick => "Picked",
        Flag::Reject => "Rejected",
        Flag::None => "Unflagged",
    }
}

/// What the Stars button shows: "All" without a rating filter, else the ratings it lets through
/// ("2★, 4★"; "3 Ratings" when that is too long for the button).
pub fn stars_summary(f: &Filter) -> String {
    let Some(on) = f.ratings_shown() else { return crate::i18n::tr("All").to_string() };
    let names: Vec<String> = (0u8..).zip(on).filter(|(_, on)| *on).map(|(n, _)| rating_name(n)).collect();
    let joined = names.join(", ");
    match names.len() {
        0 => crate::i18n::tr("None").to_string(),
        n if joined.chars().count() > 11 => crate::i18n::tr_format!("{n} Ratings", n = n),
        _ => joined,
    }
}

fn active(f: &Filter) -> bool {
    f.ratings_shown().is_some() || f.flags_shown().is_some()
}

/// "Showing 12 of 298 photos" while a filter hides some, for the tooltips.
fn showing(app: &mut LightcraftApp) -> String {
    let (n, total) = (app.session.visible_shared().1.len(), app.session.source_total());
    match total {
        Some(t) if active(&app.session.filter) && t > n => crate::i18n::tr_format!("Showing {n} of {t} photos", n = n, t = t),
        _ => String::new(),
    }
}

fn run(app: &mut LightcraftApp, patch: Value) {
    let _ = app.run("library.filter", patch);
}

/// A row of the Stars menu: `stars` (0 = unrated) and whether it is ticked.
pub struct Row {
    pub stars: u8,
    pub label: String,
    pub on: bool,
}

/// The Stars menu's rows for `f`, Unrated first.
pub fn rows(f: &Filter) -> Vec<Row> {
    let on = f.ratings_shown().unwrap_or([false; 6]);
    (0u8..=5).map(|stars| Row { stars, label: rating_name(stars), on: on.get(usize::from(stars)).copied().unwrap_or(false) }).collect()
}

/// The checkable rows whose label contains `query` (case-insensitive; empty = all of them), so a
/// search field above them has one place to hook in. Returns the star count of the row clicked.
pub fn rows_menu(ui: &mut egui::Ui, rows: &[Row], query: &str) -> Option<u8> {
    let query = query.trim().to_lowercase();
    let mut clicked = None;
    for row in rows.iter().filter(|r| query.is_empty() || r.label.to_lowercase().contains(&query)) {
        let mut on = row.on;
        let resp = ui.checkbox(&mut on, &row.label);
        register(ui, format!("quickfilter:star:{}", row.stars), resp.rect);
        if resp.changed() {
            clicked = Some(row.stars);
        }
    }
    clicked
}

fn clear_menu(app: &mut LightcraftApp, resp: &egui::Response) {
    crate::widgets::context_menu(resp, |ui| {
        if ui.add_enabled(active(&app.session.filter), egui::Button::new(crate::i18n::tr("Clear Filters"))).clicked() {
            run(app, Filter::clear_quick_patch());
        }
    });
}

/// The Stars button: a star, the choice and a chevron; lit while a rating filter is on.
fn stars_button(ui: &mut egui::Ui, lit: bool, text: &str, tip: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(STARS_W, BUTTON), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, tip));
    register(ui, "quickfilter:stars", r);
    let p = ui.painter();
    if lit {
        p.rect_filled(r, 5.0, t.tool_active);
        p.rect_stroke(r, 5.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    } else {
        if resp.hovered() {
            p.rect_filled(r, 5.0, t.hover);
        }
        p.rect_stroke(r, 5.0, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
    }
    let ink = if lit { t.text } else if resp.hovered() { t.text } else { t.text_label };
    paint(p, Rect::from_center_size(pos2(r.left() + 14.0, r.center().y), vec2(14.0, 14.0)), if lit { Icon::StarFilled } else { Icon::Star }, if lit { t.accent } else { t.icon });
    let text_area = Rect::from_min_max(pos2(r.left() + 26.0, r.top()), pos2(r.right() - 18.0, r.bottom()));
    let galley = p.layout_no_wrap(text.to_string(), t.font(12.5), ink);
    p.with_clip_rect(text_area).galley(pos2(text_area.left(), r.center().y - galley.size().y / 2.0), galley, ink);
    paint(p, Rect::from_center_size(pos2(r.right() - 11.0, r.center().y), vec2(14.0, 14.0)), Icon::ChevronDown, ink);
    resp.on_hover_text(tip)
}

/// A square toggle drawing a flag icon, lit (and coloured) while its flag is let through.
fn flag_button(ui: &mut egui::Ui, id: &str, flag: Flag, on: bool, tip: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(BUTTON, BUTTON), Sense::click());
    crate::widgets::announce_selected(&resp, tip, on);
    register(ui, format!("quickfilter:flag:{id}"), r);
    let p = ui.painter();
    if on {
        p.rect_filled(r, 5.0, t.tool_active);
        p.rect_stroke(r, 5.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(r, 5.0, t.hover);
    }
    let (icon, ink) = match flag {
        Flag::Pick if on => (Icon::FlagPickFilled, t.pick),
        Flag::Reject if on => (Icon::FlagReject, t.reject),
        Flag::Pick => (Icon::FlagPick, if resp.hovered() { t.text_label } else { t.icon }),
        Flag::Reject => (Icon::FlagReject, if resp.hovered() { t.text_label } else { t.icon }),
        Flag::None => (Icon::Circle, if on { t.text } else if resp.hovered() { t.text_label } else { t.icon }),
    };
    paint(p, r.shrink(5.0), icon, ink);
    resp.on_hover_text(tip)
}

/// A small ✕ that clears both groups; drawn only while a filter is on, its room kept either way.
fn clear_button(ui: &mut egui::Ui, on: bool, tip: &str) -> Option<egui::Response> {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(22.0, BUTTON), if on { Sense::click() } else { Sense::hover() });
    if !on {
        return None;
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
    register(ui, "quickfilter:clear", r);
    let c = if resp.hovered() { t.text } else { t.text_dim };
    let m = 4.0;
    let (a, b) = (r.center() - vec2(m, m), r.center() + vec2(m, m));
    ui.painter().line_segment([a, b], Stroke::new(1.4, c));
    ui.painter().line_segment([pos2(a.x, b.y), pos2(b.x, a.y)], Stroke::new(1.4, c));
    Some(resp.on_hover_text(tip))
}

/// The controls, added right to left into the bottom bar's right group (so the first one drawn is
/// the right-most): ✕, the three flags, the Stars dropdown.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, mode: Mode) {
    match mode {
        Mode::Hidden => {}
        Mode::Compact => compact(app, ui),
        Mode::Full => full(app, ui),
    }
}

fn full(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let f = app.session.filter.clone();
    let lit = active(&f);
    let count = showing(app);
    let with_count = |tip: &str| if count.is_empty() { tip.to_string() } else { format!("{tip} — {count}") };
    if let Some(x) = clear_button(ui, lit, &with_count(crate::i18n::tr("Clear Star and Flag Filters"))) {
        if x.clicked() {
            run(app, Filter::clear_quick_patch());
        }
    }
    let shown = f.flags_shown();
    let mut buttons = Vec::new();
    // right to left: Unflagged, Rejected, Picked reads Picked, Rejected, Unflagged
    for (id, flag, tip) in [
        ("none", Flag::None, "Show Unflagged Photos"),
        ("reject", Flag::Reject, "Show Rejected Photos"),
        ("pick", Flag::Pick, "Show Picked Photos"),
    ] {
        let on = shown.as_ref().is_some_and(|s| s.contains(&flag));
        let resp = flag_button(ui, id, flag, on, &with_count(crate::i18n::tr(tip)));
        if resp.clicked() {
            run(app, f.toggle_flag_patch(flag));
        }
        buttons.push(resp);
    }
    let summary = stars_summary(&f);
    let tip = with_count(&crate::i18n::tr_format!("Filter by Stars: {choice}", choice = summary));
    let stars = stars_button(ui, f.ratings_shown().is_some(), &summary, &tip);
    // (no right-click menu on the Stars button: it would share the dropdown's popup; its menu has Clear Filters)
    stars_popup(app, &stars);
    for b in &buttons {
        clear_menu(app, b);
    }
}

fn stars_popup(app: &mut LightcraftApp, stars: &egui::Response) {
    let mut toggled = None;
    egui::Popup::menu(stars).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        let f = &app.session.filter;
        let rows = rows(f);
        toggled = rows_menu(ui, &rows, "");
        ui.separator();
        let resp = ui.add_enabled(active(f), egui::Button::new(crate::i18n::tr("Clear Filters")));
        register(ui, "quickfilter:menuClear", resp.rect);
        if resp.clicked() {
            run(app, Filter::clear_quick_patch());
        }
    });
    if let Some(stars) = toggled {
        let patch = app.session.filter.toggle_rating_patch(stars);
        run(app, patch);
    }
}

/// One button for a bar with no room for the rest; its menu holds both groups.
fn compact(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let f = app.session.filter.clone();
    let lit = active(&f);
    let count = showing(app);
    let tip = if count.is_empty() { crate::i18n::tr("Filter by Stars and Flags").to_string() } else { format!("{} — {count}", crate::i18n::tr("Filter by Stars and Flags")) };
    let button = crate::widgets::icon_button(ui, "quickFilters", Icon::Filter, vec2(30.0, 30.0), lit, true, &tip);
    let mut star = None;
    let mut flag = None;
    egui::Popup::menu(&button).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("Stars")).weak());
        star = rows_menu(ui, &rows(&f), "");
        ui.separator();
        ui.label(egui::RichText::new(crate::i18n::tr("Flag")).weak());
        let shown = f.flags_shown();
        for (id, fl) in [("pick", Flag::Pick), ("reject", Flag::Reject), ("none", Flag::None)] {
            let mut on = shown.as_ref().is_some_and(|s| s.contains(&fl));
            let resp = ui.checkbox(&mut on, crate::i18n::tr(flag_name(fl)));
            register(ui, format!("quickfilter:flag:{id}"), resp.rect);
            if resp.changed() {
                flag = Some(fl);
            }
        }
        ui.separator();
        let resp = ui.add_enabled(lit, egui::Button::new(crate::i18n::tr("Clear Filters")));
        register(ui, "quickfilter:clear", resp.rect);
        if resp.clicked() {
            run(app, Filter::clear_quick_patch());
        }
    });
    if let Some(stars) = star {
        run(app, f.toggle_rating_patch(stars));
    }
    if let Some(fl) = flag {
        run(app, f.toggle_flag_patch(fl));
    }
}

/// Used by the filter bar: the flags it lights, from the same state as these buttons.
pub fn flag_lit(f: &Filter, flag: Flag) -> bool {
    f.flags_shown().is_some_and(|s| s.contains(&flag))
}
