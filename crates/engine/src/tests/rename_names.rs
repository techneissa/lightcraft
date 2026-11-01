//! Renaming albums, masks, presets and versions follows one rule (`crate::names`): the name is
//! trimmed, never empty, and never the name of another item in the same list. A refused name
//! changes nothing and says why.

use serde_json::json;

use crate::Session;

fn album(s: &mut Session, name: &str, parent: Option<u64>) -> u64 {
    let mut p = json!({"name": name});
    if let Some(parent) = parent {
        p["parent"] = json!(parent);
    }
    s.execute("album.create", &p).unwrap()["id"].as_u64().unwrap()
}

fn folder(s: &mut Session, name: &str) -> u64 {
    s.execute("album.create", &json!({"name": name, "folder": true})).unwrap()["id"].as_u64().unwrap()
}

fn album_name(s: &Session, id: u64) -> String {
    s.catalog.album(lightcraft_catalog::AlbumId(id)).unwrap().name.clone()
}

/// Given an album, when it is renamed to nothing or to spaces, then it is refused and keeps its name
/// (`album.rename` used to store any text, untrimmed).
#[test]
fn an_album_is_not_renamed_to_nothing() {
    let mut s = Session::new();
    let a = album(&mut s, "Trip", None);
    for empty in ["", "   "] {
        let e = s.execute("album.rename", &json!({"id": a, "name": empty})).unwrap_err().to_string();
        assert!(e.contains("empty"), "{e}");
    }
    assert_eq!(album_name(&s, a), "Trip");
}

/// Given two albums side by side, when one takes the other's name (in any letter case), then it is
/// refused with the other's name in the reason; the same name in another folder is fine.
#[test]
fn an_album_does_not_take_a_siblings_name() {
    let mut s = Session::new();
    let (a, b) = (album(&mut s, "Trip", None), album(&mut s, "Garden", None));
    let e = s.execute("album.rename", &json!({"id": b, "name": " trip "})).unwrap_err().to_string();
    assert!(e.contains("\u{201c}Trip\u{201d} is already used"), "{e}");
    assert_eq!(album_name(&s, b), "Garden");
    let f = folder(&mut s, "Years");
    let inside = album(&mut s, "Other", Some(f));
    s.execute("album.rename", &json!({"id": inside, "name": "Trip"})).unwrap();
    assert_eq!(album_name(&s, inside), "Trip", "another folder may reuse the name");
    assert_eq!(album_name(&s, a), "Trip");
}

/// A name is saved trimmed; a change of letter case only is allowed; an unchanged name adds no undo step.
#[test]
fn an_album_name_is_trimmed_and_may_change_case() {
    let mut s = Session::new();
    let a = album(&mut s, "Trip", None);
    s.execute("album.rename", &json!({"id": a, "name": "  Holiday  "})).unwrap();
    assert_eq!(album_name(&s, a), "Holiday");
    s.execute("album.rename", &json!({"id": a, "name": "HOLIDAY"})).unwrap();
    assert_eq!(album_name(&s, a), "HOLIDAY");
    let commits = s.commits();
    s.execute("album.rename", &json!({"id": a, "name": "HOLIDAY "})).unwrap();
    assert_eq!(s.commits(), commits, "the same name is not a step");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(album_name(&s, a), "Holiday");
}

/// Masks of one photo keep distinct, non-empty names; the UI used to drop an empty name silently and
/// the command stored it.
#[test]
fn a_mask_is_not_renamed_to_nothing_or_to_another_masks_name() {
    let mut s = Session::with_demo();
    s.execute("mask.add", &json!({"kind": "linear"})).unwrap();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    let d = s.develop_of(s.active().unwrap()).unwrap();
    let (first, second) = (d.masks[0].clone(), d.masks[1].clone());
    s.execute("mask.rename", &json!({"id": first.id, "name": "Sky"})).unwrap();
    let e = s.execute("mask.rename", &json!({"id": second.id, "name": "sky"})).unwrap_err().to_string();
    assert!(e.contains("already used"), "{e}");
    let e = s.execute("mask.rename", &json!({"id": second.id, "name": "  "})).unwrap_err().to_string();
    assert!(e.contains("empty"), "{e}");
    let d = s.develop_of(s.active().unwrap()).unwrap();
    assert_eq!((d.masks[0].name.as_str(), d.masks[1].name.as_str()), ("Sky", second.name.as_str()));
}

/// A component's own name must be new in its mask; an empty name gives it its automatic name back.
#[test]
fn a_component_name_is_new_in_its_mask() {
    let mut s = Session::with_demo();
    s.execute("mask.add", &json!({"kind": "linear"})).unwrap();
    let mid = s.develop_of(s.active().unwrap()).unwrap().masks[0].id;
    s.execute("mask.component", &json!({"id": mid, "component": 0, "action": "duplicate"})).unwrap();
    let rename = |s: &mut Session, k: u64, name: &str| s.execute("mask.component", &json!({"id": mid, "component": k, "action": "rename", "name": name}));
    rename(&mut s, 0, "Horizon").unwrap();
    let e = rename(&mut s, 1, " horizon").unwrap_err().to_string();
    assert!(e.contains("already used"), "{e}");
    let names = |s: &Session| s.develop_of(s.active().unwrap()).unwrap().masks[0].components.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&s), [Some("Horizon".to_string()), None]);
    rename(&mut s, 0, "").unwrap();
    assert_eq!(names(&s), [None, None]);
}

/// Presets of one group, and versions of one photo, keep distinct names too (`preset.rename` and
/// `version.rename` stored a name another item already had).
#[test]
fn presets_and_versions_keep_distinct_names() {
    let mut s = Session::with_demo();
    let a = s.execute("preset.create", &json!({"name": "Warm", "group": "Mine"})).unwrap()["id"].as_str().unwrap().to_string();
    let b = s.execute("preset.create", &json!({"name": "Cool", "group": "Mine"})).unwrap()["id"].as_str().unwrap().to_string();
    let e = s.execute("preset.rename", &json!({"id": b, "name": "warm"})).unwrap_err().to_string();
    assert!(e.contains("\u{201c}Warm\u{201d} is already used"), "{e}");
    assert!(s.execute("preset.rename", &json!({"id": b, "name": " "})).is_err());
    s.execute("preset.rename", &json!({"id": a, "name": "  Warmer "})).unwrap();
    let name_of = |s: &Session, id: &str| s.presets.iter().find(|p| p.id == id).unwrap().name.clone();
    assert_eq!((name_of(&s, &a), name_of(&s, &b)), ("Warmer".to_string(), "Cool".to_string()));

    s.execute("version.create", &json!({"name": "A"})).unwrap();
    s.execute("version.create", &json!({"name": "B"})).unwrap();
    let e = s.execute("version.rename", &json!({"index": 1, "name": "a"})).unwrap_err().to_string();
    assert!(e.contains("already used"), "{e}");
    s.execute("version.rename", &json!({"index": 1, "name": "B2"})).unwrap();
    let id = s.active().unwrap();
    let versions: Vec<_> = s.catalog.photo(id).unwrap().versions.iter().map(|v| v.name.clone()).collect();
    assert_eq!(versions, ["A", "B2"]);
}
