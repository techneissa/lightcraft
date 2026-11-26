//! Fork feature (`fork/glare-highlights.md`, issues #73 and #74): the Reflections section's mode, area and glare.
//!
//! - **Mode**: Reflections (a model separates a reflection in glass) or Highlights (glare is found and filled).
//! - **Area**: the whole photo, boxes drawn on the photo (`crate::reflections_boxes`), or a mask of the Masks panel.
//!   A model runs on each area at the panel's Quality; Highlights looks for glare only inside them.
//! - **Highlights**: Threshold and Expand for the finder, the fill (LaMa or Classic), and strokes painted on the photo
//!   to add to or erase from the found glare (each stroke recalculates).
//!
//! The panel keeps these as what the next run uses; a finished run saves them with its result, and opening a photo
//! (or undo) takes them back from it. Commands: `reflection.mode`, `reflection.area`, `reflection.box`,
//! `reflection.glare`, `reflection.paint`, `reflection.tool`, `reflection.boxTool`, and their parameters on
//! `reflection.apply`. Coordinates are percent of the original photo before
//! crop or rotation.

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, Glare, GlareStroke, Reflection, ReflectionArea, ReflectionMode, SpotFill};
use lightcraft_geom::Point;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::tr;
use crate::panels::detail::geometry::CanvasMap;

/// What the photo's tool does while the Reflections section uses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Box,
    Paint,
    Erase,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::Box => "box",
            Tool::Paint => "paint",
            Tool::Erase => "erase",
        }
    }
}

/// What the next run works on.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Area {
    #[default]
    Whole,
    /// Any number of boxes, none yet while the first is still to be drawn.
    Boxes(Vec<ReflectionArea>),
    /// A mask of the Masks panel (its id).
    Mask(u32),
}

impl Area {
    /// The area of a saved result's list.
    pub fn of(list: &[ReflectionArea]) -> Area {
        match list {
            [] => Area::Whole,
            [ReflectionArea { mask: Some(id), .. }] => Area::Mask(*id),
            boxes => Area::Boxes(boxes.iter().filter(|a| a.mask.is_none()).cloned().collect()),
        }
    }

    /// The list a result saves.
    pub fn list(&self) -> Vec<ReflectionArea> {
        match self {
            Area::Whole => Vec::new(),
            Area::Boxes(boxes) => boxes.clone(),
            Area::Mask(id) => vec![ReflectionArea { mask: Some(*id), ..Default::default() }],
        }
    }

    pub fn boxes(&self) -> &[ReflectionArea] {
        match self {
            Area::Boxes(boxes) => boxes,
            _ => &[],
        }
    }
}

/// The panel's choices for the next run, and the tool on the photo.
#[derive(Clone, Debug, Default)]
pub struct GlarePanel {
    pub mode: ReflectionMode,
    pub area: Area,
    pub glare: Glare,
    pub tool: Option<Tool>,
    /// The box picked on the photo or in the list.
    pub selected: Option<usize>,
    /// The drag in progress: points as fractions of the source (a stroke).
    pub drag: Vec<Point>,
    /// A box being drawn, moved or resized.
    pub box_drag: Option<crate::reflections_boxes::Drag>,
    /// A stroke or a box change on this photo came while a run was going: run again when it ends, if the photo is
    /// still the one shown.
    pub rerun: Option<PhotoId>,
    /// The boxes changed since the last run began: a run that then finds no glare leaves none filled.
    pub boxes_changed: bool,
    /// The photo these choices belong to.
    pub photo: Option<PhotoId>,
    /// The areas and strokes of that photo's saved result, as last seen: a change (undo, redo, reset, paste) is taken
    /// back into the panel.
    pub saved: (Vec<ReflectionArea>, Vec<GlareStroke>),
    /// The Remove tool chosen when the tool on the photo was picked up: choosing another one puts it down.
    held_with: String,
    /// The panels of photos left while an edit of theirs still waited for its run.
    waiting: Vec<GlarePanel>,
}

/// Take the panel's choices back from a photo's saved result.
pub fn load(app: &mut LightcraftApp, r: &Reflection) {
    put_down(app);
    let p = &mut app.reflection_glare;
    p.drag.clear();
    // a box change still waiting for its run belonged to the photo left
    p.boxes_changed = false;
    p.mode = r.mode;
    p.area = Area::of(&r.area);
    p.glare = r.glare.clone().sanitized();
    p.saved = (r.area.clone(), r.glare.paint.clone());
}

/// Pick up `tool` on the photo.
pub fn hold(app: &mut LightcraftApp, tool: Tool) {
    app.reflection_open = true;
    app.session.active_spot = None;
    let p = &mut app.reflection_glare;
    p.held_with = app.ui.tool.clone();
    p.tool = Some(tool);
    p.drag.clear();
    p.box_drag = None;
    if tool != Tool::Box {
        p.selected = None;
    } else if p.area == Area::Whole {
        // (a mask stays until the first box is drawn)
        p.area = Area::Boxes(Area::of(&p.saved.0).boxes().to_vec());
    }
}

/// Put the tool down and deselect its box; whether there was anything to.
pub fn put_down(app: &mut LightcraftApp) -> bool {
    let p = &mut app.reflection_glare;
    let had = p.tool.is_some() || p.selected.is_some();
    p.tool = None;
    p.selected = None;
    p.box_drag = None;
    p.drag.clear();
    had
}

/// Put the tool down once it is no longer what the user works with: another view or panel, the section closed, another
/// Remove tool or a spot picked.
pub fn release_if_hidden(app: &mut LightcraftApp) {
    let p = &app.reflection_glare;
    if p.tool.is_some()
        && (app.ui.view != crate::state::ViewMode::Detail
            || app.ui.right != crate::state::RightPanel::Remove
            || !app.reflection_open
            || app.ui.tool != p.held_with
            || app.session.active_spot.is_some())
    {
        put_down(app);
    }
}

