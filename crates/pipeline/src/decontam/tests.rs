use lightcraft_raster::{Plane, Rgb32f};

use super::*;

/// Red hair strands over a blue sky: a column of pixels each `k`/10 hair.
fn strands(w: usize, h: usize) -> (Rgb32f, Plane) {
    let hair = [0.6f32, 0.25, 0.1];
    let sky = [0.1f32, 0.3, 0.8];
    let alpha_at = |x: usize| {
        if x < w * 45 / 100 {
            1.0
        } else if x < w * 55 / 100 {
            ((x * 7) % 10) as f32 / 10.0
        } else {
            0.0
        }
    };
    let img = Rgb32f::from_fn(w, h, |x, _| {
        let a = alpha_at(x);
        std::array::from_fn(|c| a * hair[c] + (1.0 - a) * sky[c])
    });
    let mut alpha = Plane::new(w, h);
    for (i, v) in alpha.data.iter_mut().enumerate() {
        *v = alpha_at(i % w);
    }
    (img, alpha)
}

/// The whole frame's estimate, as the first version computed it: the reference the banded one must equal.
fn whole_frame(img: &Rgb32f, alpha: &Plane, long_px: f64) -> (Rgb32f, Rgb32f) {
    let (w, h) = (img.width, img.height);
    let all = Rect { x0: 0, y0: 0, x1: w, y1: h };
    let a = MaskAlpha::from_plane(alpha.clone());
    let radius = |f: f64| ((f * long_px).round().max(1.0) as usize).min(w.max(h));
    let (s1, u1) = pass(img, &a, None, all, all, radius(WIDE));
    let (s, u) = pass(img, &a, Some((&s1, &u1)), all, all, radius(NARROW));
    let (mut s, mut u) = (Rgb32f { width: w, height: h, data: s.data }, Rgb32f { width: w, height: h, data: u.data });
    for ((sp, up), (i_px, al)) in s.data.iter_mut().zip(u.data.iter_mut()).zip(img.data.iter().zip(&alpha.data)) {
        let al = sane(*al);
        let norm = al * al + (1.0 - al) * (1.0 - al);
        for c in 0..3 {
            let r = (i_px[c] - (al * sp[c] + (1.0 - al) * up[c])) / norm;
            sp[c] = (sp[c] + al * r).max(0.0);
            up[c] = (up[c] + (1.0 - al) * r).max(0.0);
        }
    }
    (s, u)
}

#[test]
fn a_mixed_strand_splits_into_hair_and_sky() {
    let (w, h) = (300, 40);
    let (img, alpha) = strands(w, h);
    let edge = estimate_edge(&img, &MaskAlpha::from_plane(alpha.clone()), w as f64);
    for x in (w * 45 / 100..w * 55 / 100).filter(|x| (x * 7) % 10 >= 3 && (x * 7) % 10 <= 7) {
        let (sp, up) = edge.at(x, h / 2).expect("an edge pixel");
        // (an approximation: at least 40 % of the sky's blue in the pixel leaves the strand's colour)
        let i = img.get(x, h / 2);
        assert!(i[2] - sp[2] >= 0.4 * (i[2] - 0.1) && sp[0] > i[0], "x {x}: the strand keeps the sky's blue: {sp:?} from {i:?}");
        assert!(up[2] > 0.55 && up[2] > 2.0 * up[0], "x {x}: the sky behind it stays blue: {up:?}");
        // they mix back to the pixel
        let a = alpha.data[h / 2 * w + x];
        for c in 0..3 {
            assert!((a * sp[c] + (1.0 - a) * up[c] - i[c]).abs() < 0.02, "x {x}");
        }
    }
    // pixels wholly in one layer are not kept
    assert_eq!(edge.at(2, 2), None);
    assert_eq!(edge.at(w - 2, 2), None);
}

/// Band by band, only the edge: the same colours as the whole frame's estimate, at every edge pixel, whatever the band
/// height (a band reads its windows' reach around it).
#[test]
fn the_edge_band_by_band_equals_the_whole_frame() {
    let (w, h) = (180, 150);
    // a soft disc with a ragged rim, on a textured photo
    let mut alpha = Plane::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let d = (((x as f32 - 90.0).powi(2) + (y as f32 - 70.0).powi(2)).sqrt() - 45.0 + ((x * 13 + y * 7) % 9) as f32) / 8.0;
            alpha.data[y * w + x] = (0.5 - d).clamp(0.0, 1.0);
        }
    }
    let img = Rgb32f::from_fn(w, h, |x, y| {
        let t = ((x * 31 + y * 17) % 23) as f32 / 23.0;
        [0.2 + 0.5 * t, 0.3 + 0.2 * (x as f32 / w as f32), 0.6 - 0.3 * (y as f32 / h as f32)]
    });
    let (s, u) = whole_frame(&img, &alpha, 400.0);
    let a = MaskAlpha::from_plane(alpha.clone());
    for band in [7, 40, BAND] {
        let edge = estimate_in_bands(&img, &a, 400.0, band);
        let mut n = 0;
        for y in 0..h {
            for x in 0..w {
                let v = alpha.data[y * w + x];
                match edge.at(x, y) {
                    Some((sp, up)) => {
                        n += 1;
                        for c in 0..3 {
                            assert!((sp[c] - s.get(x, y)[c]).abs() < 1e-4 && (up[c] - u.get(x, y)[c]).abs() < 1e-4, "band {band}, ({x}, {y})");
                        }
                    }
                    None => assert!(!is_edge(v), "band {band}: edge pixel ({x}, {y}) missing"),
                }
            }
        }
        assert!(n > 200, "band {band}: {n} edge pixels");
        assert_eq!(edge.len(), n);
    }
}

#[test]
fn hostile_input_keeps_nothing() {
    let img = Rgb32f::from_fn(4, 4, |_, _| [0.5; 3]);
    assert!(estimate_edge(&img, &MaskAlpha::from_plane(Plane::new(3, 3)), 4.0).is_empty());
    let mut nan = Plane::new(4, 4);
    nan.data.iter_mut().for_each(|v| *v = f32::NAN);
    assert!(estimate_edge(&img, &MaskAlpha::from_plane(nan), 4.0).is_empty());
    let mut half = Plane::new(4, 4);
    half.data.iter_mut().for_each(|v| *v = 0.5);
    let e = estimate_edge(&img, &MaskAlpha::from_plane(half), 0.0);
    assert_eq!(e.len(), 16);
    assert!(e.px.iter().all(|p| p.selected.iter().chain(&p.unselected).all(|v| v.is_finite())));
    let empty = Rgb32f::new(0, 0);
    assert!(estimate_edge(&empty, &MaskAlpha::zero(0, 0), 0.0).is_empty());
    assert_eq!(Edge::default().at(usize::MAX, usize::MAX), None);
}
