//! Settings ▸ Import ▸ Raw defaults ▸ Starting Colour for Raw Photos: the colour source new raws start on
//! (`library.preferences {import: {startingColour}}`, `lightcraft_engine::import::StartingColour`).

use lightcraft_engine::import::{ImportDefaults, StartingColour};

use super::widgets::{choices, row};
use super::{LightcraftApp, Tokens, json};

/// The choices, in order, with their Title Case labels.
pub(crate) const CHOICES: [(StartingColour, &str); 3] = [
    (StartingColour::Macos, "macOS Colour"),
    (StartingColour::CameraMatching, "Camera Matching"),
    (StartingColour::NoCameraMatching, "No Camera Matching"),
];

pub(super) fn starting_colour_row(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, d: &ImportDefaults) {
    let mut value = d.starting_colour;
    row(ui, t, crate::i18n::tr("Starting Colour for Raw Photos"), |ui| {
        let labels: Vec<(StartingColour, &str)> = CHOICES.iter().map(|(v, l)| (*v, crate::i18n::tr(l))).collect();
        if choices(ui, "settingsStartingColour", &labels, &mut value) {
            let _ = app.run("library.preferences", json!({"import": {"startingColour": value.value()}}));
        }
    });
    if value == StartingColour::Macos && !lightcraft_macraw_available() {
        let line = crate::i18n::tr("This computer isn't a Mac: new raw photos start on Camera Matching.");
        let r = ui.label(egui::RichText::new(line).size(11.0).color(t.text_dim));
        crate::widgets::register(ui, "label:settingsStartingColourNotMac", r.rect);
    }
}

/// Whether this computer can make macOS Colour (a test can say it can't).
fn lightcraft_macraw_available() -> bool {
    #[cfg(test)]
    if let Some(v) = NOT_A_MAC_FOR_TESTS.with(std::cell::Cell::get) {
        return !v;
    }
    lightcraft_engine::files::macos_available()
}

#[cfg(test)]
thread_local! {
    /// Draw the Settings row as on a computer that isn't a Mac (this test's thread).
    pub(crate) static NOT_A_MAC_FOR_TESTS: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}
