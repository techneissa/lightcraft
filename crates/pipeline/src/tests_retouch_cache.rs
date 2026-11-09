//! Fork (`fork/perf-portrait-draft.md`): a Portrait slider tick starts from the image before the retouch step, and the
//! step changes pixels only inside the boxes it declares ([`retouch_step::write_boxes`]).

use std::sync::Arc;

use lightcraft_develop::retouch::input::Strands;
use lightcraft_develop::retouch::reshape::sketch_mesh;
use lightcraft_develop::retouch::{FEATURES, Face, Input, NormRect, Part, PartMask, Scope};
use lightcraft_develop::{DevelopSettings, Spot, SpotMode, controls};
use lightcraft_geom::Point;
use lightcraft_raster::{Plane, Rgb32f};

use crate::{PixelWindow, Quality, RenderRequest, SourceInfo, StageCache, render, render_cached, retouch_step};

/// One slider of each Portrait feature a drag moves.
const SLIDERS: [&str; 5] =
    ["retouch.skin.smoothness", "retouch.eyes.whites", "retouch.skinTone.evenTone", "retouch.teethLips.teethWhitening", "retouch.blemish.amount"];

/// The pixels of one face in `bounds` (skin with texture and a few blemishes, eyes, lips, teeth) and its part masks,
/// as functions of the normalised point.
struct Painted {
    bounds: NormRect,
}

impl Painted {
    fn at(&self, fx: f64, fy: f64) -> (f64, f64) {
        (self.bounds.x0 + fx * self.bounds.width(), self.bounds.y0 + fy * self.bounds.height())
    }

    fn ell(&self, x: f64, y: f64, c: (f64, f64), rx: f64, ry: f64) -> f32 {
        let d = (((x - c.0) / (rx * self.bounds.width())).powi(2) + ((y - c.1) / (ry * self.bounds.height())).powi(2)).sqrt();
        ((1.1 - d) / 0.2).clamp(0.0, 1.0) as f32
    }

    fn skin(&self, x: f64, y: f64) -> f32 {
        self.ell(x, y, self.at(0.5, 0.5), 0.48, 0.5)
    }

    fn whites(&self, x: f64, y: f64) -> f32 {
        [self.at(0.33, 0.4), self.at(0.67, 0.4)].iter().map(|c| self.ell(x, y, *c, 0.09, 0.04)).fold(0.0, f32::max)
    }

    fn iris(&self, x: f64, y: f64) -> f32 {
        [self.at(0.33, 0.4), self.at(0.67, 0.4)].iter().map(|c| self.ell(x, y, *c, 0.035, 0.035)).fold(0.0, f32::max)
    }

    fn lips(&self, x: f64, y: f64) -> f32 {
        self.ell(x, y, self.at(0.5, 0.75), 0.16, 0.06)
    }

    fn teeth(&self, x: f64, y: f64) -> f32 {
        self.ell(x, y, self.at(0.5, 0.75), 0.1, 0.025)
    }

    fn dot(&self, x: f64, y: f64) -> f32 {
        (0..8).map(|i| self.ell(x, y, self.at(0.28 + 0.06 * (i % 4) as f64, 0.55 + 0.08 * (i / 4) as f64), 0.02, 0.015)).fold(0.0, f32::max)
    }

