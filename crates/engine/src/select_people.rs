//! Fork feature (`fork/select-people.md`): Select People masks. A mask of parts of one person or of everyone
//! (Entire Person, Entire Face, Facial Skin, Hair, Lips, Clothes, Background…), drawn from the photo's people parse
//! (`fork/people-parsing.md`) into the grids an ordinary mask stores ([`grid`]), so every mask slider, Refine
//! Edges, Recolour and Remove work on it and a render never needs the model.
//!
//! [`SelectPeople`] keeps, per photo, the parse and a round-thumbnail picture of each person's face for the
//! picker on the photo. Finding them runs off the main thread: the photo is rendered and its faces found (a row
//! "Finding people"), then the parse runs or comes from its cache (the parse's own row). Nothing here runs a model
//! while a slider moves: a mask's grids are drawn once, when its parts or face are picked.
//!
//! A mask remembers its face as portrait retouching does (`crates/retouch/README.md`): by the person's name when the
//! face is named in People, else by its box on the photo before the user's rotation ([`FaceKey`]).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::FaceKey;
use lightcraft_parsing::{FaceHint, Parsing, Person, RgbImage};
use lightcraft_raster::Rgba8;

use crate::Session;
use crate::people_parsing::{Outcome, PARSE_EDGE};

pub mod grid;
pub mod parts;

/// The parser's types, for hosts that show or fake a parse (the UI's tests).
pub use lightcraft_parsing as parsing;
pub use parts::Pick;

/// Photos whose parse is kept here.
const KEEP: usize = 8;
/// Side of a face thumbnail.
pub const THUMB: usize = 96;

/// A photo's people, ready to pick from.
pub struct Parsed {
    pub parsing: Arc<Parsing>,
    /// A square close-up of each person's face (or of the top of them, without one), in the order of
    /// `parsing.people`.
    pub thumbs: Vec<Rgba8>,
}

#[derive(Clone)]
enum Entry {
    Busy,
    Ready(Arc<Parsed>),
    Failed(String),
}

/// Where a photo's people stand.
#[derive(Clone, Default)]
pub struct PeopleState {
    pub ready: Option<Arc<Parsed>>,
    pub busy: bool,
    pub error: Option<String>,
}

/// The people found per photo (the newest [`KEEP`]). Cheap to clone; every clone is the same.
#[derive(Clone, Default)]
pub struct SelectPeople {
    entries: Arc<Mutex<Vec<(String, Entry)>>>,
}

impl SelectPeople {
    fn lock(&self) -> MutexGuard<'_, Vec<(String, Entry)>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, key: &str) -> Option<Entry> {
        self.lock().iter().find(|(k, _)| k == key).map(|(_, e)| e.clone())
    }

    fn put(&self, key: &str, e: Entry) {
        let mut st = self.lock();
        st.retain(|(k, _)| k != key);
        st.push((key.to_string(), e));
        // (the oldest finished entries go first; a search still running stays)
        while st.len() > KEEP {
            let Some(i) = st.iter().position(|(_, e)| !matches!(e, Entry::Busy)) else { break };
            st.remove(i);
        }
    }

    /// Forget every photo (the model set or the hair edges changed: their parses differ).
    pub fn clear(&self) {
        self.lock().retain(|(_, e)| matches!(e, Entry::Busy));
    }
}

/// The key of a person's face in a saved mask: the person's name when the face is named, and the face's box (the
/// person's box without a face) on the photo before the user's rotation.
pub fn face_key(q: &Person) -> FaceKey {
    let b = q.face.or(q.bounds).unwrap_or(lightcraft_parsing::NormRect::new(0.0, 0.0, 1.0, 1.0));
    FaceKey {
        person: q.person_id.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_lowercase),
        bounds: lightcraft_develop::retouch::NormRect::new(f64::from(b.x0), f64::from(b.y0), f64::from(b.x1), f64::from(b.y1)),
    }
}

