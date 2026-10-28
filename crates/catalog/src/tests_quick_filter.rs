//! Quick filters: the rating set and flag set on [`Filter`].

use crate::*;

fn library() -> (Catalog, Vec<PhotoId>) {
    let mut c = Catalog::new();
    let mut ids = Vec::new();
    // one photo per rating 0..=5, each picked / rejected / unflagged in turn
    for i in 0..12u32 {
        let id = c.alloc_photo_id();
        let mut p = Photo::new(id, Source::File { path: format!("/lib/IMG_{i}.JPG") }, &format!("IMG_{i}.JPG"), "JPEG", 6, 4, "2026-08-01T10:00:00");
        p.rating = (i % 6) as u8;
        p.flag = [Flag::Pick, Flag::Reject, Flag::None][(i / 6) as usize % 3];
        c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        ids.push(id);
    }
    (c, ids)
}

fn shown(c: &Catalog, f: &Filter) -> Vec<(u8, Flag)> {
    let mut v: Vec<_> = c.query(f, &Sort::default()).into_iter().filter_map(|id| c.photo(id)).map(|p| (p.rating, p.flag)).collect();
    v.sort_by_key(|(r, f)| (*r, *f as u8));
    v
}

#[test]
fn several_ratings_are_or() {
    let (c, _) = library();
    let f = Filter { ratings: vec![1, 2], ..Default::default() };
    let got = shown(&c, &f);
    assert_eq!(got.len(), 4);
    assert!(got.iter().all(|(r, _)| *r == 1 || *r == 2));
    // unrated is a choice of its own
    let f = Filter { ratings: vec![0, 5], ..Default::default() };
    assert!(shown(&c, &f).iter().all(|(r, _)| *r == 0 || *r == 5));
    assert_eq!(shown(&c, &f).len(), 4);
}

#[test]
fn several_flags_are_or() {
    let (c, _) = library();
    let f = Filter { flags: vec![Flag::Pick, Flag::None], ..Default::default() };
    let got = shown(&c, &f);
    assert_eq!(got.len(), 12 - 6, "only the 6 rejected photos are left out");
    assert!(got.iter().all(|(_, fl)| *fl != Flag::Reject));
}

#[test]
fn ratings_and_flags_are_and() {
    let (c, _) = library();
    let f = Filter { ratings: vec![1, 2], flags: vec![Flag::Pick], ..Default::default() };
    // photos 0..6 are picked (ratings 0..=5), so the 1★ and 2★ picks are two photos
    assert_eq!(shown(&c, &f), vec![(1, Flag::Pick), (2, Flag::Pick)]);
}

#[test]
fn nothing_chosen_shows_everything() {
    let (c, ids) = library();
    assert_eq!(c.query(&Filter::default(), &Sort::default()).len(), ids.len());
    assert_eq!(Filter::default().ratings_shown(), None);
    assert_eq!(Filter::default().flags_shown(), None);
    // choosing every rating (or flag) writes no filter, too
    let p = Filter::ratings_patch(&[true; 6]);
    assert_eq!(p["ratings"], serde_json::json!([]));
    let p = Filter::flags_patch(&[Flag::Pick, Flag::Reject, Flag::None]);
    assert_eq!(p["flags"], serde_json::json!([]));
    assert_eq!(Filter::ratings_patch(&[false; 6])["ratings"], serde_json::json!([]));
}

#[test]
fn the_set_combines_with_the_range() {
    let (c, _) = library();
    // ≥ 3 and {1, 4}: both must let a photo through
    let f = Filter { rating: 3, ratings: vec![1, 4], ..Default::default() };
    assert!(shown(&c, &f).iter().all(|(r, _)| *r == 4));
    assert_eq!(f.ratings_shown(), Some([false, false, false, false, true, false]));
    // flag pick and flags {pick, reject}: only picks
    let f = Filter { flag: Some(Flag::Pick), flags: vec![Flag::Pick, Flag::Reject], ..Default::default() };
    assert_eq!(f.flags_shown(), Some(vec![Flag::Pick]));
    assert!(shown(&c, &f).iter().all(|(_, fl)| *fl == Flag::Pick));
}

