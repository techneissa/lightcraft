//! Core ML against candle (issue #105, `fork/coreml.md`). The tests read the models where the app keeps them, the
//! folder shared with PhotoCraft (`~/Library/Application Support/ArtCraft/models`: each parser's `model.safetensors`
//! and `coreml/<folder>.mlpackage` from `tools/convert-coreml-parsing.py`), only through links in a temporary folder
//! (Core ML compiles there), and skip without them. Nothing is downloaded and nothing is written in the shared folder.

use std::path::{Path, PathBuf};

use crate::coreml::{CONVERTED, paths};
use crate::image::Crop;
use crate::models::{ModelFile, ModelSet};
use crate::{Backend, Error, Parser, RgbImage, Runtimes};
use lightcraft_coreml::Runtime;

/// The shared models folder, when it holds the SegFormer set and both Core ML packages.
fn shared() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let dir = home.join("Library/Application Support/ArtCraft/models");
    let ok = ModelSet::Segformer.installed_in(&dir) && CONVERTED.iter().all(|f| paths(&dir, f).0.is_dir());
    ok.then_some(dir)
}

fn link(from: &Path, to: &Path) {
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(from, to).unwrap();
    #[cfg(not(unix))]
    let _ = (from, to);
}

/// A models folder in a temporary folder holding links to the shared set's weights, and to the Core ML packages of
/// `coreml`.
fn linked(name: &str, models: &Path, coreml: &[ModelFile]) -> PathBuf {
    let dir = super::scratch(name);
    for f in ModelSet::Segformer.files() {
        link(&f.path_in(models), &f.path_in(&dir));
    }
    for f in coreml {
        link(&paths(models, f).0, &paths(&dir, f).0);
    }
    dir
}

/// A 600 × 400 picture: a sky, a field and a figure (a skin-coloured disc over a dark block), made here.
fn fixture() -> RgbImage {
    let (w, h) = (600usize, 400usize);
    let mut d = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let head = (fx - 0.5).powi(2) + ((fy - 0.3) * 1.2).powi(2) < 0.006;
            let body = (fx - 0.5).abs() < 0.12 && fy > 0.42;
            let px = if head {
                [205.0, 160.0, 135.0]
            } else if body {
                [40.0 + 30.0 * fy, 50.0, 90.0]
            } else if fy < 0.5 {
                [120.0 + 60.0 * fy, 170.0 + 40.0 * fy, 235.0]
            } else {
                [80.0 + 50.0 * fx, 140.0 - 30.0 * fy, 60.0]
            };
            d.extend(px.map(|v: f32| v.clamp(0.0, 255.0) as u8));
        }
    }
    RgbImage::new(w, h, d).unwrap()
}

/// The share of pixels where two sets of planes (class-major, `n` each) pick the same plane.
fn argmax_agreement(a: &[f32], b: &[f32], n: usize) -> f64 {
    let pick = |v: &[f32], i: usize| (0..v.len() / n).max_by(|&p, &q| v[p * n + i].total_cmp(&v[q * n + i])).unwrap_or(0);
    (0..n).filter(|&i| pick(a, i) == pick(b, i)).count() as f64 / n as f64
}

/// The share of pixels where a plane is on (over 0.5) in both or neither.
fn mask_agreement(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).filter(|(x, y)| (**x > 0.5) == (**y > 0.5)).count() as f64 / a.len().max(1) as f64
}

