//! The search box of the Keyboard Shortcuts window understands modifier words: `cmd`, `shift`,
//! `ctrl` and `alt` (also `command`, `control`, `option`, `opt`) match the shortcuts that use ⌘, ⇧,
//! ⌃ and ⌥, words combine (`cmd shift` is a shortcut with both), and the symbols work too (`⌘⇧`,
//! `⌘n`, `cmd+shift+k`). Anything else still matches command names, menus and ids.

const CMD: u8 = 1;
const CTRL: u8 = 2;
const ALT: u8 = 4;
const SHIFT: u8 = 8;

fn word_mask(word: &str) -> u8 {
    match word {
        "command" | "cmd" => CMD,
        "control" | "ctrl" => CTRL,
        "option" | "opt" | "alt" => ALT,
        "shift" => SHIFT,
        _ => 0,
    }
}

fn symbol_mask(c: char) -> u8 {
    match c {
        '⌘' => CMD,
        '⌃' => CTRL,
        '⌥' => ALT,
        '⇧' => SHIFT,
        _ => 0,
    }
}

/// A search, read once per list build.
#[derive(Debug, PartialEq, Eq)]
pub struct Query {
    /// The whole search, for a search without modifiers.
    whole: String,
    /// The modifiers a shortcut must use (a set of the four bits above).
    mods: u8,
    /// The key a step must end in (`⌘n`, `cmd+shift+k`).
    key: Option<String>,
    /// What is left of the search once the modifiers are taken out: matches names and menus.
    text: String,
}

/// Read `needle` (already lower case).
pub fn parse(needle: &str) -> Query {
    let mut query = Query { whole: needle.to_string(), mods: 0, key: None, text: String::new() };
    let mut rest: Vec<&str> = Vec::new();
    for token in needle.split_whitespace() {
        let word = word_mask(token);
        let lead = token.chars().take_while(|c| symbol_mask(*c) != 0).count();
        if word != 0 {
            query.mods |= word;
        } else if lead > 0 {
            query.mods |= token.chars().take(lead).fold(0, |m, c| m | symbol_mask(c));
            let tail: String = token.chars().skip(lead).collect();
            if !tail.is_empty() {
                query.key = Some(tail);
            }
        } else if let Some((mods, key)) = plus_form(token) {
            query.mods |= mods;
            query.key = key.or(query.key.take());
        } else {
            rest.push(token);
        }
    }
    query.text = rest.join(" ");
    query
}

/// `cmd+shift+k` or `ctrl+alt`: modifier words joined by `+`, then a key or one more modifier.
fn plus_form(token: &str) -> Option<(u8, Option<String>)> {
    let parts: Vec<&str> = token.split('+').filter(|p| !p.is_empty()).collect();
    let (last, lead) = parts.split_last()?;
    if lead.is_empty() || lead.iter().any(|p| word_mask(p) == 0) {
        return None;
    }
    let mods = lead.iter().fold(0, |m, p| m | word_mask(p));
    match word_mask(last) {
        0 => Some((mods, Some((*last).to_string()))),
        m => Some((mods | m, None)),
    }
}

/// The modifiers and the key of one step in the canonical spelling (`Cmd+Shift+K`).
fn split(step: &str) -> (u8, &str) {
    let (head, key) = match step.strip_suffix("++") {
        Some(head) => (head, "+"),
        None => step.rsplit_once('+').unwrap_or(("", step)),
    };
    (head.split('+').fold(0, |m, p| m | word_mask(&p.to_ascii_lowercase())), key)
}

impl Query {
    /// Whether the search is empty.
    pub fn is_empty(&self) -> bool {
        self.whole.is_empty()
    }

    /// Does a command match? `shortcuts` are its shortcuts as saved (`Cmd+K C`), `text_hit` says
    /// whether a piece of text is in its name, menu, id or the shortcuts as shown.
    pub fn matches(&self, shortcuts: &[&str], text_hit: &dyn Fn(&str) -> bool) -> bool {
        if self.whole.is_empty() {
            return true;
        }
        if self.mods == 0 && self.key.is_none() {
            return text_hit(&self.whole);
        }
        (self.text.is_empty() || text_hit(&self.text))
            && shortcuts.iter().flat_map(|sc| sc.split_whitespace()).any(|step| {
                let (mods, key) = split(step);
                mods & self.mods == self.mods && self.key.as_deref().is_none_or(|k| key.eq_ignore_ascii_case(k))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(needle: &str, shortcuts: &[&str], name: &str) -> bool {
        let name = name.to_lowercase();
        parse(&needle.to_lowercase()).matches(shortcuts, &|t| name.contains(t))
    }

    #[test]
    fn modifier_words_match_the_shortcuts_that_use_them() {
        assert!(hit("cmd", &["Cmd+N"], "New"));
        assert!(!hit("cmd", &["N"], "New"));
        assert!(hit("shift", &["Cmd+Shift+S"], "Save As"));
        assert!(!hit("shift", &["Cmd+S"], "Save"));
        assert!(hit("ctrl", &["Ctrl+Alt+K"], "x"));
        assert!(hit("option", &["Alt+K"], "x"));
        assert!(hit("alt", &["Alt+K"], "x"));
    }

    #[test]
    fn words_combine_and_the_symbols_work() {
        assert!(hit("cmd shift", &["Cmd+Shift+S"], "Save As"));
        assert!(!hit("cmd shift", &["Cmd+S", "Shift+S"], "Save"), "both modifiers must be on one step");
        assert!(hit("⌘⇧", &["Cmd+Shift+S"], "Save As"));
        assert!(hit("⌥", &["Alt+K"], "x") && hit("⌃", &["Ctrl+K"], "x"));
        assert!(hit("⌘n", &["Cmd+N"], "New") && !hit("⌘n", &["Cmd+M"], "New"));
        assert!(hit("cmd+shift+k", &["Cmd+Shift+K"], "x") && !hit("cmd+shift+k", &["Cmd+K"], "x"));
        assert!(hit("ctrl+alt", &["Ctrl+Alt+K"], "x"));
    }

    #[test]
    fn a_chord_matches_through_either_step() {
        assert!(hit("cmd", &["Cmd+K C"], "x"));
        assert!(hit("cmd+k", &["Cmd+K C"], "x"));
        assert!(!hit("shift", &["Cmd+K C"], "x"));
    }

    #[test]
    fn words_that_are_not_modifiers_still_match_names() {
        assert!(hit("survey", &[], "Survey"));
        assert!(hit("cmd survey", &["Cmd+N"], "Survey"));
        assert!(!hit("cmd compare", &["Cmd+N"], "Survey"));
        assert!(hit("", &[], "anything"));
    }
}
