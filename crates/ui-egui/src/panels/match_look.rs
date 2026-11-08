//! Fork feature (`fork/match-look.md`): Photo ▸ Match Look… gives the selected photos the colour and tone of a
//! reference picture (`develop.matchLook`). The window picks the reference (a photo of the view, an image file, a
//! pasted image or an https address), shows it small, and runs the match in the background; the result lands on the
//! photos as slider values, one undo step. The window stays open, so another reference or Amount can be tried after
//! Undo.
//!
//! Commands: `dialog.matchLook` {photo?, path?, url?} opens it (with a reference set), `matchLook.chooseFile` asks for
//! an image file, `matchLook.apply` {amount?} runs the match with the window's reference.

use egui::{Color32, Rect, Sense, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_raster::Rgba8;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::pick::{PickRequest, Picked};
use crate::theme::Tokens;
use crate::widgets::text_field::TextField;

pub const OPEN: &str = "dialog.matchLook";
pub const CHOOSE_FILE: &str = "matchLook.chooseFile";
pub const APPLY: &str = "matchLook.apply";
/// The image files a reference may be.
const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "webp", "heic", "heif", "avif", "gif", "bmp", "jxl", "psd"];
/// Photos of the view offered as references.
const STRIP_PHOTOS: usize = 60;
const STRIP_THUMB: f32 = 56.0;
const PREVIEW: f32 = 180.0;
/// How often a running match is asked whether it is done.
const PUMP_SECS: f64 = 0.2;

/// The picture the photos are matched to.
#[derive(Clone, Debug, PartialEq)]
pub enum Reference {
    Photo(PhotoId),
    File(String),
    Url(String),
    /// The picture last pasted into the window (held by the session).
    Pasted,
}

/// The window while it is open (and while its match runs, even closed).
#[derive(Clone, Default)]
pub struct State {
    open: bool,
    reference: Option<Reference>,
    url: String,
    /// 0..100.
    amount: f32,
    running: bool,
    next_pump: f64,
    message: Option<String>,
    /// The picture shown for a file or pasted reference, and what it shows.
    preview: Option<(Reference, egui::TextureHandle, [usize; 2])>,
    /// A picture to turn into the preview texture on the next draw.
    pending_preview: Option<(Reference, Rgba8)>,
    /// A file's preview being read on a worker thread: it fills the slot when done.
    loading: Option<(Reference, PreviewSlot)>,
}

/// Where a background preview read leaves its picture (`None` inside while it runs or when the file can't be read).
type PreviewSlot = std::sync::Arc<std::sync::Mutex<Option<Option<Rgba8>>>>;

impl State {
    /// A match started from the window is still running.
    pub fn running(&self) -> bool {
        self.running
    }

    /// The reference's small picture is ready to show (a file's is read in the background).
    pub fn preview_ready(&self) -> bool {
        self.pending_preview.is_some() || self.preview.is_some()
    }
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchLook").field("open", &self.open).field("reference", &self.reference).field("running", &self.running).finish()
    }
}

fn state(app: &mut LightcraftApp) -> &mut State {
    app.ui.match_look.get_or_insert_with(|| State { amount: 100.0, ..State::default() })
}

/// A file reference's small preview, read and decoded on a worker thread (the UI never waits for a file).
fn file_preview(path: &str) -> PreviewSlot {
    let slot: PreviewSlot = Default::default();
    let (out, path) = (slot.clone(), path.to_string());
    let spawned = std::thread::Builder::new().name("match-look-preview".into()).spawn(move || {
        let read = || {
            let bytes = lightcraft_look::reference::read_file(std::path::Path::new(&path)).ok()?;
            lightcraft_look::reference::decode(&bytes).ok()
        };
        // a decoder bug in one file is that file's problem: no preview, never a crash
        let pic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)).ok().flatten();
        *out.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pic);
    });
    if spawned.is_err() {
        *slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(None);
    }
    slot
}

fn set_reference(app: &mut LightcraftApp, r: Reference) {
    let loading = match &r {
        Reference::File(p) => Some((r.clone(), file_preview(p))),
        _ => None,
    };
    let s = state(app);
    s.open = true;
    s.message = None;
    s.loading = loading;
    s.reference = Some(r);
}

