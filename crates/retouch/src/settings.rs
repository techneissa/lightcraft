//! The Retouch settings block: one [`Features`] for every face, and per-face overrides.
//!
//! [`Features`] is generated from the feature list in `lib.rs` (`features!`): one field per feature module, each
//! that module's own settings struct. A feature adds a slider by adding a field and a [`Control`] to its own module;
//! nothing here changes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::control::{Control, Spec};
use crate::feature::{Feature, Scope, Stage};
use crate::input::{Face, KeyFrame, KeyMap, NormRect};
use crate::refine::Refine;

/// A feature as the panel and the control list see it, without its settings type.
#[derive(Clone, Copy, Debug)]
pub struct FeatureInfo {
    pub key: &'static str,
    pub label: &'static str,
    pub stage: Stage,
    pub scope: Scope,
    /// Its sliders' specs, in panel order.
    pub specs: fn() -> Vec<&'static Spec>,
    /// The part masks it reads ([`Feature::PARTS`]).
    pub parts: &'static [crate::Part],
    /// How far past its face and masks it reads ([`Feature::REACH`]).
    pub reach: f64,
    /// The masks a Refine Add stroke widens ([`Feature::ADD_PARTS`]); empty: the panel offers no Add.
    pub add_parts: &'static [crate::Part],
}

/// What retouching reads around a face: the largest [`Feature::REACH`] of the features on, and the parts whose
/// masks they read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reads {
    pub reach: f64,
    pub parts: Vec<crate::Part>,
    /// Where Refine Add strokes of the features that are on can widen their masks: the box of their points
    /// (normalised, the frame [`crate::View::to_edit`] speaks) and their largest radius (a fraction of the long
    /// edge). A face's reads reach there too.
    pub add: Option<([f64; 4], f64)>,
}

impl Reads {
    pub(crate) fn add(&mut self, reach: f64, parts: &[crate::Part]) {
        if reach.is_finite() {
            self.reach = self.reach.max(reach);
        }
        for p in parts {
            if !self.parts.contains(p) {
                self.parts.push(*p);
            }
        }
    }

    fn merge(&mut self, o: &Reads) {
        self.add(o.reach, &o.parts);
        if let Some(b) = o.add {
            self.add_area(b);
        }
    }

    fn add_area(&mut self, (b, r): ([f64; 4], f64)) {
        self.add = Some(match self.add {
            None => (b, r),
            Some((a, ra)) => ([a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])], ra.max(r)),
        });
    }
}

pub(crate) fn specs_of<T: Feature>() -> Vec<&'static Spec> {
    T::CONTROLS.iter().map(|c| &c.spec).collect()
}

/// The control of feature `T` named `id`, if `id` is one of `T`'s.
pub(crate) fn control_of<T: Feature>(id: &str) -> Option<&'static Control<T>> {
    let rest = id.strip_prefix("retouch.")?;
    rest.strip_prefix(T::KEY).filter(|r| r.starts_with('.'))?;
    crate::control::find(T::CONTROLS, id)
}

/// A feature's settings from JSON: a missing or malformed value is the feature off, and never fails the rest.
pub(crate) fn lenient<T: Feature>(v: Option<&Value>) -> T {
    v.and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// Which face per-face settings belong to: its person when the face is named, else where its box was.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FaceKey {
    /// The person's name (trimmed, lower case), when the face was named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// The face's box when the settings were made, on the photo before the user's rotation ([`Face::key_bounds`]).
    pub bounds: NormRect,
    /// The frame `bounds` is in (keys saved before it was kept are [`KeyFrame::Edit`]): a face of the other frame is
    /// compared after the photo's lens corrections and transform map one into the other ([`crate::overrides`]).
    #[serde(skip_serializing_if = "KeyFrame::is_edit")]
    pub frame: KeyFrame,
}

impl FaceKey {
    /// The key of `face` as detected now.
    pub fn of(face: &Face) -> FaceKey {
        FaceKey { person: face.person.clone(), bounds: face.key_bounds(), frame: face.key_frame }
    }
}

/// One face's own settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FaceOverride {
    pub face: FaceKey,
    pub settings: crate::Features,
    /// The face's own Refine strokes ([`crate::refine`]), copied from those for every face when it got its own.
    #[serde(skip_serializing_if = "Refine::is_empty")]
    pub refine: Refine,
}

/// The whole block: `global` for every face, `faces` for faces with their own settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Retouch {
    pub global: crate::Features,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub faces: Vec<FaceOverride>,
    /// The Refine strokes for every face ([`crate::refine`]).
    #[serde(skip_serializing_if = "Refine::is_empty")]
    pub refine: Refine,
}

/// Most faces with their own settings kept per photo.
pub const MAX_OVERRIDES: usize = 200;

impl Retouch {
    /// Nothing set: what a photo without the block reads as. Not written to settings.
    pub fn is_default(&self) -> bool {
        *self == Retouch::default()
    }

