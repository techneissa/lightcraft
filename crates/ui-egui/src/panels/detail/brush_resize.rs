//! ⌃⌥-drag (or Alt+right-drag, the Windows and Linux gesture of Photoshop) on the photo resizes the brush in use, for the Masking brush and the Remove tool alike
//! (the on-canvas resize of Photoshop, as in PhotoCraft): sideways changes the size (the circle's
//! edge follows the pointer), up and down the feather (down is harder). It begins on the press, not
//! once egui calls the movement a drag, and moves the brush by the pointer's travel, so a value held
//! at its limit leaves it as soon as the pointer turns back. The circle stays where the press was,
//! tinted to show the feather, with a small dark Size / Feather label beside the pointer.

use super::{Color32, LightcraftApp, Pos2, Stroke, vec2};

/// Screen points of vertical drag that take the feather from 0 % to 100 %.
const FEATHER_TRAVEL: f32 = 200.0;
/// The tint's alpha: about half.
const TINT_ALPHA: u8 = 128;

/// A resize in progress: where the press was, and the unrounded brush (size as a fraction of the
/// photo's long side, feather in percent).
#[derive(Clone, Copy)]
struct Resize {
    anchor: Pos2,
    size: f32,
    feather: f32,
    start_feather: f32,
    /// Begun by an Alt+right-drag: it lasts as long as the right button is down.
    secondary: bool,
}

/// The size (fraction of the long side, within `limits`) and feather (percent) after the pointer
/// travelled (`dx`, `dy`) screen points over a photo whose long side is `long` points.
pub(super) fn travel(size: f32, feather: f32, dx: f32, dy: f32, long: f32, limits: (f32, f32)) -> (f32, f32) {
    let fin = |v: f32| if v.is_finite() { v } else { 0.0 };
    let size = (size + fin(dx) / long.max(1.0)).clamp(limits.0, limits.1);
    let feather = (feather - fin(dy) * 100.0 / FEATHER_TRAVEL).clamp(0.0, 100.0);
    (size, feather)
}

/// Handles ⌃⌥-drag for the Remove tool's brush (`remove`) or the Masking brush; `long` is the
/// photo's long side on screen and `scale` the factor of the Size slider (the number the label
/// shows). True while a resize lasts (and on its last frame), when the tool must neither paint nor
/// draw its own cursor. A press without ⌃⌥ never turns into a resize.
pub(crate) fn resize_drag(
    app: &mut LightcraftApp,
    ui: &egui::Ui,
    resp: &egui::Response,
    p: &egui::Painter,
    long: f32,
    scale: f32,
    remove: bool,
) -> bool {
    let id = egui::Id::unique("brush-resize-drag");
    let mut live: Option<Resize> = ui.data(|d| d.get_temp(id));
    let (pressed, secondary_pressed, primary_down, secondary_down, origin, delta, mods) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.button_pressed(egui::PointerButton::Secondary),
            i.pointer.primary_down(),
            i.pointer.secondary_down(),
            i.pointer.press_origin().or(i.pointer.latest_pos()),
            i.pointer.delta(),
            i.modifiers,
        )
    });
    let (limits, size, feather) =
        if remove { ((0.001, 0.25), app.ui.remove_size, app.ui.remove_feather) } else { ((0.002, 0.5), app.ui.brush_size, app.ui.brush_feather) };
    // ⌃⌥ with the left button, or Alt (without Shift, which is the HUD colour picker in Photoshop) with the right
    let begins = |secondary: bool| {
        resp.hovered() && app.gesture.is_none() && if secondary { mods.alt && !mods.shift && !mods.ctrl } else { mods.ctrl && mods.alt }
    };
    let started = if secondary_pressed && begins(true) {
        Some(true)
    } else if pressed && begins(false) {
        Some(false)
    } else {
        None
    };
    if pressed || secondary_pressed {
        live = started.zip(origin.filter(|o| resp.rect.contains(*o))).map(|(secondary, anchor)| Resize {
            anchor,
            size,
            feather,
            start_feather: feather,
            secondary,
        });
    }
    let down = live.is_some_and(|r| if r.secondary { secondary_down } else { primary_down });
    if let Some(r) = live.as_mut()
        && down
        && !(pressed || secondary_pressed)
    {
        (r.size, r.feather) = travel(r.size, r.feather, delta.x, delta.y, long, limits);
    }
    let Some(r) = live else {
        ui.data_mut(|d| d.remove::<Resize>(id));
        return false;
    };
    if remove {
        (app.ui.remove_size, app.ui.remove_feather) = (r.size, r.feather.round());
    } else {
        (app.ui.brush_size, app.ui.brush_feather) = (r.size, r.feather.round());
    }
    // the release frame still belongs to the resize (its click must not paint), then it is over
    ui.data_mut(|d| {
        if down {
            d.insert_temp(id, r);
        } else {
            d.remove::<Resize>(id);
        }
    });
    draw(ui, p, &r, (r.size * long, r.feather.round()), (r.size * scale).round(), pressed || secondary_pressed || down);
    true
}

