//! Reflection removal in the Remove panel: native background inference, per-photo safety controls.
use crate::{LightcraftApp, i18n::tr};
use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};
pub(crate) const TASK: &str = "Reflection Removal";
const MODELS: [(&str, &str); 6] = [
    ("dirs-mugi", "DIRS-MuGI"),
    ("dirs-pair", "DIRS-PAIR"),
    ("rdnet", "RDNet"),
    ("dsrnet-s", "DSRNet-S"),
    ("dsrnet-l-nature", "DSRNet-L (Nature)"),
    ("dsrnet-l", "DSRNet-L"),
];

fn model_label(id: &str) -> &'static str {
    MODELS.iter().find(|(m, _)| *m == id).map(|(_, name)| *name).unwrap_or("DIRS-MuGI")
}

/// Whether `id` is one of the models that can be downloaded (none on the web, which fetches no models).
#[cfg(not(target_arch = "wasm32"))]
fn downloadable(id: &str) -> bool {
    lightcraft_engine::reflections::dirs::MODELS.contains(&id)
}

#[cfg(target_arch = "wasm32")]
fn downloadable(_id: &str) -> bool {
    false
}

/// Where the files of `model` come from, for the instructions link.
fn model_page(model: &str) -> &'static str {
    #[cfg(not(target_arch = "wasm32"))]
    if downloadable(model) {
        return lightcraft_engine::reflections::dirs::page(model);
    }
    let _ = model;
    "https://github.com/mingcv/DSRNet#trained-weights"
}