    /// Whether the block changes no pixel: every feature is off for every face.
    pub fn is_off(&self) -> bool {
        self.global.is_off() && self.faces.iter().all(|f| f.settings.is_off())
    }

    /// What the features on for some face read ([`Features::reads`], and where their Refine Add strokes widen
    /// masks); `None` while the block is off.
    pub fn reads(&self) -> Option<Reads> {
        let mut out: Option<Reads> = None;
        let sets = std::iter::once((&self.global, &self.refine)).chain(self.faces.iter().map(|f| (&f.settings, &f.refine)));
        for (features, refine) in sets {
            let Some(mut r) = features.reads() else { continue };
            let adds = crate::FEATURES
                .iter()
                .filter(|f| !f.add_parts.is_empty() && features.is_on(f.key))
                .flat_map(|f| refine.strokes(f.key))
                .filter(|s| s.mode == crate::refine::RefineMode::Add);
            if let Some(b) = crate::refine::points_box(adds) {
                r.add_area(b);
            }
            out.get_or_insert_with(Reads::default).merge(&r);
        }
        out
    }

    /// The block from JSON, leniently: a malformed part reads as off and the rest is kept, so one bad value never
    /// makes a photo's settings unreadable. Values are kept within their sliders' ranges.
    pub fn from_value(v: &Value) -> Retouch {
        let global = crate::Features::from_value(v.get("global").unwrap_or(&Value::Null));
        let refine = Refine::from_value(v.get("refine").unwrap_or(&Value::Null));
        let faces = v
            .get("faces")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .take(MAX_OVERRIDES)
                    .filter_map(|f| {
                        let face: FaceKey = serde_json::from_value(f.get("face")?.clone()).ok()?;
                        face.bounds.is_valid().then(|| FaceOverride {
                            face,
                            settings: crate::Features::from_value(f.get("settings").unwrap_or(&Value::Null)),
                            refine: Refine::from_value(f.get("refine").unwrap_or(&Value::Null)),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Retouch { global, faces, refine }
    }

    /// Settings for every face (no face of its own, no Refine strokes).
    pub fn with_global(global: crate::Features) -> Retouch {
        Retouch { global, ..Retouch::default() }
    }

    /// The settings face `index` of `faces` renders with: its own when an override is matched to it
    /// ([`crate::overrides::resolve`]), else the global ones.
    pub fn settings_for<'a>(&'a self, faces: &[Face], index: usize) -> &'a crate::Features {
        self.settings_for_in(faces, index, None)
    }

    /// [`Retouch::settings_for`], with `map` to compare keys of the other frame ([`crate::overrides::resolve_in`]).
    pub fn settings_for_in<'a>(&'a self, faces: &[Face], index: usize, map: Option<&dyn KeyMap>) -> &'a crate::Features {
        crate::overrides::resolve_in(self, faces, map)
            .get(index)
            .copied()
            .flatten()
            .and_then(|i| self.faces.get(i))
            .map_or(&self.global, |o| &o.settings)
    }

    /// Face `index`'s own settings, made from the global ones when it has none yet (`None` for no such face, or
    /// when [`MAX_OVERRIDES`] faces already have theirs).
    pub fn override_mut(&mut self, faces: &[Face], index: usize) -> Option<&mut crate::Features> {
        self.override_mut_in(faces, index, None)
    }

    /// [`Retouch::override_mut`], with `map` to compare keys of the other frame ([`crate::overrides::resolve_in`]).
    pub fn override_mut_in(&mut self, faces: &[Face], index: usize, map: Option<&dyn KeyMap>) -> Option<&mut crate::Features> {
        self.own_mut_in(faces, index, map).map(|o| &mut o.settings)
    }

    /// Face `index`'s own entry (its settings and Refine strokes), made from the global ones when it has none yet;
    /// as [`Retouch::override_mut_in`].
    pub fn own_mut_in(&mut self, faces: &[Face], index: usize, map: Option<&dyn KeyMap>) -> Option<&mut FaceOverride> {
        let face = faces.get(index)?;
        let found = crate::overrides::resolve_in(self, faces, map).get(index).copied().flatten();
        let i = match found {
            Some(i) => i,
            None => {
                if self.faces.len() >= MAX_OVERRIDES || !face.bounds.is_valid() {
                    return None;
                }
                self.faces.push(FaceOverride { face: FaceKey::of(face), settings: self.global.clone(), refine: self.refine.clone() });
                self.faces.len() - 1
            }
        };
        self.faces.get_mut(i)
    }

    /// Remove face `index`'s own settings, so it follows the global ones again. Whether it had any.
    pub fn remove_override(&mut self, faces: &[Face], index: usize) -> bool {
        self.remove_override_in(faces, index, None)
    }