/// Make the panel's choices the photo `id`'s, when they belong to another photo (or none): a photo never runs with
/// another photo's mode, box or strokes, whether the section was open when the photo changed or not.
pub fn sync(app: &mut LightcraftApp, id: PhotoId) {
    let r = app.session.develop_of(id).map(|d| d.reflection.clone()).unwrap_or_default();
    if app.reflection_glare.photo != Some(id) {
        let p = &mut app.reflection_glare;
        // an edit of the photo left that still waits for its run keeps its panel
        if p.rerun.is_some() && p.rerun == p.photo {
            p.rerun = None;
            let old = GlarePanel { waiting: Vec::new(), ..p.clone() };
            p.waiting.push(old);
        }
        if let Some(i) = p.waiting.iter().position(|w| w.photo == Some(id)) {
            let back = p.waiting.remove(i);
            let waiting = std::mem::take(&mut p.waiting);
            *p = GlarePanel { waiting, rerun: Some(id), ..back };
            put_down(app);
        } else {
            load(app, &r);
            app.reflection_glare.photo = Some(id);
        }
        return;
    }
    let p = &mut app.reflection_glare;
    if p.saved.0 == r.area && p.saved.1 == r.glare.paint {
        return;
    }
    // the boxes may be others now: the selection (and with it the arrows) lets go
    if p.saved.0 != r.area {
        p.selected = None;
    }
    p.saved = (r.area.clone(), r.glare.paint.clone());
    p.glare.paint = r.glare.clone().sanitized().paint;
    // no boxes saved while boxes are being worked on: still boxes, none of them
    p.area = match (&p.area, Area::of(&r.area)) {
        (Area::Boxes(_), Area::Whole) => Area::Boxes(Vec::new()),
        (_, area) => area,
    };
    p.box_drag = None;
}

/// What this app saves is not an outside change: the panel of `id`, shown or waiting, keeps its newer choices.
fn mark_saved(app: &mut LightcraftApp, id: PhotoId, r: &Reflection) {
    let seen = (r.area.clone(), r.glare.paint.clone());
    let p = &mut app.reflection_glare;
    for waiting in p.waiting.iter_mut().filter(|w| w.photo == Some(id)) {
        waiting.saved = seen.clone();
    }
    if p.photo == Some(id) {
        p.saved = seen;
    }
}

fn area_json(a: &Area) -> Value {
    match a {
        Area::Whole => json!("whole"),
        Area::Boxes(boxes) => serde_json::to_value(boxes).unwrap_or(Value::Null),
        Area::Mask(id) => json!({"mask": id}),
    }
}

/// The panel's state, for `reflection.status`.
pub fn status(app: &LightcraftApp) -> Value {
    let p = &app.reflection_glare;
    json!({"mode": p.mode.name(), "area": area_json(&p.area), "selected": p.selected, "glare": p.glare, "tool": p.tool.map(Tool::name)})
}

/// A box from `{left, top, right, bottom}` (percent).
pub(crate) fn parse_box(v: &Value) -> Result<ReflectionArea, String> {
    let n = |k: &str| v.get(k).and_then(Value::as_f64).ok_or(format!("area needs left, top, right and bottom (percent) or mask: {k} is missing"));
    let a = ReflectionArea::boxed(n("left")?, n("top")?, n("right")?, n("bottom")?);
    a.ok_or_else(|| "area: each side 0–100, left below right and top below bottom".into())
}

/// An area from a command's `area` value: `"whole"` or null, `"boxes"` (the ones in the panel, else the saved
/// result's), one box or a list of them, or `{mask: id}`.
fn parse_area(v: &Value, current: &Area, saved: &[ReflectionArea], d: &DevelopSettings) -> Result<Area, String> {
    if v.is_null() || v.as_str() == Some("whole") || v.get("whole").and_then(Value::as_bool) == Some(true) {
        return Ok(Area::Whole);
    }
    if v.as_str() == Some("boxes") {
        let boxes = match current {
            Area::Boxes(boxes) => boxes.clone(),
            _ => Area::of(saved).boxes().to_vec(),
        };
        return Ok(Area::Boxes(boxes));
    }
    if let Some(id) = v.get("mask") {
        let id = id.as_u64().and_then(|i| u32::try_from(i).ok()).ok_or("mask must be a mask id")?;
        if !d.masks.iter().any(|m| m.id == id) {
            return Err(format!("No mask {id} on this photo"));
        }
        return Ok(Area::Mask(id));
    }
    let list = v.as_array().map_or(std::slice::from_ref(v), Vec::as_slice);
    if list.len() > lightcraft_develop::reflection_area::MAX_AREAS {
        return Err("Too many boxes".into());
    }
    list.iter().map(parse_box).collect::<Result<_, _>>().map(Area::Boxes)
}

