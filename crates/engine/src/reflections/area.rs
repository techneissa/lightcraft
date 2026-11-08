//! Fork feature (`fork/glare-highlights.md`, issue #73): a reflection model run on a chosen area of the photo.
//!
//! The model gets the area (a box, or a Masks-panel mask's bounding box) and a margin of context around it, at the
//! panel's Quality, instead of the whole photo shrunk to that size: a small reflection keeps its detail. Its result is
//! kept inside the area with a soft edge (a box fades in over its border, a mask uses its own alpha) and stored as one
//! tile ([`super::tiles`]), so nothing outside the area changes.
//!
//! Areas are fractions of the original photo before crop or rotation, so the result stays on the same content when the
//! crop changes. A mask is evaluated on the photo as shot (user orientation, before lens corrections), then turned
//! back to the source's orientation, and baked into the tile when the result is made.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, Reflection, ReflectionArea};
use lightcraft_preview::Hash128;
use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Plane, Rgb32f};

use super::tiles::{self, Tile};

/// Long edge at which a mask is evaluated to find its box and soft edge.
const MASK_EDGE: usize = 1024;
/// Context around the area handed to the model, as a share of the area's longer side.
pub const CONTEXT: f64 = 0.25;
/// Width of a box's soft edge, as a share of its shorter side.
pub const BOX_FEATHER: f64 = 0.08;
/// Shortest side a model is given, pixels.
const MIN_SIDE: usize = 64;

/// Where a run works: a box in fractions of the photo, and for a mask its alpha over the whole photo.
#[derive(Clone)]
pub struct Region {
    /// Left, top, right, bottom.
    pub rect: [f64; 4],
    /// The mask's alpha over the whole photo (any size); `None` for a box.
    pub alpha: Option<Arc<Plane>>,
}

impl Region {
    pub fn whole() -> Self {
        Region { rect: [0.0, 0.0, 1.0, 1.0], alpha: None }
    }

    /// How much of the point `(x, y)` (fractions of the photo) belongs to the region, before any soft box edge.
    pub fn alpha_at(&self, x: f64, y: f64) -> f32 {
        let [l, t, r, b] = self.rect;
        if x < l || x > r || y < t || y > b {
            return 0.0;
        }
        match &self.alpha {
            None => 1.0,
            Some(a) => {
                let (px, py) = (x * a.width as f64 - 0.5, y * a.height as f64 - 0.5);
                bilinear(a, px as f32, py as f32)
            }
        }
    }

    /// The region in a `w × h` picture's pixels, `[x0, y0, x1, y1)`; `None` when empty.
    pub fn pixel_box(&self, w: usize, h: usize) -> Option<[usize; 4]> {
        let [l, t, r, b] = self.rect;
        let px = |v: f64, n: usize| ((v * n as f64).round().max(0.0) as usize).min(n);
        let bx = [px(l, w), px(t, h), px(r, w), px(b, h)];
        (bx[2] > bx[0] && bx[3] > bx[1]).then_some(bx)
    }
}