/// The circle at the press point with its tint and the label beside the pointer.
fn draw(ui: &egui::Ui, p: &egui::Painter, r: &Resize, (radius, feather): (f32, f32), shown_size: f32, active: bool) {
    let c = r.anchor;
    tip(p, c, radius, 1.0 - feather / 100.0, Color32::from_rgba_unmultiplied(255, 0, 0, TINT_ALPHA));
    p.circle_stroke(c, radius + 0.5, Stroke::new(1.0, Color32::from_black_alpha(140)));
    p.circle_stroke(c, radius, Stroke::new(1.0, Color32::from_white_alpha(220)));
    ui.ctx().set_cursor_icon(egui::CursorIcon::None);
    if !active {
        return;
    }
    let at = ui.ctx().pointer_latest_pos().unwrap_or(c);
    let mut lines = vec![format!("{}: {}", crate::i18n::tr("Size"), shown_size as i64)];
    if feather != r.start_feather {
        lines.push(format!("{}: {}%", crate::i18n::tr("Feather"), feather as i64));
    }
    egui::Area::new(egui::Id::unique("brush-resize-readout"))
        .order(egui::Order::Tooltip)
        .fixed_pos(at + vec2(16.0, 18.0))
        .interactable(false)
        .constrain(true)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(28, 28, 28, 235))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
                    for line in &lines {
                        ui.label(egui::RichText::new(line).size(12.0).color(Color32::from_gray(240)));
                    }
                });
        });
}

/// A round tip of `radius` points: `color` out to `hardness` (0..1) of the radius, fading to clear at
/// the edge. One mesh, so its cost doesn't grow with the brush.
fn tip(p: &egui::Painter, c: Pos2, radius: f32, hardness: f32, color: Color32) {
    if !(radius.is_finite() && radius > 0.0 && p.clip_rect().expand(radius).contains(c)) {
        return;
    }
    let n = ((radius * 0.5) as usize).clamp(24, 160);
    let inner = radius * hardness.clamp(0.0, 1.0);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(c, color);
    for k in 0..n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        let d = vec2(a.cos(), a.sin());
        mesh.colored_vertex(c + d * inner, color);
        mesh.colored_vertex(c + d * radius, Color32::TRANSPARENT);
    }
    let n = n as u32;
    for k in 0..n {
        let (i0, o0) = (1 + 2 * k, 2 + 2 * k);
        let (i1, o1) = (1 + 2 * ((k + 1) % n), 2 + 2 * ((k + 1) % n));
        mesh.add_triangle(0, i0, i1);
        mesh.add_triangle(i0, o0, o1);
        mesh.add_triangle(i0, o1, i1);
    }
    p.add(mesh);
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: (f32, f32) = (0.002, 0.5);

    #[test]
    fn the_circle_edge_follows_the_pointer_and_down_hardens() {
        let (size, feather) = travel(0.04, 50.0, 100.0, 0.0, 1000.0, LIMITS);
        assert!((size - 0.14).abs() < 1e-6 && feather == 50.0, "{size} {feather}");
        let (_, feather) = travel(0.04, 50.0, 0.0, 20.0, 1000.0, LIMITS);
        assert_eq!(feather, 40.0, "down is harder");
        let (_, feather) = travel(0.04, 50.0, 0.0, -20.0, 1000.0, LIMITS);
        assert_eq!(feather, 60.0, "up is softer");
    }

    #[test]
    fn a_value_held_at_a_limit_leaves_it_as_soon_as_the_pointer_turns_back() {
        let (size, feather) = travel(0.04, 50.0, 100_000.0, -100_000.0, 1000.0, LIMITS);
        assert_eq!((size, feather), (0.5, 100.0));
        let (size, feather) = travel(size, feather, -10.0, 10.0, 1000.0, LIMITS);
        assert!(size < 0.5 && feather < 100.0, "{size} {feather}");
        assert_eq!(travel(0.04, 50.0, f32::NAN, f32::INFINITY, 1000.0, LIMITS), (0.04, 50.0));
    }
}
