//! A Select People selection (parts of one person or of everyone, from a people parse) drawn into the grids a
//! mask stores ([`SegMask`]): one grid over the selection, plus up to [`MAX_DETAIL`] finer grids over the people
//! whose parts the first one draws too coarsely (small faces in a group). The grids are on the mask's frame (the
//! photo with the user's rotation, before its crop and lens corrections); the parse is on the photo before the
//! user's rotation, so each cell is mapped back through `orient`.
//!
//! Pure: no model, no session. Every size is capped; a parse with damaged planes draws what it can.

use lightcraft_develop::SegMask;
use lightcraft_develop::segmask::{MAX_DETAIL, MAX_SIDE};
use lightcraft_geom::Orientation;
use lightcraft_parsing::{NormRect, Parsing, Part, Plane};

use super::parts::Pick;

/// Smallest grid side stored.
const MIN_SIDE: usize = 64;
/// Room around the selection, as a fraction of the photo, so the edge's soft fall-off is inside the grid.
const PAD: f32 = 0.01;
/// A finer grid is added for a person when the main grid's cells are this much coarser than their parts'.
const DETAIL_GAIN: f64 = 1.5;
/// Logits are clamped here (fully in or out once quantized, as SAM's grids are).
const LOGIT: f32 = 15.9;

/// What to draw: `parts` of person `person` (an index into `Parsing::people`; `None`: everyone), and the
/// background (everything that is not that person, or not anybody) when `background`.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub person: Option<usize>,
    pub parts: Vec<Part>,
    pub background: bool,
}

impl Selection {
    /// The selection of `picks` for `person`.
    pub fn of(person: Option<usize>, picks: &[Pick]) -> Selection {
        let mut parts: Vec<Part> = picks.iter().flat_map(|p| p.parts().iter().copied()).collect();
        parts.sort_unstable();
        parts.dedup();
        Selection { person, parts, background: picks.contains(&Pick::Background) }
    }
}

/// A plane drawn into the grids, with its rectangle on the mask's frame.
struct Placed<'a> {
    plane: &'a Plane,
    rect: NormRect,
    /// The plane's cell on the mask's frame (width, height, in fractions of the photo).
    cell: (f64, f64),
    /// Which person it belongs to.
    person: usize,
}

/// The grids of `sel` in `p` on the frame `orient` turns the parse's frame into: (the main grid, the finer ones).
/// `None` when the selection covers nothing (the parts are not in the photo, or the person is not there).
pub fn build(p: &Parsing, sel: &Selection, orient: Orientation) -> Option<(SegMask, Vec<SegMask>)> {
    let people: Vec<usize> = match sel.person {
        Some(k) if k < p.people.len() => vec![k],
        Some(_) => return None,
        None => (0..p.people.len()).collect(),
    };
    let place = |k: usize, part: Part| -> Vec<Placed<'_>> {
        p.people.get(k).and_then(|q| q.parts.get(&part)).into_iter().flatten().filter(|pl| pl.is_valid()).map(|pl| placed(pl, k, orient)).collect()
    };
    let parts: Vec<Placed<'_>> = people.iter().flat_map(|&k| sel.parts.iter().flat_map(move |&part| place(k, part))).collect();
    // the people the background leaves out (with no people, everything is background)
    let bodies: Vec<Placed<'_>> = if sel.background { people.iter().flat_map(|&k| place(k, Part::EntirePerson)).collect() } else { Vec::new() };
    if parts.is_empty() && !sel.background {
        return None;
    }
    let model = model_id(p, sel);
    // the people and everything that is not them: the whole photo (their outline would otherwise stay half selected,
    // where each side's soft edge is at 0.5)
    if sel.background && sel.parts.contains(&Part::EntirePerson) {
        return Some((grid(&vec![1.0; MIN_SIDE * MIN_SIDE], MIN_SIDE, &NormRect::new(0.0, 0.0, 1.0, 1.0), &model), Vec::new()));
    }
    let main_rect = if sel.background { NormRect::new(0.0, 0.0, 1.0, 1.0) } else { padded(union(parts.iter().map(|q| q.rect))?)? };
    let side = side_for(&main_rect, parts.iter().chain(&bodies));
    let draw = |rect: &NormRect, side: usize| draw(rect, side, &parts, &bodies, sel.background, orient);
    let main = grid(&draw(&main_rect, side), side, &main_rect, &model);
    // finer grids for the people the main grid draws too coarsely, largest first
    let main_cell = (f64::from(main_rect.width()) / side as f64, f64::from(main_rect.height()) / side as f64);
    let mut detail: Vec<(f32, NormRect)> = people
        .iter()
        .filter_map(|&k| {
            let mine: Vec<&Placed<'_>> = parts.iter().filter(|q| q.person == k).collect();
            let finest = mine.iter().map(|q| q.cell).reduce(|a, b| (a.0.min(b.0), a.1.min(b.1)))?;
            (main_cell.0 > DETAIL_GAIN * finest.0 || main_cell.1 > DETAIL_GAIN * finest.1).then_some(())?;
            let rect = padded(union(mine.iter().map(|q| q.rect))?)?;
            Some((rect.width() * rect.height(), rect))
        })
        .collect();
    detail.sort_by(|a, b| b.0.total_cmp(&a.0));
    let detail = detail
        .into_iter()
        .filter_map(|(_, rect)| {
            let side = side_for(&rect, parts.iter().filter(|q| overlaps(&q.rect, &rect)));
            // (a grid no finer than the main one adds nothing: one person alone, whose box is the main grid's, at the
            // largest side both)
            let cell = (f64::from(rect.width()) / side as f64, f64::from(rect.height()) / side as f64);
            (main_cell.0 > DETAIL_GAIN * cell.0 || main_cell.1 > DETAIL_GAIN * cell.1).then(|| grid(&draw(&rect, side), side, &rect, &model))
        })
        .take(MAX_DETAIL)
        .collect();
    Some((main, detail))
}

