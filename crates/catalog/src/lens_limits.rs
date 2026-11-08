//! What a lens name says about the lens: its focal range and widest aperture ("16-35mm f/4", "50mm f/1.8",
//! "18-55mm F3.5-5.6", "70-200mm F2.8 GM"). An edited focal length or aperture (`Meta::exif`) outside them is
//! refused ([`refusal`]); a name that says nothing, or something unreadable, sets no limit. A teleconverter in the
//! name ("+ 1.4x", "TC-20E") scales both limits by its factor; one whose factor cannot be read sets no limit at all.
//! Only edits are checked: the file's own values are shown as they are.

use crate::Meta;
use crate::exif_override::{APERTURE_RANGE, ExifField, FOCAL_RANGE, format_aperture, format_focal};

/// Longest number (in characters) read from a name; a longer one is not a lens spec.
const MAX_DIGITS: usize = 8;
/// How far a focal length may stray from the range (rounding in the file's own values).
const FOCAL_SLACK: f32 = 0.5;
/// How far an f-number may stray below the limit.
const APERTURE_SLACK: f32 = 0.005;

/// The limits a lens name gives; `None` where it says nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LensLimits {
    /// The focal range in millimetres (one value twice for a prime).
    pub focal_mm: Option<(f32, f32)>,
    /// The smallest f-number: the widest the lens opens.
    pub max_aperture: Option<f32>,
}

impl LensLimits {
    /// The limits in `lens`, a lens name as the file or a person wrote it. Never fails: what is not understood is no
    /// limit.
    pub fn of(lens: &str) -> LensLimits {
        let chars: Vec<char> = lens.chars().take(400).collect();
        let base = LensLimits {
            focal_mm: (0..chars.len()).find_map(|i| focal_at(&chars, i)),
            max_aperture: (0..chars.len()).find_map(|i| aperture_at(&chars, i)),
        };
        match converter(&chars) {
            Converter::None => base,
            Converter::By(k) => {
                LensLimits { focal_mm: base.focal_mm.map(|(lo, hi)| (lo * k, hi * k)), max_aperture: base.max_aperture.map(|f| f * k) }
            }
            Converter::Unreadable => LensLimits::default(),
        }
    }

    /// Why an aperture of `f` is not possible on this lens, if it is not.
    pub fn aperture_refusal(&self, f: f32) -> Option<LensRefusal> {
        let limit = self.max_aperture?;
        (f + APERTURE_SLACK < limit).then_some(LensRefusal::Aperture(limit))
    }

    /// Why a focal length of `mm` is not possible on this lens, if it is not.
    pub fn focal_refusal(&self, mm: f32) -> Option<LensRefusal> {
        let (lo, hi) = self.focal_mm?;
        (mm + FOCAL_SLACK < lo || mm - FOCAL_SLACK > hi).then_some(LensRefusal::Focal(lo, hi))
    }
}

/// Why an edited value does not fit the lens. [`Display`](std::fmt::Display) is the English sentence commands
/// answer with; the UI words it in the interface language from the parts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LensRefusal {
    /// The widest the lens opens: the smallest f-number it has.
    Aperture(f32),
    /// The focal range (one value twice for a prime).
    Focal(f32, f32),
}

impl std::fmt::Display for LensRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            LensRefusal::Aperture(limit) => write!(f, "This lens opens to f/{} at most", format_aperture(limit)),
            LensRefusal::Focal(lo, hi) if lo == hi => write!(f, "This lens is {} mm", format_focal(lo)),
            LensRefusal::Focal(lo, hi) => write!(f, "This lens covers {}–{} mm", format_focal(lo), format_focal(hi)),
        }
    }
}

/// Why the edited value of `field` in `m` does not fit the lens `m` shows, if it does not. Only the focal length and
/// the aperture are limited, and only when edited (the file's values are never checked).
pub fn refusal(m: &Meta, field: ExifField) -> Option<LensRefusal> {
    match field {
        ExifField::Focal => LensLimits::of(m.effective_lens()).focal_refusal(m.exif.focal_mm?),
        ExifField::Aperture => LensLimits::of(m.effective_lens()).aperture_refusal(m.exif.aperture?),
        _ => None,
    }
}

fn is_dash(c: char) -> bool {
    matches!(c, '-' | '\u{2013}' | '\u{2014}')
}

fn skip_space(c: &[char], mut i: usize) -> usize {
    while c.get(i).is_some_and(|ch| ch.is_whitespace()) {
        i += 1;
    }
    i
}

/// A number (digits, one decimal point or comma) at `i`, and where it ends; `None` for none, or one too long to be a lens spec.
fn number(c: &[char], i: usize) -> Option<(f32, usize)> {
    let mut end = i;
    let mut dots = 0;
    while let Some(ch) = c.get(end) {
        if ch.is_ascii_digit() {
        } else if matches!(*ch, '.' | ',') && dots == 0 && c.get(end + 1).is_some_and(|n| n.is_ascii_digit()) {
            dots += 1;
        } else {
            break;
        }
        end += 1;
    }
    if end == i || end - i > MAX_DIGITS {
        return None;
    }
    // (a decimal comma reads as a point: "f/1,4")
    let text: String = c.get(i..end)?.iter().map(|ch| if *ch == ',' { '.' } else { *ch }).collect();
    text.parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| (v, end))
}

