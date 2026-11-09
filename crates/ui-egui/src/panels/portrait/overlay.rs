//! The Portrait tab's face picker on the loupe (fork, `fork/retouch-foundation.md`): the people picker
//! (`panels/detail/people_picker.rs`) at the image view's top-right corner while the Portrait tab is open, with
//! "All" and each face found. No parts row yet (Select People masks fills one). Folded or open is kept in the saved
//! `openFlyouts` ([`FOLDED`]). Hidden while the tool overlay is off (View ▸ Tool Overlay ▸ Never, Shift+H) and in
//! the full-screen preview.

use std::sync::Arc;

use egui::Rect;
use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::Face;

use crate::LightcraftApp;
use crate::panels::detail::people_picker::{self, FaceChip, Picker};
use crate::state::{RightPanel, ToolOverlay};

/// The id in `openFlyouts` that keeps the picker folded (absent: open, the default).
pub(crate) const FOLDED: &str = "portrait.facesFolded";

/// "Face 2", or the person's name.
pub(super) fn face_name(f: &Face, i: usize) -> String {
    f.person.as_deref().map(title_case).unwrap_or_else(|| crate::i18n::tr_format!("Face {n}", n = i + 1))
}

/// Draw the picker over `canvas` (the image view) for `photo`.
pub(crate) fn overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, canvas: Rect, photo: PhotoId) {
    let hidden = app.ui.fullscreen || app.ui.tool_overlay == ToolOverlay::Never;
    if hidden || app.ui.right != RightPanel::Portrait {
        return;
    }
    let st = app.session.retouch_faces_of(photo);
    // (the store's faces, read through their `Arc`: their masks are never copied per frame)
    let input = st.found.as_ref().map(|f| f.input.clone()).unwrap_or_default();
    let faces: &[Face] = &input.faces;
    let textures = st.found.as_ref().map(|f| thumbs(ui.ctx(), photo, f)).unwrap_or_default();
    let chips: Vec<FaceChip> =
        faces.iter().enumerate().map(|(i, f)| FaceChip { texture: textures.get(i).map(|t| t.id()), name: face_name(f, i) }).collect();
    let face = super::picked(ui.ctx(), photo).filter(|i| *i < faces.len());
    let pk = Picker { id: "portrait", faces: &chips, face, parts: &[], selected_parts: &[], folded: app.ui.flyout_open(FOLDED), note: None };
    let picked = people_picker::show(ui, canvas, &pk);
    if picked.toggle_fold {
        app.ui.toggle_flyout(FOLDED);
    }
    if let Some(choice) = picked.face {
        super::pick(ui.ctx(), photo, choice);
    }
}

/// `name` with each word's first letter in capitals (the stored person is lower case).
fn title_case(name: &str) -> String {
    name.split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The textures of the photo's face thumbnails, made once per detection.
fn thumbs(ctx: &egui::Context, photo: PhotoId, found: &lightcraft_engine::retouch_input::Found) -> Arc<Vec<egui::TextureHandle>> {
    let id = egui::Id::unique(("portraitThumbs", photo.0, found.input.fingerprint()));
    if let Some(t) = ctx.data(|d| d.get_temp::<Arc<Vec<egui::TextureHandle>>>(id)) {
        return t;
    }
    let textures: Vec<egui::TextureHandle> = found
        .thumbs
        .iter()
        .enumerate()
        .map(|(i, img)| {
            let bytes: Vec<u8> = img.data.iter().flatten().copied().collect();
            let image = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &bytes);
            ctx.load_texture(format!("portrait-face-{}-{i}", photo.0), image, egui::TextureOptions::LINEAR)
        })
        .collect();
    let textures = Arc::new(textures);
    ctx.data_mut(|d| d.insert_temp(id, textures.clone()));
    textures
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_read_in_title_case() {
        assert_eq!(super::title_case("ann marie"), "Ann Marie");
        assert_eq!(super::title_case(""), "");
    }
}