pub fn command(app: &mut LightcraftApp, name: &str, p: &Value) -> Result<Value, String> {
    if name == "reflection.instructions" {
        let open = app.services.open_url.as_mut().ok_or("No browser service available")?;
        let model = p.get("model").and_then(Value::as_str).unwrap_or_default();
        open(model_page(model))?;
        return Ok(json!({"opened":true}));
    }
    if name == "reflection.install" {
        return install(app, p);
    }
    if name == "reflection.download" {
        return download(app, p);
    }
    if name == "reflection.models" {
        return Ok(
            json!({"models":MODELS.iter().map(|(id,label)|json!({"id":id,"label":label,"installed":lightcraft_engine::reflections::available_model(id)})).collect::<Vec<_>>() }),
        );
    }
    // Fork feature (fork/glare-highlights.md): mode, area, glare, paint and tool
    if let Some(result) = crate::reflections_glare::command(app, name, p) {
        return result;
    }
    let id = app.session.active().ok_or("No photo selected")?;
    match name {
        "reflection.panel" => {
            app.reflection_open = p.get("open").and_then(Value::as_bool).unwrap_or(!app.reflection_open);
            Ok(json!({"open":app.reflection_open}))
        }
        "reflection.toggle" => {
            if app.tasks.is_running(TASK) {
                return Err(tr("Wait for reflection removal to finish").into());
            }
            let mut d = (*app.session.develop_of(id).ok_or("Photo missing")?).clone();
            let on = p.get("enabled").and_then(Value::as_bool).unwrap_or(!d.reflection.enabled);
            if on
                && (d.reflection.correction.is_empty()
                    || d.reflection.mode != app.reflection_glare.mode
                    || p.get("quality").and_then(Value::as_u64).is_some_and(|q| q != d.reflection.quality as u64)
                    || p.get("model").and_then(Value::as_str).is_some_and(|m| m != d.reflection.model))
            {
                return start(app, id, p);
            }
            d.reflection.enabled = on;
            app.session.set_develop(id, d, "Reflection visibility").map_err(|e| e.to_string())?;
            Ok(json!({"enabled":on}))
        }
        "reflection.reset" => {
            if app.tasks.is_running(TASK) {
                return Err(tr("Wait for reflection removal to finish").into());
            }
            let mut d = (*app.session.develop_of(id).ok_or("Photo missing")?).clone();
            d.reflection = Default::default();
            app.session.set_develop(id, d, "Reset Reflection Removal").map_err(|e| e.to_string())?;
            Ok(json!({"reset":true}))
        }
        "reflection.status" => {
            let d = app.session.develop_of(id).ok_or("Photo missing")?;
            Ok(
                json!({"running":app.tasks.is_running(TASK),"ready":!d.reflection.correction.is_empty(),"settings":d.reflection,"amount":d.reflection.amount,"quality":d.reflection.quality,"enabled":d.reflection.enabled,"panel":crate::reflections_glare::status(app)}),
            )
        }
        "reflection.apply" | "photo.removeReflections" => start(app, id, p),
        _ => Err("Unknown reflection command".into()),
    }
}
#[cfg(not(target_arch = "wasm32"))]
fn install(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    use crate::pick::{PickRequest, Picked};
    let path = match p.get("path").and_then(Value::as_str) {
        Some(path) => path.to_string(),
        None => match crate::pick::ask(app, "reflection.install", p, "path", PickRequest::folder(tr("Install Reflection Model…")), |s| {
            s.pick_folder.as_mut().and_then(|f| f().map(|p| vec![p]))
        }) {
            Picked::Now(paths) => match paths.into_iter().next() {
                Some(p) => p,
                None => return Ok(Value::Null),
            },
            Picked::Later | Picked::Unavailable => return Ok(Value::Null),
        },
    };
    if app.tasks.is_running(TASK) {
        return Err(tr("Wait for reflection removal to finish").into());
    }
    let root = lightcraft_engine::reflections::models_dir().ok_or("No model directory")?;
    crate::tasks::spawn(
        app,
        TASK,
        Some("reflection"),
        move || -> Result<String, String> {
            let source = std::path::Path::new(&path);
            if let Some(model) = lightcraft_engine::reflections::dirs::is_dirs_folder(source) {
                return lightcraft_engine::reflections::dirs::install_folder(model, source, &root);
            }
            let (m, bytes) = lightcraft_reflection_install(source)?;
            let target = root.join(&m);
            if target.exists() {
                return Err("Reflection model already installed".into());
            }
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            // Exclusive directory creation; a failure never replaces another installation.
            std::fs::create_dir(&target).map_err(|e| e.to_string())?;
            let result = (|| {
                std::fs::write(target.join("model.safetensors"), bytes).map_err(|e| e.to_string())?;
                std::fs::copy(source.join("manifest.json"), target.join("manifest.json")).map_err(|e| e.to_string())?;
                Ok(m)
            })();
            if result.is_err() {
                let _ = std::fs::remove_dir_all(target);
            }
            result
        },
        |app, ctx, result| match result {
            Ok(model) => {
                app.reflection_model = model;
                app.toast(ctx, tr("Reflection model installed"));
            }
            Err(e) => app.toast(ctx, format!("{}: {e}", tr("Reflection removal failed"))),
        },
    )?;
    Ok(json!({"background":true}))
}
/// Downloads DIRS-MuGI's two files (about 1.1 GB) from Hugging Face into the models folder. Nothing is fetched
/// unless the caller passes `accepted: true`, which the panel's Download button sets only after its question.
#[cfg(not(target_arch = "wasm32"))]
fn download(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    use lightcraft_engine::reflections::dirs;
    let model = p.get("model").and_then(Value::as_str).unwrap_or(dirs::MODEL).to_string();
    if !dirs::MODELS.contains(&model.as_str()) {
        return Err(format!("{model} cannot be downloaded: only DIRS-MuGI, DIRS-PAIR and RDNet can"));
    }
    if p.get("accepted").and_then(Value::as_bool) != Some(true) {
        return Err(format!(
            "Downloading needs a yes first: {:.1} GB from {}. Run the command again with accepted=true.",
            dirs::download_bytes(&model) as f64 / 1e9,
            dirs::page(&model)
        ));
    }
    if app.tasks.is_running(TASK) {
        return Err(tr("Wait for reflection removal to finish").into());
    }
    let root = lightcraft_engine::reflections::models_dir().ok_or("No model directory")?;
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = cancel.clone();
    crate::tasks::spawn_tracked(
        app,
        TASK,
        "reflection",
        cancel,
        move |handle| {
            handle.detail(model_label(&model));
            dirs::download(&model, &root, &flag, &mut |done, total| handle.progress(done, total))
        },
        |app, ctx, result| match result {
            Ok(model) => {
                app.reflection_model = model;
                app.toast(ctx, tr("Reflection model installed"));
            }
            Err(e) => app.toast(ctx, format!("{}: {e}", tr("Reflection removal failed"))),
        },
    )?;
    Ok(json!({"background":true}))
}
#[cfg(target_arch = "wasm32")]
fn download(_app: &mut LightcraftApp, _p: &Value) -> Result<Value, String> {
    Err(tr("Reflection removal requires the desktop app").into())
}
#[cfg(not(target_arch = "wasm32"))]
fn lightcraft_reflection_install(path: &std::path::Path) -> Result<(String, Vec<u8>), String> {
    lightcraft_engine::reflections::install_check(path)
}
#[cfg(target_arch = "wasm32")]
fn install(_app: &mut LightcraftApp, _p: &Value) -> Result<Value, String> {
    Err(tr("Reflection removal requires the desktop app").into())
}