fn bilinear(p: &Plane, x: f32, y: f32) -> f32 {
    if p.width == 0 || p.height == 0 || !x.is_finite() || !y.is_finite() {
        return 0.0;
    }
    let (x, y) = (x.clamp(0.0, (p.width - 1) as f32), y.clamp(0.0, (p.height - 1) as f32));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(p.width - 1), (y0 + 1).min(p.height - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let g = |x: usize, y: usize| p.data.get(y * p.width + x).copied().unwrap_or(0.0);
    let top = g(x0, y0) + (g(x1, y0) - g(x0, y0)) * fx;
    let bottom = g(x0, y1) + (g(x1, y1) - g(x0, y1)) * fx;
    top + (bottom - top) * fy
}

/// The region of `area` on the photo `src` (the source, any size). A mask that is gone, or selects nothing, is an
/// error the user can act on.
pub fn resolve(
    area: &ReflectionArea,
    settings: &DevelopSettings,
    src: &Rgb32f,
    mattes: Option<&lightcraft_pipeline::masks::Mattes>,
) -> Result<Region, String> {
    let Some(id) = area.mask else {
        if !area.is_valid() {
            return Err("Invalid reflection area".into());
        }
        let (l, t, r, b) = area.fractions();
        return Ok(Region { rect: [l, t, r, b], alpha: None });
    };
    let small = lightcraft_raster::resample::fit(src, MASK_EDGE, MASK_EDGE, Filter::Lanczos3);
    let alpha = mask_alpha(settings, id, &small, mattes).ok_or("The mask chosen as the area is gone. Choose another area.")?;
    let rect = bounds(&alpha, 0.01).ok_or("The mask chosen as the area selects nothing.")?;
    Ok(Region { rect, alpha: Some(Arc::new(alpha)) })
}

/// The alpha of the Masks-panel mask `id` over `small` (a reduced copy of the source, in the source's orientation).
pub fn mask_alpha(settings: &DevelopSettings, id: u32, small: &Rgb32f, mattes: Option<&lightcraft_pipeline::masks::Mattes>) -> Option<Plane> {
    let mask = settings.masks.iter().find(|m| m.id == id)?;
    let orient = settings.orientation;
    let oriented = small.oriented(orient);
    // the photo as shot: user orientation, no crop, flip or lens warp; mask coordinates are its fractions
    let mut frame = lightcraft_pipeline::geometry::Frame::new(small.width, small.height, settings, false);
    frame.flip_h = false;
    frame.flip_v = false;
    frame.warp = None;
    frame.masks_on_photo = true;
    let log_l = oriented.map(lightcraft_pipeline::local::log_lum);
    let alpha = lightcraft_pipeline::masks::evaluate_alpha(mask, &frame, oriented.width, oriented.height, &oriented, &log_l, 0.0, mattes, None);
    let plane = alpha.to_plane();
    Some(plane.oriented(orient.inverse()))
}

/// The box (fractions) where `alpha` is above `min`, or `None`.
pub fn bounds(alpha: &Plane, min: f32) -> Option<[f64; 4]> {
    let (w, h) = (alpha.width, alpha.height);
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if alpha.data.get(y * w + x).is_some_and(|v| *v > min) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
    }
    (x1 > x0 && y1 > y0).then(|| [x0 as f64 / w as f64, y0 as f64 / h as f64, x1 as f64 / w as f64, y1 as f64 / h as f64])
}

/// The crop a model sees for `region` on a `w × h` source: the region's box and [`CONTEXT`] around it, in pixels.
pub fn context_box(region: &Region, w: usize, h: usize) -> Option<[usize; 4]> {
    let [x0, y0, x1, y1] = region.pixel_box(w, h)?;
    let ctx = ((x1 - x0).max(y1 - y0) as f64 * CONTEXT).round() as usize;
    Some([x0.saturating_sub(ctx), y0.saturating_sub(ctx), (x1 + ctx).min(w), (y1 + ctx).min(h)])
}

/// How much of the crop's pixel `(x, y)` (of `cw × ch`, covering `outer` of a `w × h` source) the result keeps: the
/// region's alpha, faded in over a box's border.
pub fn keep(region: &Region, outer: [usize; 4], (w, h): (usize, usize), (cw, ch): (usize, usize), (x, y): (usize, usize)) -> f32 {
    let fx = (outer[0] as f64 + (x as f64 + 0.5) * (outer[2] - outer[0]) as f64 / cw.max(1) as f64) / w.max(1) as f64;
    let fy = (outer[1] as f64 + (y as f64 + 0.5) * (outer[3] - outer[1]) as f64 / ch.max(1) as f64) / h.max(1) as f64;
    let a = region.alpha_at(fx, fy);
    if a <= 0.0 || region.alpha.is_some() {
        return a;
    }
    let [l, t, r, b] = region.rect;
    // fractions of the photo to its long-edge units, so the border is as wide across as down
    let long = w.max(h).max(1) as f64;
    let (sx, sy) = (w as f64 / long, h as f64 / long);
    let edge = ((fx - l) * sx).min((r - fx) * sx).min((fy - t) * sy).min((b - fy) * sy);
    let feather = (((r - l) * sx).min((b - t) * sy) * BOX_FEATHER).max(1e-9);
    let k = (edge / feather).clamp(0.0, 1.0) as f32;
    a * k * k * (3.0 - 2.0 * k)
}

/// The size a crop of `cw × ch` is given to a model at `edge` (the panel's Quality): its long side at `edge`, never
/// more than twice its own size, and its short side at least [`MIN_SIDE`] where the crop allows.
pub fn model_size(cw: usize, ch: usize, edge: usize) -> (usize, usize) {
    let long = cw.max(ch).max(1) as f64;
    let mut s = (edge as f64 / long).min(2.0);
    let short = cw.min(ch).max(1) as f64;
    if short * s < MIN_SIDE as f64 {
        s = (MIN_SIDE as f64 / short).min(edge as f64 / long);
    }
    (((cw as f64 * s).round() as usize).max(1), ((ch as f64 * s).round() as usize).max(1))
}

/// A folder that replaces `<config>/reflection-cache` (tests: a run never writes into the real one).
static CACHE_ROOT: std::sync::RwLock<Option<std::path::PathBuf>> = std::sync::RwLock::new(None);

