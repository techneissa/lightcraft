//! The order of a multi-photo selection, shown as a number in each selected thumbnail's badge
//! bar (1 = the first photo picked; ⌘-click adds at the end, Shift-click adds the range in grid
//! order). File ▸ Export Animated GIF… takes the photos in this order.

use egui::{Color32, Rect, pos2, vec2};
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

/// Paint `n` as a pill whose right edge is at `right`, centred on `y`; the width it took.
pub fn paint(p: &egui::Painter, right: f32, y: f32, n: usize, t: &Tokens) -> f32 {
    let g = p.layout_no_wrap(n.to_string(), t.semibold(10.5), Color32::WHITE);
    let w = (g.size().x + 10.0).max(18.0);
    let r = Rect::from_min_max(pos2(right - w, y - 9.0), pos2(right, y + 9.0));
    p.rect_filled(r, 9.0, t.accent);
    p.galley(r.center() - g.size() / 2.0 + vec2(0.0, 0.5), g, Color32::WHITE);
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
}
