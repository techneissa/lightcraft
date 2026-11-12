//! Apple Vision edges (`fork/apple-vision-masks.md`, issue #115): the third choice of [`super::Edge`], next to Current
//! and BiRefNet Matting. Vision's own masks run on the Neural Engine in a fraction of a second and need no download
//! (`lightcraft_coreml::vision`).
//!
//! - **Entire Person / Background**: person segmentation (Accurate, fine hair) draws the edge; Vision's per-person
//!   masks say whose each pixel is, so where two people meet each keeps their own, whether or not the parse has the
//!   other one. Each Vision person is matched to the parsed person it overlaps most; a parsed person no Vision person
//!   matches keeps the parse's edge. The parse still decides who is where; the matte only redraws a band around each
//!   person's edge ([`super::people`]).
//! - **Subject**: the foreground instance masks (every subject together) redraw the Subject part's edge, the same way
//!   BiRefNet Matting does ([`super::subject_edge`]); the person segmentation adds the hair of the people who are part of
//!   the subject ([`subject_people`], [`with_hair`]).
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
/// Longest side of a people matte: the widest plane a cached parse may have (a full-frame person's new plane is the
/// matte's side), so a parse with Vision edges is read back rather than made again on every open.
const PEOPLE_SIDE: usize = lightcraft_parsing::cache::MAX_PLANE_SIDE as usize;
/// Side of a Subject matte, the size BiRefNet Matting stores (so a document's mask is the same size either way).
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
const SUBJECT_SIDE: usize = 1024;
/// Where the Vision people together cover less than this, the person segmentation alone decides.
const SHARE_FLOOR: f32 = 0.02;
/// Least overlap (intersection over union) for a Vision person to be a parsed person.
const MATCH_IOU: f32 = 0.1;
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
/// Least share of a Vision person inside the subject for them to be part of it (their hair is then added).
const SUBJECT_OVERLAP: f32 = 0.5;
/// Cells a side of the grid the overlap is measured on.
const MATCH_GRID: usize = 128;
/// Salt for Subject mattes in the shared matte memory, so a Vision matte never answers for BiRefNet's.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
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
        let row = self.inner.activity.start_background("matting", "Refining hair edges", Cancel::Flag(cancel.clone()));
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

    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    /// The Subject matte of an 8-bit RGB picture from Vision's foreground masks (every subject together) and the
    /// person segmentation (fine hair) of the people who are part of the subject, the larger at each pixel: from memory
    /// when the same pixels were matted before. `Ok(None)`: Vision found no subject.
    pub(crate) fn vision_subject(&self, rgb: &[u8], w: usize, h: usize, cancel: &Arc<AtomicBool>) -> Result<Option<Arc<Matte>>, String> {
        let img = image(rgb, w, h);
        img.check().map_err(|_| "the picture to refine is damaged".to_string())?;
        let key = pixel_key(rgb, w, h) ^ SUBJECT_SALT;
        if let Some(m) = self.remembered(key) {
            return Ok(Some(m));
        }
        // (the subject mask's edge is hard and drops strands: where a person is part of the subject, the person
        // segmentation's fine hair is added, the larger of the two at each pixel; `subject_edge` keeps it to the band
        // around the part's edge)
        let found = self.with_vision(cancel, || {
            let Some(subject) = av::foreground_union(&img).map_err(|e| e.to_string())? else { return Ok(None) };
            let each = av::person_instances(&img).map_err(|e| e.to_string())?.masks;
            let theirs = subject_people(&each, &subject);
            let seg = match theirs.is_empty() {
                true => None,
                false => Some(av::person_segmentation(&img).map_err(|e| e.to_string())?),
            };
            Ok(Some((subject, seg, each, theirs)))
        })?;
        let Some((subject, seg, each, theirs)) = found else { return Ok(None) };
        let all = match seg {
            Some(seg) => with_hair(subject, &People { all: seg, each }, &theirs),
            None => subject,
        };
        let m = Arc::new(to_matte(&all, SUBJECT_SIDE));
        if !m.is_valid() {
            return Err("Apple Vision produced an invalid matte".into());
        }
        self.remember(key, m.clone());
        Ok(Some(m))
    }

    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    /// The Subject matte for `edge` (BiRefNet Matting or Apple Vision); `Ok(None)` when Vision found no subject.
    pub(crate) fn subject_matte(&self, edge: Edge, rgb: &[u8], w: usize, h: usize, cancel: &Arc<AtomicBool>) -> Result<Option<Arc<Matte>>, String> {
        match edge {
            Edge::Vision => self.vision_subject(rgb, w, h, cancel),
            _ => self.matte_for(edge, super::EdgeKind::Subject, rgb, w, h, cancel).map(Some),
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

/// The centre of pixel `i` of `m`, as a fraction of the photo.
fn centre(m: &av::Matte, i: usize) -> (f32, f32) {
    let (w, h) = (m.width.max(1), m.height.max(1));
    (((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32)
}

/// At each pixel of the person segmentation `seg`, Vision's people `each` added up (once per picture, shared by
/// every person).
fn totals(seg: &av::Matte, each: &[av::Matte]) -> Vec<f32> {
    (0..seg.width * seg.height)
        .map(|i| {
            let (x, y) = centre(seg, i);
            each.iter().map(|m| m.at(x, y)).sum()
        })
        .collect()
}

/// The person segmentation `seg` at its pixel `i`, times the share Vision people `mine` hold there of all of `each`.
/// Where they together cover less than [`SHARE_FLOOR`], the segmentation alone.
fn held(seg: &av::Matte, each: &[av::Matte], totals: &[f32], mine: &[usize], i: usize) -> f32 {
    let s = seg.data.get(i).copied().unwrap_or(0.0);
    let total = totals.get(i).copied().unwrap_or(0.0);
    if s <= 0.0 || total < SHARE_FLOOR {
        return s;
    }
    let (x, y) = centre(seg, i);
    let ours: f32 = mine.iter().filter_map(|j| each.get(*j)).map(|m| m.at(x, y)).sum();
    s * (ours / total).clamp(0.0, 1.0)
}

/// The matte of the parsed person Vision person `j` is: the person segmentation less every other Vision person's
/// share, those the parse has and those it does not.
fn person_matte(v: &People, totals: &[f32], j: usize, side: usize) -> Matte {
    let data = (0..v.all.width * v.all.height).map(|i| held(&v.all, &v.each, totals, &[j], i)).collect();
    to_matte(&av::Matte { width: v.all.width, height: v.all.height, data }, side)
}

#[cfg_attr(not(feature = "sam"), allow(dead_code))]
/// The Vision people who are part of `subject`: at least [`SUBJECT_OVERLAP`] of their mask inside it.
pub(crate) fn subject_people(each: &[av::Matte], subject: &av::Matte) -> Vec<usize> {
    let inside = |m: &av::Matte| {
        let (mut inter, mut area) = (0.0f32, 0.0f32);
        for i in 0..MATCH_GRID * MATCH_GRID {
            let (x, y) = (((i % MATCH_GRID) as f32 + 0.5) / MATCH_GRID as f32, ((i / MATCH_GRID) as f32 + 0.5) / MATCH_GRID as f32);
            let a = m.at(x, y);
            inter += a.min(subject.at(x, y));
            area += a;
        }
        if area > 0.0 { inter / area } else { 0.0 }
    };
    each.iter().enumerate().filter(|(_, m)| inside(m) >= SUBJECT_OVERLAP).map(|(j, _)| j).collect()
}

#[cfg_attr(not(feature = "sam"), allow(dead_code))]
/// `subject` with the hair of Vision people `theirs` added: at each pixel the larger of the subject and their share of
/// the person segmentation `v.all`.
pub(crate) fn with_hair(mut subject: av::Matte, v: &People, theirs: &[usize]) -> av::Matte {
    if theirs.is_empty() {
        return subject;
    }
    let resized;
    let seg = if (v.all.width, v.all.height) == (subject.width, subject.height) {
        &v.all
    } else {
        resized = v.all.resized(subject.width, subject.height);
        &resized
    };
    let totals = totals(seg, &v.each);
    for (i, a) in subject.data.iter_mut().enumerate() {
        *a = a.max(held(seg, &v.each, &totals, theirs, i));
    }
    subject
}

/// Redraw the Entire Person edge of every parsed person Vision matched with Vision's mattes (`super::people::refine`,
/// one matte per person), and add the strands they gain beside their hair to their Hair. A parsed person no Vision
/// person matches (often a part of one person the parse split off, such as a hand, or someone Vision missed) keeps the
/// parse's edge: the segmentation there is someone else's or nobody's, and drawn from it their edge would open a ring
/// of background or vanish. Marks the parse's version.
pub(crate) fn refine_people(parsing: &mut Parsing, v: &People) {
    let side = v.all.width.max(v.all.height).clamp(1, PEOPLE_SIDE);
    let theirs = assign(&v.each, &parsing.people);
    let before: Vec<Person> = parsing.people.clone();
    let totals = if theirs.iter().any(Option::is_some) { totals(&v.all, &v.each) } else { Vec::new() };
    for (k, person) in parsing.people.iter_mut().enumerate() {
        let Some(j) = theirs.get(k).copied().flatten() else { continue };
        let Some(rect) = entire_rect(person) else { continue };
        let matte = person_matte(v, &totals, j, side);
        if !matte.is_valid() {
            continue;
        }
        let others: Vec<&Person> = before.iter().enumerate().filter(|(i, _)| *i != k).map(|(_, p)| p).collect();
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
