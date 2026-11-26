//! What Describe says when it selects nothing (issue #439): what it looked for and where, and what to try; or what
//! failed and why, never a bare "failed".

/// Nothing matched `text` (in the zoomed area when `zoomed`, issue #281): say so, and what to try instead.
pub(crate) fn not_found(text: &str, zoomed: bool) -> String {
    if zoomed {
        format!("Nothing matching “{text}” was found in the zoomed area. Try other or fewer words, zoom out, or click it with Object.")
    } else {
        format!("Nothing matching “{text}” was found in this photo. Try other or fewer words, zoom in on something small, or click it with Object.")
    }
}

/// Looking for `text` failed with `why` (the model's or the photo's own error): what failed and why, and to try again
/// unless `why` is a full sentence, which carries its own advice (a missing photo file's, a remote worker's).
pub(crate) fn failed(text: &str, why: &str) -> String {
    let why = why.trim();
    if why.ends_with('.') {
        format!("Describe could not look for “{text}”: {why}")
    } else {
        format!("Describe could not look for “{text}”: {why}. Try again.")
    }
}