/// A finished background preview becomes the preview texture's picture.
fn take_loaded(s: &mut State, ctx: &egui::Context) {
    let Some((r, slot)) = s.loading.as_ref() else { return };
    let done = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    match done {
        None => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        Some(pic) => {
            if let Some(pic) = pic {
                s.pending_preview = Some((r.clone(), pic));
            }
            s.loading = None;
        }
    }
}

/// The UI commands of Match Look; `None` for any other id.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    match id {
        OPEN => {
            state(app).open = true;
            if let Some(photo) = p.get("photo").and_then(Value::as_u64) {
                set_reference(app, Reference::Photo(PhotoId(photo)));
            } else if let Some(path) = p.get("path").and_then(Value::as_str) {
                set_reference(app, Reference::File(path.to_string()));
            } else if let Some(url) = p.get("url").and_then(Value::as_str) {
                state(app).url = url.to_string();
                set_reference(app, Reference::Url(url.to_string()));
            }
            Some(Ok(Value::Null))
        }
        CHOOSE_FILE => Some(choose_file(app, p)),
        APPLY => {
            if let Some(a) = p.get("amount").and_then(Value::as_f64) {
                state(app).amount = a.clamp(0.0, 100.0) as f32;
            }
            Some(apply(app))
        }
        _ => None,
    }
}

fn choose_file(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let path = match p.get("path").and_then(Value::as_str) {
        Some(path) => Some(path.trim().to_string()),
        None => {
            let req = PickRequest::file(tr("Choose Reference Image…"), tr("Images"), EXTENSIONS);
            match crate::pick::ask(app, CHOOSE_FILE, p, "path", req, |s| s.pick_files.as_mut().map(|f| f())) {
                Picked::Now(paths) => paths.into_iter().next(),
                Picked::Later => return Ok(Value::Null),
                Picked::Unavailable => return Err("no file dialog on this platform; supply path".into()),
            }
        }
    };
    if let Some(path) = path.filter(|p| !p.is_empty()) {
        set_reference(app, Reference::File(path));
    }
    Ok(Value::Null)
}

/// The engine's `reference` parameter for `r`.
fn reference_param(r: &Reference) -> Value {
    match r {
        Reference::Photo(id) => json!({"photo": id.0}),
        Reference::File(p) => json!({"path": p}),
        Reference::Url(u) => json!({"url": u}),
        Reference::Pasted => json!({"pasted": true}),
    }
}

/// Start the match with the window's reference on the selected photos.
fn apply(app: &mut LightcraftApp) -> Result<Value, String> {
    let s = state(app);
    if s.running {
        return Err(tr("A match is already running").to_string());
    }
    let Some(r) = s.reference.clone() else { return Err(tr("Choose a reference first").to_string()) };
    let amount = s.amount as f64;
    let ids: Vec<u64> = app.session.selected().into_iter().filter(|id| Reference::Photo(*id) != r).map(|id| id.0).collect();
    if ids.is_empty() {
        return Err(tr("Select the photos to match").to_string());
    }
    let v = app.session.execute("develop.matchLook", &json!({"ids": ids, "reference": reference_param(&r), "amount": amount, "wait": false}));
    let s = state(app);
    match v {
        Ok(v) => {
            s.running = true;
            s.next_pump = 0.0;
            s.message = Some(tr("Matching…").to_string());
            Ok(v)
        }
        Err(e) => {
            s.message = Some(e.to_string());
            Err(e.to_string())
        }
    }
}

