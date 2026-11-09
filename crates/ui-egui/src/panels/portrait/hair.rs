//! The Portrait tab's section of the Stray Hair feature (`lightcraft_retouch::hair`): Amount, for every face or the
//! picked one, and what the strand search found.

use lightcraft_develop::retouch::Face;
use lightcraft_develop::retouch::input::Searched;
use serde_json::json;

use crate::i18n::{plural_s, tr, tr_format};
use crate::theme::Tokens;
use crate::widgets::register;

pub(super) fn body(cx: &mut super::Ctx<'_>) {
    cx.slider("retouch.hair.amount");
    let faces: Vec<&Face> = match cx.face {
        Some(i) => cx.input.faces.get(i).into_iter().collect(),
        None => cx.input.faces.iter().collect(),
    };
    let line = readout(&faces);
    let t = Tokens::get(cx.ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 6 }).show(cx.ui, |ui| {
        let r = ui.add(egui::Label::new(egui::RichText::new(&line.text).size(11.5).color(t.text_dim)).wrap());
        register(ui, "portrait:hair.report", r.rect);
    });
    if line.settings && crate::widgets::text_button(cx.ui, "portraitHairModels", "Settings…", false).clicked() {
        let _ = cx.app.run("app.settings", json!({"tab": "faces"}));
    }
}

/// What the readout says, and whether it points to Settings.
#[derive(Debug, PartialEq)]
struct Readout {
    text: String,
    settings: bool,
}

/// "Searched 2 Faces · 14 Strands", or why nothing was searched. `faces` are the face picked, or every face.
fn readout(faces: &[&Face]) -> Readout {
    let said = |text: String, settings: bool| Readout { text, settings };
    if faces.is_empty() {
        return said(tr("Faces Not Found Yet").to_string(), false);
    }
    let (mut searched, mut strands, mut small, mut none, mut unsearched) = (0usize, 0usize, 0usize, 0usize, 0usize);
    for f in faces {
        match f.strands.as_ref() {
            None => unsearched += 1,
            Some(s) => match s.searched {
                Searched::Yes => {
                    searched += 1;
                    strands += s.count as usize;
                }
                Searched::TooSmall => small += 1,
                Searched::NoParts => none += 1,
            },
        }
    }
    if unsearched == faces.len() {
        return said(tr("Face Parts Not Found: Settings…").to_string(), true);
    }
    let mut lines = Vec::new();
    if searched > 0 {
        lines.push(tr_format!(
            "Searched {faces} Face{fs} · {strands} Strand{ss}",
            faces = searched,
            fs = plural_s(searched),
            strands = strands,
            ss = plural_s(strands)
        ));
    }
    if small > 0 {
        lines.push(if small == 1 { tr("Face Too Small").to_string() } else { tr_format!("{n} Faces Too Small", n = small) });
    }
    if none > 0 {
        lines.push(if none == 1 { tr("Face Parts Not Found").to_string() } else { tr_format!("{n} Faces Without Parts", n = none) });
    }
    if unsearched > 0 {
        lines.push(tr_format!("{n} Face{s} Not Searched Yet", n = unsearched, s = plural_s(unsearched)));
    }
    said(lines.join("\n"), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::retouch::NormRect;
    use lightcraft_develop::retouch::input::Strands;

    fn face(strands: Option<Strands>) -> Face {
        Face { strands, ..Face::boxed(NormRect::new(0.4, 0.4, 0.6, 0.6)) }
    }

    fn found(count: u32) -> Strands {
        Strands { count, ..Strands::default() }
    }

    fn say(faces: &[Face]) -> String {
        readout(&faces.iter().collect::<Vec<_>>()).text
    }

    #[test]
    fn the_readout_says_what_was_searched_or_why_not() {
        assert_eq!(say(&[]), "Faces Not Found Yet");
        assert_eq!(say(&[face(Some(found(14))), face(Some(found(0)))]), "Searched 2 Faces · 14 Strands");
        assert_eq!(say(&[face(Some(found(1)))]), "Searched 1 Face · 1 Strand");
        let small = Strands { searched: Searched::TooSmall, ..Strands::default() };
        assert_eq!(say(&[face(Some(small.clone()))]), "Face Too Small");
        assert_eq!(say(&[face(Some(found(3))), face(Some(small))]), "Searched 1 Face · 3 Strands\nFace Too Small");
        let no_parts = readout(&[&face(None)]);
        assert_eq!(no_parts, Readout { text: "Face Parts Not Found: Settings…".into(), settings: true });
    }
}
