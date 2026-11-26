use super::*;

const W: usize = 100;
const H: usize = 80;

/// A mask that is `inside` where `f` says so.
fn mask(f: impl Fn(usize, usize) -> bool) -> Mask {
    Mask::new(W, H, (0..W * H).map(|i| if f(i % W, i / W) { 1.0 } else { 0.0 }).collect()).expect("size")
}

fn block(x0: usize, x1: usize) -> Mask {
    mask(move |x, y| (x0..x1).contains(&x) && (20..60).contains(&y))
}

fn of(pred: &Mask, truth: &Mask) -> Scores {
    score(pred, truth, None).expect("same size")
}

#[test]
fn a_perfect_mask_scores_best_and_a_shifted_edge_scores_lower() {
    let truth = block(20, 60);
    let perfect = of(&truth, &truth);
    let near = of(&block(22, 62), &truth);
    let far = of(&block(30, 70), &truth);
    assert!(perfect.iou > near.iou && near.iou > far.iou, "{perfect:?} {near:?} {far:?}");
    assert!(perfect.bg_iou > near.bg_iou && near.bg_iou > far.bg_iou);
    assert!(perfect.band_iou > near.band_iou);
    assert!(perfect.boundary_f > near.boundary_f && near.boundary_f > far.boundary_f, "{near:?} {far:?}");
    assert!(perfect.sad < near.sad && near.sad < far.sad);
    assert!(perfect.mse < near.mse);
}

#[test]
fn a_soft_edge_costs_less_matte_error_than_a_wrong_one() {
    // truth fading from in to out between x = 40 and 50
    let ramp = |at: f32| Mask::new(W, H, (0..W * H).map(|i| ((at - (i % W) as f32) / 10.0).clamp(0.0, 1.0)).collect()).expect("size");
    let truth = ramp(50.0);
    let (close, off) = (of(&ramp(51.0), &truth), of(&ramp(58.0), &truth));
    assert!(close.sad < off.sad && close.mse < off.mse, "{close:?} {off:?}");
}

#[test]
fn filling_an_enclosed_gap_is_seen() {
    let ring = mask(|x, y| (20..60).contains(&x) && (20..60).contains(&y) && !((35..45).contains(&x) && (35..45).contains(&y)));
    let filled = block(20, 60);
    assert!(of(&ring, &ring).gaps_kept > of(&filled, &ring).gaps_kept);
    // no hole in the truth: nothing to keep
    assert_eq!(of(&filled, &filled).gaps_kept, None);
}

#[test]
fn unlabelled_pixels_do_not_count() {
    let truth = block(20, 60);
    let wide = block(20, 70);
    let ignore: Vec<bool> = (0..W * H).map(|i| i % W >= 60).collect();
    let seen = score(&wide, &truth, Some(&ignore)).expect("same size");
    assert!(seen.iou > of(&wide, &truth).iou);
}

#[test]
fn clicks_go_to_the_largest_error_first() {
    let truth = block(20, 60);
    let empty = mask(|_, _| false);
    let first = next_click(&empty, &truth, None).expect("a click");
    assert!(first.include && (30..50).contains(&first.x) && (30..50).contains(&first.y), "the middle: {first:?}");
    // a mask that spills far to the right and misses a sliver on the left: the spill is the bigger error
    let spilled = block(22, 90);
    let next = next_click(&spilled, &truth, None).expect("a click");
    assert!(!next.include && (60..90).contains(&next.x), "{next:?}");
    assert_eq!(next_click(&truth, &truth, None), None);
}

#[test]
fn the_distance_is_to_the_nearest_target() {
    let mut target = vec![false; 7 * 5];
    target[0] = true;
    target[4 * 7 + 6] = true;
    let d = squared_distance(&target, 7, 5);
    for y in 0..5usize {
        for x in 0..7usize {
            let want = (x * x + y * y).min((6 - x) * (6 - x) + (4 - y) * (4 - y));
            assert_eq!(d[y * 7 + x], want as f64, "at {x},{y}");
        }
    }
}
