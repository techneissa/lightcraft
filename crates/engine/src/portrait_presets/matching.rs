//! Finding a portrait preset's person on a photo: by name, else by how alike the faces' embeddings are, ranked
//! against every named person in the library with the margin name suggestions use, so a face more like someone else
//! is never taken for them.
//!
//! The library's named faces and their embeddings ([`Gallery`]) are gathered once and kept until a name, a photo or
//! an embedding changes; the hover preview and a click share them, and each photo's pick is kept too, so the
//! Presets panel asking every frame costs a lookup.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::{Aspects, PhotoId};
use lightcraft_develop::retouch::Face;
use lightcraft_meta::RegionKind;

use super::{Who, clean_name, faces_of};
use crate::Session;

/// How alike (cosine) two faces' embeddings must be to count as one person, when the recognition model gives no
/// threshold of its own (as name suggestions).
const DEFAULT_THRESHOLD: f32 = 0.45;

/// How far the person must be ahead of the next most alike (as name suggestions, `faces.suggest`).
pub const MARGIN: f32 = 0.06;

/// Most of one person's named faces compared against, and most named faces in all.
const PER_PERSON: usize = 256;
const MOST: usize = 50_000;

/// The ranking key of a preset's own embedding when it names nobody (never a lower-case name).
const UNNAMED: &str = "\u{1}preset";

/// Most picks kept (the Presets panel's rows times the photos hovered).
const MOST_PICKS: usize = 4096;

/// Why a photo did not get the face settings.
#[derive(Clone, Debug, PartialEq)]
pub enum Miss {
    /// No face found on it (yet).
    NoFaces,
    /// None of its faces is the person.
    NotThere,
    /// The face can't have its own settings: too many faces of the photo have theirs.
    Full,
}

impl Miss {
    pub fn as_str(&self) -> &'static str {
        match self {
            Miss::NoFaces => "no faces found on the photo yet",
            Miss::NotThere => "the person is not on the photo",
            Miss::Full => "too many faces on the photo have their own settings",
        }
    }
}

/// One photo's faces as matching sees them, and which one is the person.
pub struct Target {
    pub faces: Vec<Face>,
    pub index: usize,
}

/// Every named person's embeddings in the library, and the bar a match must pass.
#[derive(Default)]
pub struct Gallery {
    /// (name, its faces' embeddings), names trimmed and lower case.
    people: Vec<(String, Vec<Arc<[f32]>>)>,
    /// The model the embeddings were made by.
    model: Option<String>,
    threshold: f32,
}

impl Gallery {
    fn of(s: &Session) -> Gallery {
        let mut at: HashMap<String, usize> = HashMap::new();
        let mut people: Vec<(String, Vec<Arc<[f32]>>)> = Vec::new();
        let mut n = 0usize;
        'photos: for ph in s.catalog.photos().filter(|ph| ph.in_library()) {
            for r in ph.meta.regions.iter().filter(|r| r.kind == RegionKind::Face) {
                let Some(name) = r.name.as_deref().and_then(clean_name) else { continue };
                let Some(e) = recognition::embedding(s, ph.id, &r.rect) else { continue };
                let i = *at.entry(name.clone()).or_insert_with(|| {
                    people.push((name, Vec::new()));
                    people.len() - 1
                });
                if let Some((_, list)) = people.get_mut(i)
                    && list.len() < PER_PERSON
                {
                    list.push(e);
                    n += 1;
                    if n >= MOST {
                        break 'photos;
                    }
                }
            }
        }
        Gallery { people, model: recognition::tag(s), threshold: recognition::threshold(s) }
    }

    fn of_person(&self, name: &str) -> &[Arc<[f32]>] {
        self.people.iter().find(|(n, _)| n == name).map_or(&[], |(_, l)| l.as_slice())
    }
}

/// The gallery and the picks made with it, kept while they hold.
struct Kept {
    revision: u64,
    index: (String, usize),
    gallery: Arc<Gallery>,
    picks: HashMap<(u64, u64), Result<usize, Miss>>,
}