/// Take in a finished match (each frame while one runs).
fn pump(app: &mut LightcraftApp, ctx: &egui::Context, now: f64) {
    let Some(s) = app.ui.match_look.as_mut() else { return };
    if !s.running || now < s.next_pump {
        return;
    }
    s.next_pump = now + PUMP_SECS;
    ctx.request_repaint_after(std::time::Duration::from_secs_f64(PUMP_SECS));
    let Ok(v) = app.session.execute("develop.matchLookPump", &json!({})) else { return };
    if v["active"] == true {
        return;
    }
    let last = &v["last"];
    let done = last["photos"].as_array().map_or(0, Vec::len);
    let failed = last["failed"].as_array().cloned().unwrap_or_default();
    let message = if last["cancelled"] == true {
        tr("Match Look stopped").to_string()
    } else if let Some(e) = failed.first().and_then(|f| f["error"].as_str()) {
        if done == 0 {
            e.to_string()
        } else {
            tr_format!("Matched {done} photo(s); {n} could not be matched: {e}", done = done, n = failed.len(), e = e)
        }
    } else {
        tr_format!("Matched {done} photo(s). Edit ▸ Undo takes it back.", done = done)
    };
    let s = state(app);
    s.running = false;
    s.message = Some(message);
}

/// A pasted image (⌘V while the window is open) becomes the reference.
fn take_paste(app: &mut LightcraftApp, ctx: &egui::Context) {
    let pasted = ctx.input(|i| {
        i.events.iter().rev().find_map(|e| match e {
            egui::Event::PasteImage(img) => Some(img.clone()),
            _ => None,
        })
    });
    let Some(img) = pasted else { return };
    let [w, h] = img.size;
    if let Err(e) = lightcraft_look::reference::check_pixels(w, h) {
        state(app).message = Some(e.to_string());
        return;
    }
    let Some(rgba) = Rgba8::from_pixels(w, h, img.pixels.iter().map(|c| c.to_srgba_unmultiplied())) else { return };
    match app.session.set_match_look_pasted(&rgba) {
        Ok(small) => {
            let s = state(app);
            s.pending_preview = Some((Reference::Pasted, (*small).clone()));
            s.reference = Some(Reference::Pasted);
            s.message = None;
        }
        Err(e) => state(app).message = Some(e),
    }
}

fn preview_texture(ctx: &egui::Context, s: &mut State) {
    let Some((r, img)) = s.pending_preview.take() else { return };
    let size = [img.width, img.height];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, &img.as_bytes());
    let tex = ctx.load_texture("matchLook:preview", color, egui::TextureOptions::LINEAR);
    s.preview = Some((r, tex, size));
}

fn fit(size: [usize; 2], max: f32) -> egui::Vec2 {
    let (w, h) = (size[0].max(1) as f32, size[1].max(1) as f32);
    let k = (max / w.max(h)).min(1.0e4);
    vec2(w * k, h * k)
}

/// The reference, small: a photo's thumbnail, a file's or pasted picture, or the address.
fn draw_reference(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let Some(r) = app.ui.match_look.as_ref().and_then(|s| s.reference.clone()) else {
        ui.label(egui::RichText::new(tr("No reference chosen")).color(t.text_dim));
        return;
    };
    match &r {
        Reference::Photo(id) => {
            crate::panels::grid::request_thumb(app, *id, 256, 10);
            if let Some(tex) = app.renderer.thumb(*id) {
                let size = fit(tex.size, PREVIEW);
                let resp = ui.add(egui::Image::new((tex.id(), size)));
                crate::widgets::register_response("matchLook:reference", resp.rect, &resp);
            }
            let name = app.session.catalog.photo(*id).map(|p| p.file_name.clone()).unwrap_or_default();
            ui.label(egui::RichText::new(name).color(t.text_dim));
        }
        Reference::Url(u) => {
            ui.label(egui::RichText::new(u.as_str()).color(t.text_dim));
        }
        Reference::File(_) | Reference::Pasted => {
            if let Some((shown, tex, size)) = app.ui.match_look.as_ref().and_then(|s| s.preview.clone())
                && shown == r
            {
                let resp = ui.add(egui::Image::new((tex.id(), fit(size, PREVIEW))));
                crate::widgets::register_response("matchLook:reference", resp.rect, &resp);
            }
            let name = match &r {
                Reference::File(p) => std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                _ => tr("Pasted image").to_string(),
            };
            ui.label(egui::RichText::new(name).color(t.text_dim));
        }
    }
}

