//! BiRefNet-matting edges for people parsing's Entire Person (and so its Background).
//!
//! The parse decides who is where; the matte draws the edge. Away from a person's edge their mask stays as parsed (the
//! matte's mistakes elsewhere, and other foreground things it cuts out, never come in). Within a band around the edge
//! the matte decides: strands the parse cut off come in, background between them goes out. A person never grows into
//! another person, and the strands a person gains next to their hair are added to their Hair part.

use lightcraft_parsing::{NormRect, Parsing, Part, Person, Plane};

use super::Matte;

/// Half-width of the band where the matte decides, as a fraction of the photo's side (3 % ≈ 31 matte cells at 1024):
/// wide enough for the parse's soft edge and for loose hair.
pub const BAND: f32 = 0.03;
/// Largest grid a person's new mask may have on a side (the matte's own resolution is the limit in practice).
const MAX_CELLS: usize = 2048;

/// Redraw every person's Entire Person edge with `matte`, and add the strands they gain beside their hair to their
/// Hair. A matte that does not add up leaves the parse as it was. Marks the parse's version.
pub fn refine(parsing: &mut Parsing, matte: &Matte) {
    if !matte.is_valid() {
        return;
    }
    let before: Vec<Person> = parsing.people.clone();
    for (k, person) in parsing.people.iter_mut().enumerate() {
        let Some(rect) = entire_rect(person) else { continue };
        let others: Vec<&Person> = before.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, p)| p).collect();
        let Some(Refined { entire, gained }) = refine_person(person, &others, rect, matte) else { continue };
        person.parts.insert(Part::EntirePerson, vec![entire]);
        if let Some(g) = gained
            && let Some(hair) = person.parts.get_mut(&Part::Hair)
        {
            hair.push(g);
        }
        person.bounds = Some(person.bounds.map_or(rect, |b| b.union(&rect)));
    }
    parsing.model_version = format!("{}+{}", parsing.model_version, super::VERSION);
}

/// The rectangle covering a person's Entire Person planes, grown by the band.
pub(super) fn entire_rect(p: &Person) -> Option<NormRect> {
    let planes = p.parts.get(&Part::EntirePerson)?;
    let r = planes.iter().filter(|q| q.is_valid()).map(|q| q.rect).reduce(|a, b| a.union(&b))?;
    NormRect::new(r.x0 - BAND, r.y0 - BAND, r.x1 + BAND, r.y1 + BAND).clamped()
}

pub(super) struct Refined {
    pub(super) entire: Plane,
    pub(super) gained: Option<Plane>,
}

pub(super) fn refine_person(p: &Person, others: &[&Person], r: NormRect, matte: &Matte) -> Option<Refined> {
    let n = matte.side as f32;
    let gw = ((r.width() * n).ceil() as usize).clamp(1, MAX_CELLS);
    let gh = ((r.height() * n).ceil() as usize).clamp(1, MAX_CELLS);
    let at = |i: usize| {
        (
            r.x0 + (i % gw) as f32 / gw as f32 * r.width() + r.width() / (2 * gw) as f32,
            r.y0 + (i / gw) as f32 / gh as f32 * r.height() + r.height() / (2 * gh) as f32,
        )
    };
    let old: Vec<f32> = (0..gw * gh)
        .map(|i| {
            let (x, y) = at(i);
            p.sample(Part::EntirePerson, x, y)
        })
        .collect();
    let inside: Vec<bool> = old.iter().map(|v| *v > 0.5).collect();
    if !inside.iter().any(|v| *v) {
        return None;
    }
    let dist = distance_to_edge(&inside, gw, gh);
    // the band in grid cells (a cell is about one matte cell)
    let band = (BAND * n).max(1.0);
    let hair_near = near(p, Part::Hair, &at, gw, gh, band);
    let mut entire = Vec::with_capacity(gw * gh);
    let mut gained = Vec::with_capacity(gw * gh);
    for i in 0..gw * gh {
        let (x, y) = at(i);
        let o = old.get(i).copied().unwrap_or(0.0);
        let px = dist.get(i).copied().unwrap_or(u32::MAX) as f32 / 3.0;
        // 1 within half the band of the parse's edge, 0 beyond the band, smooth between
        let t = ((px - band / 2.0) / (band / 2.0)).clamp(0.0, 1.0);
        let w = 1.0 - t * t * (3.0 - 2.0 * t);
        let mut v = w * matte.coverage(x, y) + (1.0 - w) * o;
        // (never into another person: where someone else is, this person may only lose)
        if v > o && others.iter().any(|q| q.sample(Part::EntirePerson, x, y) > 0.5) {
            v = o;
        }
        entire.push(v.clamp(0.0, 1.0));
        let g = if hair_near.get(i).copied().unwrap_or(false) { (v - o).max(0.0) } else { 0.0 };
        gained.push(g);
    }
    let gained = gained.iter().any(|g| *g > 0.05).then_some(Plane { rect: r, width: gw, height: gh, data: gained });
    Some(Refined { entire: Plane { rect: r, width: gw, height: gh, data: entire }, gained })
}

