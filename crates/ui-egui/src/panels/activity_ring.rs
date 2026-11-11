//! The ring in the top bar (issue #198): the work the app starts by itself to get photos ready (previews, faces,
//! focus points, the full-size view…) is `background` in the engine's registry, so it takes no row in the activity
//! stack. A small ring beside the top bar's icons fills with the tasks' combined progress instead, and sweeps while no
//! task knows its total yet. Hovering it lists each task with its name, percentage and count; a click opens the
//! same list with ✕ for the tasks that can stop. With nothing running there is no ring.

use egui::epaint::PathStroke;
use egui::{Rect, Sense, Shape, Stroke, pos2, vec2};
use lightcraft_engine::activity::{TaskInfo, Unit};

use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::icons::{Icon, paint};
use crate::panels::activity::{SHOW_AFTER_MS, count_text};
use crate::theme::Tokens;
use crate::widgets::register;

/// The ring's circle (points).
const RADIUS: f32 = 7.5;
const CROSS: f32 = 16.0;

/// The background tasks the ring shows: top-level ones that have run long enough not to flash, oldest first.
pub fn tasks(app: &LightcraftApp) -> Vec<TaskInfo> {
    app.session.activity.list().into_iter().filter(|t| t.background && t.parent.is_none() && t.age_ms >= SHOW_AFTER_MS).collect()
}

/// How far along the tasks are together: the mean of the fractions of those that know their total. `None` when none
/// does (the ring sweeps).
pub fn fraction(tasks: &[TaskInfo]) -> Option<f32> {
    let known: Vec<f64> = tasks.iter().filter(|t| t.total > 0).map(|t| t.done as f64 / t.total as f64).collect();
    (!known.is_empty()).then(|| (known.iter().sum::<f64>() / known.len() as f64).clamp(0.0, 1.0) as f32)
}

/// One task's line: "Building previews · 40 % · 24 of 60".
pub fn line(t: &TaskInfo) -> String {
    let mut parts = vec![tr(&t.label).to_string()];
    if t.cancelling {
        parts.push(tr("Stopping…").to_string());
    } else {
        // a task counted in percent says it once
        if t.total > 0 && t.unit != Unit::Percent {
            parts.push(tr_format!("{pct} %", pct = t.done.saturating_mul(100) / t.total));
        }
        let count = count_text(t);
        if !count.is_empty() {
            parts.push(count);
        }
    }
    parts.join(" · ")
}

/// The ring at `x` in the top bar; `tasks` are what [`tasks`] returned this frame (not empty).
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, full: Rect, x: f32, tasks: &[TaskInfo]) {
    // a sweep and moving counts: look again soon
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    let r = Rect::from_center_size(pos2(x, full.center().y), vec2(28.0, 28.0));
    let resp = ui.interact(r, egui::Id::unique(("top", "activity-ring")), Sense::click());
    let name = tr("Preparing photos in the background");
    crate::widgets::announce(&resp, name);
    register(ui, "icon:activityRing", r);
    paint_ring(ui, t, r.center(), fraction(tasks), resp.hovered());
    // (the list under the ring says the same as the tooltip: no tooltip over it)
    let open = egui::Popup::is_any_open(ui.ctx());
    let resp = if open {
        resp
    } else {
        resp.on_hover_ui(|ui| {
            list(ui, t, tasks, false);
        })
    };
    let mut cancel = None;
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(240.0);
        cancel = list(ui, t, tasks, true);
    });
    if let Some(id) = cancel {
        crate::panels::activity::cancel(app, id);
    }
}

/// The ring: a faint circle, and over it an arc that follows `fraction` from the top, or a short arc turning while
/// it isn't known.
fn paint_ring(ui: &egui::Ui, t: &Tokens, centre: egui::Pos2, fraction: Option<f32>, hovered: bool) {
    let painter = ui.painter();
    let track = if hovered { t.text } else { t.icon }.gamma_multiply(0.35);
    painter.circle_stroke(centre, RADIUS, Stroke::new(2.0, track));
    let top = -std::f32::consts::FRAC_PI_2;
    let (from, sweep) = match fraction {
        Some(f) => (top, f * std::f32::consts::TAU),
        None => (top + ui.ctx().input(|i| i.time) as f32 * 4.0, std::f32::consts::FRAC_PI_2),
    };
    if sweep <= 0.0 {
        return;
    }
    let steps = ((sweep / std::f32::consts::TAU) * 48.0).ceil().max(2.0) as usize;
    let points: Vec<egui::Pos2> = (0..=steps)
        .map(|i| {
            let a = from + sweep * i as f32 / steps as f32;
            centre + vec2(a.cos(), a.sin()) * RADIUS
        })
        .collect();
    painter.add(Shape::line(points, PathStroke::new(2.0, t.accent)));
}

/// One line per task; in the popup (`cancel`) each that can stop has a ✕. The task whose ✕ was clicked.
fn list(ui: &mut egui::Ui, t: &Tokens, tasks: &[TaskInfo], cancel: bool) -> Option<u64> {
    let mut clicked = None;
    ui.spacing_mut().item_spacing.y = 4.0;
    for task in tasks {
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(egui::RichText::new(line(task)).font(t.font(12.5)).color(t.text)).wrap_mode(egui::TextWrapMode::Extend));
            if cancel && task.cancellable && !task.cancelling {
                ui.add_space(8.0);
                let (rect, resp) = ui.allocate_exact_size(vec2(CROSS, CROSS), Sense::click());
                crate::widgets::announce(&resp, tr("Cancel"));
                paint(ui.painter(), rect.shrink(2.0), Icon::Close, if resp.hovered() { t.text } else { t.icon });
                register(ui, format!("activity:cancel:{}", task.id), rect);
                if resp.on_hover_text(tr("Cancel")).clicked() {
                    clicked = Some(task.id);
                }
            }
        });
    }
    clicked
}