/// Save corrections into `dir` instead of `<config>/reflection-cache` (`None`: the usual folder again).
pub fn set_cache_root(dir: Option<std::path::PathBuf>) {
    *CACHE_ROOT.write().unwrap_or_else(std::sync::PoisonError::into_inner) = dir;
}

/// Where corrections are saved.
pub fn cache_root() -> Result<std::path::PathBuf, String> {
    if let Some(dir) = CACHE_ROOT.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone() {
        return Ok(dir);
    }
    Ok(crate::config::config_dir().ok_or("No local configuration directory")?.join("reflection-cache"))
}

/// Write `tiles` into a new file of the reflection cache and the settings that use it.
pub fn save(tiles: &[Tile], key: Option<Hash128>, base: Reflection) -> Result<Reflection, String> {
    save_in(&cache_root()?, tiles, key, base)
}

/// [`save`] into the folder `root`.
pub fn save_in(root: &std::path::Path, tiles: &[Tile], key: Option<Hash128>, base: Reflection) -> Result<Reflection, String> {
    let bytes = tiles::encode(tiles)?;
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    let path = root.join(format!("{}-{nonce}.lcr", std::process::id()));
    let written = (|| {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create_new(true).write(true).open(&path).map_err(|e| e.to_string())?;
        f.write_all(&bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok(Reflection { enabled: true, correction: path.to_string_lossy().into(), source_key: super::identity(key), ..base })
}

/// The linear Rec. 2020 picture as sRGB bytes for a model.
pub fn to_srgb8(img: &Rgb32f) -> Vec<u8> {
    let matrix = lightcraft_color::REC2020.to_space(&lightcraft_color::SRGB);
    img.data.iter().flat_map(|p| matrix.apply_f32(*p).map(lightcraft_color::transfer::encode_srgb8)).collect()
}

/// Run reflection `model` on `region` of `src` (the full source) at `edge`, and save the result. A run whose `cancel`
/// is set returns "Stopped" and saves nothing.
pub fn make(
    src: &Rgb32f,
    key: Option<Hash128>,
    edge: u32,
    model: &str,
    region: &Region,
    area: ReflectionArea,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Reflection, String> {
    let stopped = || cancel.load(std::sync::atomic::Ordering::Relaxed);
    if stopped() {
        return Err("Stopped".into());
    }
    let tile = run_model(src, edge, model, region)?;
    if stopped() {
        return Err("Stopped".into());
    }
    let base = Reflection {
        amount: 100.0,
        quality: edge,
        model: model.into(),
        protect_sky: false,
        detail_protection: 0.0,
        area: Some(area),
        ..Default::default()
    };
    save(&[tile], key, base)
}

/// The tile [`make`] saves: the model's change over the area's crop, kept inside the area with its soft edge.
pub fn run_model(src: &Rgb32f, edge: u32, model: &str, region: &Region) -> Result<Tile, String> {
    if !matches!(edge, 384 | 768 | 1024) || !super::valid_model(model) {
        return Err("Invalid reflection model or quality".into());
    }
    let (w, h) = (src.width, src.height);
    let outer = context_box(region, w, h).ok_or("The area is empty")?;
    let crop = src.crop(outer[0], outer[1], outer[2] - outer[0], outer[3] - outer[1]);
    let (mw, mh) = model_size(crop.width, crop.height, edge as usize);
    if mw.min(mh) < 16 {
        return Err("The area is too thin. Draw a larger box.".into());
    }
    let small = resize(&crop, mw, mh, Filter::Lanczos3);
    let dir = super::models_dir().ok_or("No reflection model directory")?.join(model);
    let net = lightcraft_reflections::Runner::load(&dir, model, false)?;
    let bytes = to_srgb8(&small);
    let clean = net.infer(&bytes, mw, mh)?;
    if clean.len() != mw * mh {
        return Err("The reflection model returned the wrong size".into());
    }
    let to_working = lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020);
    let mut grid = Rgb32f::new(mw, mh);
    for (k, ((a, b), out)) in bytes.as_chunks::<3>().0.iter().zip(&clean).zip(grid.data.iter_mut()).enumerate() {
        let keep = keep(region, outer, (w, h), (mw, mh), (k % mw, k / mw));
        if keep <= 0.0 {
            continue;
        }
        let a = to_working.apply_f32(a.map(lightcraft_color::transfer::decode_srgb8));
        let b = to_working.apply_f32(b.map(lightcraft_color::transfer::srgb_to_linear));
        *out = [0, 1, 2].map(|c| (a[c] - b[c]) * keep);
    }
    let rect = [outer[0] as f32 / w as f32, outer[1] as f32 / h as f32, outer[2] as f32 / w as f32, outer[3] as f32 / h as f32];
    Ok(Tile { rect, grid })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_keeps_its_inside_fades_at_its_border_and_leaves_the_outside() {
        let region = Region { rect: [0.25, 0.25, 0.75, 0.75], alpha: None };
        let outer = context_box(&region, 400, 400).unwrap();
        assert_eq!(outer, [50, 50, 350, 350]);
        let (cw, ch) = (300, 300);
        let at = |x, y| keep(&region, outer, (400, 400), (cw, ch), (x, y));
        assert_eq!(at(150, 150), 1.0, "middle of the box");
        assert_eq!(at(10, 150), 0.0, "context outside the box");
        let border = at(52, 150);
        assert!(border > 0.0 && border < 1.0, "soft border {border}");
    }

    #[test]
    fn an_area_tile_changes_only_its_box_and_view_and_export_match() {
        let mut session = crate::Session::with_demo();
        let id = session.active().unwrap();
        let plain = session.render_job(id, 128, 128, false, false).unwrap().run().rendered.unwrap().image;
        let job = session.render_job(id, 128, 128, false, false).unwrap();
        let dir = std::env::temp_dir().join(format!("lc-area-tile-{}", std::process::id()));
        let tile = Tile { rect: [0.5, 0.5, 1.0, 1.0], grid: Rgb32f::filled(4, 4, [0.2, 0.2, 0.2]) };
        let r = save_in(&dir, &[tile], job.source_key, Reflection { amount: 100.0, ..Default::default() }).unwrap();
        let mut d = (*session.develop_of(id).unwrap()).clone();
        d.reflection = r;
        session.set_develop(id, d, "Area").unwrap();
        let view = session.render_job(id, 128, 128, false, false).unwrap().run().rendered.unwrap().image;
        let (w, h) = (view.width, view.height);
        assert_eq!(view.data[0], plain.data[0], "outside the box nothing changes");
        let inside = (h * 3 / 4) * w + w * 3 / 4;
        assert_ne!(view.data[inside], plain.data[inside]);
        let export = session
            .export_job(id, w, h, lightcraft_pipeline::OutputSpace::Srgb, lightcraft_pipeline::OutputDepth::U8)
            .unwrap()
            .run()
            .rendered
            .unwrap()
            .image;
        assert_eq!(view, export);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_mask_region_follows_its_alpha() {
        let alpha = Plane::from_fn(100, 100, |x, _| if x < 50 { 1.0 } else { 0.0 });
        let rect = bounds(&alpha, 0.01).unwrap();
        assert_eq!(rect, [0.0, 0.0, 0.5, 1.0]);
        let region = Region { rect, alpha: Some(Arc::new(alpha)) };
        assert_eq!(region.alpha_at(0.2, 0.5), 1.0);
        assert_eq!(region.alpha_at(0.7, 0.5), 0.0);
        assert!(bounds(&Plane::new(10, 10), 0.01).is_none());
    }

    #[test]
    fn a_small_area_is_given_to_the_model_larger_than_the_whole_photo_would_give_it() {
        // a 300 px box on a 6000 px photo: the whole photo at 768 would give it 38 px
        assert_eq!(model_size(300, 200, 768), (600, 400));
        assert_eq!(model_size(3000, 2000, 768), (768, 512));
        assert_eq!(model_size(400, 20, 768), (768, 38));
        let (w, h) = model_size(100, 10, 1024);
        assert!(h >= MIN_SIDE && w <= 1024);
    }

    #[test]
    fn a_mask_from_the_masks_panel_is_found_in_the_source_orientation() {
        use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape};
        use lightcraft_geom::{Orientation, Point};
        let mut s = DevelopSettings::default();
        let shape = MaskShape::Radial { center: Point::new(0.25, 0.5), rx: 0.1, ry: 0.1, angle: 0.0, feather: 0.0, invert: false };
        s.masks.push(Mask {
            id: 7,
            components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }],
            on_photo: true,
            ..Default::default()
        });
        let src = Rgb32f::filled(200, 100, [0.2; 3]);
        let a = mask_alpha(&s, 7, &src, None).unwrap();
        assert!(a.get(50, 50) > 0.5 && a.get(150, 50) == 0.0);
        // turned a quarter: the left of the photo as shown is the top of the source
        s.orientation = Orientation::Rotate90;
        let a = mask_alpha(&s, 7, &src, None).unwrap();
        assert_eq!((a.width, a.height), (200, 100));
        assert!(mask_alpha(&s, 8, &src, None).is_none());
        let none = ReflectionArea { mask: Some(8), ..Default::default() };
        assert!(resolve(&none, &s, &src, None).is_err());
    }
}
