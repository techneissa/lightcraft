//! [`gaps`]: the background a matte is sure of inside a coarse selection.

/// A cell is background to the matte below this logit (coverage under 5 %).
const SURE: f32 = -3.0;
/// …and a region of such cells counts only when its mean is below this (coverage under 0.7 %).
const SURE_MEAN: f32 = -5.0;
/// Regions smaller than this many cells are noise.
const MIN_CELLS: usize = 3;
/// A region must be this thick somewhere (its deepest cell this far from its border, in thirds of a cell: one cell,
/// a region at least three cells across): a gap between an arm and the body is, while a thin dark line the matte doubts
/// (a zipper, a fold) is not, and opening it would cut the person in two.
const MIN_DEPTH: u32 = 3;

/// The cells of a `w × h` grid inside the coarse selection (`inside`) that the matte (`matte`, logits on the same
/// grid) is sure are background, in connected regions (4-neighbour) each at most `cap` of the selection's area: the gap
/// between an arm and the body, between fingers, the background a coarse model filled in. Larger regions are left
/// alone (the matte has missed part of the subject, as a matte trained on people misses a held bag). Sizes that do not
/// add up select nothing. A region must also be three cells thick somewhere: a thin line the matte doubts is no
/// gap.
pub fn gaps(inside: &[bool], matte: &[f32], w: usize, h: usize, cap: f32) -> Vec<bool> {
    let n = w.saturating_mul(h);
    let mut out = vec![false; n];
    if n == 0 || inside.len() != n || matte.len() != n || !cap.is_finite() {
        return out;
    }
    let area = inside.iter().filter(|v| **v).count();
    let most = (cap.clamp(0.0, 1.0) * area as f32) as usize;
    let candidate = |i: usize| inside.get(i).copied().unwrap_or(false) && matte.get(i).is_some_and(|l| *l < SURE);
    let cand: Vec<bool> = (0..n).map(candidate).collect();
    let depth = crate::distance_to_edge(&cand, w, h);
    let mut seen = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut region: Vec<usize> = Vec::new();
    for start in 0..n {
        if seen[start] || !candidate(start) {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        region.clear();
        let mut sum = 0.0f64;
        let mut deepest = 0u32;
        while let Some(i) = stack.pop() {
            region.push(i);
            sum += f64::from(matte.get(i).copied().unwrap_or(0.0));
            deepest = deepest.max(depth.get(i).copied().unwrap_or(0));
            let (x, y) = (i % w, i / w);
            let next = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
            for j in next.into_iter().flatten() {
                if !seen[j] && candidate(j) {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        let sure = sum / region.len() as f64 <= f64::from(SURE_MEAN);
        // (and thick for its size: a long thin strip, a fold or a zipper's dark line the matte doubts, is no gap; a
        // gap between an arm and the body, or between fingers, is less than about ten times as long as it is wide)
        let thick = f64::from(deepest) / 3.0 >= 0.15 * (region.len() as f64).sqrt();
        if sure && region.len() >= MIN_CELLS && region.len() <= most && deepest >= MIN_DEPTH && thick {
            for &i in &region {
                out[i] = true;
            }
        }
    }
    out
}
