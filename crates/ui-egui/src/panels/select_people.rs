//! Fork feature (`fork/select-people.md`): Select People masks in the Masking panel.
//!
//! - Under Create New Mask, a choice of Person, Background and Custom. Person adds an Entire Person mask of
//!   everyone, Background a mask of everything that is not a person; Custom shows the usual tiles (Object, Subject,
//!   Brush…). The choice is not saved: the panel opens on Custom.
//! - The people picker (`panels/detail/people_picker.rs`) on the photo, at the image view's top-right corner,
//!   while Person or Background is chosen or a Select People mask is selected: "All" and a round chip per person,
//!   then the parts (Entire Person, Entire Face, Facial Skin… Background). A plain click picks one part, ⌘-click
//!   adds or removes one; a face chip picks whose. With a Select People mask selected, a click changes that mask;
//!   without one, it makes a new mask. Folded or open is kept in the saved `openFlyouts` ([`FOLDED`]).
//! - A photo's people are found once, on a worker thread (the parse's activity row); a mask asked for before then
//!   is made when they are found.

use std::sync::Arc;

use egui::Rect;
use lightcraft_catalog::PhotoId;
use lightcraft_develop::MaskShape;
use lightcraft_engine::select_people::{Parsed, PeopleState, Pick, current, person_name};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::panels::detail::people_picker::{self, FaceChip, PartItem, Picker};
use crate::state::{RightPanel, ToolOverlay};
use crate::theme::Tokens;

/// The id in `openFlyouts` that keeps the picker folded (absent: open, the default).
pub(crate) const FOLDED: &str = "selectPeople.folded";

/// Which way of making a mask the panel offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    Person,
    Background,
    #[default]
    Custom,
}

fn mode_id() -> egui::Id {
    egui::Id::unique("selectPeople-mode")
}

pub(crate) fn mode(ctx: &egui::Context) -> Mode {
    ctx.data(|d| d.get_temp::<Mode>(mode_id())).unwrap_or_default()
}

pub(crate) fn set_mode(ctx: &egui::Context, m: Mode) {
    ctx.data_mut(|d| d.insert_temp(mode_id(), m));
}

/// A mask asked for before the photo's people were found: its `selectPeople.add` parameters, one per photo (made
/// when the people are found and the photo is shown, also after showing another photo meanwhile).
#[derive(Clone)]
struct Pending(Value);

fn pending_id(photo: PhotoId) -> egui::Id {
    egui::Id::unique(("selectPeople-pending", photo.0))
}

/// The photo whose people the picker asked for since the Masking panel last opened on it.
fn asked_id() -> egui::Id {
    egui::Id::unique("selectPeople-asked")
}

/// The face picked while no Select People mask is selected (for the next new mask), per photo.
fn face_id(photo: PhotoId) -> egui::Id {
    egui::Id::unique(("selectPeople-face", photo.0))
}

/// Add a Select People mask of `params` now, or once the photo's people are found.
fn add(app: &mut LightcraftApp, ctx: &egui::Context, photo: PhotoId, params: Value) {
    if app.session.people_of(photo).ready.is_some() {
        if let Err(e) = app.run("selectPeople.add", params) {
            app.toast_error(ctx, e);
        }
        return;
    }
    app.session.people_find(photo, true);
    ctx.data_mut(|d| d.insert_temp(pending_id(photo), Pending(params)));
}

/// The Person / Background / Custom row under Create New Mask. True when the usual tiles show (Custom).
pub(crate) fn mode_row(app: &mut LightcraftApp, ui: &mut egui::Ui, photo: PhotoId) -> bool {
    let t = Tokens::get(ui.ctx());
    let now = mode(ui.ctx());
    if let Some(i) = segments(ui, now) {
        let ctx = ui.ctx().clone();
        match i {
            0 => {
                set_mode(&ctx, Mode::Person);
                let face = ctx.data(|d| d.get_temp::<usize>(face_id(photo)));
                add(app, &ctx, photo, json!({"parts": ["entirePerson"], "face": face}));
            }
            1 => {
                set_mode(&ctx, Mode::Background);
                add(app, &ctx, photo, json!({"parts": ["background"]}));
            }
            _ => set_mode(&ctx, Mode::Custom),
        }
    }
    let now = mode(ui.ctx());
    if now == Mode::Custom {
        ui.add_space(6.0);
        return true;
    }
    ui.add_space(4.0);
    let st = app.session.people_of(photo);
    let line = match (&st.ready, &st.error) {
        _ if st.busy => "Finding people…".to_string(),
        (Some(p), _) if p.parsing.people.is_empty() => "No people found in this photo".into(),
        (Some(_), _) => "Pick parts on the photo · ⌘-click adds more".into(),
        (None, Some(e)) => e.clone(),
        (None, None) => String::new(),
    };
    ui.horizontal_wrapped(|ui| {
        if st.busy {
            crate::progress::spinner(ui, 14.0);
        }
        ui.add(egui::Label::new(egui::RichText::new(crate::i18n::tr(&line)).color(t.text_dim).size(11.5)).wrap());
    });
    if st.error.as_deref().is_some_and(|e| e.contains("not downloaded"))
        && crate::widgets::text_button(ui, "selectPeopleModels", "Settings…", false).clicked()
    {
        let _ = app.run("app.settings", json!({"tab": "faces"}));
    }
    ui.add_space(4.0);
    false
}

