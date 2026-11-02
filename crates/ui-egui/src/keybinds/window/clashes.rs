//! Which commands share a key: the table behind the caution colour and the status line.

use std::collections::HashMap;

use crate::shortcuts::{self, Keymap};

use super::super::chord::{self, Step};
use super::super::model;

/// For each binding that shares its key with another command's while the two conditions can hold
/// together: who. Commands separated by conditions (the X pair) are no clash.
pub(super) fn clashes(keymap: &Keymap) -> HashMap<(&'static str, usize), Vec<&'static str>> {
    let mut by_first: HashMap<String, Vec<(&'static str, usize, Vec<Step>)>> = HashMap::new();
    for b in shortcuts::bindable() {
        for slot in 0..model::SLOTS {
            if let Some(steps) = model::effective(keymap, b.id, slot).and_then(chord::parse_chord)
                && let Some(first) = steps.first().and_then(|s| chord::spell(&[*s]))
            {
                by_first.entry(first).or_default().push((b.id, slot, steps));
            }
        }
    }
    let mut out: HashMap<(&'static str, usize), Vec<&'static str>> = HashMap::new();
    for group in by_first.values().filter(|g| g.len() > 1) {
        for (id, slot, steps) in group {
            for (other, _, theirs) in group {
                if other != id && chord::overlaps(steps, theirs) && model::conditions::overlap(keymap, id, other).is_some() {
                    out.entry((*id, *slot)).or_default().push(other);
                }
            }
        }
    }
    out
}

/// The clashes of a keymap, and why each command's is one.
#[derive(Clone)]
pub(super) struct Clashes {
    pub(super) slots: HashMap<(&'static str, usize), Vec<&'static str>>,
    /// Per command: the other command it clashes with and the reason, in words.
    pub(super) reasons: HashMap<&'static str, (&'static str, String)>,
}

impl Clashes {
    pub(super) fn of(keymap: &Keymap) -> Self {
        let slots = clashes(keymap);
        let mut reasons = HashMap::new();
        for ((id, _), others) in &slots {
            if let Some(other) = others.first() {
                reasons.entry(*id).or_insert((*other, reason(keymap, id, other)));
            }
        }
        Clashes { slots, reasons }
    }
}

/// Why two commands on one key clash: when both work.
fn reason(keymap: &Keymap, id: &str, other: &str) -> String {
    use super::super::when::expr;
    let (a, b) = (model::conditions::of(keymap, id), model::conditions::of(keymap, other));
    match expr::overlap(a.as_ref(), b.as_ref()) {
        _ if a.is_none() && b.is_none() => crate::i18n::tr("both always work").to_string(),
        Some(cx) => crate::i18n::tr_format!("both work at the same time, for example when {}", expr::describe(&cx, a.as_ref(), b.as_ref())),
        None => String::new(),
    }
}
