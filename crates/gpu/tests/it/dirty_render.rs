//! An edit renders only the tiles it changed (issue #111): with a view's stages, a spot edit recomputes the tiles under
//! the spot and as far as each later stage spreads them, and the result has the bits of a whole render. Anything else
//! (a global slider) renders the whole frame.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape, Spot, SpotMode};
use lightcraft_geom::Point;
use lightcraft_gpu::tiles_rendered;
use lightcraft_pipeline::{RenderRequest, SourceInfo, StageCache};
use lightcraft_raster::{Rgb32f, Rgba8};

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

fn spot(mode: SpotMode, x: f64, y: f64) -> Spot {
    Spot {
        mode,
        points: vec![Point::new(x, y)],
        size: 0.012,
        feather: 50.0,
        opacity: 100.0,
        source_offset: Some(Point::new(0.03, 0.0)),
        ..Default::default()
    }
}

/// Every plane the per-pixel stage reads: the highlights / shadows base, clarity, texture, the sharpening band, both
/// noise reductions, and a gradient mask (which does not read the image).
fn planes(s: &mut DevelopSettings) {
    s.light.exposure = 0.2;
    s.light.highlights = -40.0;
    s.light.shadows = 30.0;
    s.effects.clarity = 20.0;
    s.effects.texture = 15.0;
    s.detail.nr_luminance = 30.0;
    s.detail.nr_color = 25.0;
    s.detail.sharpen_amount = 40.0;
    s.vignette.amount = -20.0;
    s.grain.amount = 15.0;
    s.masks.push(Mask {
        components: vec![MaskComponent {
            name: None,
            op: MaskOp::Add,
            shape: MaskShape::Linear { start: Point::new(0.0, 0.0), end: Point::new(0.0, 0.6) },
            invert: false,
        }],
        adjust: lightcraft_develop::LocalAdjustments { exposure: 0.4, ..Default::default() },
        ..Default::default()
    });
}

/// A view's renders of one photo: each edit's result, the tiles its per-pixel stage computed, and the same settings
/// rendered whole (no stages) to compare with.
struct View {
    src: Arc<Rgb32f>,
    info: SourceInfo,
    req: RenderRequest,
    cache: StageCache,
    tiles: usize,
}

impl View {
    fn new(w: usize, h: usize, fit: usize) -> View {
        let src = scene(w, h);
        View { src, info: SourceInfo { raw: true, ..Default::default() }, req: RenderRequest::fit(fit, fit), cache: StageCache::default(), tiles: 0 }
    }

    /// Render `s` with the view's stages: (tiles computed, the image), checked against a whole render.
    fn render(&mut self, s: &DevelopSettings, what: &str) -> (usize, Rgba8) {
        let before = tiles_rendered();
        let img = lightcraft_gpu::render(&self.src, &self.info, s, &self.req, Some(&self.cache)).expect("gpu").image;
        let tiles = tiles_rendered() - before;
        let whole = lightcraft_gpu::render(&self.src, &self.info, s, &self.req, None).expect("gpu").image;
        let differ = img.data.iter().zip(&whole.data).filter(|(a, b)| a != b).count();
        assert!(differ == 0, "{what}: {differ} pixels differ from a whole render");
        self.tiles = img.width.div_ceil(lightcraft_gpu::TILE) * img.height.div_ceil(lightcraft_gpu::TILE);
        (tiles, img)
    }
}

#[test]
fn a_spot_edit_renders_only_the_tiles_it_reaches_with_a_whole_render_s_pixels() {
    if !gpu() {
        return;
    }
    // (a loupe's size: the highlights / shadows base reaches about 250 px at it)
    let mut v = View::new(2800, 1867, 2560);
    let mut s = DevelopSettings::default();
    planes(&mut s);
    s.spots = vec![spot(SpotMode::Heal, 0.2, 0.3), spot(SpotMode::Clone, 0.7, 0.6), spot(SpotMode::Remove, 0.5, 0.8)];
    let (first, _) = v.render(&s, "first render");
    let all = v.tiles;
    assert_eq!(first, all, "the first render is whole");
    let small = |n: usize, what: &str| {
        assert!(n > 0 && n * 3 < all, "{what}: {n} of {all} tiles");
    };
    s.spots.push(spot(SpotMode::Heal, 0.4, 0.45));
    let (n, _) = v.render(&s, "add");
    small(n, "add");
    s.spots[3].points[0].x += 0.03;
    let (n, _) = v.render(&s, "move");
    small(n, "move");
    s.spots[3].size = 0.02;
    let (n, _) = v.render(&s, "resize");
    small(n, "resize");
    let shown = s.clone();
    s.spots[1].hidden = true;
    let (n, _) = v.render(&s, "hide");
    small(n, "hide");
    s.spots.remove(0);
    let (n, _) = v.render(&s, "delete");
    small(n, "delete");
    s = shown;
    let (n, _) = v.render(&s, "undo the delete and the hide");
    small(n, "undo");
    let (n, _) = v.render(&s, "the same settings again");
    assert_eq!(n, 0, "nothing changed, nothing rendered");
    s.light.exposure = 0.5;
    let (n, _) = v.render(&s, "a global slider");
    assert_eq!(n, all, "a global slider renders everything");
    s.spots.push(spot(SpotMode::Remove, 0.8, 0.2));
    let (n, _) = v.render(&s, "a spot after the slider");
    small(n, "a spot after the slider");
}