/// The model id the grids are marked with: the parse's set, and the edge model's mark (`+matting` for BiRefNet
/// Matting, `+vision` for Apple Vision) when the parse's people edges came from one and the selection takes an edge
/// it drew: Entire Person, Background (what the people are not) or Hair (it gains the strands the edge found). Such a
/// grid is not filtered again by a render at any size (`lightcraft_pipeline::masks::sample_seg`), so preview and
/// export draw the same outline.
pub fn model_id(p: &Parsing, sel: &Selection) -> String {
    let set = if p.model_set.is_empty() { "people" } else { p.model_set.as_str() };
    let edged = sel.background || sel.parts.iter().any(|part| matches!(part, Part::EntirePerson | Part::Hair));
    let vision = crate::hair_edges::vision::SUFFIX;
    let mark = if !edged {
        ""
    } else if p.model_version.contains("+matting") {
        "+matting"
    } else if p.model_version.contains(vision) {
        vision
    } else {
        ""
    };
    format!("people:{set}{mark}")
}

/// A grid that selects nothing, over the whole photo (a saved face not in this photo, or no parse of it).
pub fn nothing(model: &str) -> SegMask {
    grid(&vec![0.0; MIN_SIDE * MIN_SIDE], MIN_SIDE, &NormRect::new(0.0, 0.0, 1.0, 1.0), model)
}

fn placed(plane: &Plane, person: usize, orient: Orientation) -> Placed<'_> {
    let r = plane.rect;
    let (ax, ay) = orient.map(f64::from(r.x0), f64::from(r.y0), 1.0, 1.0);
    let (bx, by) = orient.map(f64::from(r.x1), f64::from(r.y1), 1.0, 1.0);
    let rect = NormRect::new(ax.min(bx) as f32, ay.min(by) as f32, ax.max(bx) as f32, ay.max(by) as f32);
    // (a quarter turn puts the plane's columns along the frame's height)
    let (cols, rows) = if orient.swaps_axes() { (plane.height, plane.width) } else { (plane.width, plane.height) };
    let cell = (f64::from(rect.width()) / cols.max(1) as f64, f64::from(rect.height()) / rows.max(1) as f64);
    Placed { plane, rect, cell, person }
}

fn union(mut rects: impl Iterator<Item = NormRect>) -> Option<NormRect> {
    let first = rects.next()?;
    Some(rects.fold(first, |a, b| a.union(&b)))
}

fn padded(r: NormRect) -> Option<NormRect> {
    NormRect::new(r.x0 - PAD, r.y0 - PAD, r.x1 + PAD, r.y1 + PAD).clamped()
}

fn overlaps(a: &NormRect, b: &NormRect) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

/// The side of a grid over `rect` whose cells are as fine as the finest of `planes` (64 to 1024).
fn side_for<'a, 'b: 'a>(rect: &NormRect, planes: impl Iterator<Item = &'a Placed<'b>>) -> usize {
    let need = planes
        .filter(|q| q.cell.0 > 0.0 && q.cell.1 > 0.0)
        .map(|q| (f64::from(rect.width()) / q.cell.0).max(f64::from(rect.height()) / q.cell.1))
        .fold(0.0, f64::max);
    if need.is_finite() { (need.ceil() as usize).clamp(MIN_SIDE, MAX_SIDE) } else { MAX_SIDE }
}