#[test]
fn core_ml_parses_as_candle_does() {
    let Some(models) = shared() else {
        lightcraft_testkit::skip!(Model, "the SegFormer set and its Core ML packages are not in the shared models folder");
        return;
    };
    if !lightcraft_coreml::available() {
        lightcraft_testkit::skip!(Model, "Core ML runs only on a Mac");
        return;
    }
    let dir = linked("coreml-vs-candle", &models, &CONVERTED);
    // candle on the GPU, as the app runs it: candle 0.9.2's CPU convolution gives other numbers for the face parser's
    // 4 × 4 stride-4 reduction (its Metal one matches PyTorch)
    let dev = crate::best_device();
    let coreml = Parser::load_with(ModelSet::Segformer, &dir, dev.clone(), Runtimes { face: Runtime::CoreMl, body: Runtime::CoreMl }).unwrap();
    let candle = Parser::load_with(ModelSet::Segformer, &dir, dev, Runtimes { face: Runtime::Candle, body: Runtime::Candle }).unwrap();
    assert_eq!(coreml.backends(), (Backend::CoreMl, Backend::CoreMl));
    assert_eq!(candle.backends(), (Backend::Candle, Backend::Candle));
    let img = fixture();

    // the clothes parser, on a grid smaller than its canvas (768 × 512 in 768 × 512; 800 × 544 in 1024 × 704)
    for side in [768, 800] {
        let (a, b) = (coreml.body_map(&img, side).unwrap(), candle.body_map(&img, side).unwrap());
        assert_eq!((a.w, a.h, a.probs.len()), (b.w, b.h, b.probs.len()));
        let agree = argmax_agreement(&a.probs, &b.probs, a.w * a.h);
        let diff = a.probs.iter().zip(&b.probs).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        assert!(agree >= 0.99, "body at {side}: {agree}");
        assert!(diff < 0.25, "body at {side}: largest difference {diff}");
    }

    // the face parser, a batch of three crops (one turned)
    let crops: Vec<Crop> = [(300.0, 120.0, 0.0), (310.0, 130.0, 0.3), (280.0, 200.0, -0.2)]
        .iter()
        .map(|&(cx, cy, angle)| Crop { cx, cy, scale: 0.4, angle, side: crate::face::SIDE })
        .collect();
    let a = coreml.face_groups(&img, &crops).unwrap();
    // candle one crop at a time: on Metal, candle 0.9.2 gives the crops after the first in a batch other numbers
    // (`fork/coreml.md`)
    let b: Vec<_> = crops.iter().flat_map(|c| candle.face_groups(&img, std::slice::from_ref(c)).unwrap()).collect();
    assert_eq!(a.len(), 3);
    for (fa, fb) in a.iter().zip(&b) {
        assert_eq!(fa.len(), crate::face::GROUP_COUNT);
        for (k, (ga, gb)) in fa.iter().zip(fb).enumerate() {
            assert_eq!(ga.len(), gb.len());
            let agree = mask_agreement(ga, gb);
            let mean = ga.iter().zip(gb).map(|(x, y)| (x - y).abs()).sum::<f32>() / ga.len().max(1) as f32;
            assert!(agree >= 0.99 && mean < 0.005, "face group {k}: {agree} agree, mean difference {mean}");
        }
    }
}

#[test]
fn a_missing_or_damaged_core_ml_model_falls_back_to_candle() {
    let Some(models) = shared() else {
        lightcraft_testkit::skip!(Model, "the SegFormer set and its Core ML packages are not in the shared models folder");
        return;
    };
    let dev = candle_core::Device::Cpu;
    // no Core ML package: Automatic is candle, Core ML is an error that says so
    let dir = linked("coreml-missing", &models, &[]);
    let p = Parser::load_with(ModelSet::Segformer, &dir, dev.clone(), Runtimes::default()).unwrap();
    assert_eq!(p.backends(), (Backend::Candle, Backend::Candle));
    let e = Parser::load_with(ModelSet::Segformer, &dir, dev.clone(), Runtimes { face: Runtime::CoreMl, body: Runtime::Candle }).err().unwrap();
    assert!(matches!(&e, Error::Model(m) if m.contains("Core ML")), "{e}");
    // a damaged compiled model with no package: Automatic is candle
    let dir = linked("coreml-damaged", &models, &[]);
    for f in CONVERTED {
        let c = paths(&dir, &f).1;
        std::fs::create_dir_all(c.join("weights")).unwrap();
        std::fs::write(c.join("model.mil"), b"not a model").unwrap();
    }
    let p = Parser::load_with(ModelSet::Segformer, &dir, dev.clone(), Runtimes::default()).unwrap();
    assert_eq!(p.backends(), (Backend::Candle, Backend::Candle));
    // the BiSeNet set has no Core ML version: Core ML is an error, Automatic is candle
    if ModelSet::BiSeNet.installed_in(&models) {
        let dir = super::scratch("coreml-bisenet");
        for f in ModelSet::BiSeNet.files() {
            link(&f.path_in(&models), &f.path_in(&dir));
        }
        let e = Parser::load_with(ModelSet::BiSeNet, &dir, dev.clone(), Runtimes { face: Runtime::CoreMl, body: Runtime::Automatic }).err().unwrap();
        assert!(e.to_string().contains("no Core ML version"), "{e}");
        let p = Parser::load_with(ModelSet::BiSeNet, &dir, dev, Runtimes::default()).unwrap();
        assert_eq!(p.backends(), (Backend::Candle, Backend::Candle));
    }
}