/// What matching keeps between calls (`Session::portrait_presets`).
#[derive(Default)]
pub struct State {
    kept: Mutex<Option<Kept>>,
    /// Apply to Every Photo running in the background.
    pub(crate) job: Option<super::person::Job>,
}

impl Session {
    /// The library's named people with their embeddings: kept until a name, a photo or the face index changes.
    pub(crate) fn portrait_gallery(&self) -> Arc<Gallery> {
        let mut kept = self.portrait_presets.kept.lock().unwrap_or_else(PoisonError::into_inner);
        self.portrait_fresh(&mut kept);
        if let Some(k) = kept.as_ref() {
            return k.gallery.clone();
        }
        let gallery = Arc::new(Gallery::of(self));
        *kept = Some(Kept { revision: self.catalog.revision, index: recognition::stamp(self), gallery: gallery.clone(), picks: HashMap::new() });
        gallery
    }

    /// Forget what was kept when what it was made of changed (a develop edit changes neither).
    fn portrait_fresh(&self, kept: &mut Option<Kept>) {
        let Some(k) = kept.as_mut() else { return };
        let same_index = k.index == recognition::stamp(self);
        let unchanged = k.revision == self.catalog.revision
            || self.catalog.aspects_since(k.revision).is_some_and(|a| !a.any_of(Aspects::META.with(Aspects::PRESENCE)));
        if same_index && unchanged {
            k.revision = self.catalog.revision;
        } else {
            *kept = None;
        }
    }

    fn portrait_pick_kept(&self, key: (u64, u64)) -> Option<Result<usize, Miss>> {
        let mut kept = self.portrait_presets.kept.lock().unwrap_or_else(PoisonError::into_inner);
        self.portrait_fresh(&mut kept);
        kept.as_ref()?.picks.get(&key).cloned()
    }

    fn portrait_keep_pick(&self, key: (u64, u64), pick: Result<usize, Miss>) {
        let mut kept = self.portrait_presets.kept.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(k) = kept.as_mut() {
            if k.picks.len() >= MOST_PICKS {
                k.picks.clear();
            }
            k.picks.insert(key, pick);
        }
    }
}

/// Finds the person on photos.
#[derive(Clone)]
pub struct Matcher {
    who: Who,
    /// Their ranking key: the name, or [`UNNAMED`].
    key: String,
    gallery: Arc<Gallery>,
    /// The preset's own embedding, when made by the model the gallery's are.
    own: Option<Arc<[f32]>>,
    /// A photo's only face may take the settings when it can't be compared ([`Matcher::pick`]).
    lone: bool,
    hash: u64,
}