    /// The face's colour over `c` at `(x, y)`.
    fn paint(&self, c: [f32; 3], x: f64, y: f64, tex: f32) -> [f32; 3] {
        let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t) };
        let dot = self.dot(x, y);
        let face = [0.5 * tex * (1.0 - 0.3 * dot), 0.36 * tex * (1.0 - 0.45 * dot), 0.28 * tex * (1.0 - 0.4 * dot)];
        let mut c = mix(c, face, self.skin(x, y));
        c = mix(c, [0.62, 0.5, 0.42], self.whites(x, y));
        c = mix(c, [0.1, 0.07, 0.05], self.iris(x, y));
        c = mix(c, [0.42, 0.12, 0.13], self.lips(x, y));
        mix(c, [0.6, 0.55, 0.36], self.teeth(x, y))
    }

    /// The face with its part masks and landmarks (YuNet's five, then a stand-in face mesh for Face Reshape).
    fn face(&self) -> Face {
        let b = self.bounds;
        let mask = |f: &dyn Fn(f64, f64) -> f32| {
            let n = 96;
            let plane =
                Plane::from_fn(n, n, |x, y| f(b.x0 + (x as f64 + 0.5) / n as f64 * b.width(), b.y0 + (y as f64 + 0.5) / n as f64 * b.height()));
            PartMask { plane, area: b }
        };
        Face {
            landmarks: sketch_mesh(b),
            masks: vec![
                (Part::FacialSkin, mask(&|x, y| self.skin(x, y) * (1.0 - self.whites(x, y).max(self.lips(x, y))))),
                (Part::EyeWhites, mask(&|x, y| self.whites(x, y) * (1.0 - self.iris(x, y)))),
                (Part::Iris, mask(&|x, y| self.iris(x, y))),
                (Part::Lips, mask(&|x, y| self.lips(x, y) * (1.0 - self.teeth(x, y)))),
                (Part::Teeth, mask(&|x, y| self.teeth(x, y))),
            ],
            ..Face::boxed(b)
        }
    }
}

/// A dark horizontal stray strand across `area`'s upper half, and its [`Strands`] (one tile of cells, as the search
/// stores them) for a `picture`-sized search.
fn strand(area: NormRect, picture: (usize, usize)) -> (impl Fn(f64, f64) -> bool, Strands) {
    const N: usize = 32;
    const ROW: usize = 8;
    let mut cells = vec![[0u8; 3]; Strands::TILE * Strands::TILE];
    for x in 0..Strands::TILE {
        // (weight 1, across the strand: down, darker than its sides)
        cells[ROW * Strands::TILE + x] = [255, 128, 1];
    }
    let mut tiles = vec![u32::MAX; (N / Strands::TILE) * (N / Strands::TILE)];
    tiles[0] = 0;
    let strands = Strands { area, width: N, height: N, picture, side: 3.0, step: 1.0, tiles, cells };
    let (y0, y1) = (area.y0 + ROW as f64 / N as f64 * area.height(), area.y0 + (ROW + 1) as f64 / N as f64 * area.height());
    let x1 = area.x0 + Strands::TILE as f64 / N as f64 * area.width();
    (move |x: f64, y: f64| x >= area.x0 && x < x1 && y >= y0 && y < y1, strands)
}

/// Faces in `faces` on a `w × h` photo (on the photo before its lens corrections when `on_photo`), their spots found
/// as the engine finds them; with a stray strand in `strand_area` given to the first face.
fn photo(w: usize, h: usize, faces: &[NormRect], on_photo: bool, strand_area: Option<NormRect>) -> (Arc<Rgb32f>, SourceInfo) {
    let painted: Vec<Painted> = faces.iter().map(|b| Painted { bounds: *b }).collect();
    let hair = strand_area.map(|a| strand(a, (w, h)));
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
        let tex = 1.0 + 0.05 * (((x * 7 + y * 13) % 5) as f32 - 2.0) / 2.0 + 0.08 * ((x as f32 / 3.1).sin() * (y as f32 / 2.7).sin());
        let back = [0.12 + 0.05 * nx as f32, 0.14, 0.16];
        let c = painted.iter().fold(back, |c, p| p.paint(c, nx, ny, tex));
        if hair.as_ref().is_some_and(|(on, _)| on(nx, ny)) { [0.03, 0.025, 0.02] } else { c }
    });
    let mut faces: Vec<Face> = painted.iter().map(Painted::face).collect();
    faces = lightcraft_develop::retouch::blemish::with_spots(&src, faces);
    if let (Some(f), Some((_, s))) = (faces.first_mut(), hair) {
        f.strands = Some(s);
    }
    let input = if on_photo { Input::new_on_photo(faces) } else { Input::new(faces) };
    (Arc::new(src), SourceInfo { retouch: Some(Arc::new(input)), ..SourceInfo::default() })
}

fn portrait(w: usize, h: usize, bounds: NormRect) -> (Arc<Rgb32f>, SourceInfo) {
    photo(w, h, &[bounds], false, None)
}

