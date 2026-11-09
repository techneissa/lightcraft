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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::FaceKey;
use lightcraft_parsing::{FaceHint, Parsing, Person, RgbImage};
use lightcraft_raster::Rgba8;

use crate::Session;
use crate::people_parsing::{Outcome, PARSE_EDGE};

pub mod grid;
pub mod parts;
mod rebase;

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
    /// A search is running; only the one started with this ticket files its result.
    Busy(u64),
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

#[derive(Default)]
struct Store {
    entries: Mutex<Vec<(String, Entry)>>,
    /// Signalled when a search files its result.
    done: Condvar,
    tickets: AtomicU64,
}

/// How long a command waits for a search already running before it gives up (a guard against a hang only: a parse
/// takes seconds).
const WAIT: std::time::Duration = std::time::Duration::from_secs(600);

/// The people found per photo (the newest [`KEEP`]). Cheap to clone; every clone is the same.
#[derive(Clone, Default)]
pub struct SelectPeople {
    store: Arc<Store>,
}

impl SelectPeople {
    fn lock(&self) -> MutexGuard<'_, Vec<(String, Entry)>> {
        self.store.entries.lock().unwrap_or_else(PoisonError::into_inner)
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
            let Some(i) = st.iter().position(|(_, e)| !matches!(e, Entry::Busy(_))) else { break };
            st.remove(i);
        }
        drop(st);
        self.store.done.notify_all();
    }

    /// Mark a search of `key` as running: its ticket.
    fn start(&self, key: &str) -> u64 {
        let ticket = self.store.tickets.fetch_add(1, Ordering::Relaxed);
        self.put(key, Entry::Busy(ticket));
        ticket
    }

    /// File the result of search `ticket`: only while it is the one running for `key` (a search started since, or a
    /// result filed since, wins).
    fn finish(&self, key: &str, ticket: u64, e: Entry) {
        let mut st = self.lock();
        if let Some(slot) = st.iter_mut().find(|(k, now)| k == key && matches!(now, Entry::Busy(t) if *t == ticket)) {
            slot.1 = e;
        }
        drop(st);
        self.store.done.notify_all();
    }

    /// `key`'s entry once no search of it is running (at most [`WAIT`]; still running then: `Busy`).
    fn settled(&self, key: &str) -> Option<Entry> {
        let deadline = web_time::Instant::now() + WAIT;
        let mut st = self.lock();
        loop {
            match st.iter().find(|(k, _)| k == key).map(|(_, e)| e.clone()) {
                Some(Entry::Busy(t)) => {
                    let left = deadline.saturating_duration_since(web_time::Instant::now());
                    if left.is_zero() {
                        return Some(Entry::Busy(t));
                    }
                    st = self.store.done.wait_timeout(st, left).unwrap_or_else(PoisonError::into_inner).0;
                }
                other => return other,
            }
        }
    }
}