/// The largest difference between two face parses (crop by crop, group by group).
fn groups_diff(a: &crate::face::Groups, b: &crate::face::Groups) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().flatten().zip(b.iter().flatten()).flat_map(|(x, y)| x.iter().zip(y).map(|(p, q)| (p - q).abs())).fold(0.0f32, f32::max)
}

/// Core ML failing on a photo's first faces hands them to candle, which parses them as the candle runtime does: one
/// face a call on the GPU. It got them eight at a time, and on Metal candle gives the faces after the first in a call
/// other numbers.
#[test]
fn core_ml_failing_on_the_first_faces_gives_the_masks_of_candle_one_face_at_a_time() {
    let Some(models) = shared() else {
        lightcraft_testkit::skip!(Model, "the SegFormer set and its Core ML packages are not in the shared models folder");
        return;
    };
    if !lightcraft_coreml::available() {
        lightcraft_testkit::skip!(Model, "Core ML runs only on a Mac");
        return;
    }
    let dir = linked("coreml-fails", &models, &CONVERTED);
    let dev = crate::best_device();
    let coreml = Parser::load_with(ModelSet::Segformer, &dir, dev.clone(), Runtimes::default()).unwrap();
    let candle = Parser::load_with(ModelSet::Segformer, &dir, dev, Runtimes { face: Runtime::Candle, body: Runtime::Candle }).unwrap();
    assert_eq!(coreml.backends().0, Backend::CoreMl);
    let img = fixture();
    let crops: Vec<Crop> = [(300.0, 120.0, 0.0), (310.0, 130.0, 0.3), (280.0, 200.0, -0.2)]
        .iter()
        .map(|&(cx, cy, angle)| Crop { cx, cy, scale: 0.4, angle, side: crate::face::SIDE })
        .collect();
    let alone: crate::face::Groups = crops.iter().flat_map(|c| candle.face_groups(&img, std::slice::from_ref(c)).unwrap()).collect();
    coreml.fail_next_face_call();
    let handed_over = coreml.face_groups(&img, &crops).unwrap();
    assert_eq!(coreml.backends().0, Backend::Candle);
    let d = groups_diff(&handed_over, &alone);
    assert!(d < 1e-5, "Core ML's faces handed to candle: largest difference {d} from candle one face at a time");
    // candle as chosen, with the faces of a step together
    let d = groups_diff(&candle.face_groups(&img, &crops).unwrap(), &alone);
    assert!(d < 1e-5, "candle's faces of one step: largest difference {d} from one face at a time");
}

/// candle's face parser on the CPU (the fallback off a Mac) gives what it gives on the GPU: its stage-2 reduction,
/// a 4 × 4 stride-4 convolution of 64 channels at 64 × 64, was read channels-last by candle 0.9.2's CPU convolution.
#[test]
fn candle_parses_a_face_on_the_cpu_as_on_the_gpu() {
    let Some(models) = shared() else {
        lightcraft_testkit::skip!(Model, "the SegFormer set and its Core ML packages are not in the shared models folder");
        return;
    };
    let gpu = crate::best_device();
    if !gpu.is_metal() {
        lightcraft_testkit::skip!(Gpu, "no Metal device to compare with");
        return;
    }
    let dir = linked("candle-cpu-gpu", &models, &[]);
    let on = |dev| Parser::load_with(ModelSet::Segformer, &dir, dev, Runtimes { face: Runtime::Candle, body: Runtime::Candle }).unwrap();
    let crop = [Crop { cx: 300.0, cy: 120.0, scale: 0.4, angle: 0.1, side: crate::face::SIDE }];
    let img = fixture();
    let (cpu, metal) = (on(candle_core::Device::Cpu).face_groups(&img, &crop).unwrap(), on(gpu).face_groups(&img, &crop).unwrap());
    let d = groups_diff(&cpu, &metal);
    assert!(d < 1e-3, "largest difference {d}");
}