/// Apply a command's `mode`, `area`, `threshold`, `expand` and `fill` to the panel.
pub fn take_params(app: &mut LightcraftApp, id: PhotoId, p: &Value) -> Result<(), String> {
    sync(app, id);
    if let Some(m) = p.get("mode") {
        let m = m.as_str().and_then(ReflectionMode::from_name).ok_or("mode is reflections or highlights")?;
        app.reflection_glare.mode = m;
    }
    if let Some(a) = p.get("area") {
        let d = app.session.develop_of(id).ok_or("Photo missing")?;
        let area = parse_area(a, &app.reflection_glare.area, &app.reflection_glare.saved.0, &d)?;
        if area != app.reflection_glare.area {
            app.reflection_glare.selected = None;
            app.reflection_glare.area = area;
        }
    }
    let g = &mut app.reflection_glare.glare;
    for (key, slot) in [("threshold", &mut g.threshold), ("expand", &mut g.expand)] {
        if let Some(v) = p.get(key) {
            let v = v.as_f64().filter(|v| v.is_finite() && (0.0..=100.0).contains(v)).ok_or(format!("{key} is a number 0–100"))?;
            *slot = v;
        }
    }
    if let Some(f) = p.get("fill") {
        g.fill = match f.as_str() {
            Some("lama") => SpotFill::Lama,
            Some("classic") => SpotFill::Classic,
            _ => return Err("fill is lama or classic".into()),
        };
    }
    Ok(())
}

/// The commands of this module; `None` for another name.
pub fn command(app: &mut LightcraftApp, name: &str, p: &Value) -> Option<Result<Value, String>> {
    let id = match app.session.active() {
        Some(id) => id,
        None => {
            return matches!(
                name,
                "reflection.mode" | "reflection.area" | "reflection.box" | "reflection.glare" | "reflection.paint" | "reflection.tool"
            )
            .then(|| Err("No photo selected".into()));
        }
    };
    // every reflection command sees the shown photo's choices
    sync(app, id);
    let result = match name {
        "reflection.mode" | "reflection.area" | "reflection.glare" => {
            let key = name.trim_start_matches("reflection.");
            let params = if key == "glare" || p.get(key).is_some() { p.clone() } else { json!({ key: p.clone() }) };
            take_params(app, id, &params).map(|_| status(app))
        }
        "reflection.tool" => match p.get("tool").and_then(Value::as_str) {
            Some("box") => Ok(Some(Tool::Box)),
            Some("paint") => Ok(Some(Tool::Paint)),
            Some("erase") => Ok(Some(Tool::Erase)),
            Some("none") | None => Ok(None),
            Some(other) => Err(format!("Unknown tool {other}: box, paint, erase or none")),
        }
        .map(|t| {
            match t {
                Some(t) => hold(app, t),
                None => {
                    put_down(app);
                }
            }
            status(app)
        }),
        "reflection.box" => crate::reflections_boxes::command(app, id, p),
        "reflection.paint" => paint(app, id, p),
        _ => return None,
    };
    Some(result)
}

/// `reflection.paint`: add a stroke (`points` [[x, y], …] in percent of the photo, `size` the radius in percent of its
/// long edge, `erase`), or `clear` them all; in Highlights mode the glare is found and filled again.
fn paint(app: &mut LightcraftApp, id: PhotoId, p: &Value) -> Result<Value, String> {
    if p.get("clear").and_then(Value::as_bool) == Some(true) {
        app.reflection_glare.glare.paint.clear();
    } else {
        let points: Vec<Point> = p
            .get("points")
            .and_then(Value::as_array)
            .ok_or("points: [[x, y], …] in percent of the photo")?
            .iter()
            .filter_map(|q| Some(Point::new(q.get(0)?.as_f64()? / 100.0, q.get(1)?.as_f64()? / 100.0)))
            .collect();
        let size = p.get("size").and_then(Value::as_f64).map(|s| s / 100.0).unwrap_or(app.ui.remove_size as f64);
        let erase = p.get("erase").and_then(Value::as_bool).unwrap_or(false);
        let stroke = GlareStroke { erase, size, points }.sanitized().ok_or("The stroke has no usable points")?;
        let strokes = &mut app.reflection_glare.glare.paint;
        if strokes.len() >= lightcraft_develop::reflection_area::MAX_GLARE_STROKES {
            return Err("Too many strokes: clear them first".into());
        }
        strokes.push(stroke);
    }
    app.reflection_glare.mode = ReflectionMode::Highlights;
    let running = app.tasks.is_running(crate::reflections::TASK);
    if running {
        app.reflection_glare.rerun = Some(id);
    } else {
        crate::reflections::run(app, id, &json!({}))?;
    }
    Ok(json!({"strokes": app.reflection_glare.glare.paint.len(), "background": true}))
}

/// After the boxes changed: find and fill the glare again in Highlights mode (a reflection model runs only when asked),
/// or, when no box is left, take away what the boxes' result changed.
pub fn boxes_changed(app: &mut LightcraftApp, id: PhotoId) -> Result<(), String> {
    // the web build runs neither: the panel says so once
    if cfg!(target_arch = "wasm32") {
        return Ok(());
    }
    if app.reflection_glare.mode != ReflectionMode::Highlights && app.reflection_glare.area != Area::Boxes(Vec::new()) {
        // nothing waits to run for a box that only the model uses
        app.reflection_glare.rerun = app.reflection_glare.rerun.filter(|waiting| *waiting != id);
        return Ok(());
    }
    app.reflection_glare.boxes_changed = true;
    if app.tasks.is_running(crate::reflections::TASK) {
        app.reflection_glare.rerun = Some(id);
        Ok(())
    } else if app.reflection_glare.area == Area::Boxes(Vec::new()) {
        let glare = app.reflection_glare.glare.clone();
        clear_boxes(app, id, glare)
    } else {
        crate::reflections::run(app, id, &json!({})).map(|_| ())
    }
}

