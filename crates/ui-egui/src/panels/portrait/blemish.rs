//! The Portrait tab's section of the blemish feature (`lightcraft_retouch::blemish`): Amount, the Keep Moles and
//! Freckles switch, and what they do to the spots found (how many are removed and kept, and the share of the skin
//! repaired), for every face or the picked one.

use lightcraft_develop::retouch::blemish::{Blemish, Report};
use lightcraft_develop::retouch::{Feature, KeyMap};
use serde_json::json;

use crate::theme::Tokens;
use crate::widgets::register;

/// The switch's control.
pub(super) const KEEP: &str = "retouch.blemish.keepMoles";

pub(super) fn body(cx: &mut super::Ctx<'_>) {
    cx.slider("retouch.blemish.amount");
    keep(cx);
    report(cx);
}

/// The settings of face `i` (`None`: the ones for every face).
fn settings_of<'a>(cx: &'a super::Ctx<'_>, i: Option<usize>) -> &'a Blemish {
    match i {
        Some(i) => {
            let keys = cx.keys.as_ref().map(|k| k as &dyn KeyMap);
            &cx.d.retouch.settings_for_in(&cx.input.faces, i, keys).blemish
        }
        None => &cx.d.retouch.global.blemish,
    }
}

fn keep(cx: &mut super::Ctx<'_>) {
    let mut on = settings_of(cx, cx.face).keep_moles;
    let face = cx.face;
    let changed = egui::Frame::NONE
        .inner_margin(egui::Margin { left: 24, right: 22, top: 2, bottom: 2 })
        .show(cx.ui, |ui| {
            let r = ui.checkbox(&mut on, crate::i18n::tr("Keep Moles and Freckles"));
            register(ui, "portrait:blemish.keepMoles", r.rect);
            r.changed()
        })
        .inner;
    if changed && let Err(e) = cx.app.run("retouch.set", json!({"control": KEEP, "value": if on { 1.0 } else { 0.0 }, "face": face})) {
        cx.app.toast(cx.ui.ctx(), e);
    }
}

/// What the settings do to the spots of the picked face, or of every face (each with its own settings); nothing
/// while no face has its spots found yet.
fn report(cx: &mut super::Ctx<'_>) {
    let faces: Vec<usize> = match cx.face {
        Some(i) => vec![i],
        None => (0..cx.input.faces.len()).collect(),
    };
    let mut sum = Report::default();
    let mut skin = 0.0;
    let mut any = false;
    for i in faces {
        let Some(spots) = cx.input.faces.get(i).and_then(|f| f.spots.as_ref()) else { continue };
        let s = settings_of(cx, Some(i));
        let r = if s.is_off() { Report::default() } else { s.report(spots) };
        any = true;
        sum.removed += r.removed;
        sum.moles += r.moles;
        sum.freckles += r.freckles;
        sum.area += r.area * spots.skin;
        skin += spots.skin;
    }
    if !any {
        return;
    }
    let area = if skin > 0.0 { sum.area / skin * 100.0 } else { 0.0 };
    let t = Tokens::get(cx.ui.ctx());
    use crate::i18n::{plural_s, tr_format};
    let mut lines =
        vec![tr_format!("Removed {n} spot{s}, {area} % of the skin", n = sum.removed, s = plural_s(sum.removed), area = format!("{area:.1}"))];
    let mut kept = Vec::new();
    if sum.moles > 0 {
        kept.push(tr_format!("{n} mole{s}", n = sum.moles, s = plural_s(sum.moles)));
    }
    if sum.freckles > 0 {
        kept.push(tr_format!("{n} freckle{s}", n = sum.freckles, s = plural_s(sum.freckles)));
    }
    if !kept.is_empty() {
        lines.push(tr_format!("Kept {what}", what = kept.join(crate::i18n::tr(" and "))));
    }
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 6 }).show(cx.ui, |ui| {
        let r = ui.label(egui::RichText::new(lines.join("\n")).size(11.5).color(t.text_dim));
        register(ui, "portrait:blemish.report", r.rect);
    });
}
