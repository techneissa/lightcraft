//! The head of the Masking panel (fork issue #401): its title and the list of the photo's masks, pinned above the
//! panel's scroll area so switching masks never needs a scroll. A list box to screen readers: with a row focused, Up and
//! Down move to the neighbouring mask.

use egui::accesskit::{Live, Role};
use egui::style::ScrollAnimation;
use egui::{Rect, Sense, Stroke, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, Mask};
use serde_json::json;

use super::mask_thumbs::{self, MaskThumbs};
use super::masking::kind_label;
use super::right::header;
use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{divider, icon_button, register};

/// The least height of a row (a long name wraps and makes it taller).
const ROW: f32 = 36.0;
const THUMB: f32 = 28.0;
/// Where a row's name starts.
const NAME_LEFT: f32 = 44.0;
/// The list grows to this many rows (fewer in a short window, never under two), then scrolls.
const ROWS_SHOWN: f32 = 5.0;
const MOST_OF_PANEL: f32 = 0.35;

#[derive(PartialEq)]
struct Seen {
    photo: PhotoId,
    ids: Vec<u32>,
    active: Option<u32>,
}

#[derive(Default)]
pub(crate) struct MaskList {
    pub(crate) thumbs: MaskThumbs,
    seen: Option<Seen>,
    /// The panel's scroll area has the picked mask's section to bring to its top.
    jump: bool,
    /// The rows drawn last.
    rows: Vec<egui::Id>,
}

fn row_id(mask: u32) -> egui::Id {
    egui::Id::unique(("maskRow", mask))
}

/// Whether a row of the list has the keyboard: it keeps Enter, Space, Tab, Esc and the arrows, and every other key is a
/// shortcut still.
pub(crate) fn row_has_focus(app: &LightcraftApp, ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.focused()).is_some_and(|f| app.mask_list.rows.contains(&f))
}

/// Whether the picked mask's section is to be scrolled to now (asked once).
pub(super) fn take_jump(app: &mut LightcraftApp) -> bool {
    std::mem::take(&mut app.mask_list.jump)
}

/// The title and the mask list.
pub(crate) fn pinned(app: &mut LightcraftApp, ui: &mut egui::Ui, photo: PhotoId) {
    let d = app.session.develop_of(photo).unwrap_or_default();
    header(ui, "Masking");
    // (it draws on the heading row just above the cursor, so it follows the heading and shares its clip)
    super::selection_model::show(app, ui);
    let reveal = follow_selection(app, ui.ctx(), photo, &d);
    app.mask_list.rows.clear();
    if d.masks.is_empty() {
        return;
    }
    mask_thumbs::update(app, ui.ctx(), photo, &d);
    let height = (ROWS_SHOWN * ROW).min((ui.available_height() * MOST_OF_PANEL).max(2.0 * ROW));
    egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 8 }).show(ui, |ui| {
        title_row(app, ui);
        egui::ScrollArea::vertical().id_salt("mask-list").max_height(height).min_scrolled_height(height).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            crate::widgets::announce_region(ui, Role::ListBox, crate::i18n::tr("Masks"));
            for index in 0..d.masks.len() {
                row(app, ui, photo, &d.masks, index, reveal);
            }
        });
        announce_selection(app, ui, &d);
        // (fork, issue #407: what Stop AI did)
        super::mask_stop_ai::announce(app, ui);
    });
    divider(ui);
}

/// Notice a change of the picked mask: another mask of the same set was picked (the panel then scrolls to it), and a
/// keyboard on the list moves with it. Returns whether the list is to scroll to the picked row.
fn follow_selection(app: &mut LightcraftApp, ctx: &egui::Context, photo: PhotoId, d: &DevelopSettings) -> bool {
    let now = Seen { photo, ids: d.masks.iter().map(|m| m.id).collect(), active: app.session.active_mask };
    let before = app.mask_list.seen.take();
    let changed = before.as_ref() != Some(&now);
    // (a mask added, duplicated or deleted is no pick: the panel stays where it is)
    if before.as_ref().is_some_and(|b| b.photo == now.photo && b.ids == now.ids && b.active != now.active) && now.active.is_some() {
        app.mask_list.jump = true;
    }
    if changed
        && let Some(active) = now.active
        && row_has_focus(app, ctx)
    {
        ctx.memory_mut(|m| m.request_focus(row_id(active)));
    }
    app.mask_list.seen = Some(now);
    changed
}