/// No box left: what the boxes' result changed comes back, as one undo step. The rest of its choices stay.
fn clear_boxes(app: &mut LightcraftApp, id: PhotoId, glare: Glare) -> Result<(), String> {
    app.reflection_glare.boxes_changed = false;
    let mut d = (*app.session.develop_of(id).ok_or("Photo missing")?).clone();
    // only a result made with boxes: a whole-photo or mask result stays
    if Area::of(&d.reflection.area).boxes().is_empty() {
        return Ok(());
    }
    let r = &mut d.reflection;
    r.area.clear();
    r.correction.clear();
    r.source_key.clear();
    r.enabled = false;
    if r.mode == ReflectionMode::Highlights {
        r.glare = glare;
    }
    if r.amount == 0.0 {
        r.amount = 100.0;
    }
    mark_saved(app, id, &d.reflection);
    app.session.set_develop(id, d, "Remove Glare Box").map_err(|e| e.to_string())
}

/// After a run: start the one a stroke or a box asked for while it went, for the photo it was asked on, shown or not.
pub fn after_run(app: &mut LightcraftApp) {
    loop {
        let p = &mut app.reflection_glare;
        let next = match p.rerun.take() {
            Some(id) if p.photo == Some(id) => {
                let panel = GlarePanel { waiting: Vec::new(), ..p.clone() };
                p.boxes_changed = false;
                Some(panel)
            }
            _ => p.waiting.pop(),
        };
        let Some(panel) = next else { return };
        match run_waiting(app, panel) {
            // one run at a time: the others wait for this one
            Ok(true) => return,
            Ok(false) => {}
            Err(e) => log::warn!("reflection rerun: {e}"),
        }
    }
}

/// Run what `panel` waited for, on its own photo; whether a run started.
fn run_waiting(app: &mut LightcraftApp, panel: GlarePanel) -> Result<bool, String> {
    let Some(id) = panel.photo else { return Ok(false) };
    if panel.area == Area::Boxes(Vec::new()) {
        return clear_boxes(app, id, panel.glare).map(|()| false);
    }
    // a reflection model runs only when asked
    if panel.mode != ReflectionMode::Highlights {
        return Ok(false);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (edge, model) = (app.reflection_quality, app.reflection_model.clone());
        start_job(app, id, edge, model, panel).map(|_| true)
    }
    #[cfg(target_arch = "wasm32")]
    Ok(false)
}

/// Another library was opened: what waited belonged to the old one's photos.
pub fn forget_waiting(app: &mut LightcraftApp) {
    app.reflection_glare.rerun = None;
    app.reflection_glare.waiting.clear();
}

/// What a recalculation keeps of the photo's earlier result `old`: Amount, Protect Sky and Detail Protection when it
/// is of the same kind, and Limit to Area only for a whole-photo result (an area is its own limit, and an old limit
/// would keep part of the new area unchanged).
pub fn keep_choices(old: &Reflection, made: &mut Reflection) {
    // (a result whose boxes were all removed has no correction, but its choices)
    if (old.correction.is_empty() && old.amount == 0.0) || old.mode != made.mode {
        return;
    }
    made.amount = old.amount;
    made.protect_sky = old.protect_sky;
    made.detail_protection = old.detail_protection;
    if made.area.is_empty() {
        made.limit_region = old.limit_region;
        (made.left, made.top, made.right, made.bottom) = (old.left, old.top, old.right, old.bottom);
    } else {
        made.limit_region = false;
    }
}

// ------------------------------------------------------------------------ runs

/// Start the run the panel asks for, when it is not the whole-photo reflection model (that one stays in
/// `crate::reflections`). `None`: not ours.
#[cfg(not(target_arch = "wasm32"))]
pub fn start(app: &mut LightcraftApp, id: PhotoId, edge: u32, model: &str) -> Option<Result<Value, String>> {
    let panel = app.reflection_glare.clone();
    if panel.mode == ReflectionMode::Reflections && panel.area == Area::Whole {
        return None;
    }
    if panel.area == Area::Boxes(Vec::new()) {
        return Some(Err(tr("Draw a box on the photo first").into()));
    }
    app.reflection_glare.boxes_changed = false;
    Some(start_job(app, id, edge, model.to_string(), panel))
}

#[cfg(not(target_arch = "wasm32"))]
fn start_job(app: &mut LightcraftApp, id: PhotoId, edge: u32, model: String, panel: GlarePanel) -> Result<Value, String> {
    use lightcraft_engine::reflections::{area, highlights};
    use std::sync::atomic::AtomicBool;
    // the whole photo at its own size: an area keeps all its detail
    let job = app.session.render_job(id, 1 << 15, 1 << 15, false, false).ok_or("Photo cannot be loaded")?;
    let lama_installed = app.session.lama.installed();
    let learned = (panel.glare.fill == SpotFill::Lama && lama_installed).then(lightcraft_pipeline::learned_fill::current).flatten();
    let generation = app.session.library_generation();
    let key = job.source_key;
    let identity = lightcraft_engine::reflections::identity(key);
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let highlights = panel.mode == ReflectionMode::Highlights;
    let boxes_changed = panel.boxes_changed;
    crate::tasks::spawn_tracked(
        app,
        crate::reflections::TASK,
        "reflection",
        cancel,
        move |handle| -> Result<(Reflection, Option<String>), String> {
            handle.detail(if highlights { "Finding glare" } else { "Area" });
            let settings = job.settings.clone();
            let source = job.source.load_source()?;
            let src = source.image_for(settings.denoise_amount());
            let info = source.info_or(job.info.clone());
            let areas = panel.area.list();
            let mut regions = areas.iter().map(|a| area::resolve(a, &settings, &src, info.mattes.as_deref())).collect::<Result<Vec<_>, _>>()?;
            if regions.is_empty() {
                regions.push(area::Region::whole());
            }
            if highlights {
                let made = match highlights::make(&src, &panel.glare, &regions, learned, &flag) {
                    // the changed boxes hold no glare: keep the boxes, fill nothing
                    Err(e) if boxes_changed && e == highlights::NO_GLARE => {
                        let none =
                            Reflection { mode: ReflectionMode::Highlights, area: areas, glare: panel.glare, amount: 100.0, ..Default::default() };
                        return Ok((none, Some(tr(highlights::NO_GLARE).to_string())));
                    }
                    made => made?,
                };
                let note = (panel.glare.fill == SpotFill::Lama && made.fill == SpotFill::Classic).then(|| {
                    if lama_installed { "LaMa could not fill: the Classic fill was used" } else { "LaMa is not installed: the Classic fill was used" }
                        .to_string()
                });
                let mut r = made.reflection;
                r.area = areas;
                // ✕ after the fill: nothing is saved
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err("Stopped".into());
                }
                Ok((area::save(&made.tiles, key, r)?, note))
            } else {
                Ok((area::make(&src, key, edge, &model, &regions, areas, &flag)?, None))
            }
        },
        move |app, ctx, result| {
            if generation == app.session.library_generation() {
                finish(app, ctx, id, &identity, result);
                after_run(app);
            } else {
                forget_waiting(app);
            }
            ctx.request_repaint();
        },
    )?;
    Ok(json!({"background": true, "photo": id.0, "mode": panel_mode_name(highlights)}))
}

