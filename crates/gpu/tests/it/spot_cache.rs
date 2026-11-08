//! Spots on the device (issue #82): hundreds of them render without exhausting the device, and with a view's stages a
//! spot edit draws only the spots it reaches, with the result of drawing them all.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, Spot, SpotMode};
use lightcraft_geom::Point;
use lightcraft_pipeline::spots_keep::spots_drawn;
use lightcraft_pipeline::{RenderRequest, SourceInfo, StageCache};
use lightcraft_raster::Rgb32f;

fn gpu() -> bool {
    let ok = lightcraft_gpu::available();
    if !ok {
        let why = lightcraft_gpu::unavailable_reason().unwrap_or_default();
        assert!(!why.contains("kernels failed to build"), "{why}");
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
    }
    ok
}

fn scene(w: usize, h: usize) -> Arc<Rgb32f> {
    Arc::new(lightcraft_scenes::demo_library()[0].render(w, h))
}

/// `n` small spots on a grid, every mode, apart from each other.
fn grid(n: usize, cols: usize) -> Vec<Spot> {
    (0..n)
        .map(|i| {
            let rows = n.div_ceil(cols).max(1);
            let (x, y) = (0.05 + 0.9 * (i % cols) as f64 / cols as f64, 0.05 + 0.9 * (i / cols) as f64 / rows as f64);
            Spot {
                mode: [SpotMode::Heal, SpotMode::Clone, SpotMode::Remove][i % 3],
                points: vec![Point::new(x, y)],
                size: 0.004,
                feather: 50.0,
                opacity: 100.0,
                source_offset: Some(Point::new(0.0, 0.01)),
                ..Default::default()
            }
        })
        .collect()
}

/// 300 spots took about 4,200 compute passes in one submission, past the 4,096 Metal command buffers a device holds
/// (wgpu opens two per pass): the device was lost and the app went down with it.
#[test]
fn hundreds_of_spots_render_on_the_device() {
    if !gpu() {
        return;
    }
    let src = scene(1200, 800);
    let s = DevelopSettings { spots: grid(300, 20), ..Default::default() };
    let out = lightcraft_gpu::render(&src, &SourceInfo { raw: true, ..Default::default() }, &s, &RenderRequest::fit(1200, 1200), None);
    assert!(out.is_some(), "rendered on the device: {:?}", lightcraft_gpu::unavailable_reason());
    assert!(lightcraft_gpu::available(), "the device is still there: {:?}", lightcraft_gpu::unavailable_reason());
}

#[test]
fn a_spot_edit_draws_only_the_spots_it_reaches_on_the_device() {
    if !gpu() {
        return;
    }
    let src = scene(960, 640);
    let (info, req) = (SourceInfo { raw: true, ..Default::default() }, RenderRequest::fit(720, 720));
    let cache = StageCache::default();
    let drawn = |s: &DevelopSettings| {
        let before = spots_drawn();
        let img = lightcraft_gpu::render(&src, &info, s, &req, Some(&cache)).expect("gpu").image;
        let fresh = lightcraft_gpu::render(&src, &info, s, &req, None).expect("gpu").image;
        (spots_drawn() - before - s.spots.len(), img == fresh)
    };
    let mut s = DevelopSettings { spots: grid(30, 6), ..Default::default() };
    assert_eq!(drawn(&s), (30, true));
    s.spots.push(Spot { mode: SpotMode::Remove, points: vec![Point::new(0.5, 0.97)], size: 0.004, source_offset: None, ..Default::default() });
    assert_eq!(drawn(&s), (1, true), "the 31st spot alone");
    s.spots[7].points[0].x += 0.01;
    assert_eq!(drawn(&s), (1, true), "a moved spot alone");
    s.spots[7].opacity = 40.0;
    assert_eq!(drawn(&s), (1, true), "a spot's opacity");
    s.spots.remove(3);
    assert_eq!(drawn(&s), (0, true), "a deleted spot's box comes back");
    s.light.exposure = 0.5;
    assert_eq!(drawn(&s), (0, true), "a slider outside the spots draws none");
    // a spot over an earlier one: changing it never draws the earlier one; changing the earlier one draws both
    let mut over = s.spots[10].clone();
    over.points[0].x += 0.003;
    over.mode = SpotMode::Clone;
    s.spots.push(over);
    assert_eq!(drawn(&s), (1, true));
    let last = s.spots.len() - 1;
    s.spots[last].feather = 0.0;
    assert_eq!(drawn(&s), (1, true), "the spot under it stays");
    s.spots[10].opacity = 50.0;
    assert_eq!(drawn(&s), (2, true), "it and the spot drawn over it");
    // and any sequence of edits gives what drawing every spot gives
    for step in 0..12usize {
        let i = (step * 7) % s.spots.len();
        match step % 4 {
            0 => s.spots[i].points[0].y += 0.004,
            1 => s.spots[i].size = 0.003 + 0.001 * (step % 3) as f64,
            2 => s.spots[i].source_offset = Some(Point::new(0.006, 0.0)),
            _ => {
                s.spots.remove(i);
            }
        }
        assert!(drawn(&s).1, "step {step}");
    }
}