/// The three choices in a row sharing the width, each label at 11.5 pt or smaller so the longest fits its third
/// of a narrow panel (each one `button:maskMode-<key>`). The one clicked, by index.
fn segments(ui: &mut egui::Ui, now: Mode) -> Option<usize> {
    const ITEMS: [(&str, &str, &str, Mode); 3] = [
        ("Person", "person", "A mask of people or their parts", Mode::Person),
        ("Background", "background", "A mask of everything that is not a person", Mode::Background),
        ("Custom", "custom", "Object, Subject, Brush and the other masks", Mode::Custom),
    ];
    const GAP: f32 = 4.0;
    let t = Tokens::get(ui.ctx());
    // (rounded down: the row never reaches past the panel, which would widen what comes below it)
    let seg_w = ((ui.available_width() - 2.0 * GAP) / 3.0).floor().max(24.0);
    let widest = |size: f32| {
        ITEMS.iter().map(|(l, ..)| ui.painter().layout_no_wrap(crate::i18n::tr(l).to_string(), t.semibold(size), t.text).size().x).fold(0.0, f32::max)
    };
    let size = [11.5, 11.0, 10.5, 10.0, 9.5, 9.0].into_iter().find(|s| widest(*s) + 8.0 <= seg_w).unwrap_or(9.0);
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for (i, (label, key, tip, mode)) in ITEMS.iter().enumerate() {
            let label = crate::i18n::tr(label);
            let (r, resp) = ui.allocate_exact_size(egui::vec2(seg_w, 24.0), egui::Sense::click());
            crate::widgets::announce_selected(&resp, label.to_string(), now == *mode);
            crate::widgets::register(ui, format!("button:maskMode-{key}"), r);
            let resp = resp.on_hover_text(crate::i18n::tr(tip));
            let fill = if now == *mode {
                t.pressed
            } else if resp.hovered() {
                t.hover
            } else {
                t.button
            };
            let p = ui.painter();
            p.rect(r, egui::CornerRadius::same(4), fill, egui::Stroke::new(1.0, t.button_border), egui::StrokeKind::Inside);
            let g = crate::widgets::one_line(p, label, t.semibold(size), t.text, seg_w - 6.0);
            p.galley(r.center() - g.size() / 2.0, g, t.text);
            if resp.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// The selected mask's Select People part, if it has one: (the mask's id, its shape).
fn active_part(app: &LightcraftApp, photo: PhotoId) -> Option<(u32, MaskShape)> {
    let mid = app.session.active_mask?;
    let d = app.session.develop_of(photo)?;
    let m = d.masks.iter().find(|m| m.id == mid)?;
    let c = m.components.iter().find(|c| matches!(c.shape, MaskShape::People { seg: Some(_), .. }))?;
    Some((mid, c.shape.clone()))
}

/// The textures of the photo's people's face thumbnails, made once per parse. One set is kept: the photo shown.
fn thumbs(ctx: &egui::Context, photo: PhotoId, parsed: &Arc<Parsed>) -> Arc<Vec<egui::TextureHandle>> {
    type Kept = (PhotoId, usize, Arc<Vec<egui::TextureHandle>>);
    let id = egui::Id::unique("selectPeopleThumbs");
    let made = Arc::as_ptr(parsed) as usize;
    if let Some((p, m, t)) = ctx.data(|d| d.get_temp::<Kept>(id))
        && p == photo
        && m == made
    {
        return t;
    }
    let textures: Vec<egui::TextureHandle> = parsed
        .thumbs
        .iter()
        .enumerate()
        .map(|(i, img)| {
            let bytes: Vec<u8> = img.data.iter().flatten().copied().collect();
            let image = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &bytes);
            ctx.load_texture(format!("select-people-{}-{i}", photo.0), image, egui::TextureOptions::LINEAR)
        })
        .collect();
    let textures = Arc::new(textures);
    // (the set kept before is dropped here, and its textures with it)
    ctx.data_mut(|d| d.insert_temp::<Kept>(id, (photo, made, textures.clone())));
    textures
}

/// The face chip the picker shows picked and the line under the faces, for photo people `st` and the selected Select
/// People part's (person, everyone's) when one is selected (else the face picked last on the photo, `remembered`).
/// A saved face this photo's people don't have picks no chip (an index past them) and says so.
fn face_and_note(st: &PeopleState, now: Option<(Option<usize>, bool)>, remembered: Option<usize>) -> (Option<usize>, Option<String>) {
    let people = st.ready.as_ref().map(|p| p.parsing.people.len());
    let missing = people.is_some() && matches!(now, Some((None, false)));
    let note = match (people, &st.error) {
        _ if st.busy => Some("Finding people…".to_string()),
        (Some(0), _) => Some("No people found".into()),
        (Some(_), _) if missing => Some("Face not in this photo".into()),
        (None, Some(e)) => Some(e.clone()),
        (None, None) => Some("Finding people…".into()),
        _ => None,
    };
    let n = people.unwrap_or(0);
    let face = match now {
        _ if missing => Some(n),
        Some((person, _)) => person.filter(|i| *i < n),
        None => remembered.filter(|i| *i < n),
    };
    (face, note)
}

/// The menu group part `p` starts, with its header (`""`: a divider only).
fn group(p: Pick) -> Option<&'static str> {
    match p {
        Pick::EntirePerson => Some("Person"),
        Pick::Hair => Some("Face"),
        Pick::Clothes => Some("Clothes"),
        Pick::Background => Some(""),
        _ => None,
    }
}

