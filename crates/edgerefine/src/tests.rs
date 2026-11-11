use super::*;

/// A 480 × 320 photo: a dark brown hand (a palm and two fingers 60 px wide, 12 px apart and joined at the tips, so the
/// gap between them is enclosed, as between an arm on a hip and the body) and a lock of hair 5 px wide and 24 px long (a strand the 6 px matte cells see as one row)
/// leaving the palm, on a light green background. Returns the picture and the true coverage.
fn hand() -> (Vec<u8>, Vec<f32>, usize, usize) {
    let (w, h) = (480usize, 320usize);
    let mut truth = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let palm = (100..300).contains(&x) && (150..280).contains(&y);
            let finger = (30..150).contains(&y) && ((100..160).contains(&x) || (172..232).contains(&x));
            let tip = (30..60).contains(&y) && (100..232).contains(&x);
            let wisp = (178..183).contains(&y) && (300..324).contains(&x);
            if palm || finger || tip || wisp {
                truth[y * w + x] = 1.0;
            }
        }
    }
    let rgb = truth.iter().flat_map(|t| if *t > 0.5 { [70u8, 45, 30] } else { [190u8, 215, 170] }).collect();
    (rgb, truth, w, h)
}

/// The true coverage averaged over an `n × n` grid over the picture (stretched to a square), as logits of `strength`.
fn grid_of(truth: &[f32], w: usize, h: usize, n: usize, strength: f32) -> Vec<f32> {
    (0..n * n)
        .map(|i| {
            let (cx, cy) = (i % n, i / n);
            let (xa, xb) = (cx * w / n, ((cx + 1) * w / n).max(cx * w / n + 1));
            let (ya, yb) = (cy * h / n, ((cy + 1) * h / n).max(cy * h / n + 1));
            let mut s = 0.0;
            for y in ya..yb {
                for x in xa..xb {
                    s += truth[y * w + x];
                }
            }
            let c = s / ((xb - xa) * (yb - ya)) as f32;
            strength * (2.0 * c - 1.0)
        })
        .collect()
}

/// An `n × n` grid of logits enlarged bilinearly to the picture's pixels (as a render samples a stored grid).
fn alpha_of(logits: &[f32], n: usize, w: usize, h: usize) -> Alpha {
    Alpha::from_fn(w, h, |fx, fy| {
        let (gx, gy) = ((fx * n as f32 - 0.5).clamp(0.0, (n - 1) as f32), (fy * n as f32 - 0.5).clamp(0.0, (n - 1) as f32));
        let (x0, y0) = (gx as usize, gy as usize);
        let (x1, y1) = ((x0 + 1).min(n - 1), (y0 + 1).min(n - 1));
        let (tx, ty) = (gx - x0 as f32, gy - y0 as f32);
        let at = |x: usize, y: usize| logits[y * n + x];
        sigmoid((at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty) + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty)
    })
}

/// (the gap's worst coverage along its middle, the wisp's mean coverage, the worst error inside the palm and on the
/// background far from the hand)
fn measure(a: &Alpha) -> (f32, f32, f32) {
    let w = a.w;
    let at = |x: usize, y: usize| a.data[y * w + x];
    let gap = (75..135).map(|y| at(165, y).max(at(166, y))).fold(0.0, f32::max);
    let wisp = (302..318).map(|x| at(x, 180)).sum::<f32>() / 16.0;
    let solid = (1.0 - at(200, 220)).max(at(30, 300)).max(at(450, 20));
    (gap, wisp, solid)
}

/// The photo, today's coarse selection (a model whose cells are 40 px: a people parse or SAM 3's grid on a large
/// photo) and an edge model's matte (6 px cells: BiRefNet Matting's 1024² on a 24 MP photo).
fn scene() -> (Vec<u8>, usize, usize, Vec<f32>, Vec<f32>) {
    let (rgb, truth, w, h) = hand();
    (rgb, w, h, grid_of(&truth, w, h, 12, 8.0), grid_of(&truth, w, h, 80, 10.0))
}

#[test]
fn refining_opens_the_finger_gap_and_keeps_the_wisp() {
    let (rgb, w, h, base, matte) = scene();
    // today, Current edge: the coarse selection fills the gap and loses the wisp
    let (gap, wisp, _) = measure(&alpha_of(&upsample(&base, 12, 80), 80, w, h));
    assert!(gap > 0.5 && wisp < 0.3, "the coarse mask fills the gap ({gap:.2}) and loses the wisp ({wisp:.2})");
    // today, BiRefNet Matting edge: the matte decides only in a band around the coarse edge, which misses the gap
    let (gap, ..) = measure(&alpha_of(&decide(&base, 12, &matte, 80, 0.1, None), 80, w, h));
    assert!(gap > 0.5, "matting in the band alone leaves the enclosed gap filled ({gap:.2})");
    // refined: the matte also opens the gap, and the edge is snapped to the photo
    let decided = decide(&base, 12, &matte, 80, 0.1, Some(0.05));
    let mut a = alpha_of(&decided, 80, w, h);
    let changed = snap(&Rgb { data: &rgb, w, h }, &mut a, radius_for(6.0), &|| false).unwrap();
    assert!(changed > 0);
    let (gap, wisp, solid) = measure(&a);
    // (what is left is the guided filter's bias next to a 6 px matte cell: 5 % at most)
    assert!(gap < 0.07, "the gap between the fingers is open: {gap:.3}");
    assert!(wisp > 0.7, "the wisp stays: {wisp:.3}");
    assert!(solid < 0.02, "the palm stays in and the far background out: {solid:.3}");
}

