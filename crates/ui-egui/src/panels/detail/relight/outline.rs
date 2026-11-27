//! Fork (issue #466): when a light's outline and handles are drawn over the photo. They show while the pointer is over
//! the photo, and while a slider (or handle) that moves or shapes the light is dragged, then fade out over [`FADE`]. With
//! the pointer off the photo and another slider (Amount, Softness, Warmth …) dragged, nothing is drawn, so the photo can be
//! judged clean.

use super::LightcraftApp;

/// How long the outline takes to fade out once nothing shows it any more (seconds).
const FADE: f64 = 0.3;

/// The last field of a Relight control id (`relight.2.boxWidth` and `relight.boxWidth` are both `boxWidth`) names a control
/// that moves or shapes the light on the photo: its place, a spot's aim and cone, a sun's direction and rays, a softbox's size,
/// turn and aim.
pub(super) fn is_shaping(dragging: Option<&str>) -> bool {
    const SHAPING: [&str; 14] = [
        "x",
        "y",
        "spotAngle",
        "spotAimX",
        "spotAimY",
        "sunAzimuth",
        "sunElevation",
        "raysX",
        "raysY",
        "boxWidth",
        "boxHeight",
        "boxRotation",
        "boxAimH",
        "boxAimV",
    ];
    dragging.is_some_and(|c| c.starts_with("relight.") && c.rsplit('.').next().is_some_and(|f| SHAPING.contains(&f)))
}

/// How opaque the outline is: 1 while the pointer is on the photo (`hovered`) or a shaping slider is dragged (`shaping`), then
/// fading to 0 over [`FADE`] seconds from the last moment it was (`since`: seconds since then, none if never).
pub(super) fn opacity(hovered: bool, shaping: bool, since: Option<f64>) -> f32 {
    if hovered || shaping {
        return 1.0;
    }
    match since {
        Some(s) if s < FADE => (1.0 - s.max(0.0) / FADE) as f32,
        _ => 0.0,
    }
}

fn last_id() -> egui::Id {
    egui::Id::unique("relightOutlineLast")
}

/// The outline's opacity now, with `hovered` (the pointer is on the photo, or a handle is held). Asks for repaints while it
/// fades.
pub(super) fn now(app: &LightcraftApp, ui: &egui::Ui, hovered: bool) -> f32 {
    let time = ui.input(|i| i.time);
    let shaping = is_shaping(app.ui.dragging_control.as_deref());
    if hovered || shaping {
        ui.ctx().data_mut(|m| m.insert_temp(last_id(), time));
    }
    let since = ui.ctx().data(|m| m.get_temp::<f64>(last_id())).map(|t| time - t);
    let alpha = opacity(hovered, shaping, since);
    if alpha > 0.0 && alpha < 1.0 {
        ui.ctx().request_repaint();
    }
    alpha
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_outline_shows_on_hover_and_while_shaping_and_not_for_other_sliders_off_the_photo() {
        // off the photo, nothing dragged, nothing before: hidden
        assert_eq!(opacity(false, false, None), 0.0);
        // on the photo: shown
        assert_eq!(opacity(true, false, None), 1.0);
        // a shape slider moves it (the box's width, the cone's angle, the light's place), off the photo
        for id in ["relight.0.boxWidth", "relight.spotAngle", "relight.x"] {
            assert!(is_shaping(Some(id)), "{id}");
            assert_eq!(opacity(false, is_shaping(Some(id)), None), 1.0, "{id}");
        }
        // an intensity, softness or colour slider off the photo draws nothing
        for id in ["relight.0.amount", "relight.softness", "relight.0.warmth", "light.exposure"] {
            assert!(!is_shaping(Some(id)), "{id}");
            assert_eq!(opacity(false, is_shaping(Some(id)), None), 0.0, "{id}");
        }
        assert!(!is_shaping(None));
        // after it, the outline fades out and is gone
        let (soon, later) = (opacity(false, false, Some(FADE * 0.25)), opacity(false, false, Some(FADE * 0.75)));
        assert!(soon < 1.0 && later < soon && later > 0.0);
        assert_eq!(opacity(false, false, Some(FADE)), 0.0);
    }
}