/// The photos of the view, small, to pick one as the reference.
fn draw_strip(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let ids: Vec<PhotoId> = app.session.visible_cloned().into_iter().take(STRIP_PHOTOS).collect();
    let chosen = app.ui.match_look.as_ref().and_then(|s| s.reference.clone());
    let mut picked = None;
    egui::ScrollArea::horizontal().id_salt("matchLook:strip").max_height(STRIP_THUMB + 8.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            for id in ids {
                crate::panels::grid::request_thumb(app, id, 128, 4);
                let (rect, resp) = ui.allocate_exact_size(vec2(STRIP_THUMB, STRIP_THUMB), Sense::click());
                let p = ui.painter_at(rect);
                if let Some(tex) = app.renderer.thumb(id) {
                    let r = Rect::from_center_size(rect.center(), fit(tex.size, STRIP_THUMB));
                    p.image(tex.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                } else {
                    p.rect_filled(rect, 2.0, t.skeleton);
                }
                if chosen == Some(Reference::Photo(id)) {
                    p.rect_stroke(rect, 2.0, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
                }
                crate::widgets::register_response(format!("matchLook:photo:{}", id.0), rect, &resp);
                if resp.clicked() {
                    picked = Some(id);
                }
            }
        });
    });
    if let Some(id) = picked {
        set_reference(app, Reference::Photo(id));
    }
}

/// Draw the window (and keep a running match going). Called every frame.
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    pump(app, ctx, now);
    if !app.ui.match_look.as_ref().is_some_and(|s| s.open) {
        return;
    }
    take_paste(app, ctx);
    if let Some(s) = app.ui.match_look.as_mut() {
        take_loaded(s, ctx);
        preview_texture(ctx, s);
    }
    let t = Tokens::get(ctx);
    let mut open = true;
    let mut run = false;
    let mut choose = false;
    egui::Window::new(tr("Match Look"))
        .id(egui::Id::unique("matchLook:window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(tr("Reference"));
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(PREVIEW + 8.0);
                    draw_reference(app, ui, &t);
                });
                ui.vertical(|ui| {
                    let r = ui.button(tr("Choose File…"));
                    crate::widgets::register_response("matchLook:chooseFile", r.rect, &r);
                    choose = r.clicked();
                    ui.label(egui::RichText::new(tr("Or paste an image (⌘V)")).color(t.text_dim));
                    let s = state(app);
                    let field = TextField::singleline("matchLook:url", &mut s.url).hint("https://…").width(200.0).show(ui);
                    if field.committed() && !s.url.trim().is_empty() {
                        let u = s.url.trim().to_string();
                        s.reference = Some(Reference::Url(u));
                        s.message = None;
                    }
                });
            });
            ui.add_space(6.0);
            ui.label(tr("Or a photo from this view"));
            draw_strip(app, ui, &t);
            ui.add_space(6.0);
            let s = state(app);
            let slider = ui.add(egui::Slider::new(&mut s.amount, 0.0..=100.0).text(tr("Amount")).step_by(1.0).suffix(" %"));
            crate::widgets::register_response("matchLook:amount", slider.rect, &slider);
            let n = app.session.selected().len();
            ui.label(egui::RichText::new(tr_format!("Applies to {n} selected photo(s)", n = n)).color(t.text_dim));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let running = app.ui.match_look.as_ref().is_some_and(|s| s.running);
                let ready = app.ui.match_look.as_ref().is_some_and(|s| s.reference.is_some()) && n > 0;
                let r = ui.add_enabled(ready && !running, egui::Button::new(tr("Match")));
                crate::widgets::register_response("matchLook:apply", r.rect, &r);
                run = r.clicked();
                if running {
                    ui.spinner();
                }
            });
            if let Some(m) = app.ui.match_look.as_ref().and_then(|s| s.message.clone()) {
                ui.label(egui::RichText::new(m).color(t.text_dim));
            }
        });
    if choose && let Err(e) = choose_file(app, &json!({})) {
        state(app).message = Some(e);
    }
    if run {
        let _ = apply(app);
    }
    if !open {
        let s = state(app);
        s.open = false;
        if !s.running {
            app.ui.match_look = None;
        }
    }
}
