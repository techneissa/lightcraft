//! Copy and Paste Portrait Settings (fork, issue #462).

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, SettingsGroup, controls, extract_groups};
use serde_json::json;

use crate::Session;

fn edit(s: &mut Session, photo: u64, f: impl FnOnce(&mut DevelopSettings)) {
    let id = PhotoId(photo);
    let mut d = (*s.develop_of(id).unwrap()).clone();
    f(&mut d);
    s.set_develop(id, d, "Setup").unwrap();
}

fn settings(s: &Session, photo: u64) -> DevelopSettings {
    (*s.develop_of(PhotoId(photo)).unwrap()).clone()
}

#[test]
fn paste_portrait_settings_changes_only_the_portrait_settings() {
    let mut s = Session::with_demo();
    let ids: Vec<u64> = s.catalog.photos().take(2).map(|p| p.id.0).collect();
    let (a, b) = (ids[0], ids[1]);
    s.execute("library.select", &json!({"ids": [a], "active": a})).unwrap();
    edit(&mut s, a, |d| {
        assert!(controls::set(d, "retouch.skin.smoothness", 60.0));
        d.light.exposure = 2.0;
    });
    edit(&mut s, b, |d| {
        d.light.exposure = -1.0;
        d.effects.clarity = 30.0;
    });
    s.execute("library.select", &json!({"ids": [b], "active": b})).unwrap();
    assert!(s.execute("portrait.paste", &json!({})).is_err(), "nothing copied yet");

    s.execute("library.select", &json!({"ids": [a], "active": a})).unwrap();
    s.execute("develop.copy", &json!({})).unwrap();
    let settings_clip = s.clipboard.clone();
    s.execute("portrait.copy", &json!({})).unwrap();
    assert_eq!(s.clipboard, settings_clip, "Copy Portrait Settings leaves what Copy Settings copied");

    s.execute("library.select", &json!({"ids": [b], "active": b})).unwrap();
    let before = settings(&s, b);
    s.execute("portrait.paste", &json!({})).unwrap();
    let after = settings(&s, b);
    let portrait = [SettingsGroup::Portrait];
    assert_eq!(extract_groups(&after, &portrait), extract_groups(&settings(&s, a), &portrait), "Portrait matches the source");
    let rest: Vec<_> = SettingsGroup::ALL.into_iter().filter(|g| *g != SettingsGroup::Portrait).collect();
    assert_eq!(extract_groups(&after, &rest), extract_groups(&before, &rest), "everything outside Portrait is unchanged");
    // one undo step
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(settings(&s, b), before);
}
