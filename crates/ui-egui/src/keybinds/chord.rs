//! A shortcut is one key step (`Cmd+K`) or a chord of two (`Cmd+K C`): the steps are written with a
//! space between them, so every single-step string in an old `ui.json` is already a valid shortcut.

use egui::{Key, Modifiers};

use crate::shortcuts::{format, parse};

/// One key press with its modifiers.
pub type Step = (Modifiers, Key);

/// The most steps a shortcut has: a leader key and one more.
pub const MAX_STEPS: usize = 2;

/// The steps of `sc`, or `None` when it is empty, has more than [`MAX_STEPS`] or any step is not a key.
pub fn parse_chord(sc: &str) -> Option<Vec<Step>> {
    let steps: Vec<Step> = sc.split_whitespace().map(parse).collect::<Option<_>>()?;
    (!steps.is_empty() && steps.len() <= MAX_STEPS).then_some(steps)
}

/// Whether `sc` is a shortcut this build understands (one step or a chord).
pub fn is_valid(sc: &str) -> bool {
    parse_chord(sc).is_some()
}

/// Whether `sc` has more than one step.
pub fn is_chord(sc: &str) -> bool {
    sc.split_whitespace().nth(1).is_some()
}

/// The canonical spelling of `steps` (`Cmd+K C`), so equal shortcuts compare equal as text.
pub fn spell(steps: &[Step]) -> Option<String> {
    let parts: Vec<String> = steps.iter().map(|(m, k)| format(*m, *k)).collect::<Option<_>>()?;
    (!parts.is_empty() && parts.len() <= MAX_STEPS).then(|| parts.join(" "))
}

/// Whether two shortcuts can't both be bound: they are the same, or one is the start of the other
/// (a single key that is also the leader of a chord would fire before the chord could finish).
pub fn overlaps(a: &[Step], b: &[Step]) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// `Cmd+K C` as menus and tooltips show it: each step as [`crate::menubar::shortcut_text`] does
/// (`⌘K C` on macOS), a space between the steps.
pub fn text(sc: &str, mac: bool) -> String {
    sc.split_whitespace().map(|step| crate::menubar::shortcut_text(step, mac)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_step_and_two_step_shortcuts_parse() {
        assert_eq!(parse_chord("Cmd+K").map(|s| s.len()), Some(1));
        let chord = parse_chord("Cmd+K C").unwrap();
        assert_eq!(chord.len(), 2);
        assert_eq!(chord.first().map(|s| s.1), Some(Key::K));
        assert_eq!(chord.get(1), Some(&(Modifiers::NONE, Key::C)));
        assert!(is_chord("Cmd+K C") && !is_chord("Cmd+K"));
    }

    #[test]
    fn junk_does_not_parse() {
        for bad in ["", "   ", "Hyper+K", "Cmd+K C D", "Cmd+K Nope", "Cmd+"] {
            assert!(parse_chord(bad).is_none(), "{bad:?}");
            assert!(!is_valid(bad), "{bad:?}");
        }
    }

    #[test]
    fn spelling_is_canonical() {
        let steps = parse_chord("Shift+Cmd+K   C").unwrap();
        assert_eq!(spell(&steps).as_deref(), Some("Cmd+Shift+K C"));
        assert_eq!(spell(&[]), None);
    }

    #[test]
    fn a_key_overlaps_the_chord_it_leads() {
        let k = parse_chord("Cmd+K").unwrap();
        let kc = parse_chord("Cmd+K C").unwrap();
        let kd = parse_chord("Cmd+K D").unwrap();
        let c = parse_chord("C").unwrap();
        assert!(overlaps(&k, &kc) && overlaps(&kc, &k) && overlaps(&kc, &kc));
        assert!(!overlaps(&kc, &kd) && !overlaps(&c, &kc) && !overlaps(&c, &k));
    }

    #[test]
    fn text_shows_each_step() {
        assert_eq!(text("Cmd+K C", true), "⌘K C");
        assert_eq!(text("Cmd+K Cmd+Shift+C", false), "Ctrl+K Ctrl+Shift+C");
        assert_eq!(text("F5", true), "F5");
    }
}