fn panel_mode_name(highlights: bool) -> &'static str {
    if highlights { "highlights" } else { "reflections" }
}

#[cfg(not(target_arch = "wasm32"))]
fn finish(app: &mut LightcraftApp, ctx: &egui::Context, id: PhotoId, identity: &str, result: Result<(Reflection, Option<String>), String>) {
    match result {
        Ok((mut made, note)) => {
            let current = app.session.render_job(id, 384, 384, false, false).map(|j| lightcraft_engine::reflections::identity(j.source_key));
            if current.as_deref() != Some(identity) {
                app.toast(ctx, tr("The photo changed; calculate reflections again"));
                return;
            }
            let Some(d) = app.session.develop_of(id) else { return };
            let mut d = (*d).clone();
            keep_choices(&d.reflection, &mut made);
            let label = if made.mode == ReflectionMode::Highlights { "Remove Glare" } else { "Remove Reflections" };
            // a box drawn while this ran stays in the panel
            mark_saved(app, id, &made);
            d.reflection = made;
            match app.session.set_develop(id, d, label) {
                Ok(()) => app.toast(ctx, note.unwrap_or_else(|| tr("Reflections calculated; adjust Amount on the right").into())),
                Err(e) => app.toast(ctx, e.to_string()),
            }
        }
        Err(e) if e == "Stopped" => {}
        Err(e) => app.toast(ctx, format!("{}: {e}", tr("Reflection removal failed"))),
    }
}

// ------------------------------------------------------------------------ panel

fn spec(id: &'static str, label: &'static str, default: f64) -> lightcraft_develop::ControlSpec {
    lightcraft_develop::ControlSpec {
        id,
        label,
        section: lightcraft_develop::Section::Detail,
        min: 0.0,
        max: 100.0,
        default,
        step: 1.0,
        decimals: 0,
        track: lightcraft_develop::Track::Plain,
    }
}

/// The Mode row.
pub fn mode_row(app: &mut LightcraftApp, ui: &mut egui::Ui, busy: bool) {
    use crate::widgets::register;
    ui.horizontal(|ui| {
        ui.add_sized([58.0, 22.0], egui::Label::new(tr("Mode")));
        let mut mode = app.reflection_glare.mode;
        let label = |m: ReflectionMode| if m == ReflectionMode::Highlights { tr("Highlights") } else { tr("Reflections") };
        let r = crate::widgets::combo_box("reflection-mode").selected_text(label(mode)).show_ui(ui, |ui| {
            for m in [ReflectionMode::Reflections, ReflectionMode::Highlights] {
                let here = m == ReflectionMode::Reflections || !cfg!(target_arch = "wasm32");
                let r = ui.add_enabled(!busy && here, egui::Button::selectable(mode == m, label(m)));
                if r.clicked() {
                    mode = m;
                }
                register(ui, format!("reflection:mode:{}", m.name()), r.rect);
            }
        });
        crate::widgets::name_combo(&r.response, tr("Mode"));
        register(ui, "reflection:mode", r.response.rect);
        app.reflection_glare.mode = mode;
    });
}

