//! The order photos are parsed ahead in: the photos around the open one first (nearest first, in the direction of
//! travel first), then photos imported since the library opened (newest import first), then, while the user is idle,
//! every other photo with faces (newest first). A photo is queued once, for its most urgent reason.

use std::collections::HashSet;

use lightcraft_catalog::PhotoId;

/// Why a photo is queued, most urgent first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Why {
    /// Around the open photo.
    Near,
    /// Imported since the library opened.
    Imported,
    /// Any other photo with faces, parsed only while the user is idle.
    Idle,
}

impl Why {
    pub fn id(self) -> &'static str {
        match self {
            Why::Near => "near",
            Why::Imported => "imported",
            Why::Idle => "idle",
        }
    }
}

/// Photos ahead of the open one (in the direction of travel) that are parsed.
pub const AHEAD: usize = 4;
/// Photos behind it.
pub const BEHIND: usize = 2;

/// The offsets from the open photo, nearest first; at equal distance the one ahead goes first.
fn offsets() -> Vec<isize> {
    let mut v = vec![0isize];
    for d in 1..=AHEAD.max(BEHIND) {
        if d <= AHEAD {
            v.push(d as isize);
        }
        if d <= BEHIND {
            v.push(-(d as isize));
        }
    }
    v
}

/// The photos around `at` in `view` (the filmstrip's order), the open one first. `backward`: the user steps towards
/// the start, so "ahead" is the photo before. Nothing when `at` is not in the view.
pub fn near(view: &[PhotoId], at: PhotoId, backward: bool) -> Vec<PhotoId> {
    let Some(i) = view.iter().position(|id| *id == at) else { return Vec::new() };
    offsets()
        .into_iter()
        .filter_map(|o| {
            let o = if backward { -o } else { o };
            let j = i.checked_add_signed(o)?;
            view.get(j).copied()
        })
        .collect()
}

/// The queue: `near` as given, then `imported` and `idle` newest first (by the date string beside each, ISO dates
/// sort as text; ties: the higher id first). Each photo once, for its first reason.
pub fn order(near: &[PhotoId], mut imported: Vec<(PhotoId, String)>, mut idle: Vec<(PhotoId, String)>) -> Vec<(Why, PhotoId)> {
    let newest_first = |a: &(PhotoId, String), b: &(PhotoId, String)| b.1.cmp(&a.1).then(b.0.0.cmp(&a.0.0));
    imported.sort_by(newest_first);
    idle.sort_by(newest_first);
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(near.len() + imported.len() + idle.len());
    let all = near
        .iter()
        .map(|id| (Why::Near, *id))
        .chain(imported.into_iter().map(|(id, _)| (Why::Imported, id)))
        .chain(idle.into_iter().map(|(id, _)| (Why::Idle, id)));
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
    fn the_open_photo_comes_first_then_its_neighbours_nearest_first_ahead_before_behind() {
        let view = ids(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(near(&view, PhotoId(5), false), ids(&[5, 6, 4, 7, 3, 8, 9]));
        // stepping backward: "ahead" is the photo before
        assert_eq!(near(&view, PhotoId(5), true), ids(&[5, 4, 6, 3, 7, 2, 1]));
        // at the ends: only the photos there are
        assert_eq!(near(&view, PhotoId(1), false), ids(&[1, 2, 3, 4, 5]));
        assert_eq!(near(&view, PhotoId(10), false), ids(&[10, 9, 8]));
        assert!(near(&view, PhotoId(42), false).is_empty());
        assert!(near(&[], PhotoId(1), false).is_empty());
    }

    #[test]
    fn near_photos_go_first_then_new_imports_then_the_rest_newest_first_each_once() {
        let imported =
            vec![(PhotoId(20), "2026-11-09T10:00".into()), (PhotoId(21), "2026-11-09T11:00".into()), (PhotoId(5), "2026-11-09T12:00".into())];
        let idle = vec![
            (PhotoId(30), "2024-01-01".into()),
            (PhotoId(31), "2025-06-01".into()),
            (PhotoId(21), "2026-11-09".into()),
            (PhotoId(32), "2025-06-01".into()),
        ];
        let q = order(&ids(&[5, 6]), imported, idle);
        let want = vec![
            (Why::Near, PhotoId(5)),
            (Why::Near, PhotoId(6)),
            (Why::Imported, PhotoId(21)),
            (Why::Imported, PhotoId(20)),
            (Why::Idle, PhotoId(32)),
            (Why::Idle, PhotoId(31)),
            (Why::Idle, PhotoId(30)),
        ];
        assert_eq!(q, want);
    }
}
