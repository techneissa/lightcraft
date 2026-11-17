//! Fork (issue #329, `fork/mask-presets.md`): the user's own mask presets in the session. The preset type is
//! `lightcraft_develop::mask_presets`; this reads a library's saved list, finds a preset by id or name, and names the
//! mask an applied preset makes. The commands are `cmd::mask_presets`.

use lightcraft_develop::Mask;
use lightcraft_develop::mask_presets::{MAX_NAME, MAX_USER, MaskPreset, clean_name};

use crate::Session;

/// The presets read from a library's file: none without an id, a name and a part, none whose id repeats or whose name
/// is an earlier one's (letter case aside), at most [`MAX_USER`], each name cut to [`MAX_NAME`] characters. With them,
/// how many presets were left out or cut.
pub fn read_user(list: Vec<MaskPreset>) -> (Vec<MaskPreset>, usize) {
    let mut ids: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut changed = 0usize;
    for mut p in list {
        let Some(clean) = clean_name(&p.name) else {
            changed += 1;
            continue;
        };
        let name: String = clean.chars().take(MAX_NAME).collect();
        let name = name.trim().to_string();
        if p.id.trim().is_empty() || !p.is_usable() || ids.contains(&p.id) || names.contains(&name.to_lowercase()) || out.len() >= MAX_USER {
            changed += 1;
            continue;
        }
        changed += usize::from(name.chars().count() < clean.chars().count());
        p.name = name;
        // (what a model found on the photo a hand-edited file came from is not this photo's: a preset keeps recipes only)
        lightcraft_develop::adaptive::strip_found(std::slice::from_mut(&mut p.mask));
        ids.push(p.id.clone());
        names.push(p.name.to_lowercase());
        out.push(p);
    }
    (out, changed)
}

/// `base`, or `base 2`, `base 3`… : the first of them no mask in `masks` has as its name.
pub fn free_name(masks: &[Mask], base: &str) -> String {
    let taken = |n: &str| masks.iter().any(|m| m.name.eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.to_string();
    }
    (2u32..).map(|n| format!("{base} {n}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
}

impl Session {
    /// The preset `key` names: its id, else its name (letter case aside).
    pub fn mask_preset(&self, key: &str) -> Option<MaskPreset> {
        let key = key.trim();
        self.mask_presets.iter().find(|p| p.id == key).or_else(|| self.mask_presets.iter().find(|p| p.name.eq_ignore_ascii_case(key))).cloned()
    }
}

#[cfg(test)]
mod tests;
