//! What has been run from the Command Palette, so [`super::filter`] can rank it higher: how much and how lately each
//! command was used, and which command was picked for a typed text. Kept with the app settings (`ui.json`), on this
//! computer only, and capped in size. Times are seconds since the Unix epoch, passed in by the caller.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The most commands and the most typed texts remembered.
const MAX_COMMANDS: usize = 100;
const MAX_TYPED: usize = 100;
/// A longer typed text is not remembered: nobody types it again.
const MAX_TYPED_CHARS: usize = 64;
/// After this long without a use, a command's weight has halved.
const HALF_LIFE_SECS: f64 = 14.0 * 24.0 * 3600.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Used {
    /// One per use, each worth less the older it is, as of `last`.
    weight: f64,
    last: u64,
}

impl Used {
    fn at(self, now: u64) -> f64 {
        self.weight * 0.5f64.powf(now.saturating_sub(self.last) as f64 / HALF_LIFE_SECS)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    /// By command key (the id, with the parameters of a menu item that has some).
    commands: BTreeMap<String, Used>,
    /// (typed text, command key), the latest first.
    typed: Vec<(String, String)>,
}

/// A typed text as it is remembered and compared: lower case, one space between words.
fn normalized(text: &str) -> String {
    text.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

impl History {
    /// The command `key` was run from the palette with `query` typed.
    pub fn record(&mut self, query: &str, key: &str, now: u64) {
        let weight = self.frecency(key, now) + 1.0;
        self.commands.insert(key.to_string(), Used { weight, last: now });
        while self.commands.len() > MAX_COMMANDS {
            // the one that matters least goes, never the one just run
            let least = self.commands.iter().filter(|(k, _)| *k != key).min_by(|a, b| a.1.at(now).total_cmp(&b.1.at(now)));
            let Some(least) = least.map(|(k, _)| k.clone()) else { break };
            self.commands.remove(&least);
        }
        let query = normalized(query);
        if !query.is_empty() && query.chars().count() <= MAX_TYPED_CHARS {
            self.typed.retain(|(text, _)| *text != query);
            self.typed.insert(0, (query, key.to_string()));
            self.typed.truncate(MAX_TYPED);
        }
    }

    /// How much and how lately `key` was used (0: never).
    pub fn frecency(&self, key: &str, now: u64) -> f64 {
        self.commands.get(key).map_or(0.0, |u| u.at(now))
    }

    /// The command picked before for this text, or else the latest one picked for a text that starts with it.
    pub fn picked_for(&self, query: &str) -> Option<&str> {
        let query = normalized(query);
        if query.is_empty() {
            return None;
        }
        let found = self.typed.iter().find(|(text, _)| *text == query).or_else(|| self.typed.iter().find(|(text, _)| text.starts_with(&query)));
        found.map(|(_, key)| key.as_str())
    }
}
