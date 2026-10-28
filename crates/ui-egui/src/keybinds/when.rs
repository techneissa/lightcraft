//! Shortcut conditions, VS Code's "when": a shortcut can be limited to a context (`view == detail
//! && tool == crop`, `!textFocus`). Fork feature `feat/shortcut-when`.
//!
//! - [`keys`]: the small typed set of context keys, and the [`Context`] read from the app each frame.
//! - [`parse`]: the hand-written parser (`&&`, `||`, `!`, `==`, `!=`, parentheses) with inline errors.
//! - [`expr`]: the parsed [`Expr`]: evaluation, canonical text, readable chips, overlap of two
//!   conditions (what a conflict is).
//! - [`suggest`]: the autocomplete behind the editor.

pub mod expr;
pub mod keys;
pub mod parse;
pub mod suggest;

pub use expr::{Chip, Expr};
pub use keys::Context;
pub use parse::{ParseError, parse};

#[cfg(test)]
mod tests;