/// A raw's defaults (sharpening, colour noise reduction), tone, clarity and a Heal spot off the face.
fn edit() -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.detail.sharpen_amount = 40.0;
    s.detail.nr_color = 25.0;
    s.light.highlights = -30.0;
    s.effects.clarity = 15.0;
    s.spots = vec![Spot {
        mode: SpotMode::Heal,
        points: vec![Point::new(0.1, 0.8)],
        size: 0.02,
        source_offset: Some(Point::new(0.04, 0.0)),
        ..Default::default()
    }];
    s
}

/// A tick of each Portrait slider reuses the image before the retouch step (no white balance, spots or red eye again)
/// and the noise model, and renders what a render without the cache renders, byte for byte. A change before the step
/// (a spot) makes it again.
#[test]
fn a_portrait_slider_tick_starts_from_the_image_before_retouching() {
    let (src, info) = portrait(360, 240, NormRect::new(0.3, 0.1, 0.7, 0.9));
    let req = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(360, 240) };
    for id in SLIDERS {
        let cache = StageCache::default();
        let mut s = edit();
        controls::set(&mut s, id, 40.0);
        let runs = retouch_step::pre_retouch_runs();
        render_cached(&src, &info, &s, &req, &cache);
        assert_eq!(retouch_step::pre_retouch_runs(), runs + 1, "{id}: the first render makes the image before the step");
        let mut tick = s.clone();
        controls::set(&mut tick, id, 41.0);
        let got = render_cached(&src, &info, &tick, &req, &cache);
        assert_eq!(retouch_step::pre_retouch_runs(), runs + 1, "{id}: a tick starts from the kept image");
        assert_eq!(got.image, render(&src, &info, &tick, &req).image, "{id}: the same pixels as a render without the cache");
        // (the render without the cache made its own)
        let runs = retouch_step::pre_retouch_runs() - 1;
        let (a, b) = (crate::plan(&src, &info, &s, &req), crate::plan(&src, &info, &tick, &req));
        assert_eq!((a.pre_retouch_key, a.noise_key), (b.pre_retouch_key, b.noise_key), "{id}");
        assert_ne!((a.pre_nr_key, a.lin_key), (b.pre_nr_key, b.lin_key), "{id}");
        // a tick of another slider (exposure) keeps the linear image; a spot that moved makes the image again
        let mut exposure = tick.clone();
        exposure.light.exposure = 0.4;
        render_cached(&src, &info, &exposure, &req, &cache);
        assert_eq!(retouch_step::pre_retouch_runs(), runs + 1, "{id}: exposure");
        exposure.spots[0].points[0].x += 0.05;
        let got = render_cached(&src, &info, &exposure, &req, &cache);
        assert_eq!(retouch_step::pre_retouch_runs(), runs + 2, "{id}: a moved spot");
        assert_eq!(got.image, render(&src, &info, &exposure, &req).image, "{id}: after a spot moved");
    }
}

/// A frame of one noise tile measures its colour noise apart from the retouching while retouching is on, and on the
/// image itself while it is off: a slider taken to 0 and back with one cache renders what fresh renders do.
#[test]
fn a_one_tile_frame_measures_its_noise_as_a_fresh_render_does() {
    let (clean, info) = portrait(360, 240, NormRect::new(0.3, 0.1, 0.7, 0.9));
    // (colour noise, for the noise reduction to measure: seeded, the same every run)
    let mut noisy = (*clean).clone();
    for (i, p) in noisy.data.iter_mut().enumerate() {
        let n = |k: u32| ((i as u32).wrapping_mul(2_654_435_761).wrapping_add(k.wrapping_mul(40_503)) >> 16) as f32 / 65_535.0 - 0.5;
        p[0] = (p[0] * (1.0 + 0.3 * n(1))).max(0.0);
        p[2] = (p[2] * (1.0 + 0.3 * n(2))).max(0.0);
    }
    let src = Arc::new(noisy);
    let req = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(360, 240) };
    let cache = StageCache::default();
    let mut s = edit();
    // (a large clone of the face over the background: the image the noise is measured on differs from the tile)
    s.spots = vec![Spot {
        mode: SpotMode::Clone,
        points: vec![Point::new(0.15, 0.5)],
        size: 0.12,
        source_offset: Some(Point::new(0.35, 0.0)),
        ..Default::default()
    }];
    assert!(crate::local::joint_colour_nr(&s), "the joint colour noise reduction, which measures the noise");
    for v in [40.0, 0.0, 40.0, 0.0] {
        controls::set(&mut s, "retouch.skin.smoothness", v);
        let got = render_cached(&src, &info, &s, &req, &cache);
        assert_eq!(got.image, render(&src, &info, &s, &req).image, "Skin {v}");
    }
}

