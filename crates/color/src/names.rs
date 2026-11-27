//! Colour names: the nearest of the CSS named colours to an sRGB colour, by distance in OkLab (so "nearest" follows
//! what the eye sees, not RGB steps). The table is generated (`tools/color_names.py`, source in `assets/ATTRIBUTION.md`).

use std::sync::OnceLock;

use crate::perceptual::oklab_from_2020;
use crate::transfer::srgb_to_linear;
use crate::{REC2020, SRGB};

#[path = "names_table.rs"]
mod table;

/// OkLab `[L, a, b]` of the sRGB colour `rgb` (8 bits per channel).
pub fn oklab_of_srgb8(rgb: [u8; 3]) -> [f32; 3] {
    let lin = rgb.map(|v| srgb_to_linear(f32::from(v) / 255.0));
    oklab_from_2020(SRGB.to_space(&REC2020).apply_f32(lin))
}

/// The CSS named colours with their OkLab values, in table order.
fn labs() -> &'static [[f32; 3]] {
    static LABS: OnceLock<Vec<[f32; 3]>> = OnceLock::new();
    LABS.get_or_init(|| table::NAMED.iter().map(|(_, rgb)| oklab_of_srgb8(*rgb)).collect())
}

/// The English name of the CSS named colour nearest to the sRGB colour `rgb` ("Cornflower Blue"); a tie goes to the name
/// first in alphabetical order.
pub fn nearest_name(rgb: [u8; 3]) -> &'static str {
    let at = oklab_of_srgb8(rgb);
    let dist = |l: &[f32; 3]| (0..3).map(|i| (l[i] - at[i]).powi(2)).sum::<f32>();
    let best = labs().iter().enumerate().min_by(|a, b| dist(a.1).total_cmp(&dist(b.1)));
    best.and_then(|(i, _)| table::NAMED.get(i)).map_or("", |(name, _)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Colours of the list itself come back under their own names, and an in-between colour takes the perceptually
    /// nearest one: a slightly off cornflower blue is still Cornflower Blue, a dark pure red is Dark Red, neutral greys
    /// are grey and not "Black" or "White" in the middle of the range.
    #[test]
    fn nearest_names_of_known_colours() {
        assert_eq!(nearest_name([100, 149, 237]), "Cornflower Blue");
        assert_eq!(nearest_name([102, 150, 235]), "Cornflower Blue");
        assert_eq!(nearest_name([139, 0, 0]), "Dark Red");
        assert_eq!(nearest_name([255, 255, 255]), "White");
        assert_eq!(nearest_name([0, 0, 0]), "Black");
        assert_eq!(nearest_name([128, 128, 128]), "Gray");
        assert_eq!(nearest_name([250, 8, 5]), "Red");
        assert_eq!(nearest_name([60, 180, 80]), "Medium Sea Green");
    }
}
