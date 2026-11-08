//! Apple Vision edges (`fork/apple-vision-masks.md`, issue #115): the third choice of [`super::Edge`], next to Current
//! and BiRefNet Matting. Vision's own masks run on the Neural Engine in a fraction of a second and need no download
//! (`lightcraft_coreml::vision`).
//!
//! - **Entire Person / Background**: person segmentation (Accurate, fine hair) draws the edge; Vision's per-person
//!   masks say whose each pixel is, so where two people meet each keeps their own. Each Vision person is matched to
//!   the parsed person it overlaps most. The parse still decides who is where; the matte only redraws a band around
//!   each person's edge ([`super::people`]).
//! - **Subject**: the foreground instance masks (every subject together) redraw the Subject part's edge, the same way
//!   BiRefNet Matting does ([`subject_edge`]).
//!
//! Runs on the worker thread that asked (the people-parsing thread, the Subject worker), with a "Refining hair edges"
//! row; a parse made with it is its own cache version ([`VERSION`]). A Mac without the requests, a Vision failure or a
//! panic is an error the caller reports; nothing here panics.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_coreml::vision::{self as av, Image, Pixels};
use lightcraft_parsing::{Parsing, Part, Person};

use super::people::{Refined, entire_rect, refine_person};
use super::{Edge, HairEdges, Matte, pixel_key};
use crate::activity::Cancel;

/// The cache version of a people parse whose edges Apple Vision drew.
pub const VERSION: &str = "vision1";
/// The suffix of a Subject part's model id once Apple Vision drew its edge (`birefnet+vision`).
pub const SUFFIX: &str = "+vision";
/// Longest side of a people matte (the parse's own picture is 2048 px on its long side).
const PEOPLE_SIDE: usize = 2048;
/// Side of a Subject matte, the size BiRefNet Matting stores (so a document's mask is the same size either way).
const SUBJECT_SIDE: usize = 1024;
/// Where the Vision people together cover less than this, the person segmentation alone decides.
const SHARE_FLOOR: f32 = 0.02;
/// Least overlap (intersection over union) for a Vision person to be a parsed person.
const MATCH_IOU: f32 = 0.1;
/// Cells a side of the grid the overlap is measured on.
const MATCH_GRID: usize = 128;
/// Salt for Subject mattes in the shared matte memory, so a Vision matte never answers for BiRefNet's.
const SUBJECT_SALT: u64 = 0x5649_5349_4f4e;

/// Whether this Mac runs every Vision request Apple Vision edges use.
pub fn available() -> bool {
    av::available()
}

/// A coverage matte at the photo's aspect, as a `side × side` logit grid over the photo stretched to a square.
pub(crate) fn to_matte(m: &av::Matte, side: usize) -> Matte {
    let side = side.max(1);
    let logits = (0..side * side)
        .map(|i| {
            let c = m.at(((i % side) as f32 + 0.5) / side as f32, ((i / side) as f32 + 0.5) / side as f32);
            let c = c.clamp(1e-6, 1.0 - 1e-6);
            (c / (1.0 - c)).ln()
        })
        .collect();
    Matte { side, logits }
}

fn image(rgb: &[u8], w: usize, h: usize) -> Image<'_> {
    Image { width: w, height: h, pixels: Pixels::Rgb8(rgb) }
}

/// What Vision says about the people in a picture.
pub(crate) struct People {
    /// Everyone together, with fine hair.
    pub all: av::Matte,
    /// Each person on their own.
    pub each: Vec<av::Matte>,
}