/// Portrait settings on a photo without faces change nothing: not the pixels, not the noise measurement.
#[test]
fn portrait_settings_without_faces_change_nothing() {
    let (src, _) = portrait(360, 240, NormRect::new(0.3, 0.1, 0.7, 0.9));
    let req = RenderRequest::fit(360, 240);
    let plain = edit();
    let mut s = plain.clone();
    controls::set(&mut s, "retouch.skin.smoothness", 60.0);
    for info in [SourceInfo::default(), SourceInfo { retouch: Some(Arc::new(Input::new(Vec::new()))), ..SourceInfo::default() }] {
        assert!(!retouch_step::touches(&s, &info));
        assert_eq!(crate::plan(&src, &info, &s, &req).noise_key, crate::plan(&src, &info, &plain, &req).noise_key);
        assert_eq!(render(&src, &info, &s, &req).image, render(&src, &info, &plain, &req).image);
    }
}

/// A render of another photo lets the kept image before the retouch step go: a tick back on the first photo makes it
/// again.
#[test]
fn another_photo_lets_the_kept_image_go() {
    let (a, info) = portrait(240, 160, NormRect::new(0.3, 0.1, 0.7, 0.9));
    let (b, _) = portrait(240, 160, NormRect::new(0.2, 0.1, 0.6, 0.9));
    let req = RenderRequest::fit(240, 160);
    let cache = StageCache::default();
    let mut s = edit();
    controls::set(&mut s, "retouch.skin.smoothness", 40.0);
    render_cached(&a, &info, &s, &req, &cache);
    controls::set(&mut s, "retouch.skin.smoothness", 41.0);
    let runs = retouch_step::pre_retouch_runs();
    render_cached(&a, &info, &s, &req, &cache);
    assert_eq!(retouch_step::pre_retouch_runs(), runs, "kept");
    render_cached(&b, &info, &s, &req, &cache);
    controls::set(&mut s, "retouch.skin.smoothness", 42.0);
    let runs = retouch_step::pre_retouch_runs();
    render_cached(&a, &info, &s, &req, &cache);
    assert_eq!(retouch_step::pre_retouch_runs(), runs + 1, "let go when the other photo rendered");
}

/// The pixels each slider of each face feature changes (at its ends, alone) for `s` and `req`, checked to lie in the
/// step's boxes; with how many changed.
fn writes_inside(src: &Rgb32f, info: &SourceInfo, base: &DevelopSettings, req: &RenderRequest, what: &str) -> usize {
    let mut changed = 0;
    let specs = FEATURES.iter().filter(|f| f.scope == Scope::Face).flat_map(|f| (f.specs)());
    for spec in specs.filter(|s| !s.id.starts_with("retouch.reshape.")) {
        for v in [spec.min, spec.max].into_iter().filter(|v| *v != spec.default) {
            let mut s = base.clone();
            controls::set(&mut s, spec.id, v);
            let p = crate::plan(src, info, &s, req);
            let mut img = p.frame.sample(src, p.w, p.h);
            crate::lin_cpu_pre_retouch(&mut img, info, &p);
            let before = img.clone();
            retouch_step::apply(&mut img, info, &p);
            let boxes = retouch_step::write_boxes(info, &p).unwrap_or_else(|| vec![[0, 0, p.w, p.h]]);
            for (i, (a, b)) in before.data.iter().zip(&img.data).enumerate() {
                if a != b {
                    changed += 1;
                    let (x, y) = (i % p.w, i / p.w);
                    let inside = boxes.iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3]);
                    assert!(inside, "{what}: {} = {v}: pixel ({x}, {y}) changed outside {boxes:?}", spec.id);
                }
            }
        }
    }
    changed
}

