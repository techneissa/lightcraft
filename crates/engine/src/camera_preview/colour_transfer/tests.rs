//! The colour transfer of AI Denoise on macOS Colour, on synthetic scenes (plan T21, T22, T25c).
use lightcraft_color::Mat3;

use super::*;

/// About the median matrix the 68-photo sample fitted (rows sum to 1), and its gain.
pub(crate) const M: Mat3 = Mat3([[1.12, 0.01, -0.13], [-0.07, 1.51, -0.44], [0.0, -0.30, 1.30]]);
pub(crate) const S: f64 = 1.27;

/// A small deterministic generator (no dependency for tests).
pub(crate) struct Lcg(pub(crate) u64);

impl Lcg {
    pub(crate) fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    /// About normal, mean 0, deviation 1.
    fn normal(&mut self) -> f64 {
        (0..12).map(|_| self.unit()).sum::<f64>() - 6.0
    }
}

/// A `w` × `h` camera picture (white-balanced, below clipping) of `tile`-pixel tiles of random colours, and the Mac's
/// picture of it through `M` and `S`.
fn scene(w: usize, h: usize, tile: usize, seed: u64) -> (Rgb32f, Rgb32f) {
    let mut rng = Lcg(seed);
    let (tw, th) = (w.div_ceil(tile), h.div_ceil(tile));
    let colours: Vec<[f64; 3]> = (0..tw * th)
        .map(|_| {
            let level = 0.05 + 0.5 * rng.unit();
            [0.3 + 0.7 * rng.unit(), 0.3 + 0.7 * rng.unit(), 0.3 + 0.7 * rng.unit()].map(|v| v * level)
        })
        .collect();
    let camera: Vec<[f32; 3]> = (0..w * h).map(|i| colours[(i / w / tile) * tw + (i % w) / tile].map(|v| v as f32)).collect();
    let apple = camera.iter().map(|c| M.apply(c.map(f64::from)).map(|v| (S * v) as f32)).collect();
    (Rgb32f { width: w, height: h, data: camera }, Rgb32f { width: w, height: h, data: apple })
}

fn fit_scene(camera: &Rgb32f, apple: &Rgb32f) -> Result<Transfer, Refused> {
    let (cols, rows) = grid(apple.width, apple.height);
    fit_transfer(&camera_blocks(camera, None, [1.0; 3], cols, rows), &apple_blocks(apple, cols, rows))
}

fn worst_entry(a: &Mat3, b: &Mat3) -> f64 {
    a.0.iter().flatten().zip(b.0.iter().flatten()).map(|(x, y)| (x - y).abs()).fold(0.0, f64::max)
}

/// T21: a known white-keeping matrix and gain are recovered, white stays white, and 10 % outlier blocks (a noisy or
/// misaligned pair) and clipped samples are survived.
#[test]
fn the_fit_recovers_a_known_matrix_and_keeps_white() {
    let (camera, apple) = scene(480, 320, 6, 1);
    let t = fit_scene(&camera, &apple).unwrap();
    assert!(worst_entry(&t.matrix, &M) < 1e-3, "{:?}", t.matrix);
    assert!((f64::from(t.gain) - S).abs() < 1e-3, "{}", t.gain);
    let white = t.matrix.apply([1.0; 3]);
    assert!(white.iter().all(|v| (v - 1.0).abs() < 1e-9), "white stays white: {white:?}");
    assert!(t.error < 0.01 && t.blocks >= MIN_BLOCKS, "{t:?}");

    // every tenth tile wrong on the Mac's side (×1.5 red, ×0.6 blue), and a few clipped camera samples
    let mut rng = Lcg(9);
    let (mut camera, mut apple) = (camera, apple);
    let bad: Vec<bool> = (0..80 * 54).map(|_| rng.unit() < 0.1).collect();
    for (i, p) in apple.data.iter_mut().enumerate() {
        if bad[(i / 480 / 6) * 80 + (i % 480) / 6] {
            *p = [p[0] * 1.5, p[1], p[2] * 0.6];
        }
    }
    for i in (0..camera.data.len()).step_by(997) {
        camera.data[i] = [2.0, 0.4, 0.4];
    }
    let t = fit_scene(&camera, &apple).unwrap();
    assert!(worst_entry(&t.matrix, &M) < 0.03, "{:?}", t.matrix);
    assert!((f64::from(t.gain) - S).abs() < 0.02, "{}", t.gain);
}

/// A block with one clipped camera sample (or one the binned clip mask marks) is left out; a dark block on the Mac's
/// side too.
#[test]
fn clipped_and_dark_blocks_are_left_out() {
    let img = Rgb32f { width: 4, height: 2, data: vec![[0.5; 3]; 8] };
    let mut clipped = img.clone();
    clipped.data[1] = [0.96, 0.5, 0.5];
    let b = camera_blocks(&clipped, None, [2.0, 1.0, 1.0], 2, 1);
    assert!(b.data[0][0].is_nan() && b.data[1] == [1.0, 0.5, 0.5]);
    let mask = [0, 0, 0, 0, 0, 0, 0, 1];
    assert!(camera_blocks(&img, Some(&mask), [1.0; 3], 2, 1).data[1][0].is_nan());
    let mut dark = img;
    dark.data[0] = [0.0; 3];
    dark.data[1] = [0.0; 3];
    dark.data[4] = [0.0; 3];
    dark.data[5] = [0.0; 3];
    let a = apple_blocks(&dark, 2, 1);
    assert!(a.data[0][0].is_nan() && a.data[1] == [0.5; 3]);
}