/// Draw the picker over `canvas` (the image view) for `photo`, and make or change masks from its clicks.
pub(crate) fn overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, canvas: Rect, photo: PhotoId) {
    let ctx = ui.ctx().clone();
    if app.ui.right != RightPanel::Masking {
        ctx.data_mut(|d| d.remove::<PhotoId>(asked_id()));
        return;
    }
    let st = app.session.people_of(photo);
    // a mask asked for before the people were found
    if let Some(Pending(params)) = ctx.data(|d| d.get_temp::<Pending>(pending_id(photo))) {
        if st.ready.is_some() {
            ctx.data_mut(|d| d.remove::<Pending>(pending_id(photo)));
            if let Err(e) = app.run("selectPeople.add", params) {
                app.toast_error(&ctx, e);
            }
        } else if let Some(e) = &st.error {
            ctx.data_mut(|d| d.remove::<Pending>(pending_id(photo)));
            app.toast_error(&ctx, format!("Select People: {e}"));
        }
    }
    let part = active_part(app, photo);
    if part.is_none() && mode(&ctx) == Mode::Custom {
        return;
    }
    // (asked once per photo while the panel is open, not from every frame: a click on Person or Background asks again)
    if ctx.data(|d| d.get_temp::<PhotoId>(asked_id())) != Some(photo) {
        ctx.data_mut(|d| d.insert_temp(asked_id(), photo));
        app.session.people_find(photo, false);
    }
    if st.busy || st.ready.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    if app.ui.fullscreen || app.ui.tool_overlay == ToolOverlay::Never {
        return;
    }
    let ready = st.ready.clone();
    let textures = ready.as_ref().map(|p| thumbs(&ctx, photo, p)).unwrap_or_default();
    let chips: Vec<FaceChip> = ready
        .as_ref()
        .map(|p| {
            p.parsing.people.iter().enumerate().map(|(i, q)| FaceChip { texture: textures.get(i).map(|t| t.id()), name: person_name(q, i) }).collect()
        })
        .unwrap_or_default();
    let items: Vec<PartItem> = if ready.is_some() {
        Pick::ALL.iter().map(|p| PartItem { key: p.id().into(), label: p.label().into(), group: group(*p) }).collect()
    } else {
        Vec::new()
    };
    let now = part.as_ref().and_then(|(_, shape)| current(shape, ready.as_deref()));
    let selected: Vec<usize> = now.iter().flat_map(|(picks, ..)| picks).filter_map(|p| Pick::ALL.iter().position(|q| q == p)).collect();
    let remembered = ctx.data(|d| d.get_temp::<usize>(face_id(photo)));
    let (face, note) = face_and_note(&st, now.as_ref().map(|(_, person, everyone)| (*person, *everyone)), remembered);
    let pk = Picker {
        id: "selectPeople",
        faces: &chips,
        face,
        parts: &items,
        selected_parts: &selected,
        folded: app.ui.flyout_open(FOLDED),
        note: note.as_deref(),
    };
    let picked = people_picker::show(ui, canvas, &pk);
    if picked.toggle_fold {
        app.ui.toggle_flyout(FOLDED);
    }
    let face_value = |f: Option<usize>| f.map_or(json!("all"), |i| json!(i));
    if let Some(choice) = picked.face {
        ctx.data_mut(|d| match choice {
            Some(i) => {
                d.insert_temp(face_id(photo), i);
            }
            None => {
                d.remove::<usize>(face_id(photo));
            }
        });
        let r = match &part {
            Some((mid, _)) => app.run("selectPeople.set", json!({"id": mid, "face": face_value(choice)})),
            None => app.run("selectPeople.add", json!({"parts": ["entirePerson"], "face": face_value(choice)})),
        };
        if let Err(e) = r {
            app.toast_error(&ctx, e);
        }
    }
    if let Some(sel) = picked.parts.filter(|s| !s.is_empty()) {
        let ids: Vec<&str> = sel.iter().filter_map(|i| Pick::ALL.get(*i)).map(|p| p.id()).collect();
        let r = match &part {
            Some((mid, _)) => app.run("selectPeople.set", json!({"id": mid, "parts": ids})),
            None => app.run("selectPeople.add", json!({"parts": ids, "face": face_value(face)})),
        };
        if let Err(e) = r {
            app.toast_error(&ctx, e);
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_round;
