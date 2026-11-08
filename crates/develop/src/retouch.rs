//! Fork feature (`fork/retouch-foundation.md`): portrait retouching in the develop settings.
//!
//! The settings, input contract and step live in `lightcraft-retouch`, re-exported here so every crate above reaches
//! them through `lightcraft_develop::retouch`. This module turns the features' sliders into develop control specs
//! (section [`Section::Portrait`]):
//! - `retouch.<feature>.<field>` addresses the settings for every face ([`Retouch::global`]);
//! - `retouchFace.<i>.<feature>.<field>` addresses the `i`-th face with its own settings ([`Retouch::faces`]), like
//!   the indexed `pointColor.<i>.<field>` controls (the spec is the `retouch.…` one).

use std::sync::LazyLock;

use serde_json::Value;

pub use lightcraft_retouch::*;

use crate::DevelopSettings;
use crate::controls::{ControlSpec, Section, Track};

/// The settings' JSON key of the block (a namespace of this fork's own: a build without it ignores it).
pub const KEY: &str = "fork_retouch";

/// Prefix of the per-face controls.
pub const FACE_PREFIX: &str = "retouchFace.";

static SPECS: LazyLock<Vec<ControlSpec>> = LazyLock::new(|| {
    lightcraft_retouch::controls()
        .map(|s| ControlSpec {
            id: s.id,
            label: s.label,
            section: Section::Portrait,
            min: s.min,
            max: s.max,
            default: s.default,
            step: s.step,
            decimals: s.decimals,
            track: if s.centered { Track::Centered } else { Track::Plain },
        })
        .collect()
});

/// Every retouching slider (for every face), in panel order.
pub fn specs() -> impl Iterator<Item = &'static ControlSpec> {
    SPECS.iter()
}

/// `retouchFace.3.skin.amount` → (3, `retouch.skin.amount`).
fn split_face(id: &str) -> Option<(usize, String)> {
    let (i, rest) = id.strip_prefix(FACE_PREFIX)?.split_once('.')?;
    if i.is_empty() || !i.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((i.parse().ok()?, format!("retouch.{rest}")))
}

/// The spec of a retouching control id (a per-face id gets its `retouch.…` spec).
pub fn find(id: &str) -> Option<&'static ControlSpec> {
    match split_face(id) {
        Some((_, global)) => SPECS.iter().find(|c| c.id == global),
        None => SPECS.iter().find(|c| c.id == id),
    }
}

/// Read a retouching control (`None`: not one, or no such face).
pub fn get(s: &DevelopSettings, id: &str) -> Option<f64> {
    match split_face(id) {
        Some((i, global)) => s.retouch.faces.get(i)?.settings.get(&global),
        None => s.retouch.global.get(id),
    }
}

/// Write a retouching control, kept within its range. False: not one, or no such face.
pub fn set(s: &mut DevelopSettings, id: &str, v: f64) -> bool {
    match split_face(id) {
        Some((i, global)) => s.retouch.faces.get_mut(i).is_some_and(|f| f.settings.set(&global, v)),
        None => s.retouch.global.set(id, v),
    }
}

/// Every per-face control that exists in `s` (id, spec): `retouchFace.0.skin.amount`, …
pub fn face_instances(s: &DevelopSettings) -> Vec<(String, &'static ControlSpec)> {
    let mut out = Vec::new();
    for i in 0..s.retouch.faces.len() {
        for spec in SPECS.iter() {
            if let Some(rest) = spec.id.strip_prefix("retouch.") {
                out.push((format!("{FACE_PREFIX}{i}.{rest}"), spec));
            }
        }
    }
    out
}

/// Whether the render changes anything: the Portrait section is on and some feature is on for some face.
pub fn active(s: &DevelopSettings) -> bool {
    s.section_enabled("portrait") && !s.retouch.is_off()
}