/// The key of a person's face in a saved mask: the person's name when the face is named, and the face's box (the
/// person's box without a face) on the photo before the user's rotation.
pub fn face_key(q: &Person) -> FaceKey {
    let b = q.face.or(q.bounds).unwrap_or(lightcraft_parsing::NormRect::new(0.0, 0.0, 1.0, 1.0));
    FaceKey {
        person: q.person_id.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_lowercase),
        bounds: lightcraft_develop::retouch::NormRect::new(f64::from(b.x0), f64::from(b.y0), f64::from(b.x1), f64::from(b.y1)),
        // (a parse's box: the photo before lens corrections and transform)
        frame: lightcraft_develop::retouch::KeyFrame::Photo,
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

/// The id of a mask of `d` with a Select People part that is exactly `picks` of person `person` (everyone: `None`);
/// other parts (a brush, say) do not matter. The picker selects it instead of making
/// another (`selectPeople.pick`) and marks the part; `except` is a mask not counted.
pub fn mask_of(d: &lightcraft_develop::DevelopSettings, parsed: &Parsed, picks: &[Pick], person: Option<usize>, except: Option<u32>) -> Option<u32> {
    let has = |m: &&lightcraft_develop::Mask| {
        m.components.iter().any(|c| {
            let Some((have, who, everyone)) = current(&c.shape, Some(parsed)) else { return false };
            have == picks && if everyone { person.is_none() } else { person.is_some() && who == person }
        })
    };
    d.masks.iter().filter(|m| Some(m.id) != except).find(has).map(|m| m.id)
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
    /// The key photo `id`'s people are kept under: its pixels, the parse's model set, the people edge and the names
    /// of its faces in People (a face named or renamed since is found again under its new name).
    fn people_key(&self, id: PhotoId) -> Option<String> {
        use std::hash::{Hash, Hasher};
        let p = self.catalog.photo(id)?;
        let edge = self.hair_edges.edge(crate::hair_edges::EdgeKind::People);
        let mut names = std::collections::hash_map::DefaultHasher::new();
        for r in p.meta.regions.iter().filter(|r| r.kind == lightcraft_meta::RegionKind::Face) {
            r.name.hash(&mut names);
            [r.rect.x0, r.rect.y0, r.rect.x1, r.rect.y1].map(f64::to_bits).hash(&mut names);
        }
        Some(format!("{}|{}|{edge:?}|{:x}", crate::media::content_key(p), self.parsing.model_set().id(), names.finish()))
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
            Some(Entry::Busy(_)) => PeopleState { busy: true, ..Default::default() },
            Some(Entry::Failed(e)) => PeopleState { error: Some(e), ..Default::default() },
            None => PeopleState::default(),
        }
    }

    /// Find photo `id`'s people on a worker thread, once (nothing starts while they are known, being found, or
    /// failed, unless `again`).
    pub fn people_find(&mut self, id: PhotoId, again: bool) {
        let Some(key) = self.people_key(id) else { return };
        match self.select_people.get(&key) {
            Some(Entry::Busy(_)) => return,
            Some(_) if !again => return,
            _ => {}
        }
        let prep = match self.people_prep(id) {
            Ok(p) => p,
            Err(e) => return self.select_people.put(&key, Entry::Failed(e)),
        };
        let ticket = self.select_people.start(&key);
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
                        parse_now(&parsing, img, hints)
                    }));
                    let entry = match ran {
                        Ok(Ok(p)) => Entry::Ready(p),
                        Ok(Err(e)) => Entry::Failed(e),
                        Err(_) => Entry::Failed("finding people stopped unexpectedly".into()),
                    };
                    store.finish(&key, ticket, entry);
                }
            });
            if let Err(e) = spawned {
                store.finish(&key, ticket, Entry::Failed(format!("could not start finding people: {e}")));
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = prep;
            self.select_people.finish(&key, ticket, Entry::Failed("the web build has no people parsing".into()));
        }
    }

    /// Photo `id`'s people, found now when they are not known (commands, tests). A search already running for them
    /// is waited for and its result used, so every mask of the photo is drawn from the one parse the picker shows.
    pub fn people_now(&mut self, id: PhotoId) -> Result<Arc<Parsed>, String> {
        let key = self.people_key(id).ok_or("no such photo")?;
        match self.select_people.settled(&key) {
            Some(Entry::Ready(p)) => return Ok(p),
            Some(Entry::Busy(_)) => return Err("finding people did not finish".into()),
            Some(Entry::Failed(_)) | None => {}
        }
        let ticket = self.select_people.start(&key);
        let result = self.people_prep(id).and_then(Prep::run).and_then(|(img, hints)| parse_now(&self.parsing, img, hints));
        self.select_people.finish(&key, ticket, result.clone().map_or_else(Entry::Failed, Entry::Ready));
        result
    }
}

/// Parse `img` (waiting for the model) into a photo's people.
fn parse_now(parsing: &crate::people_parsing::PeopleParsing, img: RgbImage, hints: Vec<FaceHint>) -> Result<Arc<Parsed>, String> {
    match parsing.parse(img.clone(), hints, true) {
        Outcome::Ready { parsing, .. } => Ok(parsed(parsing, &img)),
        Outcome::Running => Err("the people parse did not finish".to_string()),
        Outcome::Failed(e) => Err(e),
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_pick;
#[cfg(test)]
mod tests_round;