/// Which cells lie within `band` cells of the person's `part` (none when they have no such part).
fn near(p: &Person, part: Part, at: &dyn Fn(usize) -> (f32, f32), gw: usize, gh: usize, band: f32) -> Vec<bool> {
    if !p.parts.contains_key(&part) {
        return vec![false; gw * gh];
    }
    let has: Vec<bool> = (0..gw * gh)
        .map(|i| {
            let (x, y) = at(i);
            p.sample(part, x, y) > 0.5
        })
        .collect();
    if !has.iter().any(|v| *v) {
        return has;
    }
    // distance to the part's edge, and inside the part counts as zero
    let d = distance_to_edge(&has, gw, gh);
    has.iter().zip(&d).map(|(h, d)| *h || (*d as f32 / 3.0) <= band).collect()
}

/// Chamfer distance (3-4 weights, in thirds of a cell) of every cell to the nearest cell where `inside` differs from
/// its own value. A grid with one value everywhere is all far.
pub(crate) fn distance_to_edge(inside: &[bool], w: usize, h: usize) -> Vec<u32> {
    const BIG: u32 = u32::MAX / 4;
    let mut d: Vec<u32> = vec![BIG; w * h];
    let get_in = |x: usize, y: usize| inside.get(y * w + x).copied().unwrap_or(false);
    for y in 0..h {
        for x in 0..w {
            let me = get_in(x, y);
            let edge = (x > 0 && get_in(x - 1, y) != me)
                || (x + 1 < w && get_in(x + 1, y) != me)
                || (y > 0 && get_in(x, y - 1) != me)
                || (y + 1 < h && get_in(x, y + 1) != me);
            if edge && let Some(v) = d.get_mut(y * w + x) {
                *v = 0;
            }
        }
    }
    let get = |d: &[u32], x: isize, y: isize| -> u32 {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize { BIG } else { d.get(y as usize * w + x as usize).copied().unwrap_or(BIG) }
    };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let best =
                [get(&d, x - 1, y) + 3, get(&d, x, y - 1) + 3, get(&d, x - 1, y - 1) + 4, get(&d, x + 1, y - 1) + 4].into_iter().min().unwrap_or(BIG);
            if let Some(v) = d.get_mut(y as usize * w + x as usize) {
                *v = (*v).min(best);
            }
        }
    }
    for y in (0..h as isize).rev() {
        for x in (0..w as isize).rev() {
            let best =
                [get(&d, x + 1, y) + 3, get(&d, x, y + 1) + 3, get(&d, x + 1, y + 1) + 4, get(&d, x - 1, y + 1) + 4].into_iter().min().unwrap_or(BIG);
            if let Some(v) = d.get_mut(y as usize * w + x as usize) {
                *v = (*v).min(best);
            }
        }
    }
    d
}
