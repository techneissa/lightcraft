//! The rule every rename follows: the new name is trimmed, never empty, and never the name of another
//! item in the same list (albums in one folder, masks of one photo, presets, versions…).
//!
//! The commands refuse a bad name with [`NameProblem`]'s words, and the inline rename fields in the UI
//! ask the same function, so a field and its command always agree.

use std::fmt;

/// Why a name can't be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameProblem {
    /// Nothing but spaces, or nothing at all.
    Empty,
    /// Another item in the list has this name (letter case ignored); the other item's spelling.
    Taken(String),
}

impl fmt::Display for NameProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameProblem::Empty => f.write_str("the name is empty"),
            NameProblem::Taken(other) => write!(f, "\u{201c}{other}\u{201d} is already used"),
        }
    }
}

/// `raw` trimmed, when it is not empty and no entry of `others` (the rest of the list, without the
/// item being renamed) has the same name, letter case ignored.
pub fn check_name<'a>(raw: &str, others: impl IntoIterator<Item = &'a str>) -> Result<String, NameProblem> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(NameProblem::Empty);
    }
    let folded = name.to_lowercase();
    match others.into_iter().find(|other| other.trim().to_lowercase() == folded) {
        Some(other) => Err(NameProblem::Taken(other.trim().to_string())),
        None => Ok(name.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_trimmed() {
        assert_eq!(check_name("  Trip  ", ["Other"]), Ok("Trip".to_string()));
    }

    #[test]
    fn an_empty_name_is_refused_with_its_reason() {
        assert_eq!(check_name("   ", ["A"]), Err(NameProblem::Empty));
        assert_eq!(NameProblem::Empty.to_string(), "the name is empty");
    }

    #[test]
    fn a_name_in_use_is_refused_whatever_its_letter_case() {
        let e = check_name("trip", ["Other", " Trip "]).unwrap_err();
        assert_eq!(e, NameProblem::Taken("Trip".to_string()));
        assert_eq!(e.to_string(), "\u{201c}Trip\u{201d} is already used");
    }
}