#[test]
fn the_filter_bars_range_shows_as_a_set() {
    let f = Filter { rating: 4, rating_op: RatingOp::AtLeast, ..Default::default() };
    assert_eq!(f.ratings_shown(), Some([false, false, false, false, true, true]));
    let f = Filter { rating: 2, rating_op: RatingOp::AtMost, ..Default::default() };
    assert_eq!(f.ratings_shown(), Some([true, true, true, false, false, false]));
    let f = Filter { rating: 3, rating_op: RatingOp::Exactly, ..Default::default() };
    assert_eq!(f.ratings_shown(), Some([false, false, false, true, false, false]));
}

#[test]
fn one_side_replaces_the_other() {
    let mut f = Filter { rating: 3, flag: Some(Flag::Pick), ..Default::default() };
    // the quick filters write a set: the range and the single flag go
    let patch = serde_json::json!({"rating": 0, "ratings": [1, 2], "flag": null, "flags": ["pick", "none"]});
    for (k, v) in patch.as_object().unwrap() {
        match k.as_str() {
            "rating" => f.rating = v.as_u64().unwrap() as u8,
            "ratings" => f.ratings = serde_json::from_value(v.clone()).unwrap(),
            "flag" => f.flag = None,
            _ => f.flags = serde_json::from_value(v.clone()).unwrap(),
        }
    }
    f.settle_sets(&patch);
    assert_eq!((f.rating, f.ratings.clone(), f.flag, f.flags.clone()), (0, vec![1, 2], None, vec![Flag::Pick, Flag::None]));
    // the filter bar sets a range or a flag: the sets go
    f.rating = 4;
    f.settle_sets(&serde_json::json!({"rating": 4}));
    assert!(f.ratings.is_empty() && f.rating == 4);
    f.flag = Some(Flag::Reject);
    f.settle_sets(&serde_json::json!({"flag": "reject"}));
    assert!(f.flags.is_empty());
    // a patch that sets both keeps both (a smart album's rules)
    let mut f = Filter { rating: 2, ratings: vec![2, 3], ..Default::default() };
    f.settle_sets(&serde_json::json!({"rating": 2, "ratings": [2, 3]}));
    assert_eq!((f.rating, f.ratings.clone()), (2, vec![2, 3]));
}

#[test]
fn old_filters_load_and_new_ones_round_trip() {
    // a smart album saved before the sets existed
    let old: Filter = serde_json::from_str(r#"{"rating":4,"ratingOp":"exactly","flag":"pick","labels":["red"]}"#).unwrap();
    assert_eq!((old.rating, old.rating_op, old.flag), (4, RatingOp::Exactly, Some(Flag::Pick)));
    assert!(old.ratings.is_empty() && old.flags.is_empty());
    // and writes back without the new keys
    let text = serde_json::to_string(&old).unwrap();
    assert!(!text.contains("ratings") && !text.contains("flags"), "{text}");
    let new = Filter { ratings: vec![0, 2], flags: vec![Flag::Reject], ..Default::default() };
    let text = serde_json::to_string(&new).unwrap();
    assert_eq!(serde_json::from_str::<Filter>(&text).unwrap(), new);
    assert!(new.describe().contains("rating 0 or 2") && new.describe().contains("flag reject"), "{}", new.describe());
}

#[test]
fn local_folder_photos_are_filtered_too() {
    let mut c = Catalog::new();
    for (i, (rating, flag)) in [(1u8, Flag::Pick), (2, Flag::None), (4, Flag::Reject)].into_iter().enumerate() {
        let id = c.alloc_photo_id();
        let mut p = Photo::new(id, Source::File { path: format!("/pics/IMG_{i}.JPG") }, &format!("IMG_{i}.JPG"), "JPEG", 6, 4, "2026-08-01T10:00:00");
        p.local = true;
        p.rating = rating;
        p.flag = flag;
        c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let in_pics = Filter { folder: Some("/pics".into()), ..Default::default() };
    assert_eq!(c.query(&in_pics, &Sort::default()).len(), 3);
    let f = Filter { ratings: vec![1, 4], flags: vec![Flag::Pick, Flag::None], ..in_pics.clone() };
    let got = c.query(&f, &Sort::default());
    assert_eq!(got.len(), 1);
    assert_eq!(c.photo(got[0]).map(|p| p.rating), Some(1));
}