/// Start the run the panel (and `p`) ask for (fork feature, fork/glare-highlights.md: `reflection.paint` runs it too).
pub(crate) fn run(app: &mut LightcraftApp, id: PhotoId, p: &Value) -> Result<Value, String> {
    start(app, id, p)
}

#[cfg(not(target_arch = "wasm32"))]
fn start(app: &mut LightcraftApp, id: PhotoId, p: &Value) -> Result<Value, String> {
    if app.tasks.is_running(TASK) {
        return Err(tr("Wait for reflection removal to finish").into());
    }
    // Fork feature (fork/glare-highlights.md): mode, area and glare settings, then Highlights or an area run
    crate::reflections_glare::take_params(app, id, p)?;
    let model = p.get("model").and_then(Value::as_str).unwrap_or(&app.reflection_model).to_string();
    let highlights = app.reflection_glare.mode == lightcraft_develop::ReflectionMode::Highlights;
    if !highlights && !lightcraft_engine::reflections::available_model(&model) {
        return Err(tr("Install a reflection model first").into());
    }
    let edge = p.get("quality").and_then(Value::as_u64).unwrap_or(app.reflection_quality as u64);
    if !matches!(edge, 384 | 768 | 1024) {
        return Err("Invalid reflection quality".into());
    }
    if let Some(result) = crate::reflections_glare::start(app, id, edge as u32, &model) {
        if !highlights {
            app.reflection_model = model;
            app.reflection_quality = edge as u32;
        }
        return result;
    }
    let job = app.session.render_job(id, edge as usize, edge as usize, false, false).ok_or("Photo cannot be loaded")?;
    app.reflection_model = model.clone();
    app.reflection_quality = edge as u32;
    let generation = app.session.library_generation();
    let key = job.source_key;
    let identity = lightcraft_engine::reflections::identity(key);
    crate::tasks::spawn(
        app,
        TASK,
        Some("reflection"),
        move || lightcraft_engine::reflections::make(job.source, key, edge as u32, &model),
        move |app, ctx, result| {
            if generation != app.session.library_generation() {
                return;
            }
            match result {
                Ok(mut correction) => {
                    let current = app.session.render_job(id, 384, 384, false, false).map(|j| lightcraft_engine::reflections::identity(j.source_key));
                    if current.as_deref() != Some(&identity) {
                        app.toast(ctx, tr("The photo changed; calculate reflections again"));
                        return;
                    }
                    let Some(d) = app.session.develop_of(id) else {
                        return;
                    };
                    let mut d = (*d).clone();
                    // Recalculation keeps the user's chosen strength and protections.
                    if !d.reflection.correction.is_empty() && !d.reflection.model.is_empty() {
                        correction.amount = d.reflection.amount;
                        correction.limit_region = d.reflection.limit_region;
                        correction.left = d.reflection.left;
                        correction.top = d.reflection.top;
                        correction.right = d.reflection.right;
                        correction.bottom = d.reflection.bottom;
                        correction.protect_sky = d.reflection.protect_sky;
                        correction.detail_protection = d.reflection.detail_protection;
                    }
                    d.reflection = correction;
                    match app.session.set_develop(id, d, "Remove Reflections") {
                        Ok(()) => app.toast(ctx, tr("Reflections calculated; adjust Amount on the right")),
                        Err(e) => app.toast(ctx, e.to_string()),
                    }
                }
                Err(e) => app.toast(ctx, format!("{}: {e}", tr("Reflection removal failed"))),
            }
            crate::reflections_glare::after_run(app);
            ctx.request_repaint();
        },
    )?;
    Ok(json!({"background":true,"photo":id.0,"quality":edge}))
}
#[cfg(target_arch = "wasm32")]
fn start(_app: &mut LightcraftApp, _id: PhotoId, _p: &Value) -> Result<Value, String> {
    Err(tr("Reflection removal requires the desktop app").into())
}

