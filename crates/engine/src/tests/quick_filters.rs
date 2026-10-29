//! Quick filters (bottom bar): `library.filter` takes a rating set (`ratings`) and a flag set (`flags`);
//! the filter bar's range and the quick filters replace each other, the sets work in a browsed Local folder,
//! and a smart album saved before they existed still loads.

use std::path::{Path, PathBuf};

use lightcraft_catalog::{Flag, PhotoId, RatingOp};
use serde_json::json;

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-quick-filters-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn png(p: &Path, seed: u8) {
    let img = lightcraft_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8, (y * 11) as u8, seed, 255]);
    let b =
        crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() }).unwrap();
    std::fs::write(p, b).unwrap();
}

/// The demo library with photo `i` given rating `i % 6` and flag pick / reject / none by `i / 6`.
fn rated() -> (Session, Vec<PhotoId>) {
    let mut s = Session::with_demo();
    let ids = s.visible_cloned();
    for (i, id) in ids.iter().enumerate() {
        s.execute("photo.rate", &json!({"rating": i % 6, "ids": [id.0]})).unwrap();
        let flag = ["pick", "reject", "none"][(i / 6) % 3];
        s.execute("photo.flag", &json!({"flag": flag, "ids": [id.0]})).unwrap();
    }
    (s, ids)
}

fn shown(s: &mut Session) -> Vec<(u8, Flag)> {
    s.visible_cloned().into_iter().filter_map(|id| s.catalog.photo(id)).map(|p| (p.rating, p.flag)).collect()
}

#[test]
fn ratings_and_flags_filter_through_the_command() {
    let (mut s, ids) = rated();
    let all = ids.len();
    assert!(all >= 12, "the demo library is big enough: {all}");
    // 1★ or 2★
    s.execute("library.filter", &json!({"ratings": [1, 2]})).unwrap();
    assert!(!shown(&mut s).is_empty() && shown(&mut s).iter().all(|(r, _)| *r == 1 || *r == 2));
    // and picked: AND
    s.execute("library.filter", &json!({"flags": ["pick"]})).unwrap();
    assert!(!shown(&mut s).is_empty() && shown(&mut s).iter().all(|(r, f)| (*r == 1 || *r == 2) && *f == Flag::Pick));
    // Picked or Unflagged: OR
    s.execute("library.filter", &json!({"ratings": [], "flags": ["pick", "none"]})).unwrap();
    assert!(shown(&mut s).iter().all(|(_, f)| *f != Flag::Reject));
    assert!(shown(&mut s).iter().any(|(_, f)| *f == Flag::Pick) && shown(&mut s).iter().any(|(_, f)| *f == Flag::None));
    // nothing chosen: everything
    s.execute("library.filter", &json!({"flags": []})).unwrap();
    assert_eq!(s.visible_cloned().len(), all);
    assert_eq!(s.filter, Default::default());
}

#[test]
fn the_filter_bar_and_the_quick_filters_show_one_state() {
    let (mut s, _) = rated();
    // the filter bar picks ≥ 4★: the quick filters read it as 4★ and 5★
    s.execute("library.filter", &json!({"rating": 4, "ratingOp": "atLeast"})).unwrap();
    assert_eq!(s.filter.ratings_shown(), Some([false, false, false, false, true, true]));
    // the quick filters tick 2★ on top: one set now says it all, the range is gone
    s.execute("library.filter", &lightcraft_catalog::Filter::ratings_patch(&[false, false, true, false, true, true])).unwrap();
    assert_eq!((s.filter.rating, s.filter.ratings.clone()), (0, vec![2, 4, 5]));
    // the filter bar picks = 3★ again: the set goes, so the filter bar's stars are the whole story
    s.execute("library.filter", &json!({"rating": 3, "ratingOp": "exactly"})).unwrap();
    assert_eq!((s.filter.rating, s.filter.rating_op, s.filter.ratings.clone()), (3, RatingOp::Exactly, vec![]));
    assert!(shown(&mut s).iter().all(|(r, _)| *r == 3));
    // flags: the filter bar's single flag, then the quick filters' set
    s.execute("library.filter", &json!({"flag": "pick"})).unwrap();
    assert_eq!(s.filter.flags_shown(), Some(vec![Flag::Pick]));
    s.execute("library.filter", &lightcraft_catalog::Filter::flags_patch(&[Flag::Pick, Flag::Reject])).unwrap();
    assert_eq!((s.filter.flag, s.filter.flags.clone()), (None, vec![Flag::Pick, Flag::Reject]));
    // clearing resets both groups
    s.execute("library.clearFilter", &json!({})).unwrap();
    assert_eq!(s.filter, Default::default());
}