fn title_row(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("Masks")).font(t.semibold(13.0)).color(t.text_label));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let on = app.ui.mask_overlay;
            if icon_button(ui, "maskOverlay", Icon::Eye, vec2(24.0, 24.0), on, true, "Show overlay (O)").clicked() {
                app.ui.mask_overlay = !on;
            }
            let all = app.ui.mask_overlay_all;
            let all_mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
            let all_base = crate::i18n::tr("Show all overlays");
            let all_tip = match crate::shortcuts::shortcut_of(&app.ui.settings.keymap, "view.maskOverlayAll") {
                Some(sc) => format!("{all_base} ({})", crate::menubar::shortcut_text(sc, all_mac)),
                None => all_base.to_string(),
            };
            if icon_button(ui, "maskOverlayAll", Icon::Stack, vec2(24.0, 24.0), all, true, &all_tip).clicked() {
                app.ui.mask_overlay_all = !all;
            }
        });
    });
}

fn row(app: &mut LightcraftApp, ui: &mut egui::Ui, photo: PhotoId, masks: &[Mask], index: usize, reveal: bool) {
    let Some(m) = masks.get(index) else { return };
    let t = Tokens::get(ui.ctx());
    let sel = app.session.active_mask == Some(m.id);
    let rid = row_id(m.id);
    if let Some((renaming, mut name)) = app.ui.renaming_mask.clone().filter(|(r, _)| *r == m.id) {
        // inline rename, as in every list: Enter or leaving the field saves, Esc cancels, a name
        // that is empty or another mask's is refused with the reason
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
        let rect = row.shrink2(vec2(2.0, 5.0));
        register(ui, format!("maskRename:{renaming}"), rect);
        let others = masks.iter().filter(|o| o.id != renaming).map(|o| o.name.as_str());
        let verdict = crate::inline_rename::verdict(&name, others);
        let unchanged = name.trim() == m.name;
        let outcome = crate::inline_rename::edit(ui, &format!("field:maskRename:{renaming}"), &mut name, rect, &verdict, unchanged);
        crate::inline_rename::finish_with(app, ui.ctx(), outcome, "mask.rename", json!({"id": renaming}), "name", name, |app, name| {
            app.ui.renaming_mask = name.map(|n| (renaming, n));
        });
        return;
    }
    // the whole name, wrapped, clear of the eye (and Invert on the picked row)
    let color = if m.visible { t.text } else { t.text_disabled };
    let font = if sel { t.semibold(13.0) } else { t.font(13.0) };
    let width = ui.available_width();
    // (fork, issue #407: an AI mask's Stop AI button, left of Invert and the eye)
    let ai = super::mask_stop_ai::is_ai(app, ui, m.id);
    let room = (width - NAME_LEFT - if sel { 56.0 } else { 32.0 } - if ai { 24.0 } else { 0.0 }).max(24.0);
    let name = ui.painter().layout(m.name.clone(), font, color, room);
    let (r, _) = ui.allocate_exact_size(vec2(width, (name.size().y + 10.0).max(ROW)), Sense::hover());
    let resp = ui.interact(r, rid, Sense::click());
    app.mask_list.rows.push(rid);
    crate::widgets::announce_selected(&resp, m.name.clone(), sel);
    ui.ctx().accesskit_node_builder(rid, |node| {
        node.set_role(Role::ListBoxOption);
        node.set_selected(sel);
        node.clear_toggled();
    });
    register(ui, format!("mask:{}", m.id), r);
    if reveal && sel {
        resp.scroll_to_me_animation(None, ScrollAnimation::none());
    }
    let fill = if sel {
        t.tool_active
    } else if resp.hovered() {
        t.hover.gamma_multiply(0.7)
    } else {
        t.chrome
    };
    ui.painter().rect_filled(r, 4.0, fill);
    // (the picked mask is marked by this bar and a heavier name, not by its colour alone)
    if sel {
        ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(3.0, r.height())), 1.0, t.accent);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(r.shrink(1.0), 4.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    }
    thumbnail(app, ui, photo, m, pos2(r.left() + 8.0 + THUMB / 2.0, r.center().y));
    let at = pos2(r.left() + NAME_LEFT, r.center().y - name.size().y / 2.0);
    register(ui, format!("maskName:{}", m.id), Rect::from_min_size(at, name.size()));
    ui.painter().galley(at, name, color);
    // show / hide on hover (and always while hidden)
    // (the pointer test, not `hovered`: over the eye, the row itself no longer counts as hovered)
    if ui.rect_contains_pointer(r) || !m.visible {
        let er = Rect::from_center_size(pos2(r.right() - 16.0, r.center().y), vec2(22.0, 22.0));
        register(ui, format!("maskVisible:{}", m.id), er);
        let eye = ui.interact(er, egui::Id::unique(("maskVisible", m.id)), Sense::click());
        crate::widgets::announce_selected(&eye, crate::i18n::tr("Show mask"), m.visible);
        let eye = eye.on_hover_text(crate::i18n::tr(if m.visible { "Hide Mask" } else { "Show Mask" }));
        paint(ui.painter(), er.shrink(3.0), if m.visible { Icon::Eye } else { Icon::EyeOff }, if eye.hovered() { t.text } else { t.text_dim });
        if eye.clicked() {
            let _ = app.run("mask.visible", json!({"id": m.id}));
        }
    }
    if sel {
        // Invert of the whole mask, beside the eye
        super::mask_parts::mask_invert_button(app, ui, r, m.id, m.invert);
    }
    if ai {
        super::mask_stop_ai::button(app, ui, r, r.right() - if sel { 64.0 } else { 40.0 }, photo, m.id, &m.name);
    }
    if resp.clicked() {
        let _ = app.run("mask.select", json!({"id": m.id}));
        // (the picked row again too: back to its sliders)
        app.mask_list.jump = true;
    }
    if resp.double_clicked() {
        app.ui.renaming_mask = Some((m.id, m.name.clone()));
    }
    if resp.has_focus() {
        arrows(app, ui.ctx(), masks, index, rid);
    }
    crate::widgets::context_menu(&resp, |ui| super::mask_menus::mask_menu(app, ui, m.id, &m.name, m.visible, index, masks.len()));
}

