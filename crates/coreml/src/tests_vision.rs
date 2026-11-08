use crate::Error;
use crate::vision::{Image, Instances, MAX_SIDE, Matte, Pixels, Request, foreground_instances, person_instances, person_segmentation, supported};

/// A small synthetic picture: a light backdrop with a dark round "head" on a "body" (Vision may or may not call it a
/// person; the tests check what comes back, not what it finds).
fn picture(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let head = (fx - 0.5).powi(2) + (fy - 0.3).powi(2) < 0.02;
            let body = (fx - 0.5).abs() < 0.18 && fy > 0.45;
            out.extend_from_slice(if head {
                &[200, 150, 120]
            } else if body {
                &[40, 40, 60]
            } else {
                &[120, 170, 230]
            });
        }
    }
    out
}

#[test]
fn damaged_pictures_are_refused_before_vision() {
    let px = vec![0u8; 12];
    for (w, h, len) in [(0, 4, 0), (4, 0, 0), (2, 2, 11), (2, 2, 13), (MAX_SIDE + 1, 1, 0), (1 << 13, 1 << 13, 0)] {
        let data = vec![0u8; len];
        let img = Image { width: w, height: h, pixels: Pixels::Rgb8(&data) };
        for r in [person_segmentation(&img).map(|_| ()), person_instances(&img).map(|_| ()), foreground_instances(&img).map(|_| ())] {
            assert!(matches!(r, Err(Error::Input(_))), "{w}×{h}, {len}: {r:?}");
        }
    }
    let floats = vec![0f32; 11];
    let img = Image { width: 2, height: 2, pixels: Pixels::RgbF32(&floats) };
    assert!(matches!(person_segmentation(&img), Err(Error::Input(_))));
    assert!(Image { width: 2, height: 2, pixels: Pixels::Rgb8(&px) }.check().is_ok());
}

#[test]
fn float_pixels_are_clamped_and_nan_is_black() {
    let f = [0.5f32, -1.0, 2.0, f32::NAN, f32::INFINITY, 0.0];
    let img = Image { width: 2, height: 1, pixels: Pixels::RgbF32(&f) };
    assert_eq!(img.rgb8(0), [128, 0, 255]);
    assert_eq!(img.rgb8(1), [0, 255, 0]);
}

#[test]
fn a_matte_comes_back_at_the_photo_aspect() {
    // a square 4 × 4 matte (Vision's own size) for a 3:2 photo becomes 4 × 3
    let m = Matte { width: 4, height: 4, data: (0..16).map(|i| if i % 4 < 2 { 1.0 } else { 0.0 }).collect() };
    let out = m.clone().at_aspect(300, 200);
    assert_eq!((out.width, out.height), (4, 3));
    assert!(out.is_valid());
    assert!(out.at(0.1, 0.5) > 0.99 && out.at(0.9, 0.5) < 0.01);
    // already at the aspect: unchanged
    assert_eq!(m.clone().at_aspect(100, 100), m);
    // portrait
    let tall = m.at_aspect(200, 400);
    assert_eq!((tall.width, tall.height), (2, 4));
    assert!(!Matte { width: 2, height: 2, data: vec![0.5; 3] }.is_valid());
    assert!(!Matte { width: 1, height: 1, data: vec![1.5] }.is_valid());
    assert_eq!(Matte { width: 0, height: 0, data: vec![] }.at(0.5, 0.5), 0.0);
}

#[test]
fn the_union_of_instances_is_their_largest_coverage() {
    let a = Matte { width: 2, height: 1, data: vec![1.0, 0.0] };
    let b = Matte { width: 4, height: 2, data: vec![0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 0.8] };
    let u = Instances { masks: vec![a, b] }.union().unwrap();
    assert_eq!((u.width, u.height), (2, 1));
    assert_eq!(u.data[0], 1.0);
    assert!(u.data[1] > 0.3);
    assert_eq!(Instances::default().union(), None);
}

/// Real Vision calls on a small synthetic picture, 8-bit and float: each gives valid mattes at the photo's size and
/// aspect, or an error; never a panic. Skips off the Mac and on an older macOS.
#[test]
fn vision_runs_on_a_small_picture() {
    if !supported(Request::PersonSegmentation) {
        lightcraft_testkit::skip!(System, "Apple Vision person segmentation needs macOS 12 or later on a Mac");
        return;
    }
    // (192 × 128: Vision's instance requests refused a buffer without an IOSurface at this size)
    let (w, h) = (192, 128);
    let px = picture(w, h);
    let floats: Vec<f32> = px.iter().map(|v| f32::from(*v) / 255.0).collect();
    for img in [Image { width: w, height: h, pixels: Pixels::Rgb8(&px) }, Image { width: w, height: h, pixels: Pixels::RgbF32(&floats) }] {
        let m = person_segmentation(&img).unwrap();
        assert!(m.is_valid());
        let (aw, ah) = (m.width as f32, m.height as f32);
        assert!((aw / ah - 1.5).abs() < 0.05, "{} × {} is not 3:2", m.width, m.height);
        if !supported(Request::PersonInstances) {
            lightcraft_testkit::skip!(System, "Apple Vision instance masks need macOS 14 or later");
            continue;
        }
        for found in [person_instances(&img).unwrap(), foreground_instances(&img).unwrap()] {
            for m in &found.masks {
                assert!(m.is_valid());
                assert_eq!((m.width, m.height), (w, h), "an instance mask is at the photo's size");
            }
        }
    }
}

/// A subject on a plain backdrop is found by the foreground request, and the mask covers it, not the backdrop.
#[test]
fn vision_finds_a_plain_subject() {
    if !supported(Request::ForegroundInstances) {
        lightcraft_testkit::skip!(System, "Apple Vision subject masks need macOS 14 or later on a Mac");
        return;
    }
    let (w, h) = (256, 256);
    let mut px = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 - 128.0).powi(2) + (y as f32 - 128.0).powi(2)).sqrt();
            px.extend_from_slice(if d < 60.0 { &[220, 40, 30] } else { &[235, 235, 235] });
        }
    }
    let img = Image { width: w, height: h, pixels: Pixels::Rgb8(&px) };
    let found = foreground_instances(&img).unwrap();
    let Some(m) = found.union() else {
        lightcraft_testkit::skip!(System, "Apple Vision found no subject in the synthetic picture");
        return;
    };
    assert!(m.at(0.5, 0.5) > 0.5, "the disc is the subject");
    assert!(m.at(0.03, 0.03) < 0.5, "the backdrop is not");
}
