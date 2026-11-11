//! The order photos are parsed ahead in (issue #200: only photos likely to be retouched): the photo open in the Edit
//! panel and, once the user edits there, the photos ahead of it in the direction of travel (nearest first); then photos
//! with portrait settings, which need their parse to be drawn; then, while the user is idle, photos with faces they kept
//! (picked or starred), newest first. A photo is queued once, for its most urgent reason. Photos only stepped past are
//! never queued: none behind the open one, none while culling (outside the Edit panel), and no other photo.

use std::collections::HashSet;

use lightcraft_catalog::PhotoId;

/// Why a photo is queued, most urgent first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Why {
    /// Open in the Edit panel, or ahead of it while the user edits.
    Near,
    /// It has portrait settings: drawing it needs its parse.
    Portrait,
    /// A photo with faces the user kept (picked or starred, not rejected), parsed only while the user is idle.
    Kept,
}

impl Why {
    pub fn id(self) -> &'static str {
        match self {
            Why::Near => "near",
            Why::Portrait => "portrait",
            Why::Kept => "kept",
        }
    }
}

/// Photos ahead of the open one (in the direction of travel) that are parsed while the user edits. None behind: the
/// user has stepped past them.
pub const AHEAD: usize = 4;

/// The photo `at` and, with `ahead`, the [`AHEAD`] photos after it in `view` (the filmstrip's order), nearest first.
/// `backward`: the user steps towards the start, so "ahead" is the photos before. Nothing when `at` is not in the view.
pub fn near(view: &[PhotoId], at: PhotoId, backward: bool, ahead: bool) -> Vec<PhotoId> {
    let Some(i) = view.iter().position(|id| *id == at) else { return Vec::new() };
    let n = if ahead { AHEAD } else { 0 };
    (0..=n)
        .filter_map(|d| {
            let j = if backward { i.checked_sub(d)? } else { i.checked_add(d)? };
            view.get(j).copied()
        })
        .collect()
}

/// The queue: `near` as given, then `portrait` and `kept` newest first (by the date string beside each, ISO dates
/// sort as text; ties: the higher id first). Each photo once, for its first reason.
pub fn order(near: &[PhotoId], mut portrait: Vec<(PhotoId, String)>, mut kept: Vec<(PhotoId, String)>) -> Vec<(Why, PhotoId)> {
    let newest_first = |a: &(PhotoId, String), b: &(PhotoId, String)| b.1.cmp(&a.1).then(b.0.0.cmp(&a.0.0));
    portrait.sort_by(newest_first);
    kept.sort_by(newest_first);
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(near.len() + portrait.len() + kept.len());
    let all = near
        .iter()
        .map(|id| (Why::Near, *id))
        .chain(portrait.into_iter().map(|(id, _)| (Why::Portrait, id)))
        .chain(kept.into_iter().map(|(id, _)| (Why::Kept, id)));
    for (why, id) in all {
        if seen.insert(id) {
            out.push((why, id));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[u64]) -> Vec<PhotoId> {
        v.iter().map(|n| PhotoId(*n)).collect()
    }

    #[test]
    fn the_open_photo_comes_first_then_the_photos_ahead_nearest_first_never_those_behind() {
        let view = ids(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(near(&view, PhotoId(5), false, true), ids(&[5, 6, 7, 8, 9]));
        // stepping backward: "ahead" is the photos before
        assert_eq!(near(&view, PhotoId(5), true, true), ids(&[5, 4, 3, 2, 1]));
        // before the user edits: the open photo alone
        assert_eq!(near(&view, PhotoId(5), false, false), ids(&[5]));
        // at the ends: only the photos there are
        assert_eq!(near(&view, PhotoId(9), false, true), ids(&[9, 10]));
        assert_eq!(near(&view, PhotoId(2), true, true), ids(&[2, 1]));
        assert!(near(&view, PhotoId(42), false, true).is_empty());
        assert!(near(&[], PhotoId(1), false, true).is_empty());
    }

    #[test]
    fn near_photos_go_first_then_portraits_then_kept_photos_newest_first_each_once() {
        let portrait =
            vec![(PhotoId(20), "2026-11-09T10:00".into()), (PhotoId(21), "2026-11-09T11:00".into()), (PhotoId(5), "2026-11-09T12:00".into())];
        let kept = vec![
            (PhotoId(30), "2024-01-01".into()),
            (PhotoId(31), "2025-06-01".into()),
            (PhotoId(21), "2026-11-09".into()),
            (PhotoId(32), "2025-06-01".into()),
        ];
        let q = order(&ids(&[5, 6]), portrait, kept);
        let want = vec![
            (Why::Near, PhotoId(5)),
            (Why::Near, PhotoId(6)),
            (Why::Portrait, PhotoId(21)),
            (Why::Portrait, PhotoId(20)),
            (Why::Kept, PhotoId(32)),
            (Why::Kept, PhotoId(31)),
            (Why::Kept, PhotoId(30)),
        ];
        assert_eq!(q, want);
    }
}
