//! The autocomplete behind the condition editor: what can be typed at the cursor (context keys and
//! their negated forms, operators, the values of the key being compared), each with a one-line
//! description. Pure text in, list out, so it is tested without a window.

use std::ops::Range;

use super::keys::{self, KEYS, Kind};
use super::parse::{Tok, lex};

/// What kind of thing a suggestion is (the editor tints them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Key,
    Value,
    Operator,
    Group,
}

/// One suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// What replaces the word at the cursor.
    pub insert: String,
    /// What the list shows: `view`, `!textFocus`, `==`.
    pub label: String,
    pub detail: String,
    pub kind: ItemKind,
}

/// The suggestions at a cursor and the part of the text they replace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Suggestions {
    pub replace: Range<usize>,
    pub items: Vec<Item>,
}

/// What may come next, from the text before the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Next {
    Operand,
    AfterKey(usize),
    AfterOp(usize),
    AfterValue,
    Invalid,
}

fn next_after(tokens: &[super::parse::Token]) -> (Next, usize) {
    let (mut state, mut depth) = (Next::Operand, 0usize);
    for t in tokens {
        state = match (state, &t.tok) {
            (Next::Operand, Tok::Not) => Next::Operand,
            (Next::Operand, Tok::LParen) => {
                depth += 1;
                Next::Operand
            }
            (Next::Operand, Tok::Ident(n)) => {
                keys::key_index(n).map_or(Next::Invalid, |k| if KEYS[k].is_bool() { Next::AfterValue } else { Next::AfterKey(k) })
            }
            (Next::AfterKey(k), Tok::Eq | Tok::Ne) => Next::AfterOp(k),
            (Next::AfterOp(_), Tok::Ident(_)) => Next::AfterValue,
            (Next::AfterValue, Tok::And | Tok::Or) => Next::Operand,
            (Next::AfterValue, Tok::RParen) if depth > 0 => {
                depth -= 1;
                Next::AfterValue
            }
            _ => Next::Invalid,
        };
    }
    (state, depth)
}

fn item(insert: &str, label: &str, detail: impl Into<String>, kind: ItemKind) -> Item {
    Item { insert: insert.to_string(), label: label.to_string(), detail: detail.into(), kind }
}

/// 0 = starts with the word, 1 = contains it somewhere (name, chip or description), `None` = no match.
fn score(word: &str, fields: &[&str]) -> Option<u8> {
    if word.is_empty() || fields.first().is_some_and(|f| f.to_lowercase().starts_with(word)) {
        return Some(0);
    }
    fields.iter().any(|f| f.to_lowercase().contains(word)).then_some(1)
}

/// The suggestions for `text` with the cursor at byte `cursor` (clamped to the text).
pub fn suggest(text: &str, cursor: usize) -> Suggestions {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let head = text.get(..cursor).unwrap_or_default();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    // the word being typed, and the rest of it after the cursor
    let word_start = head.char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(cursor, |(i, _)| i);
    let word_end = cursor + text.get(cursor..).unwrap_or_default().chars().take_while(|c| is_word(*c)).map(char::len_utf8).sum::<usize>();
    let (mut start, mut end) = (word_start, word_end);
    let mut partial = head.get(word_start..).unwrap_or_default().to_lowercase();
    // no word: an operator typed so far (`&`, `=`, `!`)
    if partial.is_empty() {
        let ops = head.chars().rev().take(2).take_while(|c| "&|=!".contains(*c)).count();
        if ops > 0 {
            start = cursor - ops;
            end = cursor;
            partial = head.get(start..).unwrap_or_default().to_string();
        }
    }
    let before = head.get(..start).unwrap_or_default();
    let Ok(tokens) = lex(before) else { return Suggestions::default() };
    let (mut next, depth) = next_after(&tokens);
    // a lone `!` is "not" when an operand is expected: the keys follow it
    if next == Next::Operand && partial == "!" {
        start = cursor;
        end = cursor;
        partial.clear();
    }
    let is_word_partial = partial.chars().next().is_some_and(is_word);
    let mut items = Vec::new();
    match next {
        Next::Operand if partial.is_empty() || is_word_partial => {
            let mut scored: Vec<(u8, Item)> = Vec::new();
            for k in KEYS {
                let fields = [k.name, k.title, k.desc];
                if let Some(s) = score(&partial, &fields) {
                    scored.push((s, item(&format!("{} ", k.name), k.name, k.desc, ItemKind::Key)));
                    // the negated form of an on / off key is offered beside it
                    if matches!(k.kind, Kind::Bool { .. }) {
                        scored.push((s, item(&format!("!{} ", k.name), &format!("!{}", k.name), format!("Not: {}", k.desc), ItemKind::Key)));
                    }
                }
            }
            scored.sort_by_key(|(s, _)| *s);
            items.extend(scored.into_iter().map(|(_, i)| i));
            if partial.is_empty() {
                items.push(item("!", "!", "Not: the next part must not hold, as in !textFocus or !(view == grid)", ItemKind::Group));
                items.push(item("!(", "!(", "Not a group: none of what is in the parentheses may hold", ItemKind::Group));
                items.push(item("(", "(", "Group parts together", ItemKind::Group));
            }
        }
        Next::AfterKey(k) if !is_word_partial => {
            let name = KEYS[k].name;
            for (op, detail) in [("==", format!("Is: {name} equals the value")), ("!=", format!("Is not: {name} is anything but the value"))] {
                if op.starts_with(partial.as_str()) {
                    items.push(item(&format!("{op} "), op, detail, ItemKind::Operator));
                }
            }
        }
        Next::AfterOp(k) if partial.is_empty() || is_word_partial => {
            let mut scored: Vec<(u8, Item)> = KEYS[k]
                .values()
                .iter()
                .filter_map(|v| {
                    let s = score(&partial, &[v.name, v.chip, v.desc])?;
                    Some((s, item(&format!("{} ", v.name), v.name, format!("{}: {}", v.chip, v.desc), ItemKind::Value)))
                })
                .collect();
            scored.sort_by_key(|(s, _)| *s);
            items.extend(scored.into_iter().map(|(_, i)| i));
        }
        Next::AfterValue if !is_word_partial => {
            let mut ops = vec![("&&", "And: both parts must hold"), ("||", "Or: either part may hold")];
            if depth > 0 {
                ops.push((")", "Close the group"));
            }
            for (op, detail) in ops {
                if op.starts_with(partial.as_str()) {
                    items.push(item(&format!("{op}{}", if op == ")" { "" } else { " " }), op, detail, ItemKind::Operator));
                }
            }
        }
        _ => next = Next::Invalid,
    }
    if next == Next::Invalid {
        return Suggestions::default();
    }
    Suggestions { replace: start..end, items }
}

/// `text` with suggestion `item` taken, and where the cursor goes (a byte offset).
pub fn apply(text: &str, s: &Suggestions, item: &Item) -> (String, usize) {
    let start = s.replace.start.min(text.len());
    let end = s.replace.end.min(text.len()).max(start);
    let mut out = String::with_capacity(text.len() + item.insert.len());
    out.push_str(text.get(..start).unwrap_or_default());
    out.push_str(&item.insert);
    let cursor = out.len();
    out.push_str(text.get(end..).unwrap_or_default());
    (out, cursor)
}