impl Matcher {
    /// A matcher for `who` against the library's named people. `lone`: a photo's only face counts as the person when
    /// it can't be compared and isn't named as someone else (an explicit apply to photos the user picked; never Apply
    /// to Every Photo).
    pub fn new(s: &Session, who: &Who, lone: bool) -> Matcher {
        let gallery = s.portrait_gallery();
        let own: Option<Arc<[f32]>> =
            who.embedding.as_ref().filter(|e| gallery.model.as_deref() == Some(e.model.as_str())).map(|e| Arc::from(e.vector.as_slice()));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&who.person, lone).hash(&mut h);
        if let Some(e) = &own {
            e.iter().for_each(|x| x.to_bits().hash(&mut h));
        }
        Matcher { key: who.person.clone().unwrap_or_else(|| UNNAMED.to_string()), who: who.clone(), gallery, own, lone, hash: h.finish() }
    }

    /// The person's face on photo `id` (the pick is kept until names or embeddings change).
    pub fn find(&self, s: &Session, id: PhotoId) -> Result<Target, Miss> {
        let p = s.catalog.photo(id).ok_or(Miss::NoFaces)?;
        let (faces, embeddings) = faces_of(s, p);
        if faces.is_empty() {
            return Err(Miss::NoFaces);
        }
        // regions come from the catalog: their pick holds as long as the gallery does (a photo found by portrait
        // retouching instead is picked anew)
        let keyed = p.meta.regions.iter().any(|r| r.kind == RegionKind::Face);
        let key = (id.0, self.hash);
        let pick = match keyed.then(|| s.portrait_pick_kept(key)).flatten() {
            Some(p) => p,
            None => {
                let p = self.pick(&faces, &embeddings);
                if keyed {
                    s.portrait_keep_pick(key, p.clone());
                }
                p
            }
        };
        Ok(Target { faces, index: pick? })
    }

    /// Whether the person is known by face at all: their named faces' or the preset's own embedding.
    fn known_by_face(&self) -> bool {
        self.own.is_some() || !self.gallery.of_person(&self.key).is_empty()
    }

    /// Which of `faces` is the person: one named after them; else the face most like them that passes the bar and
    /// is at least [`MARGIN`] more like them than like anyone else named; else, with `lone`, a photo's only face when
    /// it can't be compared (no embedding on it, or none of theirs) and isn't named as someone else.
    pub fn pick(&self, faces: &[Face], embeddings: &[Option<Arc<[f32]>>]) -> Result<usize, Miss> {
        let me = self.who.person.as_deref();
        if let Some(name) = me
            && let Some(i) = faces.iter().position(|f| f.person.as_deref() == Some(name))
        {
            return Ok(i);
        }
        let known = self.known_by_face();
        let mut best: Option<(f32, usize)> = None;
        if known {
            let mut refs: Vec<(&str, &[f32])> = Vec::new();
            for (name, list) in &self.gallery.people {
                refs.extend(list.iter().map(|e| (name.as_str(), &e[..])));
            }
            if let Some(e) = &self.own {
                refs.push((self.key.as_str(), &e[..]));
            }
            for (i, f) in faces.iter().enumerate() {
                if f.person.is_some() {
                    continue;
                }
                let Some(e) = embeddings.get(i).and_then(Option::as_ref) else { continue };
                let ranked = lightcraft_faces::matching::rank(e, &refs);
                let Some(s) = lightcraft_faces::matching::suggest(&ranked, self.gallery.threshold, MARGIN) else { continue };
                if s.name.to_lowercase() == self.key && best.is_none_or(|(b, _)| s.score > b) {
                    best = Some((s.score, i));
                }
            }
        }
        if let Some((_, i)) = best {
            return Ok(i);
        }
        if let [only] = faces {
            let compared = known && embeddings.first().is_some_and(Option::is_some);
            if self.lone && !compared && (only.person.is_none() || only.person.as_deref() == me) {
                return Ok(0);
            }
        }
        Err(Miss::NotThere)
    }
}

/// The face index (recognition embeddings), where this build has one.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod recognition {
    use std::sync::Arc;

    use lightcraft_catalog::PhotoId;
    use lightcraft_geom::Rect;

    use crate::Session;

    /// The model the index's embeddings were made by; `None` while the index holds none.
    pub fn tag(s: &Session) -> Option<String> {
        Some(s.faces.index.tag.clone()).filter(|t| !t.is_empty())
    }

    /// What the gallery depends on in the index: its model and how many embeddings it holds.
    pub fn stamp(s: &Session) -> (String, usize) {
        (s.faces.index.tag.clone(), s.faces.index.len())
    }

    pub fn embedding(s: &Session, id: PhotoId, rect: &Rect) -> Option<Arc<[f32]>> {
        s.faces.index.get(id.0, rect).cloned()
    }

    pub fn threshold(s: &Session) -> f32 {
        s.faces
            .embedder
            .as_ref()
            .and_then(|(_, e)| e.manifest().thresholds.match_cosine)
            .filter(|t| t.is_finite() && *t > 0.0)
            .unwrap_or(super::DEFAULT_THRESHOLD)
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) mod recognition {
    use std::sync::Arc;

    use lightcraft_catalog::PhotoId;
    use lightcraft_geom::Rect;

    use crate::Session;

    pub fn tag(_: &Session) -> Option<String> {
        None
    }

    pub fn stamp(_: &Session) -> (String, usize) {
        (String::new(), 0)
    }

    pub fn embedding(_: &Session, _: PhotoId, _: &Rect) -> Option<Arc<[f32]>> {
        None
    }

    pub fn threshold(_: &Session) -> f32 {
        super::DEFAULT_THRESHOLD
    }
}