impl HairEdges {
    /// Run `f` (one or more Vision requests) with the activity row, catching a panic, checking `cancel` before.
    fn with_vision<T>(&self, cancel: &Arc<AtomicBool>, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        if !available() {
            return Err("Apple Vision masks need macOS 14 or later".into());
        }
        let row = self.inner.activity.start("matting", "Refining hair edges", Cancel::Flag(cancel.clone()));
        row.detail(Edge::Vision.label());
        let stop = || cancel.load(Ordering::Relaxed) || lightcraft_gpu::shutting_down();
        if stop() {
            return Err("cancelled".into());
        }
        self.inner.runs.fetch_add(1, Ordering::Relaxed);
        let t = web_time::Instant::now();
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|_| "Apple Vision stopped unexpectedly".to_string())??;
        log::info!("Apple Vision masks in {:?}", t.elapsed());
        if stop() {
            return Err("cancelled".into());
        }
        Ok(out)
    }

    /// Vision's people in an 8-bit RGB picture: the person segmentation and the per-person masks.
    pub(crate) fn vision_people(&self, rgb: &[u8], w: usize, h: usize, cancel: &Arc<AtomicBool>) -> Result<People, String> {
        let img = image(rgb, w, h);
        img.check().map_err(|_| "the picture to refine is damaged".to_string())?;
        self.with_vision(cancel, || {
            let all = av::person_segmentation(&img).map_err(|e| e.to_string())?;
            let each = av::person_instances(&img).map_err(|e| e.to_string())?.masks;
            Ok(People { all, each })
        })
    }

    /// The Subject matte of an 8-bit RGB picture from Vision's foreground masks (every subject together): from memory
    /// when the same pixels were matted before. `Ok(None)`: Vision found no subject.
    pub(crate) fn vision_subject(&self, rgb: &[u8], w: usize, h: usize, cancel: &Arc<AtomicBool>) -> Result<Option<Arc<Matte>>, String> {
        let img = image(rgb, w, h);
        img.check().map_err(|_| "the picture to refine is damaged".to_string())?;
        let key = pixel_key(rgb, w, h) ^ SUBJECT_SALT;
        if let Some(m) = self.remembered(key) {
            return Ok(Some(m));
        }
        let found = self.with_vision(cancel, || av::foreground_instances(&img).map_err(|e| e.to_string()))?;
        let Some(all) = found.union() else { return Ok(None) };
        let m = Arc::new(to_matte(&all, SUBJECT_SIDE));
        if !m.is_valid() {
            return Err("Apple Vision produced an invalid matte".into());
        }
        self.remember(key, m.clone());
        Ok(Some(m))
    }

    /// The Subject matte for `edge` (BiRefNet Matting or Apple Vision); `Ok(None)` when Vision found no subject.
    pub(crate) fn subject_matte(&self, edge: Edge, rgb: &[u8], w: usize, h: usize, cancel: &Arc<AtomicBool>) -> Result<Option<Arc<Matte>>, String> {
        match edge {
            Edge::Vision => self.vision_subject(rgb, w, h, cancel),
            _ => self.matte(rgb, w, h, cancel).map(Some),
        }
    }
}

/// The cache version of a people parse whose edges `edge` drew (`None` for Current).
pub fn version(edge: Edge) -> Option<&'static str> {
    match edge {
        Edge::Current => None,
        Edge::Matting => Some(super::VERSION),
        Edge::Vision => Some(VERSION),
    }
}

/// A Subject selection `base` with its edge redrawn by `edge`'s matte `m` (`super::subject_edge` for BiRefNet
/// Matting; for Apple Vision the same band rule, marked `<base model>+vision`).
#[cfg(feature = "sam")]
pub fn subject_edge(edge: Edge, base: &lightcraft_develop::SegMask, m: &Matte) -> Option<lightcraft_develop::SegMask> {
    if edge != Edge::Vision {
        return super::subject_edge(base, m);
    }
    let logits = base.logits()?;
    let matte = lightcraft_birefnet::Matte { side: m.side, logits: m.logits.clone() };
    let mixed = lightcraft_birefnet::combine::refine_edges(&logits, base.side as usize, &matte);
    if !mixed.iter().any(|l| *l > 0.0) {
        return None;
    }
    let id = format!("{}{SUFFIX}", base.model.as_deref().unwrap_or("sam3"));
    Some(lightcraft_develop::SegMask::from_logits(m.side, &mixed).made_by(&id))
}