pub fn panel(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    use crate::widgets::register;
    crate::widgets::divider(ui);
    crate::panels::right::header(ui, "Distraction Removal");
    if crate::widgets::flyout_row(ui, "reflections", "Reflections", crate::icons::Icon::Reflection, app.reflection_open).clicked() {
        let _ = app.run("reflection.panel", json!({}));
    }
    if !app.reflection_open {
        return;
    }
    let busy = app.tasks.is_running(TASK);
    let d = app.session.develop_of(id).unwrap_or_default();
    let ready = !d.reflection.correction.is_empty();
    if app.reflection_photo != Some(id) {
        app.reflection_photo = Some(id);
        app.reflection_quality = if matches!(d.reflection.quality, 384 | 768 | 1024) { d.reflection.quality } else { 768 };
        app.reflection_model = if lightcraft_engine::reflections::valid_model(&d.reflection.model) {
            d.reflection.model.clone()
        } else if let Some(id) = ["dirs-mugi", "dirs-pair", "rdnet", "dsrnet-l-nature", "dsrnet-s"]
            .into_iter()
            .find(|id| lightcraft_engine::reflections::available_model(id))
        {
            id.into()
        } else {
            "dirs-mugi".into()
        };
        crate::reflections_glare::load(app, &d.reflection);
    }
    let highlights = app.reflection_glare.mode == lightcraft_develop::ReflectionMode::Highlights;
    let available = highlights || lightcraft_engine::reflections::available_model(&app.reflection_model);
    let pad = egui::Margin { left: 24, right: 22, top: 4, bottom: 8 };
    egui::Frame::NONE.inner_margin(pad).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let mut on = d.reflection.enabled;
        let r = ui.add_enabled(!busy && (available || on), egui::Checkbox::new(&mut on, tr("Apply")));
        register(ui, "reflection:enabled", r.rect);
        register(ui, "reflection:apply", r.rect);
        if r.changed()
            && let Err(e) = app.run("reflection.toggle", json!({"enabled":on,"quality":app.reflection_quality,"model":app.reflection_model}))
        {
            app.toast(ui.ctx(), e);
        }
        crate::reflections_glare::mode_row(app, ui, busy);
        crate::reflections_glare::area_row(app, ui, &d, busy);
        if highlights {
            crate::reflections_glare::highlights_rows(app, ui, busy);
        }
        if !highlights {
            ui.horizontal(|ui| {
                ui.add_sized([58.0, 22.0], egui::Label::new(tr("Model")));
                let label = model_label(&app.reflection_model);
                // Installed models are listed; the ones that can be downloaded follow, each opening its Download button.
                let r = crate::widgets::combo_box("reflection-model").selected_text(label).show_ui(ui, |ui| {
                    for (id, name) in MODELS {
                        if id == app.reflection_model || lightcraft_engine::reflections::available_model(id) {
                            let r = ui.selectable_value(&mut app.reflection_model, id.into(), name);
                            register(ui, format!("reflection:model:{id}"), r.rect);
                        }
                    }
                    let selected = app.reflection_model.clone();
                    let missing = |id: &str| id != selected && downloadable(id) && !lightcraft_engine::reflections::available_model(id);
                    if MODELS.iter().any(|(id, _)| missing(id)) {
                        ui.separator();
                    }
                    for (id, name) in MODELS {
                        if missing(id) {
                            let r = ui.selectable_value(&mut app.reflection_model, id.into(), format!("{} {name}…", tr("Download")));
                            register(ui, format!("reflection:model:{id}"), r.rect);
                        }
                    }
                });
                crate::widgets::name_combo(&r.response, tr("Model"));
                register(ui, "reflection:model", r.response.rect);
            });
            ui.horizontal(|ui| {
                ui.add_sized([58.0, 22.0], egui::Label::new(tr("Quality")));
                let name = match app.reflection_quality {
                    384 => "Preview",
                    1024 => "High",
                    _ => "Standard",
                };
                let r = crate::widgets::combo_box("reflection-quality").selected_text(tr(name)).show_ui(ui, |ui| {
                    for (q, name) in [(384, "Preview"), (768, "Standard"), (1024, "High")] {
                        let r = ui.selectable_value(&mut app.reflection_quality, q, tr(name));
                        register(ui, format!("reflection:quality:{q}"), r.rect);
                    }
                });
                crate::widgets::name_combo(&r.response, tr("Quality"));
                register(ui, "reflection:quality", r.response.rect);
            });
        }
        if !available {
            ui.add(egui::Label::new(tr("Install a reflection model first")).wrap());
            if downloadable(&app.reflection_model) {
                download_row(app, ui, busy);
            }
            let r = ui.add_enabled(!busy, egui::Button::new(tr("Install Reflection Model…")));
            register(ui, "reflection:install", r.rect);
            if r.clicked() {
                let _ = app.run("reflection.install", json!({}));
            }
            let r = ui.link(tr("Model download instructions"));
            register(ui, "reflection:instructions", r.rect);
            if r.clicked()
                && let Err(e) = app.run("reflection.instructions", json!({"model":app.reflection_model}))
            {
                app.toast(ui.ctx(), e);
            }
        }
        if ready || highlights {
            let label = if ready { "Recalculate" } else { "Find Glare" };
            let r = ui.add_enabled(!busy && available, egui::Button::new(tr(label)));
            register(ui, "reflection:recompute", r.rect);
            if r.clicked()
                && let Err(e) = app.run("reflection.apply", json!({"quality":app.reflection_quality,"model":app.reflection_model}))
            {
                app.toast(ui.ctx(), e);
            }
        }
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Analyzing reflections…"));
            });
        }
    });
    controls(app, ui, &d, "reflection.amount", ready && !busy);
    if !highlights {
        egui::Frame::NONE.inner_margin(pad).show(ui, |ui| {
            let mut sky = d.reflection.protect_sky;
            let r = ui.add_enabled(ready && !busy, egui::Checkbox::new(&mut sky, tr("Protect sky (estimate)")));
            register(ui, "reflection:protectSky", r.rect);
            if r.on_hover_text(tr("Uses an embedded sky matte or an estimate. Limit the area for exact protection.")).changed() {
                let _ = app.run("develop.merge", json!({"settings":{"reflection":{"protect_sky":sky}},"label":"Reflection Sky Protection"}));
            }
        });
        controls(app, ui, &d, "reflection.detailProtection", ready && !busy);
        egui::Frame::NONE.inner_margin(pad).show(ui, |ui| {
            let mut limited = d.reflection.limit_region;
            let r = ui.add_enabled(ready && !busy, egui::Checkbox::new(&mut limited, tr("Limit to area")));
            register(ui, "reflection:limitRegion", r.rect);
            if r.changed() {
                let _ = app.run("develop.merge", json!({"settings":{"reflection":{"limit_region":limited}},"label":"Reflection Area"}));
            }
            if limited {
                ui.add(egui::Label::new(tr("Percent of the original image, before crop or rotation. Outside this area stays unchanged.")).wrap());
            }
        });
        if d.reflection.limit_region {
            for name in ["reflection.left", "reflection.top", "reflection.right", "reflection.bottom"] {
                controls(app, ui, &d, name, ready && !busy);
            }
        }
    }
    egui::Frame::NONE.inner_margin(pad).show(ui, |ui| {
        let r = ui.add_enabled(ready && !busy, egui::Button::new(tr("Reset")));
        register(ui, "reflection:reset", r.rect);
        if r.clicked() {
            let _ = app.run("reflection.reset", json!({}));
        }
        ui.add_space(10.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(tr("Local AI · experimental. Start gently and compare clouds and fine details."))
                    .size(11.0)
                    .color(crate::theme::Tokens::get(ui.ctx()).text_dim),
            )
            .wrap(),
        );
    });
}
/// The Download button, and the question it asks first: nothing is fetched until the second click.
#[cfg(not(target_arch = "wasm32"))]
fn download_row(app: &mut LightcraftApp, ui: &mut egui::Ui, busy: bool) {
    use crate::widgets::register;
    use lightcraft_engine::reflections::dirs;
    let asking = egui::Id::unique("reflection-download-asking");
    if !ui.data(|d| d.get_temp::<bool>(asking).unwrap_or(false)) {
        let r = ui.add_enabled(!busy, egui::Button::new(format!("{}…", tr("Download"))));
        register(ui, "reflection:download", r.rect);
        if r.clicked() {
            ui.data_mut(|d| d.insert_temp(asking, true));
        }
        return;
    }
    let model = app.reflection_model.clone();
    ui.add(egui::Label::new(format!("{} · {:.1} GB · huggingface.co", model_label(&model), dirs::download_bytes(&model) as f64 / 1e9)).wrap());
    ui.horizontal(|ui| {
        let yes = ui.add_enabled(!busy, egui::Button::new(tr("Download")));
        register(ui, "reflection:download:yes", yes.rect);
        let no = ui.button(tr("Cancel"));
        register(ui, "reflection:download:no", no.rect);
        if yes.clicked() || no.clicked() {
            ui.data_mut(|d| d.insert_temp(asking, false));
        }
        if yes.clicked()
            && let Err(e) = app.run("reflection.download", json!({"accepted":true,"model":model}))
        {
            app.toast(ui.ctx(), e);
        }
    });
}
#[cfg(target_arch = "wasm32")]
fn download_row(_app: &mut LightcraftApp, _ui: &mut egui::Ui, _busy: bool) {}