/// Every pixel the step changes lies in one of its boxes, for every slider of every face feature at its ends: one face,
/// two faces, a face cut by the frame's edge, faces under Face Reshape and under a lens distortion (on the photo), and
/// a stray strand away from its face (Stray Hair). A feature that may write outside its face makes the boxes `None`.
#[test]
fn the_step_writes_only_inside_its_boxes() {
    let whole = RenderRequest::fit(300, 200);
    let zoomed = RenderRequest { window: Some(PixelWindow { x: 120, y: 60, w: 40, h: 30 }), ..whole };
    let plain = DevelopSettings::default();
    let (src, info) = portrait(300, 200, NormRect::new(0.35, 0.15, 0.6, 0.75));
    assert!(writes_inside(&src, &info, &plain, &whole, "one face") > 1000);
    assert!(writes_inside(&src, &info, &plain, &zoomed, "one face, zoomed") > 100);
    let two = [NormRect::new(0.08, 0.15, 0.33, 0.7), NormRect::new(0.6, 0.2, 0.85, 0.75)];
    let (src2, info2) = photo(300, 200, &two, false, None);
    assert!(writes_inside(&src2, &info2, &plain, &whole, "two faces") > 1000);
    let (src3, info3) = photo(300, 200, &[NormRect::new(0.82, 0.1, 1.06, 0.7)], false, None);
    assert!(writes_inside(&src3, &info3, &plain, &whole, "a face cut by the edge") > 500);
    let (src4, info4) = photo(300, 200, &two, true, None);
    let mut reshaped = plain.clone();
    controls::set(&mut reshaped, "retouch.reshape.slimFace", 80.0);
    controls::set(&mut reshaped, "retouch.reshape.chin", 60.0);
    assert!(crate::plan(&src4, &info4, &reshaped, &whole).frame.warp.is_some(), "the reshape warps the frame");
    assert!(writes_inside(&src4, &info4, &reshaped, &whole, "Face Reshape") > 1000);
    let (src5, info5) = photo(300, 200, &[NormRect::new(0.7, 0.05, 0.97, 0.6), NormRect::new(0.1, 0.3, 0.35, 0.9)], true, None);
    let mut lens = plain.clone();
    lens.optics.distortion = 80.0;
    assert!(writes_inside(&src5, &info5, &lens, &whole, "lens distortion") > 1000);
    // a strand well above its face's box and reach: only the strands' area holds it
    let (src6, info6) = photo(300, 200, &[NormRect::new(0.4, 0.55, 0.6, 0.95)], false, Some(NormRect::new(0.4, 0.0, 0.6, 0.2)));
    let mut hair = plain.clone();
    controls::set(&mut hair, "retouch.hair.amount", 100.0);
    let p = crate::plan(&src6, &info6, &hair, &whole);
    let mut img = p.frame.sample(&src6, p.w, p.h);
    let before = img.clone();
    retouch_step::apply(&mut img, &info6, &p);
    let above = before.data.iter().zip(&img.data).enumerate().filter(|(i, (a, b))| a != b && i / p.w < 50).count();
    assert!(above > 10, "Stray Hair repaired the strand above the face: {above} pixels");
    writes_inside(&src6, &info6, &plain, &whole, "Stray Hair");
    // off: nowhere; a feature that writes outside faces: anywhere
    let off = crate::plan(&src, &info, &plain, &whole);
    assert_eq!(retouch_step::write_boxes(&info, &off), Some(Vec::new()), "off: nowhere");
    let outside = FEATURES.iter().filter(|f| f.scope == Scope::Picture).flat_map(|f| (f.specs)());
    for spec in outside {
        let mut s = plain.clone();
        controls::set(&mut s, spec.id, if spec.max != spec.default { spec.max } else { spec.min });
        assert!(s.retouch.global.writes_outside_faces(), "{}", spec.id);
        assert_eq!(retouch_step::write_boxes(&info, &crate::plan(&src, &info, &s, &whole)), None, "{}: anywhere", spec.id);
    }
}
