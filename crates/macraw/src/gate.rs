//! The name gate, the size gate and the type hints for bytes input. Pure, so every system's tests
//! cover them.

use crate::Error;

/// Apple's size may differ from LightCraft's by this fraction of each edge, no more.
pub(crate) const SIZE_TOLERANCE: f64 = 0.02;

/// True when one of Apple's camera names (`Sony Alpha ILCE-7M5`) ends with the EXIF model
/// (`ILCE-7M5`) as whole words, compared case-insensitively and ignoring `-` and spaces. LightCraft's
/// joined `SONY ILCE-7M5` never matches: the maker word sits elsewhere in Apple's names.
pub(crate) fn model_listed<'a>(exif_model: &str, apple_names: impl IntoIterator<Item = &'a str>) -> bool {
    let want = squash(exif_model);
    if want.is_empty() {
        return false;
    }
    apple_names.into_iter().any(|name| {
        let words: Vec<&str> = name.split_whitespace().collect();
        (1..=words.len()).any(|k| words.get(words.len() - k..).is_some_and(|tail| squash(&tail.concat()) == want))
    })
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| *c != '-' && !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

/// The size gate: Apple's upright size against LightCraft's oriented size. Within
/// [`SIZE_TOLERANCE`] on both edges the render is resampled to exactly `expected`; otherwise
/// [`Error::SizeMismatch`].
pub(crate) fn size_gate(apple: (u32, u32), expected: (u32, u32)) -> Result<(u32, u32), Error> {
    let near = |a: u32, e: u32| e > 0 && (f64::from(a) - f64::from(e)).abs() <= SIZE_TOLERANCE * f64::from(e);
    if near(apple.0, expected.0) && near(apple.1, expected.1) { Ok(expected) } else { Err(Error::SizeMismatch { apple, expected }) }
}

/// The exact type hint Core Image needs for a raw format's bytes, by file extension. Only verified
/// formats are listed (a wrong hint gives an empty picture); others are offered only from a path.
pub fn uti_for_extension(extension: &str) -> Option<&'static str> {
    const VERIFIED: &[(&str, &str)] = &[("arw", "com.sony.arw-raw-image")];
    VERIFIED.iter().find(|(ext, _)| ext.eq_ignore_ascii_case(extension)).map(|(_, uti)| *uti)
}

#[cfg(test)]
mod tests {
    use super::{model_listed, size_gate, uti_for_extension};
    use crate::Error;

    const APPLE: &[&str] = &["Sony Alpha ILCE-7M4", "Sony Alpha ILCE-7M5", "Nikon Z 9", "Canon EOS R5"];

    /// T5b: the EXIF model matches Apple's last words; the joined LightCraft name does not.
    #[test]
    fn name_gate() {
        assert!(model_listed("ILCE-7M5", APPLE.iter().copied()));
        assert!(model_listed("ilce 7m5", APPLE.iter().copied()));
        assert!(model_listed("ILCE7M5", APPLE.iter().copied()));
        assert!(!model_listed("ILCE-7M3", APPLE.iter().copied()));
        assert!(!model_listed("ILCE-7M5", ["Sony Alpha ILCE-7M4"]));
        assert!(!model_listed("SONY ILCE-7M5", APPLE.iter().copied()), "maker + model is not Apple's form");
        assert!(!model_listed("7M5", APPLE.iter().copied()), "whole words only");
        assert!(model_listed("Z 9", APPLE.iter().copied()));
        assert!(model_listed("EOS R5", APPLE.iter().copied()));
        assert!(!model_listed("", APPLE.iter().copied()));
        assert!(!model_listed(" - ", APPLE.iter().copied()));
        assert!(!model_listed("ILCE-7M5", []));
    }

    /// T5c: 2 % on each edge, inclusive; inside it the target is exactly LightCraft's size.
    #[test]
    fn size_gate_at_two_percent() {
        assert_eq!(size_gate((7008, 4672), (7008, 4672)), Ok((7008, 4672)));
        assert_eq!(size_gate((7148, 4672), (7008, 4672)), Ok((7008, 4672)), "140 px = 1.998 %");
        assert_eq!(size_gate((6868, 4579), (7008, 4672)), Ok((7008, 4672)));
        assert_eq!(
            size_gate((7149, 4672), (7008, 4672)),
            Err(Error::SizeMismatch { apple: (7149, 4672), expected: (7008, 4672) }),
            "141 px = 2.01 %"
        );
        assert!(size_gate((7008, 4766), (7008, 4672)).is_err());
        assert!(size_gate((4672, 7008), (7008, 4672)).is_err(), "orientation must agree");
        assert!(size_gate((0, 0), (0, 0)).is_err());
    }

    #[test]
    fn only_verified_type_hints() {
        assert_eq!(uti_for_extension("ARW"), Some("com.sony.arw-raw-image"));
        assert_eq!(uti_for_extension("arw"), Some("com.sony.arw-raw-image"));
        assert_eq!(uti_for_extension("CR3"), None);
        assert_eq!(uti_for_extension(""), None);
    }
}