fn controls(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &lightcraft_develop::DevelopSettings, name: &str, enabled: bool) {
    crate::panels::edit::control(app, ui, d, name, enabled);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloading_a_model_needs_a_yes_first_and_starts_nothing_without_it() {
        let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Default::default());
        for p in [json!({}), json!({"accepted":false}), json!({"accepted":"yes"})] {
            let e = app.run("reflection.download", p).unwrap_err();
            assert!(e.contains("needs a yes first") && e.contains("huggingface.co"), "{e}");
        }
        assert!(!app.tasks.is_running(TASK));
    }

    #[test]
    fn dirs_instructions_open_the_hugging_face_page() {
        let opened = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let captured = opened.clone();
        let services = crate::Services {
            open_url: Some(Box::new(move |url| {
                *captured.lock().unwrap() = url.into();
                Ok(())
            })),
            ..Default::default()
        };
        let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
        app.run("reflection.instructions", json!({"model":"dirs-mugi"})).unwrap();
        assert_eq!(*opened.lock().unwrap(), "https://huggingface.co/huqiming513/DIRS-Models");
    }

    #[test]
    fn reflection_messages_exist_in_every_shipped_language() {
        let messages = [
            "Distraction Removal",
            "Reflections",
            "Remove Reflections",
            "Install Reflection Model…",
            "Install a reflection model first",
            "Model download instructions",
            "Reflection model installed",
            "Analyzing reflections…",
            "Protect sky (estimate)",
            "Uses an embedded sky matte or an estimate. Limit the area for exact protection.",
            "Detail Protection",
            "Limit to area",
            "Percent of the original image, before crop or rotation. Outside this area stays unchanged.",
            "Local AI · experimental. Start gently and compare clouds and fine details.",
            "Wait for reflection removal to finish",
            "Reflection removal requires the desktop app",
            "The photo changed; calculate reflections again",
            "Reflections calculated; adjust Amount on the right",
            "Recalculate",
            "Reflection removal failed",
        ];
        for locale in crate::i18n::Locale::ALL.iter().filter(|locale| **locale != crate::i18n::Locale::En) {
            for source in messages {
                assert_ne!(locale.tr(source), source, "missing {} translation: {source}", locale.code());
            }
        }
    }
}