/// The person of `p` a saved mask's face `key` is: the one of its name (the closest of several), else the one
/// whose box centre is closest to the key's, within half the key's box; never a person named as someone else.
pub fn find_person(p: &Parsing, key: &FaceKey) -> Option<usize> {
    let (kx, ky) = key.bounds.centre();
    let (kw, kh) = (key.bounds.width().abs().max(1e-6), key.bounds.height().abs().max(1e-6));
    let scored = p.people.iter().enumerate().map(|(i, q)| {
        let k = face_key(q);
        let (x, y) = k.bounds.centre();
        let (dx, dy) = ((x - kx) / kw, (y - ky) / kh);
        (i, k.person, (dx * dx + dy * dy).sqrt(), dx.abs() <= 0.5 && dy.abs() <= 0.5)
    });
    let best = |it: Vec<(usize, f64)>| it.into_iter().filter(|(_, d)| d.is_finite()).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i);
    match &key.person {
        Some(name) => {
            let all: Vec<_> = scored.collect();
            best(all.iter().filter(|(_, n, ..)| n.as_ref() == Some(name)).map(|(i, _, d, _)| (*i, *d)).collect())
                .or_else(|| best(all.iter().filter(|(_, n, _, near)| n.is_none() && *near).map(|(i, _, d, _)| (*i, *d)).collect()))
        }
        None => best(scored.filter(|(.., near)| *near).map(|(i, _, d, _)| (i, d)).collect()),
    }
}

/// The parts and person a Select People part selects now: (its parts, the person of `parsed` it is, whether it
/// is everyone's). `None` for any other part (a People part without a stored selection included).
pub fn current(shape: &lightcraft_develop::MaskShape, parsed: Option<&Parsed>) -> Option<(Vec<Pick>, Option<usize>, bool)> {
    let lightcraft_develop::MaskShape::People { parts, face, seg: Some(_), .. } = shape else { return None };
    let person = face.as_ref().and_then(|k| parsed.and_then(|p| find_person(&p.parsing, k)));
    Some((parts::picks_of(parts), person, face.is_none()))
}

/// The name a person goes by in the picker: their name in People (in title case), else "Person 2".
pub fn person_name(q: &Person, i: usize) -> String {
    match q.person_id.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => name
            .split(' ')
            .map(|w| {
                let mut c = w.chars();
                c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
            })
            .collect::<Vec<_>>()
            .join(" "),
        None => format!("Person {}", i + 1),
    }
}

/// A [`THUMB`]-pixel square around person `q`'s face (the top of their box without one), cut from `img`.
fn thumb(img: &RgbImage, q: &Person) -> Rgba8 {
    let (w, h) = (img.width as f32, img.height as f32);
    let (side, cx, cy) = match (q.face, q.bounds) {
        (Some(f), _) => ((f.width() * w).max(f.height() * h) * 1.5, f.centre().0 * w, f.centre().1 * h),
        (None, Some(b)) => {
            let side = (b.width() * w).min(b.height() * h).max(1.0);
            (side, b.centre().0 * w, b.y0 * h + side / 2.0)
        }
        (None, None) => (0.0, 0.0, 0.0),
    };
    let ok = side.is_finite() && cx.is_finite() && cy.is_finite() && side >= 1.0;
    Rgba8::from_fn(THUMB, THUMB, |x, y| {
        let sx = cx - side / 2.0 + (x as f32 + 0.5) * side / THUMB as f32;
        let sy = cy - side / 2.0 + (y as f32 + 0.5) * side / THUMB as f32;
        if !ok || sx < 0.0 || sy < 0.0 || sx >= w || sy >= h {
            return [40, 40, 40, 255];
        }
        let i = (sy as usize * img.width + sx as usize) * 3;
        match img.data.get(i..i + 3) {
            Some(&[r, g, b]) => [r, g, b, 255],
            _ => [40, 40, 40, 255],
        }
    })
}

fn parsed(parsing: Arc<Parsing>, img: &RgbImage) -> Arc<Parsed> {
    let thumbs = parsing.people.iter().map(|q| thumb(img, q)).collect();
    Arc::new(Parsed { parsing, thumbs })
}

/// What a worker needs to find a photo's people: the render of the photo as the parse sees it, the face detector,
/// the photo's face regions.
struct Prep {
    job: crate::RenderJob,
    detector: Option<Arc<lightcraft_faces::yunet::Detector>>,
    regions: Vec<lightcraft_meta::Region>,
}

impl Prep {
    fn run(self) -> Result<(RgbImage, Vec<FaceHint>), String> {
        let image = self.job.run().rendered?.image;
        let rgb: Vec<u8> = image.data.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
        let img = RgbImage::new(image.width, image.height, rgb).map_err(|e| e.to_string())?;
        let hints = crate::cmd::parsing::hints_for(self.detector.as_deref(), &img, &self.regions)?;
        Ok((img, hints))
    }
}