/// `a` or `a-b` at `i`, and where it ends.
fn span(c: &[char], i: usize) -> Option<((f32, f32), usize)> {
    let (a, end) = number(c, i)?;
    let after = skip_space(c, end);
    if c.get(after).is_some_and(|ch| is_dash(*ch))
        && let Some((b, end)) = number(c, skip_space(c, after + 1))
    {
        return Some(((a.min(b), a.max(b)), end));
    }
    Some(((a, a), end))
}

/// What a lens name says about a teleconverter.
enum Converter {
    /// None is mentioned.
    None,
    /// One of this factor is.
    By(f32),
    /// One is mentioned, but its factor cannot be read.
    Unreadable,
}

/// The factors a teleconverter can have.
const CONVERTER_FACTORS: std::ops::RangeInclusive<f32> = 1.1..=4.0;

/// The teleconverter in a lens name ("+ 1.4x", "+ EF2x", "TC-20E", "Extender 1.4x").
fn converter(chars: &[char]) -> Converter {
    let lower: Vec<char> = chars.iter().flat_map(|c| c.to_lowercase()).collect();
    let text: String = lower.iter().collect();
    let words: Vec<&str> = text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let marked = text.contains('+')
        || ["converter", "extender"].iter().any(|w| text.contains(w))
        || words.iter().any(|w| *w == "tc" || w.strip_prefix("tc").is_some_and(|r| r.starts_with(|d: char| d.is_ascii_digit())));
    if !marked {
        return Converter::None;
    }
    match (0..lower.len()).find_map(|i| converter_factor_at(&lower, i)).filter(|k| CONVERTER_FACTORS.contains(k)) {
        Some(k) => Converter::By(k),
        None => Converter::Unreadable,
    }
}

/// A factor written "1.4x", "x1.4" or "TC-14" starting at `i` (lower case).
fn converter_factor_at(c: &[char], i: usize) -> Option<f32> {
    let here = *c.get(i)?;
    let prev = i.checked_sub(1).and_then(|p| c.get(p)).copied();
    if here.is_ascii_digit() && !prev.is_some_and(|p| p.is_ascii_digit() || p == '.' || p == ',') {
        let (k, end) = number(c, i)?;
        return matches!(c.get(end), Some('x' | '\u{d7}')).then_some(k);
    }
    if matches!(here, 'x' | '\u{d7}') && !prev.is_some_and(char::is_alphanumeric) {
        return number(c, skip_space(c, i + 1)).map(|(k, _)| k);
    }
    if here == 't' && c.get(i + 1) == Some(&'c') && !prev.is_some_and(char::is_alphanumeric) {
        let start = if matches!(c.get(i + 2), Some('-' | ' ')) { i + 3 } else { i + 2 };
        let digits: String = c.get(start..)?.iter().take_while(|d| d.is_ascii_digit()).collect();
        return (digits.len() == 2).then(|| digits.parse::<f32>().ok()).flatten().map(|d| d / 10.0);
    }
    None
}

/// A focal range ("16-35mm", "50 mm") starting at `i`.
fn focal_at(c: &[char], i: usize) -> Option<(f32, f32)> {
    if !c.get(i)?.is_ascii_digit() || i.checked_sub(1).and_then(|p| c.get(p)).is_some_and(|p| p.is_ascii_digit() || *p == '.') {
        return None;
    }
    let ((lo, hi), end) = span(c, i)?;
    let unit = skip_space(c, end);
    let mm = c.get(unit).is_some_and(|m| m.eq_ignore_ascii_case(&'m')) && c.get(unit + 1).is_some_and(|m| m.eq_ignore_ascii_case(&'m'));
    (mm && lo >= FOCAL_RANGE.0 && hi <= FOCAL_RANGE.1).then_some((lo, hi))
}

/// The widest aperture of an "f/4", "F3.5-5.6", "f2.8" or "1:1.8" starting at `i`.
fn aperture_at(c: &[char], i: usize) -> Option<f32> {
    let here = *c.get(i)?;
    let prev = i.checked_sub(1).and_then(|p| c.get(p)).copied();
    let after_mm = i >= 2 && c.get(i - 2..i).is_some_and(|p| p.iter().all(|m| m.eq_ignore_ascii_case(&'m')));
    if prev.is_some_and(|p| p.is_alphanumeric()) && !after_mm {
        return None;
    }
    let start = match here {
        'f' | 'F' => {
            let n = skip_space(c, i + 1);
            if c.get(n) == Some(&'/') { skip_space(c, n + 1) } else { n }
        }
        '1' if c.get(i + 1) == Some(&':') => i + 2,
        _ => return None,
    };
    let ((lo, _), _) = span(c, start)?;
    (lo >= APERTURE_RANGE.0 && lo <= APERTURE_RANGE.1).then_some(lo)
}

#[cfg(test)]
mod tests;