/// T22: too few blocks, one colour, a matrix or a gain out of range, and a fit that does not reproduce its own blocks
/// are each refused.
#[test]
fn the_gates_refuse_bad_fits() {
    let (camera, apple) = scene(480, 320, 6, 2);
    let (cols, rows) = grid(480, 320);
    let c = camera_blocks(&camera, None, [1.0; 3], cols, rows);
    let a = apple_blocks(&apple, cols, rows);
    // too few: all but 400 blocks unusable
    let mut few = c.clone();
    for p in few.data.iter_mut().skip(400) {
        *p = [f32::NAN; 3];
    }
    assert_eq!(fit_transfer(&few, &a), Err(Refused::Blocks(400)));
    assert_eq!(fit_transfer(&c, &apple_blocks(&apple, cols - 1, rows)), Err(Refused::Blocks(0)), "another grid");
    // one colour
    let grey = Rgb32f { data: vec![[0.2; 3]; c.data.len()], ..c.clone() };
    assert_eq!(fit_transfer(&grey, &grey), Err(Refused::Singular));
    // a matrix beyond the bounds: the Mac's picture far more saturated
    let wild = Mat3([[4.5, -2.0, -1.5], [-0.07, 1.51, -0.44], [0.0, -0.30, 1.30]]);
    let wild_apple = Rgb32f { data: c.data.iter().map(|p| wild.apply(p.map(f64::from)).map(|v| v.max(0.02) as f32)).collect(), ..c.clone() };
    assert_eq!(fit_transfer(&c, &wild_apple), Err(Refused::Matrix));
    // a gain out of range
    let bright = Rgb32f { data: a.data.iter().map(|p| p.map(|v| v * 8.0)).collect(), ..a.clone() };
    assert_eq!(fit_transfer(&c, &bright), Err(Refused::Gain));
    // pictures that do not match: the Mac's blocks shuffled
    let mut rng = Lcg(5);
    let mut shuffled = a.clone();
    for i in (1..shuffled.data.len()).rev() {
        let j = (rng.unit() * (i + 1) as f64) as usize;
        shuffled.data.swap(i, j.min(i));
    }
    assert!(matches!(fit_transfer(&c, &shuffled), Err(Refused::Error(e)) if e > MAX_ERROR));
}

/// T25c: at twice a high-ISO photo's noise, fitted on the coarse grid (48-pixel blocks), the colour error against
/// the known matrix stays at most 1 ΔE00.
#[test]
fn the_fit_survives_twice_the_noise() {
    let (w, h) = (2560 / 4, 1707 / 4);
    let (camera, apple) = scene(w, h, 24, 3);
    let mut rng = Lcg(3);
    // independent noise on each side, deviation 4 % of the signal plus a floor (shot and read noise)
    let mut noisy = |img: &Rgb32f| Rgb32f {
        data: img.data.iter().map(|p| p.map(|v| (f64::from(v) + (0.04 * f64::from(v) + 0.004) * rng.normal()).max(0.0) as f32)).collect(),
        ..img.clone()
    };
    let (nc, na) = (noisy(&camera), noisy(&apple));
    let t = fit_scene(&nc, &na).unwrap();
    // the error of the clean picture through the fit, against the clean Mac picture
    let k = 0.18 / 0.2;
    let errors: Vec<f64> = camera
        .data
        .iter()
        .zip(&apple.data)
        .step_by(97)
        .map(|(c, a)| {
            let p = t.matrix.apply(c.map(f64::from)).map(|v| v * f64::from(t.gain));
            de2000(display_lab(a.map(f64::from), k), display_lab(p, k))
        })
        .collect();
    let med = median(errors.clone());
    assert!(med <= 1.0, "median ΔE00 {med:.2}, {t:?}");
}

/// CIEDE2000 against the published test pairs (Sharma, Wu, Dalal 2005).
#[test]
fn de2000_matches_the_reference_pairs() {
    let cases = [
        ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
        ([50.0, -1.0, 2.0], [50.0, 0.0, 0.0], 2.3669),
        ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
        ([60.2574, -34.0099, 36.2677], [60.4626, -34.1751, 39.4387], 1.2644),
        ([2.0776, 0.0795, -1.1350], [0.9033, -0.0636, -0.5514], 0.9082),
    ];
    for (a, b, want) in cases {
        let got = de2000(a, b);
        assert!((got - want).abs() < 1e-3, "{a:?} {b:?}: {got} vs {want}");
    }
}
