//! The parts Select People offers, in the order the picker lists them, and the parser's parts each one draws.

use lightcraft_parsing::Part;

/// One choice of the parts list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pick {
    EntirePerson,
    /// Facial skin, eyebrows, eye whites, irises, lips and teeth together.
    EntireFace,
    FacialSkin,
    BodySkin,
    Hair,
    Eyebrows,
    EyeWhites,
    Iris,
    Lips,
    Teeth,
    /// Every clothes part below.
    Clothes,
    Top,
    Bottom,
    Dress,
    Shoes,
    Hat,
    Bag,
    /// Everything that is not the person (or, for everyone, not anybody).
    Background,
}

impl Pick {
    pub const ALL: [Pick; 18] = [
        Pick::EntirePerson,
        Pick::EntireFace,
        Pick::FacialSkin,
        Pick::BodySkin,
        Pick::Hair,
        Pick::Eyebrows,
        Pick::EyeWhites,
        Pick::Iris,
        Pick::Lips,
        Pick::Teeth,
        Pick::Clothes,
        Pick::Top,
        Pick::Bottom,
        Pick::Dress,
        Pick::Shoes,
        Pick::Hat,
        Pick::Bag,
        Pick::Background,
    ];

    /// The id commands and saved masks use (`entireFace`; the parser's own ids where there is one).
    pub fn id(self) -> &'static str {
        match self {
            Pick::EntirePerson => "entirePerson",
            Pick::EntireFace => "entireFace",
            Pick::FacialSkin => "facialSkin",
            Pick::BodySkin => "bodySkin",
            Pick::Hair => "hair",
            Pick::Eyebrows => "eyebrows",
            Pick::EyeWhites => "eyeWhites",
            Pick::Iris => "iris",
            Pick::Lips => "lips",
            Pick::Teeth => "teeth",
            Pick::Clothes => "clothes",
            Pick::Top => "clothesTop",
            Pick::Bottom => "clothesBottom",
            Pick::Dress => "dress",
            Pick::Shoes => "shoes",
            Pick::Hat => "hat",
            Pick::Bag => "bag",
            Pick::Background => "background",
        }
    }

    /// The name the user sees.
    pub fn label(self) -> &'static str {
        match self {
            Pick::EntirePerson => "Entire Person",
            Pick::EntireFace => "Entire Face",
            Pick::FacialSkin => "Facial Skin",
            Pick::BodySkin => "Body Skin",
            Pick::Hair => "Hair",
            Pick::Eyebrows => "Eyebrows",
            Pick::EyeWhites => "Eyes (Whites)",
            Pick::Iris => "Iris",
            Pick::Lips => "Lips",
            Pick::Teeth => "Teeth",
            Pick::Clothes => "Clothes",
            Pick::Top => "Top",
            Pick::Bottom => "Bottom",
            Pick::Dress => "Dress",
            Pick::Shoes => "Shoes",
            Pick::Hat => "Hat",
            Pick::Bag => "Bag",
            Pick::Background => "Background",
        }
    }

    pub fn from_id(id: &str) -> Option<Pick> {
        Pick::ALL.into_iter().find(|p| p.id() == id)
    }

    /// The parser's parts this choice draws (Background draws none of its own: it is what the people are not).
    pub fn parts(self) -> &'static [Part] {
        match self {
            Pick::EntirePerson => &[Part::EntirePerson],
            Pick::EntireFace => &[Part::FacialSkin, Part::Eyebrows, Part::EyeWhites, Part::Iris, Part::Lips, Part::Teeth],
            Pick::FacialSkin => &[Part::FacialSkin],
            Pick::BodySkin => &[Part::BodySkin],
            Pick::Hair => &[Part::Hair],
            Pick::Eyebrows => &[Part::Eyebrows],
            Pick::EyeWhites => &[Part::EyeWhites],
            Pick::Iris => &[Part::Iris],
            Pick::Lips => &[Part::Lips],
            Pick::Teeth => &[Part::Teeth],
            Pick::Clothes => &[Part::ClothesTop, Part::ClothesBottom, Part::Dress, Part::Shoes, Part::Hat, Part::Bag],
            Pick::Top => &[Part::ClothesTop],
            Pick::Bottom => &[Part::ClothesBottom],
            Pick::Dress => &[Part::Dress],
            Pick::Shoes => &[Part::Shoes],
            Pick::Hat => &[Part::Hat],
            Pick::Bag => &[Part::Bag],
            Pick::Background => &[],
        }
    }
}

/// The choices of a saved mask's part ids, in list order, without repeats or ids this build does not know.
pub fn picks_of(ids: &[String]) -> Vec<Pick> {
    let mut out: Vec<Pick> = ids.iter().filter_map(|id| Pick::from_id(id)).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// "Hair", "Hair + Lips", "Hair + 2 More".
pub fn summary(picks: &[Pick]) -> String {
    match picks {
        [] => "People".into(),
        [one] => one.label().into(),
        [a, b] => format!("{} + {}", a.label(), b.label()),
        [a, rest @ ..] => format!("{} + {} More", a.label(), rest.len()),
    }
}
