//! A fixed slider label column (issue #21). The Edit and Masking panels fix one width for the labels of all their
//! sliders, from every label the panel can ever show, so opening or closing a section never moves a label or a track.
//! Outside those panels the column still follows the widest label drawn (`shared_column_width`).

use egui::{Id, Ui};

/// The width in force this pass: (pass number, width).
fn active_key() -> Id {
    Id::unique("slider-fixed-label-column")
}

/// Fix the label column of the sliders drawn until [`release`] to the widest of `labels()` (translated, at the slider
/// label font). `kind` names the panel; the width is kept per panel, font size and language, so `labels` runs once.
pub(crate) fn fix(ui: &Ui, kind: &'static str, labels: impl FnOnce() -> Vec<&'static str>) {
    let ctx = ui.ctx();
    let font = crate::theme::Tokens::get(ctx).font(super::LABEL_FONT);
    let cache = Id::unique(("slider-label-column", kind, font.size.to_bits(), crate::i18n::language().code()));
    let width = match ctx.data(|d| d.get_temp::<f32>(cache)) {
        Some(w) => w,
        None => {
            let w = labels()
                .into_iter()
                .map(|l| ui.painter().layout_no_wrap(crate::i18n::tr(l).to_string(), font.clone(), egui::Color32::WHITE).size().x)
                .fold(0.0_f32, f32::max);
            ctx.data_mut(|d| d.insert_temp(cache, w));
            w
        }
    };
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(active_key(), (pass, width)));
}

/// Go back to the labels sizing the column themselves.
pub(crate) fn release(ui: &Ui) {
    ui.ctx().data_mut(|d| d.remove::<(u64, f32)>(active_key()));
}

/// The fixed label width of this pass, if a panel fixed one.
pub(super) fn fixed(ui: &Ui) -> Option<f32> {
    let (pass, width) = ui.ctx().data(|d| d.get_temp::<(u64, f32)>(active_key()))?;
    (pass == ui.ctx().cumulative_pass_nr()).then_some(width)
}
