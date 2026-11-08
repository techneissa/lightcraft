//! Fork feature (fork/shared-models.md): the Remove button of a face or denoise model row. A model in the folder shared
//! with the other ArtCraft apps takes a second click that reads "Remove for Every ArtCraft App?" (its files are the other
//! apps' too); a model only LightCraft keeps goes with one click, as before.

/// What the button did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remove {
    /// Nothing was clicked.
    Idle,
    /// A shared model's button reads "Remove for Every ArtCraft App?" and waits for the second click.
    Asking,
    /// Remove the model (a model only LightCraft keeps).
    Own,
    /// Remove the model for every ArtCraft app (the second click on a shared model).
    EveryApp,
}

/// Draws the Remove button of the model `key` (unique on screen). `shared`: the model is in the shared folder.
pub fn button(ui: &mut egui::Ui, key: &str, shared: bool) -> (egui::Response, Remove) {
    let armed_id = ui.make_persistent_id(("model-remove-armed", key));
    let armed = shared && ui.ctx().data(|d| d.get_temp::<bool>(armed_id)).unwrap_or(false);
    let r = ui.button(if armed { crate::i18n::tr("Remove for Every ArtCraft App?") } else { crate::i18n::tr("Remove") });
    let outcome = match (r.clicked(), shared, armed) {
        (true, false, _) => Remove::Own,
        (true, true, true) => Remove::EveryApp,
        (false, true, true) => Remove::Asking,
        _ => Remove::Idle,
    };
    // the first click on a shared model arms the button; a click anywhere else, or the second click, disarms it
    let arm = shared && !armed && r.clicked();
    let elsewhere = ui.input(|i| i.pointer.any_pressed()) && !r.contains_pointer();
    if arm || matches!(outcome, Remove::Own | Remove::EveryApp) || elsewhere {
        ui.ctx().data_mut(|d| d.insert_temp(armed_id, arm));
    }
    (r, outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &egui::Context, shared: bool, click: Option<egui::Pos2>, rect: &mut egui::Rect) -> Remove {
        let mut out = Remove::Idle;
        let mut input = egui::RawInput::default();
        if let Some(p) = click {
            input.events.push(egui::Event::PointerMoved(p));
            input.events.push(egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
            input.events.push(egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            });
        }
        let mut full = ctx.run_ui(input, |ui| {
            let (r, o) = button(ui, "m", shared);
            *rect = r.rect;
            out = o;
        });
        full.textures_delta.clear();
        out
    }

    #[test]
    fn a_shared_model_needs_two_clicks_and_a_private_one_needs_one() {
        let ctx = egui::Context::default();
        let mut rect = egui::Rect::NOTHING;
        assert_eq!(frame(&ctx, true, None, &mut rect), Remove::Idle);
        let at = rect.center();
        assert_eq!(frame(&ctx, true, Some(at), &mut rect), Remove::Idle, "the first click only asks");
        assert_eq!(frame(&ctx, true, None, &mut rect), Remove::Asking, "the button now reads Remove for Every ArtCraft App?");
        assert_eq!(frame(&ctx, true, Some(at), &mut rect), Remove::EveryApp);
        assert_eq!(frame(&ctx, true, None, &mut rect), Remove::Idle, "asking again starts over");
        // a click somewhere else takes the question back
        frame(&ctx, true, Some(at), &mut rect);
        frame(&ctx, true, Some(egui::pos2(900.0, 900.0)), &mut rect);
        assert_eq!(frame(&ctx, true, None, &mut rect), Remove::Idle);
        let ctx = egui::Context::default();
        frame(&ctx, false, None, &mut rect);
        assert_eq!(frame(&ctx, false, Some(rect.center()), &mut rect), Remove::Own);
    }
}