    /// [`Retouch::remove_override`], with `map` to compare keys of the other frame ([`crate::overrides::resolve_in`]).
    pub fn remove_override_in(&mut self, faces: &[Face], index: usize, map: Option<&dyn KeyMap>) -> bool {
        match crate::overrides::resolve_in(self, faces, map).get(index).copied().flatten() {
            Some(i) if i < self.faces.len() => {
                self.faces.remove(i);
                true
            }
            _ => false,
        }
    }

    /// Every value kept within its slider's range.
    pub fn clamp_all(&mut self) {
        self.global.clamp_all();
        for f in &mut self.faces {
            f.settings.clamp_all();
        }
    }
}

impl<'de> Deserialize<'de> for Retouch {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Retouch, D::Error> {
        Ok(Retouch::from_value(&Value::deserialize(d)?))
    }
}

/// The feature list: the settings struct with one field per feature, its control dispatch and the per-stage run.
/// Called once, in `lib.rs`; the order is the render order.
macro_rules! features {
    ($($key:ident : $ty:ty),* $(,)?) => {
        /// One settings struct per feature: what every face gets, or one face's own ([`Retouch::faces`]).
        #[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
        pub struct Features {
            $(pub $key: $ty,)*
        }

        /// Every feature, in render order.
        pub static FEATURES: &[$crate::settings::FeatureInfo] = &[
            $($crate::settings::FeatureInfo {
                key: <$ty as $crate::Feature>::KEY,
                label: <$ty as $crate::Feature>::LABEL,
                stage: <$ty as $crate::Feature>::STAGE,
                scope: <$ty as $crate::Feature>::SCOPE,
                specs: $crate::settings::specs_of::<$ty>,
                parts: <$ty as $crate::Feature>::PARTS,
                reach: <$ty as $crate::Feature>::REACH,
                add_parts: <$ty as $crate::Feature>::ADD_PARTS,
            },)*
        ];

        impl Features {
            /// Every feature off.
            pub fn is_off(&self) -> bool {
                true $(&& $crate::Feature::is_off(&self.$key))*
            }

            /// Feature `key` is on (false for an unknown key).
            pub fn is_on(&self, key: &str) -> bool {
                $(if key == <$ty as $crate::Feature>::KEY {
                    return !$crate::Feature::is_off(&self.$key);
                })*
                false
            }

            /// What the features that are on read: the largest [`Feature::REACH`] and the parts' masks
            /// ([`Feature::PARTS`]); `None` while every feature is off.
            pub fn reads(&self) -> Option<$crate::settings::Reads> {
                let mut out: Option<$crate::settings::Reads> = None;
                $(if !$crate::Feature::is_off(&self.$key) {
                    out.get_or_insert_with(Default::default).add(<$ty as $crate::Feature>::REACH, <$ty as $crate::Feature>::PARTS);
                })*
                out
            }

            /// From JSON, one feature at a time: a missing or malformed feature reads as off.
            pub fn from_value(v: &serde_json::Value) -> Features {
                let mut f = Features { $($key: $crate::settings::lenient(v.get(stringify!($key))),)* };
                f.clamp_all();
                f
            }

            /// The value of control `id` (`retouch.<feature>.<field>`).
            pub fn get(&self, id: &str) -> Option<f64> {
                $(if let Some(c) = $crate::settings::control_of::<$ty>(id) {
                    return Some(c.read(&self.$key));
                })*
                None
            }

            /// Set control `id`, kept within its range. False for an unknown id.
            pub fn set(&mut self, id: &str, v: f64) -> bool {
                $(if let Some(c) = $crate::settings::control_of::<$ty>(id) {
                    c.write(&mut self.$key, v);
                    return true;
                })*
                false
            }

            /// Every value kept within its slider's range.
            pub fn clamp_all(&mut self) {
                $(for c in <$ty as $crate::Feature>::CONTROLS {
                    let v = c.read(&self.$key);
                    c.write(&mut self.$key, v);
                })*
            }

            /// Reset one feature (by key) to off. False for an unknown key.
            pub fn reset_feature(&mut self, key: &str) -> bool {
                $(if key == <$ty as $crate::Feature>::KEY {
                    self.$key = <$ty>::default();
                    return true;
                })*
                false
            }
        }

        impl<'de> serde::Deserialize<'de> for Features {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Features, D::Error> {
                Ok(Features::from_value(&<serde_json::Value as serde::Deserialize>::deserialize(d)?))
            }
        }

        /// Run every feature of `stage`, in order: `global` with its Refine strokes for the picture, and each face
        /// of `per_face` with its own.
        pub(crate) fn run_stage(
            img: &mut lightcraft_raster::Rgb32f,
            scene: &$crate::Scene<'_>,
            global: (&Features, &$crate::refine::Refine),
            per_face: &[(&Features, &$crate::refine::Refine)],
            stage: $crate::Stage,
        ) {
            $($crate::run::run_one::<$ty>(img, scene, (&global.0.$key, global.1), per_face.iter().map(|f| (&f.0.$key, f.1)), stage);)*
        }
    };
}

pub(crate) use features;
