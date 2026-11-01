//! The condition parser: `&&`, `||`, `!`, `==`, `!=` and parentheses over the context keys. It is
//! hand-written, never panics (every index goes through `get`, nesting and length are capped) and
//! says what is wrong and where, so the editor can show it inline.

use std::ops::Range;

use super::expr::Expr;
use super::keys::{self, KEYS, Kind};

/// The longest condition accepted.
pub const MAX_LEN: usize = 400;
/// The deepest nesting of `(` and `!`.
pub const MAX_DEPTH: usize = 24;

/// What is wrong with a condition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    /// Byte range of the offending text (empty at the end of the text).
    pub at: Range<usize>,
    /// The text only stopped too early (more was expected): normal while typing.
    pub incomplete: bool,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    And,
    Or,
    Not,
    LParen,
    RParen,
    Eq,
    Ne,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub at: Range<usize>,
}

fn err(message: impl Into<String>, at: Range<usize>) -> ParseError {
    ParseError { message: message.into(), at, incomplete: false }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Split `src` into tokens. A quoted word (`"detail"`) is an identifier too.
pub fn lex(src: &str) -> Result<Vec<Token>, ParseError> {
    let mut out = Vec::new();
    let mut chars = src.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let one = |tok| Token { tok, at: i..i + c.len_utf8() };
        let next_is = |chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>, want: char| chars.peek().is_some_and(|(_, n)| *n == want);
        match c {
            c if c.is_whitespace() => {}
            '(' => out.push(one(Tok::LParen)),
            ')' => out.push(one(Tok::RParen)),
            '&' if next_is(&mut chars, '&') => {
                chars.next();
                out.push(Token { tok: Tok::And, at: i..i + 2 });
            }
            '|' if next_is(&mut chars, '|') => {
                chars.next();
                out.push(Token { tok: Tok::Or, at: i..i + 2 });
            }
            '=' if next_is(&mut chars, '=') => {
                chars.next();
                out.push(Token { tok: Tok::Eq, at: i..i + 2 });
            }
            '!' if next_is(&mut chars, '=') => {
                chars.next();
                out.push(Token { tok: Tok::Ne, at: i..i + 2 });
            }
            '!' => out.push(one(Tok::Not)),
            '&' => return Err(err("Use && for “and” (two ampersands)", i..i + 1)),
            '|' => return Err(err("Use || for “or” (two bars)", i..i + 1)),
            '=' => return Err(err("Use == to compare (two equals signs)", i..i + 1)),
            '"' | '\'' => {
                let start = i;
                let mut word = String::new();
                let mut end = None;
                for (j, n) in chars.by_ref() {
                    if n == c {
                        end = Some(j + n.len_utf8());
                        break;
                    }
                    word.push(n);
                }
                let Some(end) = end else {
                    return Err(ParseError { message: "This quote is never closed".into(), at: start..src.len(), incomplete: false });
                };
                out.push(Token { tok: Tok::Ident(word), at: start..end });
            }
            c if is_ident(c) => {
                let mut end = i + c.len_utf8();
                let mut word = String::from(c);
                while let Some((j, n)) = chars.peek().copied() {
                    if !is_ident(n) {
                        break;
                    }
                    word.push(n);
                    end = j + n.len_utf8();
                    chars.next();
                }
                out.push(Token { tok: Tok::Ident(word), at: i..end });
            }
            other => return Err(err(format!("Unexpected character “{other}”"), i..i + other.len_utf8())),
        }
    }
    Ok(out)
}

/// The key or value among `names` that `word` is probably a typo of.
fn closest<'a>(word: &str, names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let w = word.to_lowercase();
    names
        .map(|n| (distance(&w, &n.to_lowercase()), n))
        .filter(|(d, n)| *d <= 2 || n.to_lowercase().starts_with(&w) && !w.is_empty())
        .min_by_key(|(d, _)| *d)
        .map(|(_, n)| n)
}