#[test]
fn chips_name_the_sets_and_remove_them() {
    let (mut s, _) = rated();
    s.execute("library.filter", &json!({"ratings": [0, 2], "flags": ["pick", "none"]})).unwrap();
    let chips = crate::filter_chips(&s.filter, &s.catalog);
    let labels: Vec<&str> = chips.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["Rating: Unrated, 2★", "Flag: Picked, Unflagged"]);
    for c in chips {
        s.execute("library.filter", &c.clear).unwrap();
    }
    assert_eq!(s.filter, Default::default());
}

#[test]
fn a_smart_album_saved_before_the_sets_still_works() {
    let (mut s, _) = rated();
    // an old rule (no sets) and a new one (a set)
    let old = s.execute("album.createSmart", &json!({"name": "Old", "rules": {"rating": 4, "ratingOp": "exactly", "flag": "pick"}})).unwrap()["id"]
        .as_u64()
        .unwrap();
    let new = s.execute("album.createSmart", &json!({"name": "New", "rules": {"ratings": [1, 2], "flags": ["pick", "reject"]}})).unwrap()["id"]
        .as_u64()
        .unwrap();
    for (id, ok) in [(old, (|r: u8, f: Flag| r == 4 && f == Flag::Pick) as fn(u8, Flag) -> bool), (new, |r, f| (r == 1 || r == 2) && f != Flag::None)]
    {
        s.execute("library.source", &json!({"kind": "album", "id": id})).unwrap();
        let got = shown(&mut s);
        assert!(!got.is_empty() && got.iter().all(|(r, f)| ok(*r, *f)), "album {id}: {got:?}");
    }
}

#[test]
fn browsed_local_folders_are_filtered_by_the_sets() {
    let dir = temp_dir("local");
    for (i, n) in ["a", "b", "c", "d"].into_iter().enumerate() {
        png(&dir.join(format!("{n}.png")), i as u8 + 1);
    }
    let mut s = Session::new().with_fs();
    s.execute("library.browse", &json!({"path": dir.to_string_lossy()})).unwrap();
    let mut ids: Vec<PhotoId> = s.visible_cloned();
    ids.sort();
    assert_eq!(ids.len(), 4);
    for (id, (rating, flag)) in ids.iter().zip([(1, "pick"), (2, "none"), (2, "pick"), (5, "reject")]) {
        s.execute("photo.rate", &json!({"rating": rating, "ids": [id.0]})).unwrap();
        s.execute("photo.flag", &json!({"flag": flag, "ids": [id.0]})).unwrap();
    }
    s.execute("library.filter", &json!({"ratings": [1, 2]})).unwrap();
    assert_eq!(s.visible_cloned().len(), 3);
    s.execute("library.filter", &json!({"flags": ["pick"]})).unwrap();
    let mut got = s.visible_cloned();
    got.sort();
    assert_eq!(got, vec![ids[0], ids[2]]);
    // the count says so: 2 of 4
    assert_eq!(s.source_total(), Some(4));
    s.execute("library.clearFilter", &json!({})).unwrap();
    assert_eq!(s.visible_cloned().len(), 4);
}
