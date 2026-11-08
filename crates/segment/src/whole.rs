//! Fork feature (`fork/object-remove.md`): which of SAM's alternatives a click that removes an object takes.
//!
//! One click gives three alternatives, from a part to the whole thing (a shirt, the person wearing it). A mask takes
//! the one with the best predicted IoU, which is often the part: SAM 3 scores the whole person lower than the shirt
//! the click landed on (0.55 against 0.95 on a street photo). Removing takes the whole object: the largest alternative
//! that holds the best one and still scores [`MIN_SCORE`], so a weak guess (the street around the person) is not
//! taken, nor a larger alternative somewhere else.

/// Lowest predicted IoU an alternative needs to be taken for holding the best one.
pub const MIN_SCORE: f32 = 0.5;
/// Share of the best alternative's pixels a larger one must hold.
pub const HOLDS: f64 = 0.9;

/// One of SAM's alternatives: its area (pixels of its grid), predicted IoU, and how many of the best scoring
/// alternative's pixels it holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alternative {
    pub area: usize,
    pub score: f32,
    pub holds: usize,
}

/// The best scoring of `(area, score)` alternatives (the one a mask takes).
pub fn best(candidates: &[(usize, f32)]) -> Option<usize> {
    let score = |s: f32| if s.is_finite() { s } else { f32::MIN };
    candidates.iter().enumerate().max_by(|a, b| score(a.1.1).total_cmp(&score(b.1.1))).map(|(i, _)| i)
}

/// The alternative to remove: the largest that holds [`HOLDS`] of the best one (`best`, an index into `alts`) and
/// scores at least [`MIN_SCORE`], else the best one.
pub fn whole_object(alts: &[Alternative], best: usize) -> usize {
    let Some(b) = alts.get(best) else { return best };
    let holds_best = |a: &Alternative| a.holds as f64 >= HOLDS * b.area as f64;
    alts.iter()
        .enumerate()
        .filter(|(i, a)| *i == best || (a.score.is_finite() && a.score >= MIN_SCORE && a.area > b.area && holds_best(a)))
        .max_by_key(|(_, a)| a.area)
        .map_or(best, |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alt(area: usize, score: f32, holds: usize) -> Alternative {
        Alternative { area, score, holds }
    }

    // SAM 3's answer for one click on a walking man's shirt: the shirt scores best, the man holds it and scores enough
    #[test]
    fn the_whole_person_wins_over_the_shirt_the_click_landed_on() {
        let alts = [alt(476, 0.93, 470), alt(949, 0.55, 487), alt(487, 0.95, 487)];
        let b = best(&alts.iter().map(|a| (a.area, a.score)).collect::<Vec<_>>()).unwrap();
        assert_eq!(b, 2, "a mask takes the shirt");
        assert_eq!(whole_object(&alts, b), 1, "removing takes the man");
    }

    #[test]
    fn a_weak_or_unrelated_larger_alternative_is_not_taken() {
        // the street around a click on trousers scores 0.33
        assert_eq!(whole_object(&[alt(151, 0.32, 140), alt(954, 0.33, 301), alt(301, 0.93, 301)], 2), 2);
        // larger and scoring well, but somewhere else (holds little of the best)
        assert_eq!(whole_object(&[alt(300, 0.9, 300), alt(5_000, 0.8, 20)], 0), 0);
    }

    #[test]
    fn odd_inputs_never_panic() {
        assert_eq!(best(&[]), None);
        assert_eq!(best(&[(10, f32::NAN), (20, 0.9)]), Some(1));
        assert_eq!(whole_object(&[], 0), 0);
        assert_eq!(whole_object(&[alt(10, 0.9, 10)], 5), 5);
        assert_eq!(whole_object(&[alt(10, 0.9, 10), alt(99, f32::NAN, 10)], 0), 0);
    }
}
