//! Quick filters (the bottom bar's stars dropdown and flag buttons): a rating set and a flag set on
//! [`Filter`], next to the filter bar's `rating` / `ratingOp` / `flag`.
//!
//! * `ratings` (0 = unrated … 5) and `flags` are **any-of** sets; empty means no constraint.
//! * Choices inside one set are OR, the sets and every other field AND.
//! * They AND with `rating` / `ratingOp` / `flag` too, so a smart album or saved filter that sets
//!   both still means both. The commands never leave both set by accident: a `library.filter`
//!   patch that sets one side clears the other (see [`Filter::settle_sets`]), so the filter bar and
//!   the quick filters always show one state. Old files have neither field and load unchanged.

use serde_json::{Value, json};

use crate::{Filter, Flag, RatingOp};

/// The flags in the order the buttons show them.
pub const FLAG_ORDER: [Flag; 3] = [Flag::Pick, Flag::Reject, Flag::None];

fn flag_name(f: Flag) -> &'static str {
    match f {
        Flag::Pick => "pick",
        Flag::Reject => "reject",
        Flag::None => "none",
    }
}

impl Filter {
    /// Which star ratings (index 0 = unrated … 5) the rating constraints let through, `rating` /
    /// `ratingOp` and `ratings` together; `None` when neither is set. The quick filters and the
    /// filter bar show this, so they match what is filtered.
    pub fn ratings_shown(&self) -> Option<[bool; 6]> {
        if self.rating == 0 && self.ratings.is_empty() {
            return None;
        }
        let mut out = [true; 6];
        for (stars, on) in (0u8..).zip(out.iter_mut()) {
            if self.rating > 0 {
                *on = match self.rating_op {
                    RatingOp::AtLeast => stars >= self.rating,
                    RatingOp::Exactly => stars == self.rating,
                    RatingOp::AtMost => stars <= self.rating,
                };
            }
            if !self.ratings.is_empty() && !self.ratings.contains(&stars) {
                *on = false;
            }
        }
        Some(out)
    }

    /// The flags `flag` and `flags` let through together, in [`FLAG_ORDER`]; `None` when neither is
    /// set.
    pub fn flags_shown(&self) -> Option<Vec<Flag>> {
        if self.flag.is_none() && self.flags.is_empty() {
            return None;
        }
        Some(
            FLAG_ORDER
                .into_iter()
                .filter(|f| self.flag.is_none_or(|x| x == *f) && (self.flags.is_empty() || self.flags.contains(f)))
                .collect(),
        )
    }

    /// The `library.filter` patch that shows exactly these ratings: none or all of them is no
    /// filter. The range (`rating`) is reset, as the set now says it all.
    pub fn ratings_patch(on: &[bool; 6]) -> Value {
        let set: Vec<u8> = (0u8..).zip(on).filter(|(_, on)| **on).map(|(stars, _)| stars).collect();
        let set = if set.len() == on.len() { Vec::new() } else { set };
        json!({"rating": 0, "ratings": set})
    }

    /// The `library.filter` patch that shows exactly these flags: none or all three is no filter.
    pub fn flags_patch(flags: &[Flag]) -> Value {
        let set: Vec<&str> = FLAG_ORDER.into_iter().filter(|f| flags.contains(f)).map(flag_name).collect();
        let set = if set.len() == FLAG_ORDER.len() { Vec::new() } else { set };
        json!({"flag": Value::Null, "flags": set})
    }

    /// After a `library.filter` patch was merged: the rating range and the rating set replace each
    /// other (so does `flag` and `flags`) when the patch sets only one of them, so the filter bar's
    /// ≥ / = / ≤ stars and the quick filters never fight. A patch that sets both keeps both.
    pub fn settle_sets(&mut self, patch: &Value) {
        let sets = |key: &str| patch.get(key).is_some_and(|v| v.as_array().is_some_and(|a| !a.is_empty()));
        let range = |key: &str| patch.get(key).is_some_and(|v| v.as_u64().is_some_and(|n| n > 0) || v.is_string());
        if patch.get("ratings").is_none() && range("rating") {
            self.ratings.clear();
        } else if sets("ratings") && patch.get("rating").is_none() && patch.get("ratingOp").is_none() {
            self.rating = 0;
        }
        if patch.get("flags").is_none() && range("flag") {
            self.flags.clear();
        } else if sets("flags") && patch.get("flag").is_none() {
            self.flag = None;
        }
    }
}

impl Filter {
    /// The `library.filter` patch that turns `flag` on or off in what the flag filter shows now. One
    /// flag left and no set in use stays the plain `flag` the filter bar always wrote; anything
    /// else is a `flags` set.
    pub fn toggle_flag_patch(&self, flag: Flag) -> Value {
        let mut on = self.flags_shown().unwrap_or_default();
        if on.contains(&flag) {
            on.retain(|f| *f != flag);
        } else {
            on.push(flag);
        }
        if let ([only], true) = (on.as_slice(), self.flags.is_empty()) {
            return json!({"flag": flag_name(*only)});
        }
        Self::flags_patch(&on)
    }

    /// The `library.filter` patch that turns `stars` (0 = unrated … 5) on or off in what the rating
    /// filter shows now (the filter bar's range counts as the stars it lets through).
    pub fn toggle_rating_patch(&self, stars: u8) -> Value {
        let mut on = self.ratings_shown().unwrap_or([false; 6]);
        if let Some(slot) = on.get_mut(usize::from(stars)) {
            *slot = !*slot;
        }
        Self::ratings_patch(&on)
    }

    /// The patch that clears both groups: the quick filters' ✕.
    pub fn clear_quick_patch() -> Value {
        json!({"rating": 0, "ratings": [], "flag": Value::Null, "flags": []})
    }
}