/// Edit distance of two short words.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diag = row.first().copied().unwrap_or(0);
        if let Some(first) = row.first_mut() {
            *first = i + 1;
        }
        for (j, cb) in b.iter().enumerate() {
            let up = row.get(j + 1).copied().unwrap_or(0);
            let left = row.get(j).copied().unwrap_or(0);
            let cost = usize::from(ca != *cb);
            if let Some(cell) = row.get_mut(j + 1) {
                *cell = (diag + cost).min(up + 1).min(left + 1);
            }
            diag = up;
        }
    }
    row.last().copied().unwrap_or(0)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    len: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn at_end(&self) -> Range<usize> {
        self.len..self.len
    }

    fn early(&self, message: impl Into<String>) -> ParseError {
        ParseError { message: message.into(), at: self.at_end(), incomplete: true }
    }

    fn or(&mut self, depth: usize) -> Result<Expr, ParseError> {
        let mut parts = vec![self.and(depth)?];
        while self.peek().is_some_and(|t| t.tok == Tok::Or) {
            self.pos += 1;
            parts.push(self.and(depth)?);
        }
        Ok(if parts.len() == 1 { parts.remove(0) } else { Expr::Or(parts) })
    }

    fn and(&mut self, depth: usize) -> Result<Expr, ParseError> {
        let mut parts = vec![self.unary(depth)?];
        while self.peek().is_some_and(|t| t.tok == Tok::And) {
            self.pos += 1;
            parts.push(self.unary(depth)?);
        }
        Ok(if parts.len() == 1 { parts.remove(0) } else { Expr::And(parts) })
    }

    fn unary(&mut self, depth: usize) -> Result<Expr, ParseError> {
        if depth > MAX_DEPTH {
            return Err(err("Too many nested parentheses or !", self.peek().map_or(self.at_end(), |t| t.at.clone())));
        }
        let Some(token) = self.peek().cloned() else {
            return Err(self.early("The condition stops too early: a key, ! or ( should follow"));
        };
        match token.tok {
            Tok::Not => {
                self.pos += 1;
                Ok(Expr::Not(Box::new(self.unary(depth + 1)?)))
            }
            Tok::LParen => {
                self.pos += 1;
                let inner = self.or(depth + 1)?;
                match self.peek() {
                    Some(t) if t.tok == Tok::RParen => {
                        self.pos += 1;
                        Ok(inner)
                    }
                    Some(t) => Err(err(format!("Expected ) before “{}”", self.text_of(t)), t.at.clone())),
                    None => Err(self.early("A ) is missing to close this (")),
                }
            }
            Tok::Ident(name) => {
                self.pos += 1;
                self.atom(&name, token.at)
            }
            Tok::RParen => Err(err("This ) has no ( before it", token.at)),
            _ => Err(err("A key should come here (or ! or ()", token.at)),
        }
    }

    fn text_of(&self, t: &Token) -> String {
        match &t.tok {
            Tok::Ident(w) => w.clone(),
            Tok::And => "&&".into(),
            Tok::Or => "||".into(),
            Tok::Not => "!".into(),
            Tok::LParen => "(".into(),
            Tok::RParen => ")".into(),
            Tok::Eq => "==".into(),
            Tok::Ne => "!=".into(),
        }
    }

    /// A key, alone or compared with a value.
    fn atom(&mut self, name: &str, at: Range<usize>) -> Result<Expr, ParseError> {
        let Some(key) = keys::key_index(name) else {
            let hint = match name {
                "and" => " (write && for “and”)".to_string(),
                "or" => " (write || for “or”)".to_string(),
                "not" => " (write ! for “not”)".to_string(),
                _ => closest(name, KEYS.iter().map(|k| k.name)).map(|k| format!(", did you mean “{k}”?")).unwrap_or_default(),
            };
            return Err(err(format!("Unknown key “{name}”{hint}"), at));
        };
        let def = &KEYS[key];
        let op = match self.peek() {
            Some(t) if matches!(t.tok, Tok::Eq | Tok::Ne) => {
                let equal = t.tok == Tok::Eq;
                self.pos += 1;
                Some(equal)
            }
            _ => None,
        };
        match (&def.kind, op) {
            (Kind::Bool { .. }, None) => Ok(Expr::Key(key)),
            (Kind::Bool { .. }, Some(equal)) => {
                let Some(t) = self.peek().cloned() else { return Err(self.early(format!("Compare {name} with true or false"))) };
                self.pos += 1;
                match &t.tok {
                    Tok::Ident(v) if v == "true" || v == "false" => {
                        let on = (v == "true") == equal;
                        Ok(if on { Expr::Key(key) } else { Expr::Not(Box::new(Expr::Key(key))) })
                    }
                    _ => Err(err(format!("{name} is on or off: compare it with true or false, or use {name} and !{name}"), t.at)),
                }
            }
            (Kind::Enum(values), None) => {
                let example = values.first().map_or("", |v| v.name);
                let message = format!("{name} needs a value, for example {name} == {example}");
                Err(if self.peek().is_none() { self.early(message) } else { err(message, at) })
            }
            (Kind::Enum(values), Some(equal)) => {
                let Some(t) = self.peek().cloned() else { return Err(self.early(format!("A value of {name} should follow"))) };
                self.pos += 1;
                let Tok::Ident(v) = &t.tok else {
                    return Err(err(format!("A value of {name} should come here"), t.at));
                };
                match keys::value_index(key, v) {
                    Some(value) => Ok(Expr::Is { key, value, equal }),
                    None => {
                        let hint = closest(v, values.iter().map(|x| x.name)).map(|x| format!(", did you mean “{x}”?")).unwrap_or_default();
                        let all: Vec<&str> = values.iter().map(|x| x.name).collect();
                        Err(err(format!("“{v}” is not a value of {name}{hint} (one of: {})", all.join(", ")), t.at))
                    }
                }
            }
        }
    }
}

/// Parse a condition. Blank text is no condition (`Ok(None)`: the shortcut always works).
pub fn parse(src: &str) -> Result<Option<Expr>, ParseError> {
    if src.trim().is_empty() {
        return Ok(None);
    }
    if src.len() > MAX_LEN {
        return Err(err(format!("The condition is too long (at most {MAX_LEN} characters)"), 0..0));
    }
    let tokens = lex(src)?;
    let mut p = Parser { tokens: &tokens, pos: 0, len: src.len() };
    let expr = p.or(0)?;
    if let Some(t) = p.peek() {
        let what = p.text_of(t);
        return Err(err(format!("Unexpected “{what}”: join conditions with && or ||"), t.at.clone()));
    }
    Ok(Some(expr))
}
