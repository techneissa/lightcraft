//! FLUX.1 Fill with the real weights: runs only when `LIGHTCRAFT_FLUX_DIR` names a folder that holds them (the
//! weights are 17 GB and the run loads T5-XXL; nothing is ever downloaded here).

use lightcraft_flux::{FillModel, Request, Weights, best_device, text};

#[test]
fn repaints_only_the_hole_and_keeps_the_rest() {
    let Some(dir) = std::env::var_os("LIGHTCRAFT_FLUX_DIR").filter(|d| !d.is_empty()) else {
        lightcraft_testkit::skip!(Model, "LIGHTCRAFT_FLUX_DIR is not set (FLUX.1 Fill weights)");
        return;
    };
    let w = match Weights::in_dir(std::path::Path::new(&dir)) {
        Ok(w) => w,
        Err(e) => {
            lightcraft_testkit::skip!(Model, "FLUX.1 Fill weights: {e}");
            return;
        }
    };
    let dev = best_device();
    let prompt = text::encode(&w, "A grey wall.", &dev, &|| false).unwrap();
    assert!(prompt.t5.iter().chain(&prompt.clip).all(|v| v.is_finite()));
    let model = FillModel::load(&w, &dev).unwrap();
    let side = 256;
    // a smooth gradient with a square hole in the middle
    let rgb: Vec<f32> = (0..side * side)
        .flat_map(|i| {
            let x = (i % side) as f32 / side as f32;
            [0.3 + 0.2 * x, 0.4, 0.5 - 0.2 * x]
        })
        .collect();
    let mask: Vec<f32> =
        (0..side * side).map(|i| if (96..160).contains(&(i % side)) && (96..160).contains(&(i / side)) { 1.0 } else { 0.0 }).collect();
    let mut calls = Vec::new();
    let req = Request { rgb: &rgb, mask: &mask, side, steps: w.default_steps(), guidance: 30.0, seed: 1 };
    let out = model
        .fill(&req, &prompt, &mut |d, n| {
            calls.push((d, n));
            true
        })
        .unwrap();
    let n = w.default_steps();
    assert_eq!(calls, (0..=n).map(|d| (d, n)).collect::<Vec<_>>());
    assert_eq!(out.len(), rgb.len());
    assert!(out.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
    // away from the hole the picture comes back near the input (FLUX redraws a flat synthetic picture loosely at
    // 256 px: about 0.12 on average; a broken pipeline gives noise or the grey of the hole, far beyond this)
    let far: f32 = (0..side * 16).flat_map(|i| (0..3).map(move |c| i * 3 + c)).map(|j| (out[j] - rgb[j]).abs()).sum::<f32>() / (side * 16 * 3) as f32;
    assert!(far < 0.2, "mean difference away from the hole: {far}");
    // a stop asked for before the first step is a cancel, not a result
    let stopped = model.fill(&req, &prompt, &mut |_, _| false);
    assert!(matches!(stopped, Err(lightcraft_flux::Error::Cancelled)));
}
