//! [`gaps`]: the background a matte is sure of inside a coarse selection.

/// A cell is background to the matte below this logit (coverage under 5 %).
const SURE: f32 = -3.0;
/// …and a region of such cells counts only when its mean is below this (coverage under 0.7 %).
const SURE_MEAN: f32 = -5.0;
/// Regions smaller than this many cells are noise.
const MIN_CELLS: usize = 3;

/// The cells of a `w × h` grid inside the coarse selection (`inside`) that the matte (`matte`, logits on the same
/// grid) is sure are background, in connected regions (4-neighbour) each at most `cap` of the selection's area: the gap
/// between an arm and the body, between fingers, the background a coarse model filled in. Larger regions are left
/// alone (the matte has missed part of the subject, as a matte trained on people misses a held bag). Sizes that do not
/// add up select nothing.
pub fn gaps(inside: &[bool], matte: &[f32], w: usize, h: usize, cap: f32) -> Vec<bool> {
    let n = w.saturating_mul(h);
    let mut out = vec![false; n];
    if n == 0 || inside.len() != n || matte.len() != n || !cap.is_finite() {
        return out;
    }
    let area = inside.iter().filter(|v| **v).count();
    let most = (cap.clamp(0.0, 1.0) * area as f32) as usize;
    let candidate = |i: usize| inside.get(i).copied().unwrap_or(false) && matte.get(i).is_some_and(|l| *l < SURE);
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
        while let Some(i) = stack.pop() {
            region.push(i);
            sum += f64::from(matte.get(i).copied().unwrap_or(0.0));
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
        if sure && region.len() >= MIN_CELLS && region.len() <= most {
            for &i in &region {
                out[i] = true;
            }
        }
    }
    out
}