/// How much `m` and person `p`'s Entire Person overlap (intersection over union, on a coarse grid).
fn overlap(m: &av::Matte, p: &Person) -> f32 {
    let (mut inter, mut union) = (0.0f32, 0.0f32);
    for i in 0..MATCH_GRID * MATCH_GRID {
        let (x, y) = (((i % MATCH_GRID) as f32 + 0.5) / MATCH_GRID as f32, ((i / MATCH_GRID) as f32 + 0.5) / MATCH_GRID as f32);
        let (a, b) = (m.at(x, y), p.sample(Part::EntirePerson, x, y));
        inter += a.min(b);
        union += a.max(b);
    }
    if union > 0.0 { inter / union } else { 0.0 }
}

/// For each parsed person, the Vision person that is theirs: best overlap first, each Vision person used once.
pub(crate) fn assign(each: &[av::Matte], people: &[Person]) -> Vec<Option<usize>> {
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for (j, m) in each.iter().enumerate() {
        for (k, p) in people.iter().enumerate() {
            let o = overlap(m, p);
            if o >= MATCH_IOU {
                pairs.push((o, j, k));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut out = vec![None; people.len()];
    let mut used = vec![false; each.len()];
    for (_, j, k) in pairs {
        if out.get(k).is_some_and(Option::is_none) && used.get(j).is_some_and(|u| !u) {
            out[k] = Some(j);
            used[j] = true;
        }
    }
    out
}

/// Person `k`'s matte: the person segmentation, less the share of Vision people that are someone else's.
fn person_matte(v: &People, theirs: &[Option<usize>], k: usize, side: usize) -> Matte {
    let others: Vec<usize> = theirs.iter().enumerate().filter(|(i, _)| *i != k).filter_map(|(_, j)| *j).collect();
    let all = &v.all;
    let data = (0..all.width * all.height)
        .map(|i| {
            let s = all.data.get(i).copied().unwrap_or(0.0);
            if others.is_empty() {
                return s;
            }
            let (x, y) = (((i % all.width) as f32 + 0.5) / all.width as f32, ((i / all.width) as f32 + 0.5) / all.height as f32);
            let at: Vec<f32> = v.each.iter().map(|m| m.at(x, y)).collect();
            let sum: f32 = at.iter().sum();
            if sum < SHARE_FLOOR {
                return s;
            }
            let theirs: f32 = others.iter().filter_map(|j| at.get(*j)).sum();
            s * (1.0 - theirs / sum).clamp(0.0, 1.0)
        })
        .collect();
    to_matte(&av::Matte { width: all.width, height: all.height, data }, side)
}

/// Redraw every person's Entire Person edge with Vision's mattes (`super::people::refine`, one matte per person), and
/// add the strands they gain beside their hair to their Hair. Marks the parse's version.
pub(crate) fn refine_people(parsing: &mut Parsing, v: &People) {
    let side = v.all.width.max(v.all.height).clamp(1, PEOPLE_SIDE);
    let theirs = assign(&v.each, &parsing.people);
    let before: Vec<Person> = parsing.people.clone();
    for (k, person) in parsing.people.iter_mut().enumerate() {
        let Some(rect) = entire_rect(person) else { continue };
        let matte = person_matte(v, &theirs, k, side);
        if !matte.is_valid() {
            continue;
        }
        let others: Vec<&Person> = before.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, p)| p).collect();
        let Some(Refined { entire, gained }) = refine_person(person, &others, rect, &matte) else { continue };
        person.parts.insert(Part::EntirePerson, vec![entire]);
        if let Some(g) = gained
            && let Some(hair) = person.parts.get_mut(&Part::Hair)
        {
            hair.push(g);
        }
        person.bounds = Some(person.bounds.map_or(rect, |b| b.union(&rect)));
    }
    parsing.model_version = format!("{}+{VERSION}", parsing.model_version);
}

/// `p` with its Entire Person edges drawn by Apple Vision from `img` (the picture it was parsed from).
pub(crate) fn people_edged(
    edges: &HairEdges,
    mut p: Parsing,
    img: &lightcraft_parsing::RgbImage,
    cancel: &Arc<AtomicBool>,
) -> Result<Parsing, String> {
    if p.people.is_empty() {
        return Ok(p);
    }
    let v = edges.vision_people(&img.data, img.width, img.height, cancel).map_err(|e| format!("hair edges: {e}"))?;
    refine_people(&mut p, &v);
    Ok(p)
}
