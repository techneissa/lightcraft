//! Telling people apart. The body model sees "person" pixels, not persons: its person area is split into connected
//! pieces, each face claims the piece it sits in, and a piece with several faces (people touching) goes to the
//! nearest face pixel by pixel. A big piece with no face is a person seen without one; a small one joins the nearest
//! person or is dropped.

use crate::types::NormRect;

/// Which person owns each pixel of the body map (`0`: nobody; `k`: person `k − 1`).
pub struct Owners {
    pub owner: Vec<u16>,
    /// People found: those with faces first (in the order of the faces given), then those without.
    pub people: usize,
}

/// A piece of the person area with no face needs this share of the photo to count as a person.
const FACELESS_MIN_AREA: f32 = 0.015;
/// Below this share of the photo a piece is noise.
const NOISE_AREA: f32 = 0.0004;
/// Body-map pixels the owners grow into the soft edge around the person area.
const EDGE_GROW: usize = 3;

/// Connected pieces (4-neighbour) of `mask`, labelled from 1, and each piece's size.
pub fn components(mask: &[bool], w: usize, h: usize) -> (Vec<u32>, Vec<usize>) {
    let mut label = vec![0u32; mask.len()];
    let mut sizes = vec![0usize];
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask.get(start).copied().unwrap_or(false) || label.get(start).copied().unwrap_or(1) != 0 {
            continue;
        }
        let id = u32::try_from(sizes.len()).unwrap_or(u32::MAX);
        let mut size = 0;
        stack.push(start);
        if let Some(l) = label.get_mut(start) {
            *l = id;
        }
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = (i % w, i / w);
            let mut push = |j: usize| {
                if mask.get(j).copied().unwrap_or(false) && label.get(j).copied() == Some(0) {
                    if let Some(l) = label.get_mut(j) {
                        *l = id;
                    }
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        sizes.push(size);
    }
    (label, sizes)
}

/// Split the person area (`person`, 0 to 1 per body-map pixel) between `faces` (fractions of the photo).
pub fn assign(person: &[f32], w: usize, h: usize, faces: &[NormRect]) -> Owners {
    let n = w * h;
    let mask: Vec<bool> = person.iter().map(|&p| p > 0.5).collect();
    let (label, sizes) = components(&mask, w, h);
    let total = n.max(1) as f32;
    // the piece each face sits in: the label most of its box's pixels have
    let face_px: Vec<(f32, f32, f32)> = faces
        .iter()
        .map(|f| {
            let (cx, cy) = f.centre();
            (cx * w as f32, cy * h as f32, (f.width() * w as f32).max(f.height() * h as f32).max(1.0))
        })
        .collect();
    let mut piece_faces: Vec<Vec<usize>> = vec![Vec::new(); sizes.len()];
    for (k, f) in faces.iter().enumerate() {
        let mut counts: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        let (x0, x1) = ((f.x0 * w as f32).floor().max(0.0) as usize, ((f.x1 * w as f32).ceil() as usize).min(w));
        let (y0, y1) = ((f.y0 * h as f32).floor().max(0.0) as usize, ((f.y1 * h as f32).ceil() as usize).min(h));
        for y in y0..y1 {
            for x in x0..x1 {
                if let Some(&l) = label.get(y * w + x)
                    && l != 0
                {
                    *counts.entry(l).or_default() += 1;
                }
            }
        }
        if let Some((&l, _)) = counts.iter().max_by_key(|(l, c)| (**c, std::cmp::Reverse(**l)))
            && let Some(list) = piece_faces.get_mut(l as usize)
        {
            list.push(k);
        }
    }
    let mut people = faces.len();
    let mut piece_owner: Vec<Option<u16>> = vec![None; sizes.len()];
    for (l, list) in piece_faces.iter().enumerate().skip(1) {
        let area = sizes.get(l).copied().unwrap_or(0) as f32 / total;
        if list.is_empty() && area >= FACELESS_MIN_AREA && people < usize::from(u16::MAX - 1) {
            people += 1;
            if let Some(slot) = piece_owner.get_mut(l) {
                *slot = u16::try_from(people).ok();
            }
        }
    }
    let nearest = |x: f32, y: f32, among: &[usize]| -> Option<(usize, f32)> {
        among
            .iter()
            .filter_map(|&k| {
                face_px.get(k).map(|&(fx, fy, s)| (k, ((x - fx) / s).powi(2) + ((y - fy) / (2.5 * s)).powi(2) + if y < fy - s { 4.0 } else { 0.0 }))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
    };
    let all: Vec<usize> = (0..faces.len()).collect();
    let mut owner = vec![0u16; n];
    for (i, o) in owner.iter_mut().enumerate() {
        let l = label.get(i).copied().unwrap_or(0) as usize;
        if l == 0 {
            continue;
        }
        let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
        let list = piece_faces.get(l).map_or(&[][..], Vec::as_slice);
        *o = match list {
            [] => match piece_owner.get(l).copied().flatten() {
                Some(p) => p,
                None if (sizes.get(l).copied().unwrap_or(0) as f32 / total) < NOISE_AREA => 0,
                // a hand or leg cut off by something in front: the nearest face's person, when it is near
                None => nearest(x, y, &all).filter(|&(_, d)| d < 9.0).map_or(0, |(k, _)| u16::try_from(k + 1).unwrap_or(0)),
            },
            [k] => u16::try_from(k + 1).unwrap_or(0),
            many => nearest(x, y, many).map_or(0, |(k, _)| u16::try_from(k + 1).unwrap_or(0)),
        };
    }
    grow(&mut owner, person, w, h);
    Owners { owner, people }
}

/// Let owners spread a few pixels into the soft edge just outside the person area, so the mask keeps its fade.
fn grow(owner: &mut [u16], person: &[f32], w: usize, h: usize) {
    for _ in 0..EDGE_GROW {
        let prev = owner.to_vec();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if prev.get(i).copied() != Some(0) || person.get(i).copied().unwrap_or(0.0) < 0.02 {
                    continue;
                }
                let around = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
                if let Some(o) = around.into_iter().flatten().filter_map(|j| prev.get(j).copied()).find(|&o| o != 0)
                    && let Some(slot) = owner.get_mut(i)
                {
                    *slot = o;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(w: usize, h: usize, boxes: &[(usize, usize, usize, usize)]) -> Vec<f32> {
        let mut p = vec![0.0; w * h];
        for &(x0, y0, x1, y1) in boxes {
            for y in y0..y1 {
                for x in x0..x1 {
                    p[y * w + x] = 1.0;
                }
            }
        }
        p
    }

    #[test]
    fn two_people_touching_are_split_between_their_faces() {
        let (w, h) = (100, 100);
        let p = blob(w, h, &[(10, 10, 90, 100)]);
        let faces = [NormRect::new(0.15, 0.1, 0.35, 0.3), NormRect::new(0.65, 0.1, 0.85, 0.3)];
        let o = assign(&p, w, h, &faces);
        assert_eq!(o.people, 2);
        assert_eq!(o.owner[50 * w + 20], 1);
        assert_eq!(o.owner[50 * w + 80], 2);
        assert_eq!(o.owner[5 * w + 50], 0);
    }

    #[test]
    fn a_big_piece_without_a_face_is_its_own_person_and_specks_are_dropped() {
        let (w, h) = (100, 100);
        let p = blob(w, h, &[(0, 0, 30, 100), (60, 0, 100, 100), (45, 45, 46, 46)]);
        let o = assign(&p, w, h, &[NormRect::new(0.05, 0.05, 0.25, 0.2)]);
        assert_eq!(o.people, 2);
        assert_eq!(o.owner[50 * w + 10], 1);
        assert_eq!(o.owner[50 * w + 80], 2);
        assert_eq!(o.owner[45 * w + 45], 0);
    }

    #[test]
    fn no_people_and_odd_sizes_never_panic() {
        assert_eq!(assign(&[], 0, 0, &[]).people, 0);
        assert_eq!(assign(&[1.0], 1, 1, &[NormRect::new(-1.0, -1.0, 2.0, 2.0)]).people, 1);
        let o = assign(&[0.0; 4], 2, 2, &[NormRect::new(f32::NAN, 0.0, 1.0, 1.0)]);
        assert!(o.owner.iter().all(|&v| v == 0));
    }
}
