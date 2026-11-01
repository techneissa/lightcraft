//! A fixed slider label column (issues #21, #29). The Edit and Masking panels fix one width for the labels of all
//! their sliders, from the common labels of the panel (the widest of Light, Color and Effects in Edit), so opening or
//! closing a section never moves a label or a track and short labels leave no wide gap. A label wider than the column
//! shrinks a little, then wraps onto two lines at that smaller size ([`fit`]); its row grows to hold them.
//! Outside those panels the column still follows the widest label drawn (`shared_column_width`).

use std::sync::Arc;

use egui::{Color32, FontId, Id, Ui, epaint::text::LayoutJob};

/// The width in force this pass: (pass number, width).
fn active_key() -> Id {
    Id::unique("slider-fixed-label-column")
}

/// Fix the label column of the sliders drawn until [`release`] to the widest of `labels()` (the common labels,
/// translated, at the normal slider label font). `kind` names the panel; the width is kept per panel, font size and language, so `labels` runs once.
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

/// The smallest a label shrinks to before it wraps onto two lines ("slightly smaller" than the normal size).
const SMALL_FONT: f32 = 11.0;

/// The smallest a wrapped label's font gets when two lines at [`SMALL_FONT`] still do not fit.
const SMALLEST_FONT: f32 = 9.5;

/// The padding above and below a label's lines in a row that grew for them.
const ROW_PAD: f32 = 8.0;

/// `text` laid out to fit `width` (left-aligned): the normal font if it fits, else a slightly smaller one (down to
/// [`SMALL_FONT`]), else two lines at that size (smaller still if a word needs it), else one line cut with an ellipsis
/// at the smallest size. The flag says it was cut.
pub(super) fn fit(ui: &Ui, text: &str, normal: &FontId, width: f32, color: Color32) -> (Arc<egui::Galley>, bool) {
    let floor = SMALL_FONT.min(normal.size);
    let mut size = normal.size;
    loop {
        let galley = ui.painter().layout_no_wrap(text.to_string(), FontId { size, ..normal.clone() }, color);
        if galley.size().x <= width {
            return (galley, false);
        }
        if size <= floor {
            break;
        }
        size = (size - 0.5).max(floor);
    }
    let mut size = floor;
    loop {
        let mut job = LayoutJob::simple(text.to_string(), FontId { size, ..normal.clone() }, color, width);
        job.wrap.max_rows = 2;
        let galley = ui.painter().layout_job(job);
        // (a word wider than the column would be broken in the middle; shrink further instead)
        let words_fit = text.split_whitespace().all(|w| ui.painter().layout_no_wrap(w.to_string(), galley_font(&galley), color).size().x <= width);
        if !galley.elided && galley.size().x <= width && galley.rows.len() <= 2 && words_fit {
            return (galley, false);
        }
        if size <= SMALLEST_FONT {
            break;
        }
        size = (size - 0.5).max(SMALLEST_FONT);
    }
    let font = FontId { size: SMALLEST_FONT.min(normal.size), ..normal.clone() };
    (ui.painter().layout_no_wrap(super::fit_text(ui, text, &font, width), font, color), true)
}

/// The height of a row holding `galley` (never below `base`).
pub(super) fn row_height(galley: &egui::Galley, base: f32) -> f32 {
    base.max(galley.size().y + ROW_PAD)
}

/// The font `galley` was laid out in.
fn galley_font(galley: &egui::Galley) -> FontId {
    galley.job.sections.first().map_or_else(FontId::default, |s| s.format.font_id.clone())
}