/// The stored JSON leaves the block out while it is the default, and its face list while it is empty; JSON that is
/// copied, merged or compared key by key (copy / paste, presets, sync) states both, so copying an unretouched photo's
/// Portrait settings turns retouching off and drops the faces' own settings (a merge keeps what a patch leaves out).
pub(crate) fn state_for_merge(v: &mut Value, s: &DevelopSettings) {
    let Some(o) = v.as_object_mut() else { return };
    let block = o.entry(KEY).or_insert_with(|| serde_json::to_value(&s.retouch).unwrap_or(Value::Null));
    if let Some(b) = block.as_object_mut() {
        b.entry("faces").or_insert_with(|| Value::Array(Vec::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SettingsGroup, controls};
    use serde_json::json;

    #[test]
    fn retouch_controls_are_develop_controls() {
        let mut s = DevelopSettings::default();
        assert!(specs().count() >= FEATURES.len());
        for spec in specs() {
            assert_eq!(spec.section, Section::Portrait);
            assert_eq!(controls::find(spec.id).map(|c| c.id), Some(spec.id));
            assert_eq!(controls::get(&s, spec.id), Some(spec.default), "{}", spec.id);
            assert!(controls::set(&mut s, spec.id, spec.max + 5.0), "{}", spec.id);
            assert_eq!(controls::get(&s, spec.id), Some(spec.max));
        }
        assert!(controls::in_section(Section::Portrait).count() == specs().count());
        s.reset_section(Section::Portrait);
        assert!(s.retouch.is_default());
        // the long-standing controls are untouched
        assert_eq!(controls::find("light.exposure").map(|c| c.section), Some(Section::Light));
        assert!(controls::find("retouch.nope").is_none() && !controls::set(&mut s, "retouch.nope", 1.0));
    }

    #[test]
    fn per_face_controls_address_faces_with_their_own_settings() {
        let mut s = DevelopSettings::default();
        assert_eq!(controls::get(&s, "retouchFace.0.skin.amount"), None);
        assert!(!controls::set(&mut s, "retouchFace.0.skin.amount", 20.0), "no such face yet");
        let face = Face::boxed(NormRect::new(0.1, 0.1, 0.3, 0.3));
        s.retouch.override_mut(std::slice::from_ref(&face), 0);
        assert!(controls::set(&mut s, "retouchFace.0.skin.amount", 250.0));
        assert_eq!(controls::get(&s, "retouchFace.0.skin.amount"), Some(100.0), "clamped by the spec");
        assert_eq!(controls::get(&s, "retouch.skin.amount"), Some(0.0), "the global value is apart");
        assert_eq!(controls::find("retouchFace.4.eyes.amount").map(|c| c.id), Some("retouch.eyes.amount"));
        for bad in ["retouchFace.+0.skin.amount", "retouchFace..skin.amount", "retouchFace.0", "retouchFace.x.skin.amount"] {
            assert!(controls::get(&s, bad).is_none() && controls::find(bad).is_none(), "{bad}");
        }
        let ids: Vec<String> = face_instances(&s).into_iter().map(|(id, _)| id).collect();
        assert!(ids.contains(&"retouchFace.0.skin.amount".to_string()));
        assert_eq!(ids.len(), specs().count());
    }

    #[test]
    fn the_block_is_left_out_of_settings_while_it_is_the_default() {
        let s = DevelopSettings::default();
        assert!(s.to_json().get(KEY).is_none());
        assert_eq!(s.hash64(), DevelopSettings::from_json(&s.to_json()).unwrap().hash64());
        let mut r = s.clone();
        controls::set(&mut r, "retouch.eyes.amount", 40.0);
        assert_eq!(r.to_json()[KEY]["global"]["eyes"]["amount"], json!(40.0));
        assert_ne!(r.hash64(), s.hash64());
        assert_eq!(DevelopSettings::from_json(&r.to_json()).unwrap(), r);
    }

    #[test]
    fn a_build_without_the_block_and_a_bad_block_read_as_off() {
        let mut v = DevelopSettings::default().to_json();
        v[KEY] = json!({"global": {"skin": {"amount": [1, 2]}}, "faces": 4});
        let s = DevelopSettings::from_json(&v).unwrap();
        assert!(s.retouch.is_default());
        v[KEY] = json!("garbage");
        assert!(DevelopSettings::from_json(&v).unwrap().retouch.is_default());
    }

    #[test]
    fn the_portrait_group_copies_the_block_and_its_faces() {
        let mut src = DevelopSettings::default();
        let face = Face { person: Some("ann".into()), ..Face::boxed(NormRect::new(0.1, 0.1, 0.3, 0.3)) };
        src.retouch.override_mut(std::slice::from_ref(&face), 0).map(|f| f.set("retouch.skin.amount", 35.0));
        controls::set(&mut src, "retouch.eyes.amount", 25.0);
        let part = crate::extract_groups(&src, &[SettingsGroup::Portrait]);
        let dst = crate::apply_partial(&DevelopSettings::default(), &part, 1.0);
        assert_eq!(dst.retouch, src.retouch);
        // copying an unretouched photo's Portrait settings turns retouching off
        let off = crate::extract_groups(&DevelopSettings::default(), &[SettingsGroup::Portrait]);
        assert!(crate::apply_partial(&src, &off, 1.0).retouch.is_default());
        // and other groups leave it alone
        let light = crate::extract_groups(&DevelopSettings::default(), &[SettingsGroup::Light]);
        assert_eq!(crate::apply_partial(&src, &light, 1.0).retouch, src.retouch);
        assert!(SettingsGroup::default_copy().contains(&SettingsGroup::Portrait));
    }
}
