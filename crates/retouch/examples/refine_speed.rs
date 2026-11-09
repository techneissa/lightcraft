//! How long the Refine brush adds to a draft render (fork `fork/refine-brush.md`): Skin on one face of a 1936 × 1290
//! picture (2.5 MP, the loupe's draft), alone and with a Reduce and an Add stroke; the first run with the strokes
//! rasterises them, the next ones reuse the grids. A measurement, not a test: each number is the fastest of 20 runs.
//!
//! ```bash
//! cargo run -p lightcraft-retouch --release --example refine_speed
//! ```

use std::time::Instant;

use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::{Face, Input, NormRect, Part, PartMask, RefineMode, RefineStroke, Retouch, Stage, WholePhoto};

fn main() {
    let (w, h) = (1936, 1290);
    let img = Rgb32f::from_fn(w, h, |x, y| {
        let n = ((x.wrapping_mul(7919) ^ y.wrapping_mul(104_729)) % 97) as f32 / 97.0;
        [0.45 * (0.8 + 0.3 * n), 0.3 * (0.8 + 0.3 * n), 0.24 * (0.8 + 0.3 * n)]
    });
    let bounds = NormRect::new(0.35, 0.2, 0.65, 0.8);
    let skin = Plane::from_fn(128, 128, |x, y| if (x as f32 - 64.0).hypot(y as f32 - 64.0) < 56.0 { 1.0 } else { 0.0 });
    let face = Face { masks: vec![(Part::FacialSkin, PartMask { plane: skin, area: bounds })], ..Face::boxed(bounds) };
    let input = Input::new(vec![face]);
    let view = WholePhoto { width: w, height: h };
    let mut plain = Retouch::default();
    plain.global.set("retouch.skin.smoothness", 60.0);
    let mut refined = plain.clone();
    let line = |mode, x: f64| RefineStroke { mode, points: (0..60).map(|i| (x, 0.3 + i as f64 * 0.006)).collect(), size: 0.02, ..RefineStroke::default() };
    refined.refine.push("skin", line(RefineMode::Reduce, 0.45));
    refined.refine.push("skin", line(RefineMode::Add, 0.66));
    let time = |r: &Retouch| {
        let mut out = img.clone();
        let t = Instant::now();
        lightcraft_retouch::run::apply(&mut out, r, &input, &view, Stage::Retouch);
        t.elapsed().as_secs_f64() * 1000.0
    };
    let mut reduce = plain.clone();
    reduce.refine.push("skin", line(RefineMode::Reduce, 0.45));
    let mut add = plain.clone();
    add.refine.push("skin", line(RefineMode::Add, 0.66));
    let first = time(&refined);
    for (name, r) in [("reduce", &reduce), ("add", &add)] {
        time(r);
        let mut v: Vec<f64> = (0..20).map(|_| time(r)).collect();
        v.sort_by(f64::total_cmp);
        println!("{name} alone: {:.2} ms", v[0]);
    }
    let mut with = Vec::new();
    let mut without = Vec::new();
    for _ in 0..20 {
        with.push(time(&refined));
        without.push(time(&plain));
    }
    let fastest = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[0]
    };
    println!("skin alone: {:.2} ms; with two strokes: first {first:.2} ms (rasterising), then {:.2} ms", fastest(&mut without), fastest(&mut with));
}