/// The Area row.
pub fn area_row(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, busy: bool) {
    use crate::widgets::register;
    let mask_name =
        |id: u32| d.masks.iter().find(|m| m.id == id).map(|m| if m.name.is_empty() { format!("{} {id}", tr("Mask")) } else { m.name.clone() });
    ui.horizontal(|ui| {
        ui.add_sized([58.0, 22.0], egui::Label::new(tr("Area")));
        let area = &app.reflection_glare.area;
        let current = match area {
            Area::Whole => tr("Whole Photo").to_string(),
            Area::Boxes(_) => tr("Boxes").to_string(),
            Area::Mask(id) => mask_name(*id).unwrap_or_else(|| tr("Missing Mask").into()),
        };
        let mut pick: Option<Value> = None;
        let mut draw = false;
        let r = crate::widgets::combo_box("reflection-area").selected_text(current).show_ui(ui, |ui| {
            let r = ui.add_enabled(!busy, egui::Button::selectable(*area == Area::Whole, tr("Whole Photo")));
            register(ui, "reflection:area:whole", r.rect);
            if r.clicked() {
                pick = Some(json!("whole"));
            }
            let here = !busy && !cfg!(target_arch = "wasm32");
            let r = ui.add_enabled(here, egui::Button::selectable(matches!(area, Area::Boxes(_)), tr("Boxes")));
            register(ui, "reflection:area:box", r.rect);
            if r.clicked() {
                draw = true;
            }
            if !d.masks.is_empty() {
                ui.separator();
            }
            for m in &d.masks {
                let on = *area == Area::Mask(m.id);
                let r = ui.add_enabled(!busy, egui::Button::selectable(on, mask_name(m.id).unwrap_or_default()));
                register(ui, format!("reflection:area:mask:{}", m.id), r.rect);
                if r.clicked() {
                    pick = Some(json!({"mask": m.id}));
                }
            }
        });
        crate::widgets::name_combo(&r.response, tr("Area"));
        register(ui, "reflection:area", r.response.rect);
        let result = if let Some(area) = pick {
            put_down(app);
            app.run("reflection.area", json!({"area": area})).map(|_| ())
        } else if draw {
            app.run("reflection.area", json!({"area": "boxes"})).and_then(|_| app.run("reflection.tool", json!({"tool": "box"}))).map(|_| ())
        } else {
            Ok(())
        };
        if let Err(e) = result {
            app.toast(ui.ctx(), e);
        }
    });
    if app.reflection_glare.tool == Some(Tool::Box) && app.reflection_glare.area.boxes().is_empty() {
        ui.add(egui::Label::new(tr("Drag a box on the photo.")).wrap());
    }
}

/// The Highlights rows: Threshold, Expand, Fill and the strokes' Clear button.
pub fn highlights_rows(app: &mut LightcraftApp, ui: &mut egui::Ui, busy: bool) {
    use crate::widgets::register;
    for (id, label, default) in [
        ("reflection.threshold", "Threshold", lightcraft_develop::reflection_area::DEFAULT_GLARE_THRESHOLD),
        ("reflection.expand", "Expand", lightcraft_develop::reflection_area::DEFAULT_GLARE_EXPAND),
    ] {
        let s = spec(id, label, default);
        let g = &app.reflection_glare.glare;
        let value = if id == "reflection.threshold" { g.threshold } else { g.expand };
        let out = crate::widgets::slider(ui, &s, value, !busy, None);
        if let Some(v) = out.value.map(|v| s.clamp(v)).or(out.reset.then_some(default)) {
            let g = &mut app.reflection_glare.glare;
            if id == "reflection.threshold" { g.threshold = v } else { g.expand = v }
        }
    }
    ui.horizontal(|ui| {
        ui.add_sized([58.0, 22.0], egui::Label::new(tr("Fill")));
        let mut fill = app.reflection_glare.glare.fill;
        let name = |f: SpotFill| if f == SpotFill::Lama { "LaMa" } else { "Classic" };
        let r = crate::widgets::combo_box("reflection-fill").selected_text(tr(name(fill))).show_ui(ui, |ui| {
            for f in [SpotFill::Lama, SpotFill::Classic] {
                let r = ui.selectable_value(&mut fill, f, tr(name(f)));
                register(ui, format!("reflection:fill:{}", name(f).to_lowercase()), r.rect);
            }
        });
        crate::widgets::name_combo(&r.response, tr("Fill"));
        register(ui, "reflection:fill", r.response.rect);
        app.reflection_glare.glare.fill = fill;
    });
    if app.reflection_glare.glare.fill == SpotFill::Lama && !app.session.lama.installed() {
        ui.add(egui::Label::new(tr("LaMa is not installed: the Classic fill is used.")).wrap());
    }
    if !app.reflection_glare.glare.paint.is_empty() {
        let r = ui.add_enabled(!busy, egui::Button::new(tr("Clear Strokes")));
        register(ui, "reflection:paint:clear", r.rect);
        if r.clicked()
            && let Err(e) = app.run("reflection.paint", json!({"clear": true}))
        {
            app.toast(ui.ctx(), e);
        }
    }
}

// ------------------------------------------------------------------------ on the photo

/// The box (percent of the source) around `corners` (fractions of the source), kept on the photo; `None` when it has
/// no inside.
pub fn box_of(corners: [Point; 4]) -> Option<ReflectionArea> {
    let pc = |v: f64| if v.is_finite() { (v * 100.0).clamp(0.0, 100.0) } else { 0.0 };
    let (mut l, mut t, mut r, mut b) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for c in corners {
        (l, t, r, b) = (l.min(pc(c.x)), t.min(pc(c.y)), r.max(pc(c.x)), b.max(pc(c.y)));
    }
    ReflectionArea::boxed(l, t, r, b)
}

/// The long edge, in screen points, of a photo whose corner `origin` and the ends of its two edges `x_end`, `y_end`
/// are on the screen there.
pub fn long_on_screen(origin: egui::Pos2, x_end: egui::Pos2, y_end: egui::Pos2) -> f32 {
    origin.distance(x_end).max(origin.distance(y_end))
}

/// Source fractions of a point of the photo as shown (`map`'s coordinates on the photo as shot).
pub(crate) fn to_source(map: &CanvasMap, d: &DevelopSettings, q: egui::Pos2) -> Point {
    let n = map.for_mask(true).norm(q);
    lightcraft_pipeline::redeye::oriented_to_source(d.orientation, n)
}

/// Screen position of source fractions `p`.
pub(crate) fn to_screen(map: &CanvasMap, d: &DevelopSettings, p: Point) -> egui::Pos2 {
    map.for_mask(true).screen(lightcraft_pipeline::redeye::oriented_to_source(d.orientation.inverse(), p))
}