/// The selection's coverage (0 to 1) at the centre of each cell of a `side × side` grid over `rect`.
fn draw(rect: &NormRect, side: usize, parts: &[Placed<'_>], bodies: &[Placed<'_>], background: bool, orient: Orientation) -> Vec<f32> {
    let n = side * side;
    let back = orient.inverse();
    let at = |i: usize, j: usize| {
        let x = f64::from(rect.x0) + (i as f64 + 0.5) / side as f64 * f64::from(rect.width());
        let y = f64::from(rect.y0) + (j as f64 + 0.5) / side as f64 * f64::from(rect.height());
        let (u, v) = back.map(x, y, 1.0, 1.0);
        (u as f32, v as f32)
    };
    let fill = |planes: &[Placed<'_>], out: &mut Vec<f32>| fill_planes(planes, out, rect, side, &at);
    let mut out = vec![0.0f32; n];
    fill(parts, &mut out);
    if background {
        let mut people = vec![0.0f32; n];
        fill(bodies, &mut people);
        for (o, b) in out.iter_mut().zip(&people) {
            *o = o.max(1.0 - b);
        }
    }
    out
}

/// Draw `planes` into `out` (a `side × side` grid over `rect`; `at`: a cell's point on the parse) as
/// `lightcraft_parsing::render::mask` joins them: split planes add up across people, the larger within a person, and
/// every other plane by the larger value (#124). Each cell is sampled once per plane.
fn fill_planes(planes: &[Placed<'_>], out: &mut [f32], rect: &NormRect, side: usize, at: &dyn Fn(usize, usize) -> (f32, f32)) {
    let paint = |buf: &mut [f32], q: &Placed<'_>, join: &dyn Fn(f32, f32) -> f32| {
        let r = cells(rect, side, &q.rect)?;
        for j in r.2..r.3 {
            for i in r.0..r.1 {
                let (u, v) = at(i, j);
                if let Some(o) = buf.get_mut(j * side + i) {
                    *o = join(*o, q.plane.sample(u, v));
                }
            }
        }
        Some(r)
    };
    let mut split: Vec<&Placed<'_>> = planes.iter().filter(|q| q.plane.split).collect();
    split.sort_by_key(|q| q.person);
    // (a buffer of its own only for a person with several split planes, reused for the next)
    let mut one: Vec<f32> = Vec::new();
    for person in split.chunk_by(|a, b| a.person == b.person) {
        if let [q] = person {
            paint(out, q, &|o, v| (o + v).min(1.0));
            continue;
        }
        one.resize(out.len(), 0.0);
        let drawn: Vec<_> = person.iter().filter_map(|q| paint(&mut one, q, &f32::max)).collect();
        let Some(r) = drawn.into_iter().reduce(|a, b| (a.0.min(b.0), a.1.max(b.1), a.2.min(b.2), a.3.max(b.3))) else { continue };
        // add what this person drew, and clear it for the next, only over their own cells
        for j in r.2..r.3 {
            let row = j * side;
            let (Some(o), Some(v)) = (out.get_mut(row + r.0..row + r.1), one.get_mut(row + r.0..row + r.1)) else { continue };
            for (o, v) in o.iter_mut().zip(v.iter_mut()) {
                *o = (*o + *v).min(1.0);
                *v = 0.0;
            }
        }
    }
    for q in planes.iter().filter(|q| !q.plane.split) {
        paint(out, q, &f32::max);
    }
}

/// The cells of a `side × side` grid over `grid` that `r` touches: (i0, i1, j0, j1), ends excluded.
fn cells(grid: &NormRect, side: usize, r: &NormRect) -> Option<(usize, usize, usize, usize)> {
    let span = |a: f32, b: f32, g0: f32, g1: f32| {
        let w = g1 - g0;
        if w <= 0.0 {
            return None;
        }
        let lo = (((a - g0) / w) * side as f32).floor().max(0.0);
        let hi = (((b - g0) / w) * side as f32).ceil().min(side as f32);
        (lo.is_finite() && hi.is_finite() && hi > lo).then_some((lo as usize, hi as usize))
    };
    let (i0, i1) = span(r.x0, r.x1, grid.x0, grid.x1)?;
    let (j0, j1) = span(r.y0, r.y1, grid.y0, grid.y1)?;
    Some((i0, i1, j0, j1))
}

/// Coverage values as a stored grid over `rect` (the whole photo when it is).
fn grid(values: &[f32], side: usize, rect: &NormRect, model: &str) -> SegMask {
    let logits: Vec<f32> = values
        .iter()
        .map(|&a| {
            let a = if a.is_finite() { a.clamp(1e-7, 1.0 - 1e-7) } else { 0.0 };
            (a / (1.0 - a)).ln().clamp(-LOGIT, LOGIT)
        })
        .collect();
    let whole = rect.x0 <= 0.0 && rect.y0 <= 0.0 && rect.x1 >= 1.0 && rect.y1 >= 1.0;
    let seg = if whole {
        SegMask::from_logits(side, &logits)
    } else {
        SegMask::from_logits_in(side, &logits, [f64::from(rect.x0), f64::from(rect.y0), f64::from(rect.x1), f64::from(rect.y1)])
    };
    seg.made_by(model)
}
