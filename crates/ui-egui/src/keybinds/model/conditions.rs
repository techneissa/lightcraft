//! Conditions ("when") in the keymap. A command's condition is the keymap entry `<id>?when`
//! (next to `<id>` and `<id>#2`), the text of a condition ([`super::super::when`]); an old build
//! ignores it as an unknown command, an old file has none (the shortcut always works). It limits
//! both shortcuts of the command.
//!
//! A command can declare a condition: the X pair (Reject Photo outside the Crop tool, Swap Aspect
//! inside it) and the A pair (Visualize Spots, Lock Aspect Ratio) share their key by conditions
//! that exclude each other. A declared condition applies while the command keeps its declared key;
//! a saved entry (an empty one means "always") replaces it.

use crate::shortcuts::{self, Keymap, find_bindable};

use super::super::when::{self, Context, Expr, ParseError, expr};

/// What ends the keymap entry of a condition.
pub const SUFFIX: &str = "?when";

/// Declared conditions: `(command, condition)`. Each pair of [`shortcuts::CONTEXTUAL`] has two that
/// exclude each other (a test keeps it so).
pub const DECLARED: &[(&str, &str)] = &[
    ("photo.reject", "tool != crop"),
    ("crop.rotateAspect", "tool == crop"),
    ("view.visualizeSpots", "tool != crop"),
    ("crop.lockAspect", "tool == crop"),
];

/// The keymap entry of the condition of `id`.
pub fn key(id: &str) -> String {
    format!("{id}{SUFFIX}")
}

/// The command a keymap entry (of a shortcut or of a condition) belongs to.
pub fn command_of(entry: &str) -> &str {
    super::split_key(entry.strip_suffix(SUFFIX).unwrap_or(entry)).0
}

/// The declared condition of `id`.
pub fn declared(id: &str) -> Option<&'static str> {
    DECLARED.iter().find(|(c, _)| *c == id).map(|(_, w)| *w)
}

/// Whether the declared condition of `id` is in force: the command still has its declared key.
fn declared_applies(keymap: &Keymap, id: &str) -> bool {
    match (keymap.get(id), find_bindable(id).and_then(|b| b.default)) {
        (None, _) => true,
        (Some(sc), Some(default)) => shortcuts::parse(sc).is_some() && shortcuts::parse(sc) == shortcuts::parse(default),
        (Some(_), None) => false,
    }
}

/// The saved entry of `id`: its text (`""` = always).
pub fn saved<'a>(keymap: &'a Keymap, id: &str) -> Option<&'a str> {
    keymap.get(&key(id)).map(String::as_str)
}

/// The condition of `id` now, as canonical text (`None` = it always works). A saved condition that
/// doesn't parse is ignored, as if there were none ([`damaged`] lists them).
pub fn text(keymap: &Keymap, id: &str) -> Option<String> {
    if let Some(Ok(parsed)) = saved(keymap, id).map(when::parse) {
        return parsed.map(|e| e.text());
    }
    declared(id).filter(|_| declared_applies(keymap, id)).map(str::to_string)
}

/// The parsed condition of `id` now (`None` = always).
pub fn of(keymap: &Keymap, id: &str) -> Option<Expr> {
    text(keymap, id).and_then(|t| when::parse(&t).ok().flatten())
}

/// Whether the shortcuts of `id` work in `cx`.
pub fn holds(keymap: &Keymap, id: &str, cx: &Context) -> bool {
    of(keymap, id).is_none_or(|e| e.eval(cx))
}

/// A context where the shortcuts of both commands work (`None`: never at the same time).
pub fn overlap(keymap: &Keymap, a: &str, b: &str) -> Option<Context> {
    expr::overlap(of(keymap, a).as_ref(), of(keymap, b).as_ref())
}

/// The saved conditions this build can't read: command, text, what is wrong.
pub fn damaged(keymap: &Keymap) -> Vec<(String, String, ParseError)> {
    keymap
        .iter()
        .filter_map(|(k, v)| {
            let id = k.strip_suffix(SUFFIX)?;
            find_bindable(id)?;
            Some((id.to_string(), v.clone(), when::parse(v).err()?))
        })
        .collect()
}

/// Give `id` the condition `text` (`None` or blank = always), saved as its canonical spelling.
/// Returns that spelling. A condition that doesn't parse is an error and changes nothing.
pub fn set(keymap: &mut Keymap, id: &str, text: Option<&str>) -> Result<Option<String>, String> {
    find_bindable(id).ok_or_else(|| format!("unknown command: {id}"))?;
    let parsed = when::parse(text.unwrap_or_default()).map_err(|e| format!("not a condition: {e}"))?;
    let canonical = parsed.map(|e| e.text());
    let declared = declared(id).filter(|_| declared_applies(keymap, id)).map(|d| when::parse(d).ok().flatten().map(|e| e.text()));
    match (&canonical, declared) {
        // the same as the declared one (or none where none is declared): no entry needed
        (c, Some(Some(d))) if c.as_ref() == Some(&d) => {
            keymap.remove(&key(id));
        }
        (None, None) => {
            keymap.remove(&key(id));
        }
        (c, _) => {
            keymap.insert(key(id), c.clone().unwrap_or_default());
        }
    }
    Ok(canonical)
}

/// Forget the saved condition of `id` (the declared one, if any, comes back).
pub fn clear(keymap: &mut Keymap, id: &str) {
    keymap.remove(&key(id));
}

#[cfg(test)]
#[path = "conditions_tests.rs"]
mod tests;