#[test]
fn snapping_leaves_everything_away_from_the_edge_alone_and_can_stop() {
    let (rgb, w, h, base, matte) = scene();
    let img = Rgb { data: &rgb, w, h };
    let before = alpha_of(&decide(&base, 12, &matte, 80, 0.1, Some(0.05)), 80, w, h);
    let mut after = before.clone();
    snap(&img, &mut after, 9, &|| false).unwrap();
    for (b, a) in before.data.iter().zip(&after.data) {
        if *b <= 0.0 || *b >= 1.0 {
            // (a pixel whose window held no edge pixel is exactly as it was)
            continue;
        }
        assert!(a.is_finite() && (0.0..=1.0).contains(a));
    }
    assert_eq!(after.data[319 * w + 479], before.data[319 * w + 479]);
    let mut stopped = before.clone();
    assert_eq!(snap(&img, &mut stopped, 9, &|| true), Err(Stopped));
    assert_eq!(stopped, before, "a stopped snap changes nothing");
    // sizes that don't add up change nothing
    let mut odd = Alpha { w: 3, h: 3, data: vec![0.5; 9] };
    assert_eq!(snap(&Rgb { data: &[0; 5], w: 3, h: 3 }, &mut odd, 2, &|| false), Ok(0));
    assert_eq!(snap(&img, &mut odd, 2, &|| false), Ok(0));
}

#[test]
fn gaps_finds_small_sure_background_inside_the_selection_only() {
    let (w, h) = (40usize, 40usize);
    // the selection is a 30 × 30 square; inside it the matte sees a 4 × 4 hole (a gap) and a 20 × 20 region (a held
    // bag the matte drops), and one cell that is merely unsure
    let inside: Vec<bool> = (0..w * h).map(|i| (5..35).contains(&(i % w)) && (5..35).contains(&(i / w))).collect();
    let matte: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            if (8..12).contains(&x) && (8..12).contains(&y) || (14..34).contains(&x) && (14..34).contains(&y) {
                -9.0
            } else if (x, y) == (6, 30) {
                -3.5
            } else if inside[i] {
                8.0
            } else {
                -9.0
            }
        })
        .collect();
    let g = gaps(&inside, &matte, w, h, 0.05);
    assert!(g[10 * w + 10], "the small hole is a gap");
    assert!(!g[20 * w + 20], "a large region (44 % of the selection) is not");
    assert!(!g[30 * w + 6], "one unsure cell is not");
    assert!(!g[0], "outside the selection is never a gap");
    assert!(gaps(&inside, &matte[..5], w, h, 0.05).iter().all(|v| !v));
    assert!(gaps(&[], &[], 0, 0, 0.05).is_empty());
}

#[test]
fn store_keeps_every_edge_pixel_in_finer_grids_that_redraw_it() {
    let (rgb, w, h, base, matte) = scene();
    let mut a = alpha_of(&decide(&base, 12, &matte, 80, 0.1, Some(0.05)), 80, w, h);
    snap(&Rgb { data: &rgb, w, h }, &mut a, radius_for(6.0), &|| false).unwrap();
    let (main, tiles) = store(&a, 64).unwrap();
    assert_eq!((main.side, main.logits.len(), main.rect), (64, 64 * 64, [0.0, 0.0, 1.0, 1.0]));
    assert_eq!(tiles.len(), 1, "an edge box under 1024 px is one grid");
    let t = &tiles[0];
    // every edge pixel is inside it, and its cells are one pixel: it gives the coverage back
    for (i, v) in a.data.iter().enumerate() {
        if *v > 0.01 && *v < 0.99 {
            let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
            assert!(x / w as f64 >= t.rect[0] && x / w as f64 <= t.rect[2] && y / h as f64 >= t.rect[1] && y / h as f64 <= t.rect[3]);
        }
    }
    let side = t.side;
    let (x0, y0) = (t.rect[0] * w as f64, t.rect[1] * h as f64);
    let (cx, cy) = ((t.rect[2] - t.rect[0]) * w as f64 / side as f64, (t.rect[3] - t.rect[1]) * h as f64 / side as f64);
    let j = ((180.5 - y0) / cy) as usize;
    let i = ((310.5 - x0) / cx) as usize;
    assert!(sigmoid(t.logits[j * side + i]) > 0.6, "the wisp is in the finer grid");
    // a 2400 px wide edge is cut into two grids, each at most 1024 cells
    let wide = Alpha::from_fn(2400, 100, |x, y| if (y - 0.5).abs() < 0.2 && x > 0.02 && x < 0.98 { 0.5 } else { 0.0 });
    let (_, parts) = store(&wide, 64).unwrap();
    assert_eq!(parts.len(), 2);
    assert!(parts.iter().all(|g| g.side <= MAX_SIDE));
    // nothing on an edge: no finer grid; damaged sizes: nothing
    assert!(store(&Alpha { w: 4, h: 4, data: vec![1.0; 16] }, 8).unwrap().1.is_empty());
    assert!(store(&Alpha { w: 4, h: 4, data: vec![1.0; 3] }, 8).is_none());
    assert!(tile(&a, [10.0, 10.0, 5.0, 20.0], 8).is_none());
}