impl Session {
    /// The key photo `id`'s people are kept under: its pixels, the parse's model set and the people edge.
    fn people_key(&self, id: PhotoId) -> Option<String> {
        let p = self.catalog.photo(id)?;
        let edge = self.hair_edges.edge(crate::hair_edges::EdgeKind::People);
        Some(format!("{}|{}|{edge:?}", crate::media::content_key(p), self.parsing.model_set().id()))
    }

    fn people_prep(&mut self, id: PhotoId) -> Result<Prep, String> {
        let job = self.preview_job(id, PARSE_EDGE, PARSE_EDGE, false, &DevelopSettings::default()).ok_or("no such photo")?;
        let detector = crate::cmd::face_detect::detector(self).ok();
        let regions = self.catalog.photo(id).map(|p| p.meta.regions.clone()).unwrap_or_default();
        Ok(Prep { job, detector, regions })
    }

    /// Where photo `id`'s people stand (nothing is started).
    pub fn people_of(&self, id: PhotoId) -> PeopleState {
        match self.people_key(id).and_then(|k| self.select_people.get(&k)) {
            Some(Entry::Ready(p)) => PeopleState { ready: Some(p), busy: false, error: None },
            Some(Entry::Busy) => PeopleState { busy: true, ..Default::default() },
            Some(Entry::Failed(e)) => PeopleState { error: Some(e), ..Default::default() },
            None => PeopleState::default(),
        }
    }

    /// Find photo `id`'s people on a worker thread, once (nothing starts while they are known, being found, or
    /// failed, unless `again`).
    pub fn people_find(&mut self, id: PhotoId, again: bool) {
        let Some(key) = self.people_key(id) else { return };
        match self.select_people.get(&key) {
            Some(Entry::Busy) => return,
            Some(_) if !again => return,
            _ => {}
        }
        let prep = match self.people_prep(id) {
            Ok(p) => p,
            Err(e) => return self.select_people.put(&key, Entry::Failed(e)),
        };
        self.select_people.put(&key, Entry::Busy);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let store = self.select_people.clone();
            let parsing = self.parsing.clone();
            let row = self.activity.start("selectPeople", "Finding people", crate::activity::Cancel::No);
            let spawned = std::thread::Builder::new().name("select-people".into()).spawn({
                let key = key.clone();
                let store = store.clone();
                move || {
                    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let prepared = prep.run();
                        // (the parse holds its own row while its model runs)
                        drop(row);
                        let (img, hints) = prepared?;
                        match parsing.parse(img.clone(), hints, true) {
                            Outcome::Ready { parsing, .. } => Ok(parsed(parsing, &img)),
                            Outcome::Running => Err("the people parse did not finish".to_string()),
                            Outcome::Failed(e) => Err(e),
                        }
                    }));
                    let entry = match ran {
                        Ok(Ok(p)) => Entry::Ready(p),
                        Ok(Err(e)) => Entry::Failed(e),
                        Err(_) => Entry::Failed("finding people stopped unexpectedly".into()),
                    };
                    store.put(&key, entry);
                }
            });
            if let Err(e) = spawned {
                store.put(&key, Entry::Failed(format!("could not start finding people: {e}")));
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = prep;
            self.select_people.put(&key, Entry::Failed("the web build has no people parsing".into()));
        }
    }

    /// Photo `id`'s people, found now (waiting for the parse) when they are not known (commands, tests).
    pub fn people_now(&mut self, id: PhotoId) -> Result<Arc<Parsed>, String> {
        let key = self.people_key(id).ok_or("no such photo")?;
        if let Some(Entry::Ready(p)) = self.select_people.get(&key) {
            return Ok(p);
        }
        let (img, hints) = self.people_prep(id)?.run()?;
        let result = match self.parsing.parse(img.clone(), hints, true) {
            Outcome::Ready { parsing, .. } => Ok(parsed(parsing, &img)),
            Outcome::Running => Err("the people parse did not finish".to_string()),
            Outcome::Failed(e) => Err(e),
        };
        // (a search running for it on a worker files the same result when it ends)
        self.select_people.put(&key, result.clone().map_or_else(Entry::Failed, Entry::Ready));
        result
    }
}

#[cfg(test)]
mod tests;