/// Dehaze reads the whole frame's airlight: a spot that moves it renders everything, one that doesn't renders its
/// tiles; both give a whole render's pixels.
#[test]
fn dehaze_follows_the_airlight() {
    if !gpu() {
        return;
    }
    let mut v = View::new(1200, 800, 1000);
    let mut s = DevelopSettings::default();
    s.effects.dehaze = 30.0;
    s.light.shadows = 20.0;
    s.spots = vec![spot(SpotMode::Heal, 0.3, 0.3)];
    v.render(&s, "first render");
    for (i, (x, y)) in [(0.6, 0.5), (0.1, 0.9), (0.85, 0.15)].into_iter().enumerate() {
        s.spots.push(spot(SpotMode::Clone, x, y));
        let (n, _) = v.render(&s, &format!("spot {i}"));
        assert!(n > 0 && n <= v.tiles, "spot {i}: {n} tiles");
    }
}

/// A zoomed window keeps its own tiles: a spot inside it renders only part of the window.
#[test]
fn a_window_renders_only_the_tiles_a_spot_reaches() {
    if !gpu() {
        return;
    }
    let mut v = View::new(2400, 1600, 2400);
    v.req.window = Some(lightcraft_pipeline::PixelWindow { x: 400, y: 300, w: 1600, h: 1000 });
    let mut s = DevelopSettings::default();
    planes(&mut s);
    s.spots = vec![spot(SpotMode::Heal, 0.45, 0.45)];
    let (first, _) = v.render(&s, "the window");
    s.spots.push(spot(SpotMode::Heal, 0.55, 0.5));
    let (n, _) = v.render(&s, "a spot in the window");
    assert!(n > 0 && n * 2 < first, "{n} of {first} tiles");
}

/// The incremental result stays within the GPU / CPU equivalence bounds of the CPU's whole render.
#[test]
fn the_incremental_result_matches_the_cpu() {
    if !gpu() {
        return;
    }
    let mut v = View::new(1200, 800, 900);
    let mut s = DevelopSettings::default();
    planes(&mut s);
    s.spots = vec![spot(SpotMode::Heal, 0.3, 0.4)];
    v.render(&s, "first");
    s.spots.push(spot(SpotMode::Heal, 0.6, 0.5));
    s.spots[0].points[0].y += 0.05;
    let (_, img) = v.render(&s, "two edits");
    let cpu = lightcraft_pipeline::render(&v.src, &v.info, &s, &v.req).image;
    let (mut sum, mut max) = (0u64, 0u8);
    for (p, q) in img.data.iter().zip(&cpu.data) {
        for c in 0..3 {
            let d = p[c].abs_diff(q[c]);
            sum += d as u64;
            max = max.max(d);
        }
    }
    let mean = sum as f64 / (img.data.len() * 3) as f64;
    assert!(mean < 0.5 && max <= 3, "mean {mean:.4} LSB, max {max} LSB");
}

/// A mask that reads the image (a luminance range) is kept out of stage 1: every edit renders whole, and right.
#[test]
fn a_mask_that_reads_the_image_renders_whole() {
    if !gpu() {
        return;
    }
    let mut v = View::new(900, 600, 800);
    let mut s = DevelopSettings::default();
    s.masks.push(Mask {
        components: vec![MaskComponent {
            name: None,
            op: MaskOp::Add,
            shape: MaskShape::LuminanceRange { lo: 0.2, hi: 0.8, lo_feather: 0.2, hi_feather: 0.2 },
            invert: false,
        }],
        adjust: lightcraft_develop::LocalAdjustments { exposure: 0.5, ..Default::default() },
        ..Default::default()
    });
    s.spots = vec![spot(SpotMode::Heal, 0.3, 0.4)];
    v.render(&s, "first");
    s.spots.push(spot(SpotMode::Heal, 0.6, 0.5));
    let (n, _) = v.render(&s, "a spot");
    assert_eq!(n, v.tiles);
}