/// The section's tool on the photo, while it is in hand: the boxes (drawn only then), or the glare brush. Says whether
/// it took the pointer.
pub fn overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings) -> bool {
    use egui::{Color32, Stroke};
    if !app.reflection_open {
        return false;
    }
    if let Some(id) = app.session.active() {
        sync(app, id);
    }
    let Some(tool) = app.reflection_glare.tool else { return false };
    if tool == Tool::Box {
        crate::reflections_boxes::overlay(app, ui, resp, map, d);
        return true;
    }
    let p = ui.painter_at(app.canvas_rect.unwrap_or(map.rect));
    // a stroke's size is a share of the whole photo's long edge, cropped or not: so is its circle
    let m = map.for_mask(true);
    let long = long_on_screen(m.screen(Point::new(0.0, 0.0)), m.screen(Point::new(1.0, 0.0)), m.screen(Point::new(0.0, 1.0)));
    let radius = app.ui.remove_size * long;
    if let Some(h) = resp.hover_pos() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        p.circle_stroke(h, radius, Stroke::new(1.0, Color32::WHITE));
    }
    let press = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
    if resp.drag_started()
        && let Some(q) = press
    {
        app.reflection_glare.drag = vec![to_source(map, d, q)];
    }
    if resp.dragged()
        && let Some(q) = resp.interact_pointer_pos()
        && !app.reflection_glare.drag.is_empty()
    {
        let n = to_source(map, d, q);
        let drag = &mut app.reflection_glare.drag;
        if drag.len() < lightcraft_develop::reflection_area::MAX_STROKE_POINTS {
            drag.push(n);
        }
    }
    let pts: Vec<egui::Pos2> = app.reflection_glare.drag.iter().map(|q| to_screen(map, d, *q)).collect();
    p.add(egui::Shape::line(pts, Stroke::new(2.0 * radius, Color32::from_white_alpha(70))));
    if resp.drag_stopped() {
        let drag = std::mem::take(&mut app.reflection_glare.drag);
        if !drag.is_empty() {
            let size = app.ui.remove_size as f64 * 100.0;
            let points: Vec<[f64; 2]> = drag.iter().map(|q| [q.x * 100.0, q.y * 100.0]).collect();
            if let Err(e) = app.run("reflection.paint", json!({"points": points, "size": size, "erase": tool == Tool::Erase})) {
                app.toast(ui.ctx(), e);
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> LightcraftApp {
        LightcraftApp::new(lightcraft_engine::Session::with_demo(), Default::default())
    }

    #[test]
    fn mode_area_and_glare_are_set_by_commands_and_bad_values_are_refused() {
        let mut app = app();
        app.run("reflection.mode", json!({"mode": "highlights"})).unwrap();
        app.run("reflection.area", json!({"area": {"left": 10, "top": 20, "right": 40, "bottom": 50}})).unwrap();
        app.run("reflection.glare", json!({"threshold": 70, "expand": 5, "fill": "classic"})).unwrap();
        let s = app.run("reflection.status", json!({})).unwrap()["panel"].clone();
        assert_eq!(s["mode"], "highlights");
        assert_eq!(s["area"][0]["left"], 10.0);
        assert_eq!(s["glare"]["threshold"], 70.0);
        assert_eq!(s["glare"]["fill"], "classic");
        app.run("reflection.area", json!({"area": "whole"})).unwrap();
        assert_eq!(app.run("reflection.status", json!({})).unwrap()["panel"]["area"], "whole");
        for bad in [
            json!({"area": {"left": 50, "top": 0, "right": 10, "bottom": 100}}),
            json!({"area": {"left": 0}}),
            json!({"area": {"mask": 999}}),
            json!({"threshold": 101}),
            json!({"fill": "magic"}),
            json!({"mode": "glass"}),
        ] {
            assert!(app.run("reflection.glare", bad.clone()).is_err() || app.run("reflection.area", bad.clone()).is_err(), "{bad}");
        }
        assert!(app.run("reflection.tool", json!({"tool": "lasso"})).is_err());
        app.run("reflection.tool", json!({"tool": "paint"})).unwrap();
        assert_eq!(app.reflection_glare.tool, Some(Tool::Paint));
    }

    /// Corrections of the runs these tests start go into one scratch folder of the test process, never the real
    /// reflection cache (set once: tests run side by side).
    fn scratch_cache() {
        static SET: std::sync::Once = std::sync::Once::new();
        SET.call_once(|| {
            let dir = std::env::temp_dir().join(format!("lc-glare-ui-{}", std::process::id()));
            lightcraft_engine::reflections::area::set_cache_root(Some(dir));
        });
    }

    /// Run the app's finished tasks until the reflection run ends (a generous bound guards only against a hang).
    fn join(app: &mut LightcraftApp) {
        let ctx = egui::Context::default();
        assert!(crate::tasks::wait_for(app, &ctx, crate::reflections::TASK, crate::headless::HANG));
    }

    fn other_photo(app: &LightcraftApp) -> PhotoId {
        let active = app.session.active();
        app.session.catalog.photos().map(|p| p.id).find(|id| Some(*id) != active).unwrap()
    }

    #[test]
    fn a_painted_stroke_is_kept_and_starts_a_run() {
        scratch_cache();
        let mut app = app();
        let id = app.session.active().unwrap();
        let r = app.run("reflection.paint", json!({"points": [[50, 50], [52, 50]], "size": 2})).unwrap();
        assert_eq!(r["strokes"], 1);
        assert_eq!(app.reflection_glare.mode, ReflectionMode::Highlights);
        assert!((app.reflection_glare.glare.paint[0].size - 0.02).abs() < 1e-9);
        assert!(app.reflection_glare.glare.paint[0].points[0].x == 0.5);
        assert!(app.run("reflection.paint", json!({"points": []})).is_err());
        assert!(app.run("reflection.paint", json!({"points": [["a", 1]]})).is_err());
        assert!(app.tasks.is_running(crate::reflections::TASK));
        // a second stroke while the first runs waits for it, for this photo
        app.run("reflection.paint", json!({"points": [[10, 10]], "erase": true})).unwrap();
        assert_eq!(app.reflection_glare.glare.paint.len(), 2);
        assert_eq!(app.reflection_glare.rerun, Some(id));
        join(&mut app);
        // the waiting run started when the first ended
        join(&mut app);
        assert_eq!(app.reflection_glare.rerun, None);
        app.run("reflection.paint", json!({"clear": true})).unwrap();
        join(&mut app);
        assert!(app.reflection_glare.glare.paint.is_empty());
    }

    #[test]
    fn a_waiting_rerun_runs_on_its_own_photo_never_another() {
        scratch_cache();
        let mut app = app();
        let a = app.session.active().unwrap();
        app.run("reflection.paint", json!({"points": [[50, 50]], "size": 2})).unwrap();
        app.run("reflection.paint", json!({"points": [[20, 20]], "size": 2})).unwrap();
        assert!(app.reflection_glare.rerun.is_some());
        let b = other_photo(&app);
        app.session.selection.active = Some(b);
        // the panel turns to photo B while photo A's second stroke still waits
        app.run("reflection.status", json!({})).unwrap();
        join(&mut app);
        join(&mut app);
        let strokes = |app: &LightcraftApp, id| app.session.develop_of(id).unwrap().reflection.glare.paint.len();
        assert_eq!(strokes(&app, a), 2, "photo A's waiting stroke was lost");
        assert_eq!(strokes(&app, b), 0, "photo A's strokes reached photo B");
        assert!(app.reflection_glare.glare.paint.is_empty());
    }

    #[test]
    fn the_panel_follows_the_shown_photo_with_the_section_closed() {
        let mut app = app();
        app.reflection_open = false;
        app.run("reflection.mode", json!({"mode": "highlights"})).unwrap();
        app.run("reflection.area", json!({"area": {"left": 10, "top": 10, "right": 20, "bottom": 20}})).unwrap();
        let b = other_photo(&app);
        app.session.selection.active = Some(b);
        let s = app.run("reflection.status", json!({})).unwrap()["panel"].clone();
        assert_eq!(s["mode"], "reflections", "photo B took photo A's mode");
        assert_eq!(s["area"], "whole");
    }

    #[test]
    fn a_recalculation_keeps_protections_and_drops_an_old_limit_for_an_area() {
        let old = Reflection {
            correction: "x".into(),
            amount: 40.0,
            protect_sky: true,
            detail_protection: 70.0,
            limit_region: true,
            left: 50.0,
            ..Default::default()
        };
        let mut area = Reflection { amount: 100.0, area: ReflectionArea::boxed(0.0, 0.0, 30.0, 30.0).into_iter().collect(), ..Default::default() };
        keep_choices(&old, &mut area);
        assert_eq!((area.amount, area.protect_sky, area.detail_protection, area.limit_region), (40.0, true, 70.0, false));
        let mut whole = Reflection { amount: 100.0, ..Default::default() };
        keep_choices(&old, &mut whole);
        assert!(whole.limit_region && whole.left == 50.0 && whole.detail_protection == 70.0);
        let mut glare = Reflection { mode: ReflectionMode::Highlights, amount: 100.0, ..Default::default() };
        keep_choices(&old, &mut glare);
        assert_eq!(glare.amount, 100.0, "another kind of result keeps its own defaults");
    }

    #[test]
    fn a_dragged_box_is_the_box_around_all_its_corners() {
        // a straightened view: the dragged rectangle is a turned quad on the source
        let quad = [Point::new(0.30, 0.20), Point::new(0.62, 0.26), Point::new(0.56, 0.58), Point::new(0.24, 0.52)];
        let a = box_of(quad).unwrap();
        let near = |v: f64, want: f64| (v - want).abs() < 1e-9;
        assert!(near(a.left, 24.0) && near(a.top, 20.0) && near(a.right, 62.0) && near(a.bottom, 58.0), "{a:?}");
        assert!(box_of([Point::new(0.5, 0.5); 4]).is_none());
        let off = box_of([Point::new(-0.2, 0.1), Point::new(1.3, 0.1), Point::new(1.3, 0.4), Point::new(-0.2, 0.4)]).unwrap();
        assert_eq!((off.left, off.right), (0.0, 100.0));
    }

    #[test]
    fn the_brush_circle_follows_the_whole_photo_not_the_crop() {
        // a photo cropped to a quarter: shown at 400 points wide, the whole photo spans 1600
        let o = egui::pos2(-600.0, -300.0);
        assert_eq!(long_on_screen(o, egui::pos2(1000.0, -300.0), egui::pos2(-600.0, 900.0)), 1600.0);
    }

    #[test]
    fn a_photo_takes_back_its_saved_choices() {
        let mut app = app();
        let r = Reflection {
            mode: ReflectionMode::Highlights,
            area: ReflectionArea::boxed(1.0, 2.0, 3.0, 4.0).into_iter().collect(),
            glare: Glare { threshold: 80.0, ..Default::default() },
            ..Default::default()
        };
        app.reflection_glare.tool = Some(Tool::Box);
        load(&mut app, &r);
        assert_eq!(app.reflection_glare.mode, ReflectionMode::Highlights);
        assert_eq!(app.reflection_glare.area.list(), r.area);
        assert_eq!(app.reflection_glare.glare.threshold, 80.0);
        assert_eq!(app.reflection_glare.tool, None);
    }
}
