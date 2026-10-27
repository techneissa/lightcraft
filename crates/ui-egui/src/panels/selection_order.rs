//! The order of a multi-photo selection, shown as a number in each selected thumbnail's badge
//! bar (1 = the first photo picked; ⌘-click adds at the end, Shift-click adds the range from the
//! last photo picked towards the clicked one). File ▸ Export Animated GIF… takes the photos in this
//! order.

use egui::{Color32, pos2, vec2};
use lightcraft_catalog::PhotoId;

use crate::theme::Tokens;

/// `id`'s place in the selection, from 1, when two or more photos are selected.
pub fn number(session: &lightcraft_engine::Session, id: PhotoId) -> Option<usize> {
    let ids = &session.selection.ids;
    if ids.len() < 2 {
        return None;
    }
    ids.iter().position(|x| *x == id).map(|i| i + 1)
}

/// Paint `n` as plain white text whose right edge is at `right`, centred on `y`, with a dark
/// outline so it reads on any photo; the width it took.
pub fn paint(p: &egui::Painter, right: f32, y: f32, n: usize, t: &Tokens) -> f32 {
    let g = p.layout_no_wrap(n.to_string(), t.semibold(12.0), Color32::WHITE);
    let pos = pos2(right - g.size().x, y - g.size().y / 2.0);
    let outline = p.layout_no_wrap(n.to_string(), t.semibold(12.0), Color32::from_black_alpha(200));
    for d in [vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(0.0, -1.0), vec2(0.0, 1.0), vec2(1.0, 1.0)] {
        p.galley(pos + d, outline.clone(), Color32::from_black_alpha(200));
    }
    let w = g.size().x;
    p.galley(pos, g, Color32::WHITE);
    w
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn numbers_follow_the_order_photos_were_picked() {
        let mut s = lightcraft_engine::Session::with_demo();
        let v: Vec<u64> = s.visible_cloned().into_iter().take(4).map(|x| x.0).collect();
        s.execute("library.select", &json!({"ids": [v[2]]})).unwrap();
        assert_eq!(number(&s, PhotoId(v[2])), None, "one photo: no numbers");
        s.execute("library.select", &json!({"ids": [v[0]], "mode": "toggle"})).unwrap();
        s.execute("library.select", &json!({"ids": [v[3]], "mode": "toggle"})).unwrap();
        assert_eq!([v[2], v[0], v[3], v[1]].map(|x| number(&s, PhotoId(x))), [Some(1), Some(2), Some(3), None]);
        // taking one out moves the later ones up
        s.execute("library.select", &json!({"ids": [v[0]], "mode": "toggle"})).unwrap();
        assert_eq!(number(&s, PhotoId(v[3])), Some(2));
    }

    #[test]
    fn shift_click_back_numbers_outward_from_the_first_pick() {
        let mut s = lightcraft_engine::Session::with_demo();
        let v: Vec<u64> = s.visible_cloned().into_iter().take(4).map(|x| x.0).collect();
        s.execute("library.select", &json!({"ids": [v[3]]})).unwrap();
        s.execute("library.select", &json!({"ids": [v[0]], "mode": "range"})).unwrap();
        // the clicked photo comes last; the one next to the first pick is 2
        assert_eq!([v[0], v[1], v[2], v[3]].map(|x| number(&s, PhotoId(x))), [Some(4), Some(3), Some(2), Some(1)]);
    }
}
