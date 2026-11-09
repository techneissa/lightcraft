//! Select People parts carried to another photo (Paste Settings, Paste from Previous, Sync Settings, Auto Sync).
//!
//! A part's grids are drawn from one photo's people, so copied as they are they would select the other photo where
//! the first one's people were. Each part that reaches a photo is drawn again from that photo's own people: its saved
//! face is looked for there ([`find_person`]: by name, else by place), everyone's parts are everyone's. When the face
//! is not in that photo, or its people can't be found, the part selects nothing and the picker says why.

use std::sync::Arc;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, MaskShape};

use super::grid::{self, Selection};
use super::{Parsed, find_person, parts};
use crate::Session;

impl Session {
    /// `synced`, the settings photo `x` gets from another photo's (`before`: its own until now), with every Select People
    /// part drawn from `x`'s own people. A part `x` already had (same mask, same parts, same face, same rotation) keeps
    /// its own grids; the others find `x`'s people first when they are not known (waiting for the parse, from its
    /// cache when it ran before).
    pub(crate) fn people_rebased(&mut self, x: PhotoId, before: &DevelopSettings, mut synced: DevelopSettings) -> DevelopSettings {
        let orient = synced.orientation;
        let mut people: Option<Result<Arc<Parsed>, String>> = None;
        for mask in &mut synced.masks {
            let own = before.masks.iter().find(|m| m.id == mask.id);
            for (i, c) in mask.components.iter_mut().enumerate() {
                let MaskShape::People { parts, face, seg: seg @ Some(_), detail, .. } = &mut c.shape else { continue };
                let kept = own.and_then(|m| m.components.get(i)).map(|c| &c.shape).filter(|_| before.orientation == orient);
                if let Some(MaskShape::People { parts: p0, face: f0, seg: s0 @ Some(_), detail: d0, .. }) = kept
                    && p0 == parts
                    && f0 == face
                {
                    (*seg, *detail) = (s0.clone(), d0.clone());
                    continue;
                }
                let found = people.get_or_insert_with(|| self.people_now(x));
                let picks = parts::picks_of(parts);
                let drawn = found.as_ref().ok().and_then(|p| {
                    let person = match face {
                        None => None,
                        Some(k) => Some(find_person(&p.parsing, k)?),
                    };
                    grid::build(&p.parsing, &Selection::of(person, &picks), orient)
                });
                (*seg, *detail) = match drawn {
                    Some((main, fine)) => (Some(main), fine),
                    None => {
                        let model = seg.as_ref().and_then(|g| g.model.clone()).unwrap_or_else(|| "people".into());
                        (Some(grid::nothing(&model)), Vec::new())
                    }
                };
            }
        }
        synced
    }
}
