//! T5's SentencePiece unigram tokenizer, read from the `tokenizer.json` FLUX ships (`tokenizer_2/`).
//!
//! Each word (split at spaces) gets a leading `▁` and is cut into the pieces whose log-probabilities sum highest
//! (Viterbi over the vocabulary); a character no piece covers becomes `<unk>`. The end token follows, then `<pad>` up
//! to the length the pipeline encodes at. The normaliser of the file (NFKC) is not applied: prompts here are plain
//! text, and runs of spaces are collapsed as it does.

use std::collections::HashMap;
use std::path::Path;

use crate::{Error, Result};

const PAD: u32 = 0;
const EOS: u32 = 1;
const UNK: u32 = 2;
/// The longest piece looked up, in characters (T5's vocabulary has none longer than 16).
const MAX_PIECE: usize = 32;

pub struct T5Tokenizer {
    pieces: HashMap<String, (u32, f64)>,
    unk_score: f64,
}

impl T5Tokenizer {
    /// From a Hugging Face `tokenizer.json` with a unigram model.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::Model(format!("T5 tokenizer {}: {e}", path.display())))?;
        let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| Error::Model(format!("T5 tokenizer: {e}")))?;
        let vocab = json
            .get("model")
            .filter(|m| m.get("type").and_then(|t| t.as_str()) == Some("Unigram"))
            .and_then(|m| m.get("vocab"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Model("T5 tokenizer: no unigram vocabulary".into()))?;
        let entries = vocab.iter().filter_map(|e| Some((e.get(0)?.as_str()?.to_string(), e.get(1)?.as_f64()?)));
        Self::new(entries)
    }

    /// From `(piece, log-probability)` pairs, ids in order.
    pub fn new(entries: impl IntoIterator<Item = (String, f64)>) -> Result<Self> {
        let mut pieces = HashMap::new();
        let mut min = 0.0f64;
        for (id, (piece, score)) in entries.into_iter().enumerate() {
            let id = u32::try_from(id).map_err(|_| Error::Model("T5 tokenizer: vocabulary too large".into()))?;
            if id > UNK {
                min = min.min(score);
            }
            pieces.insert(piece, (id, score));
        }
        if pieces.len() <= 3 {
            return Err(Error::Model("T5 tokenizer: empty vocabulary".into()));
        }
        // as SentencePiece: an unknown character costs more than any known piece
        Ok(Self { pieces, unk_score: min - 10.0 })
    }

    /// `text`'s ids, the end token, and `<pad>` up to `len` (cut to `len` with the end token kept).
    pub fn encode(&self, text: &str, len: usize) -> Vec<u32> {
        let mut ids = Vec::new();
        for word in text.split_whitespace() {
            self.word(&format!("\u{2581}{word}"), &mut ids);
        }
        ids.truncate(len.saturating_sub(1));
        ids.push(EOS);
        ids.truncate(len.max(1));
        ids.resize(len.max(ids.len()), PAD);
        ids
    }

    /// The best segmentation of one `▁`-prefixed word.
    fn word(&self, word: &str, out: &mut Vec<u32>) {
        let chars: Vec<char> = word.chars().collect();
        let n = chars.len();
        // best[i] = (score, start of the last piece, its id) of the best cut of chars[..i]
        let mut best: Vec<Option<(f64, usize, u32)>> = vec![None; n + 1];
        if let Some(b) = best.get_mut(0) {
            *b = Some((0.0, 0, PAD));
        }
        for end in 1..=n {
            let mut pick: Option<(f64, usize, u32)> = None;
            for start in end.saturating_sub(MAX_PIECE)..end {
                let Some(Some((before, _, _))) = best.get(start) else { continue };
                let piece: String = chars.get(start..end).map(|s| s.iter().collect()).unwrap_or_default();
                let cand = match self.pieces.get(&piece) {
                    Some(&(id, score)) => Some((before + score, start, id)),
                    None if end - start == 1 => Some((before + self.unk_score, start, UNK)),
                    None => None,
                };
                if let Some(c) = cand
                    && pick.is_none_or(|p| c.0 > p.0)
                {
                    pick = Some(c);
                }
            }
            if let Some(b) = best.get_mut(end) {
                *b = pick;
            }
        }
        let mut rev = Vec::new();
        let mut at = n;
        while at > 0 {
            let Some(Some((_, start, id))) = best.get(at).copied() else { break };
            // consecutive unknown characters become one <unk>, as SentencePiece does
            if !(id == UNK && rev.last() == Some(&UNK)) {
                rev.push(id);
            }
            at = start;
        }
        out.extend(rev.into_iter().rev());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok() -> T5Tokenizer {
        let v = [
            ("<pad>", 0.0),
            ("</s>", 0.0),
            ("<unk>", 0.0),
            ("\u{2581}", -2.0),
            ("a", -3.0),
            ("b", -3.0),
            ("\u{2581}ab", -2.5),
            ("ab", -4.0),
            ("\u{2581}a", -4.0),
        ];
        T5Tokenizer::new(v.iter().map(|(p, s)| (p.to_string(), *s))).unwrap()
    }

    #[test]
    fn picks_the_most_likely_cut() {
        // "▁ab" whole (-2.5) beats "▁" + "ab" (-6) and "▁a" + "b" (-7)
        assert_eq!(tok().encode("ab", 4), vec![6, EOS, PAD, PAD]);
    }

    #[test]
    fn unknown_characters_are_one_unk_and_lengths_hold() {
        let t = tok();
        assert_eq!(t.encode("a  zz", 6), vec![8, 3, UNK, EOS, PAD, PAD]);
        assert_eq!(t.encode("ab ab ab ab", 3), vec![6, 6, EOS]);
        assert_eq!(t.encode("", 2), vec![EOS, PAD]);
        assert_eq!(t.encode("ab", 0), vec![EOS]);
    }
}
