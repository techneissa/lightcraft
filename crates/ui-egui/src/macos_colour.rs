//! macOS Colour lines in dialogs whose output can't carry it: a DNG keeps the raw data and Camera Matching colours, a
//! merge is a new DNG, and a computer that can't make macOS Colour exports such photos with Camera Matching colours.
//! Each says so when it applies, never silently (`docs/camera-preview-colour.md`, "macOS Colour").

use lightcraft_catalog::PhotoId;
use lightcraft_develop::CameraColour;
use lightcraft_engine::export::ExportFormat;

use crate::LightcraftApp;
use crate::theme::Tokens;

/// Of `ids`, the photos set to macOS Colour, and of those the ones this computer can't make it for.
pub(crate) fn counts(app: &LightcraftApp, ids: &[PhotoId]) -> (usize, usize) {
    ids.iter()
        .filter_map(|id| app.session.catalog.photo(*id))
        .filter(|p| p.develop.profile.camera_colour == CameraColour::Macos)
        .map(|p| lightcraft_engine::files::macos_shows(p))
        .fold((0, 0), |(n, off), shows| (n + 1, off + usize::from(!shows)))
}

/// The Export dialog's line for the photos it exports (the selection, else the photo open).
pub(crate) fn export_line(app: &LightcraftApp, format: ExportFormat) -> Option<String> {
    let ids: Vec<PhotoId> =
        if app.session.selection.ids.is_empty() { app.session.active().into_iter().collect() } else { app.session.selection.ids.to_vec() };
    let (n, off) = counts(app, &ids);
    match format {
        ExportFormat::Dng if n > 0 => {
            Some(crate::i18n::tr_format!("{n} of these photos use macOS Colour. A DNG keeps Camera Matching colours.", n = n))
        }
        f if f.is_rendered() && off > 0 => {
            Some(crate::i18n::tr_format!("{n} photos use macOS Colour. This computer exports them with Camera Matching colours.", n = off))
        }
        _ => None,
    }
}

/// The Photo Merge dialog's line: a merge is a new DNG with Camera Matching colours.
pub(crate) fn merge_line(app: &LightcraftApp, ids: &[PhotoId]) -> Option<String> {
    let (n, _) = counts(app, ids);
    (n > 0).then(|| crate::i18n::tr_format!("{n} of these photos use macOS Colour. The merge uses Camera Matching colours.", n = n))
}

/// Shows `line` as the dialogs show their notes.
pub(crate) fn show(ui: &mut egui::Ui, t: &Tokens, id: &str, line: Option<String>) {
    if let Some(line) = line {
        let r = ui.add(egui::Label::new(egui::RichText::new(line).color(t.text_dim)).wrap());
        crate::widgets::register(ui, id, r.rect);
    }
}

#[cfg(test)]
mod tests {
    use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
    use lightcraft_develop::CameraColour;
    use lightcraft_engine::export::ExportFormat;

    use crate::{LightcraftApp, Services};

    fn app(colours: &[CameraColour]) -> LightcraftApp {
        let mut s = lightcraft_engine::Session::new();
        for (i, c) in colours.iter().enumerate() {
            let id = PhotoId(i as u64 + 1);
            let mut p = Photo::new(id, Source::File { path: format!("/scratch/{i}.arw") }, "a.arw", "ARW", 7008, 4672, "2026-10-28");
            p.kind = MediaKind::Raw;
            p.meta.camera = "SONY ILCE-7M5".into();
            let mut d = (*p.develop).clone();
            d.profile.camera_colour = *c;
            p.develop = std::sync::Arc::new(d);
            s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
            s.selection.ids.push(id);
        }
        LightcraftApp::new(s, Services { png: None, ..Default::default() })
    }

    /// T13 (dialogs): a DNG export and a merge of photos on macOS Colour say they keep Camera Matching colours; a
    /// rendered export says so only where this computer can't make macOS Colour; nothing for Camera Matching photos.
    #[test]
    fn dialogs_say_when_macos_colour_is_not_kept() {
        let a = app(&[CameraColour::Macos, CameraColour::Auto, CameraColour::Macos]);
        let ids: Vec<PhotoId> = a.session.selection.ids.to_vec();
        assert_eq!(super::counts(&a, &ids).0, 2);
        assert_eq!(
            super::export_line(&a, ExportFormat::Dng).as_deref(),
            Some("2 of these photos use macOS Colour. A DNG keeps Camera Matching colours.")
        );
        assert_eq!(super::merge_line(&a, &ids).as_deref(), Some("2 of these photos use macOS Colour. The merge uses Camera Matching colours."));
        let here = a.session.catalog.photo(PhotoId(1)).is_some_and(|p| lightcraft_engine::files::macos_shows(p));
        let jpeg = super::export_line(&a, ExportFormat::Jpeg);
        if here {
            assert_eq!(jpeg, None, "this Mac exports them with macOS Colour");
        } else {
            assert_eq!(jpeg.as_deref(), Some("2 photos use macOS Colour. This computer exports them with Camera Matching colours."));
        }
        let plain = app(&[CameraColour::Auto]);
        assert_eq!(super::export_line(&plain, ExportFormat::Dng), None);
        assert_eq!(super::merge_line(&plain, &[PhotoId(1)]), None);
    }
}