/// Up / Down on a focused row pick the neighbouring mask and take the keyboard there (no wrapping, as in any list).
fn arrows(app: &mut LightcraftApp, ctx: &egui::Context, masks: &[Mask], index: usize, rid: egui::Id) {
    ctx.memory_mut(|m| m.set_focus_lock_filter(rid, egui::EventFilter { vertical_arrows: true, ..Default::default() }));
    let (up, down) =
        ctx.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)));
    let next = if up {
        index.checked_sub(1)
    } else if down {
        Some(index + 1)
    } else {
        None
    };
    if let Some(m) = next.and_then(|i| masks.get(i))
        && app.run("mask.select", json!({"id": m.id})).is_ok()
    {
        ctx.memory_mut(|mem| mem.request_focus(row_id(m.id)));
        ctx.request_repaint();
    }
}

/// What the mask selects, white on black; its kind's icon until that picture is made.
fn thumbnail(app: &LightcraftApp, ui: &egui::Ui, photo: PhotoId, m: &Mask, center: egui::Pos2) {
    let t = Tokens::get(ui.ctx());
    let frame = Rect::from_center_size(center, vec2(THUMB, THUMB));
    register(ui, format!("maskThumb:{}", m.id), frame);
    match app.mask_list.thumbs.get(photo, m.id) {
        Some(texture) => {
            let size = texture.size_vec2();
            let scale = (THUMB / size.x.max(1.0)).min(THUMB / size.y.max(1.0));
            let at = Rect::from_center_size(center, size * scale);
            let tint = egui::Color32::from_white_alpha(if m.visible { 255 } else { 96 });
            ui.painter().image(texture.id(), at, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
            ui.painter().rect_stroke(at, 0.0, Stroke::new(1.0, t.divider), egui::StrokeKind::Outside);
        }
        None => {
            let icon = m.components.first().map(|c| kind_label(&c.shape).1).unwrap_or(Icon::Mask);
            ui.painter().rect_filled(frame, 3.0, t.inset);
            paint(ui.painter(), Rect::from_center_size(center, vec2(16.0, 16.0)), icon, t.text_label);
        }
    }
}

/// The live region that says which mask was picked, for a change made while the keyboard is elsewhere (a key, a pin on
/// the photo, automation). On the list itself the focused row says it.
fn announce_selection(app: &LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    let (_, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 0.0), Sense::hover());
    let picked = d.masks.iter().find(|m| Some(m.id) == app.session.active_mask);
    let text = picked.map(|m| crate::i18n::tr_format!("Mask {name} selected", name = m.name)).unwrap_or_default();
    let live = if row_has_focus(app, ui.ctx()) { Live::Off } else { Live::Polite };
    ui.ctx().accesskit_node_builder(resp.id, |node| {
        node.set_role(Role::Status);
        node.set_label(text);
        node.set_live(live);
    });
}
