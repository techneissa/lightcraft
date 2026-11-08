//! Fork feature (`fork/spot-rows.md`): when one spot's circle shows on the photo, and the spot's hidden state.

use serde::{Deserialize, Serialize};

use crate::Spot;

/// When a spot's circle shows on the photo, set on the spot's row in the Remove panel. A spot without one follows the
/// Tool Overlay setting (Use Global). Only the circle: whether the spot has an effect is [`Spot::hidden`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpotCircle {
    /// While the pointer is over the photo, and not while a spot is painted.
    Auto,
    Always,
    /// While this spot is the selected one.
    Selected,
    Never,
}

impl SpotCircle {
    pub const ALL: [SpotCircle; 4] = [SpotCircle::Auto, SpotCircle::Always, SpotCircle::Selected, SpotCircle::Never];

    /// The name used in commands and files.
    pub fn name(self) -> &'static str {
        match self {
            SpotCircle::Auto => "auto",
            SpotCircle::Always => "always",
            SpotCircle::Selected => "selected",
            SpotCircle::Never => "never",
        }
    }

    /// The setting a command names (case does not matter); `global` is `Some(None)`, a name that is none of them `None`.
    pub fn from_name(name: &str) -> Option<Option<SpotCircle>> {
        let name = name.trim().to_ascii_lowercase();
        if name == "global" {
            return Some(None);
        }
        SpotCircle::ALL.into_iter().find(|c| c.name() == name).map(Some)
    }
}

impl Spot {
    /// Used by `skip_serializing_if`: a spot that is shown is saved as it always was.
    pub fn is_shown(hidden: &bool) -> bool {
        !*hidden
    }
}

/// `spots` without the hidden ones: what the renderers draw (the CPU and the GPU both start from
/// [`DevelopSettings::effective`](crate::DevelopSettings::effective)).
pub fn without_hidden(spots: &mut Vec<Spot>) {
    spots.retain(|s| !s.hidden);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DevelopSettings;
    use serde_json::json;

    #[test]
    fn a_shown_spot_with_no_circle_is_saved_as_it_always_was() {
        let v = serde_json::to_value(Spot::default()).unwrap_or_default();
        assert!(v.get("hidden").is_none() && v.get("circle").is_none(), "{v}");
    }

    #[test]
    fn hidden_and_circle_round_trip_and_older_files_read_as_shown() {
        let spot = Spot { hidden: true, circle: Some(SpotCircle::Never), ..Default::default() };
        let v = serde_json::to_value(&spot).unwrap_or_default();
        assert_eq!((v["hidden"].clone(), v["circle"].clone()), (json!(true), json!("never")), "{v}");
        let back: Spot = serde_json::from_value(v).unwrap_or_default();
        assert_eq!(back, spot);
        let old: Spot = serde_json::from_value(json!({"size": 0.03})).unwrap_or_default();
        assert!(!old.hidden && old.circle.is_none());
    }

    #[test]
    fn effective_settings_leave_out_hidden_spots_only() {
        let mut d = DevelopSettings {
            spots: vec![Spot::default(), Spot { hidden: true, size: 0.05, ..Default::default() }, Spot { size: 0.07, ..Default::default() }],
            ..Default::default()
        };
        let e = d.effective();
        assert_eq!(e.spots.iter().map(|s| s.size).collect::<Vec<_>>(), vec![0.02, 0.07]);
        assert_eq!(d.spots.len(), 3, "the edit itself keeps the hidden spot");
        d.spots.iter_mut().for_each(|s| s.hidden = false);
        assert!(matches!(d.effective(), std::borrow::Cow::Borrowed(_)), "nothing hidden: borrowed, no copy");
    }

    #[test]
    fn circle_names_parse_and_global_clears_the_setting() {
        assert_eq!(SpotCircle::from_name(" Never "), Some(Some(SpotCircle::Never)));
        assert_eq!(SpotCircle::from_name("global"), Some(None));
        assert_eq!(SpotCircle::from_name("sometimes"), None);
        for c in SpotCircle::ALL {
            assert_eq!(SpotCircle::from_name(c.name()), Some(Some(c)));
        }
    }
}
