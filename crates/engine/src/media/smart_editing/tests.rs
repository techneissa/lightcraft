//! Use Smart Previews instead of Originals for editing: when a proxy is used, and that a render from it is the render
//! from the original (the requirement: no split second of another look).

use std::path::PathBuf;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{CameraColour, DevelopSettings, WbMode};
use serde_json::json;

use super::super::{LoadOptions, SourceLevel, SourceRef};
use crate::Session;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-smartedit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A scene with the things a photograph has: a graded sky, ground with texture, saturated patches, a bright window,
/// a dark corner, hard edges and fine noise (linear light, 0..1).
fn scene(x: usize, y: usize, w: usize, h: usize) -> [f32; 3] {
    let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
    let noise = |k: u32| {
        let n = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263) ^ k.wrapping_mul(2_246_822_519);
        let n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
        ((n ^ (n >> 16)) & 0xffff) as f32 / 65535.0 - 0.5
    };
    let mut c = if v < 0.55 {
        // the sky: blue overhead, warm at the horizon
        let t = v / 0.55;
        [0.10 + 0.55 * t, 0.22 + 0.35 * t, 0.55 - 0.20 * t]
    } else {
        // the ground: textured green-brown
        let t = (v - 0.55) / 0.45;
        let grain = 0.5 + 0.5 * ((u * 90.0).sin() * (v * 70.0).cos());
        [0.12 + 0.10 * grain, 0.16 + 0.12 * grain * (1.0 - 0.4 * t), 0.06 + 0.05 * grain]
    };
    // a chart of saturated patches
    if (0.06..0.30).contains(&u) && (0.60..0.80).contains(&v) {
        let i = ((u - 0.06) / 0.04) as usize + 6 * (((v - 0.60) / 0.10) as usize);
        let hue = i as f32 / 12.0 * std::f32::consts::TAU;
        c = [0.30 + 0.28 * hue.cos(), 0.30 + 0.28 * (hue + 2.1).cos(), 0.30 + 0.28 * (hue + 4.2).cos()];
    }
    // a bright window with a hard edge, and a dark corner
    if (0.62..0.80).contains(&u) && (0.12..0.34).contains(&v) {
        c = [1.6, 1.5, 1.2];
    }
    if u < 0.12 && v < 0.12 {
        c = [0.01, 0.012, 0.015];
    }
    let n = 0.025 * noise(1);
    c.map(|p| (p * (1.0 + n)).clamp(0.0, 4.0))
}

/// A Bayer DNG of that scene (RGGB), `w × h`.
fn dng(w: usize, h: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let c = scene(x, y, w, h);
            let v = match (y % 2, x % 2) {
                (0, 0) => c[0],
                (1, 1) => c[2],
                _ => c[1],
            };
            data.push((256.0 + v.min(1.0) * 15000.0) as u16);
        }
    }
    crate::tests::xmp::synthetic_dng_of(w, h, "RGGB", data, None, lightcraft_meta::Metadata::default())
}

/// A session with a library holding one DNG of `w × h`; the photo's id.
fn session_with_dng(tag: &str, w: usize, h: usize) -> (Session, PhotoId, PathBuf) {
    let src = temp(&format!("{tag}-src"));
    let lib = temp(&format!("{tag}-lib"));
    let file = src.join("a.dng");
    std::fs::write(&file, dng(w, h)).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [file.to_string_lossy()]})).unwrap();
    let id = PhotoId(r["imported"][0].as_u64().expect("the DNG imports"));
    (s, id, src)
}

/// CIE L*a*b* (D65) of an sRGB byte triple.
fn lab(px: [u8; 4]) -> [f64; 3] {
    let lin = |v: u8| f64::from(lightcraft_color::transfer::srgb_to_linear(f32::from(v) / 255.0));
    let xyz = lightcraft_color::SRGB.to_xyz().apply([lin(px[0]), lin(px[1]), lin(px[2])]);
    let white = lightcraft_color::D65.to_xyz();
    let f = |t: f64| if t > 0.008_856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let [fx, fy, fz] = [f(xyz[0] / white[0]), f(xyz[1] / white[1]), f(xyz[2] / white[2])];
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// (median, 95th percentile, maximum) CIEDE2000 between two renders of the same size.
fn delta_e(a: &lightcraft_raster::Rgba8, b: &lightcraft_raster::Rgba8) -> (f64, f64, f64) {
    assert_eq!((a.width, a.height), (b.width, b.height), "the renders have the same size");
    let mut d: Vec<f64> = a.data.iter().zip(&b.data).map(|(p, q)| crate::camera_preview::colour_transfer::de2000(lab(*p), lab(*q))).collect();
    d.sort_by(f64::total_cmp);
    (d[d.len() / 2], d[d.len() * 95 / 100], d[d.len() - 1])
}

/// Photo `id` rendered at `edge` with the setting `on`, plus whether its source came from the smart preview.
fn render(s: &mut Session, id: PhotoId, edge: usize, on: bool) -> (lightcraft_raster::Rgba8, bool) {
    s.media.set_smart_editing(on);
    s.media.clear_sources();
    let job = s.render_job(id, edge, edge, false, true).unwrap();
    assert_eq!(matches!(job.source, SourceRef::Preferred { .. }), on, "the job prefers the smart preview only with the setting on");
    let r = job.run();
    let from_proxy = r.loaded.as_ref().is_some_and(|l| l.edit_proxy);
    (r.rendered.unwrap().image, from_proxy)
}

fn edited() -> DevelopSettings {
    let mut d = DevelopSettings::default();
    d.light.exposure = 0.45;
    d.light.contrast = 18.0;
    d.light.highlights = -35.0;
    d.light.shadows = 30.0;
    d.color.saturation = 12.0;
    d.color.vibrance = 20.0;
    d.wb.mode = WbMode::Custom;
    d.wb.temp = 4800.0;
    d.wb.tint = 12.0;
    d
}

/// The requirement: zooming from the smart preview to the original must not change colour, tone or white balance.
/// Both renders are 1200 px (the original's from its own pixels, the proxy's from its preview-size copy).
#[test]
fn a_render_from_the_smart_preview_matches_the_original() {
    for (tag, w, h) in [("small", 1800, 1200), ("large", 3300, 2200)] {
        let (mut s, id, src) = session_with_dng(tag, w, h);
        s.execute("library.smartPreviews", &json!({})).unwrap();
        for (look, settings) in [("as imported", DevelopSettings::default()), ("edited", edited())] {
            s.set_develop(id, settings, look).unwrap();
            // the loupe's size, and a grid thumbnail's (the thumbnail level is cut from the proxy)
            for edge in [1200, 400] {
                let (original, from_proxy) = render(&mut s, id, edge, false);
                assert!(!from_proxy);
                let (proxy, from_proxy) = render(&mut s, id, edge, true);
                assert!(from_proxy, "{tag} {look}: rendered from the smart preview");
                let (median, p95, max) = delta_e(&original, &proxy);
                eprintln!("smart preview vs original, {tag} {w}x{h} {look} at {edge}: median dE00 {median:.3}, 95th {p95:.3}, max {max:.2}");
                assert!(median < 1.0, "{tag} {look} {edge}: median dE00 {median:.3}");
                assert!(p95 < 3.0, "{tag} {look} {edge}: 95th percentile dE00 {p95:.3}");
            }
        }
        let _ = std::fs::remove_dir_all(src);
    }
}

/// A real raw from the corpus (`raw/*.arw`), when there is one: the same comparison.
#[test]
fn a_render_from_the_smart_preview_matches_a_real_raw() {
    let root = std::env::var_os("LIGHTCRAFT_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"));
    let arw = std::fs::read_dir(root.join("raw"))
        .ok()
        .and_then(|d| d.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("arw"))));
    let Some(arw) = arw else {
        lightcraft_testkit::skip!(Corpus, "no ARW in {}", root.join("raw").display());
        return;
    };
    let lib = temp("arw-lib");
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [arw.to_string_lossy()]})).unwrap();
    let id = PhotoId(r["imported"][0].as_u64().expect("the ARW imports"));
    s.execute("library.smartPreviews", &json!({})).unwrap();
    s.set_develop(id, edited(), "edited").unwrap();
    let (original, _) = render(&mut s, id, 1200, false);
    let (proxy, from_proxy) = render(&mut s, id, 1200, true);
    assert!(from_proxy);
    let (median, p95, max) = delta_e(&original, &proxy);
    eprintln!("smart preview vs original, {}: median dE00 {median:.3}, 95th {p95:.3}, max {max:.2}", arw.display());
    assert!(median < 1.0 && p95 < 3.0, "median dE00 {median:.3}, 95th {p95:.3}");
}

#[test]
fn the_setting_is_off_by_default_and_reads_the_original() {
    let (mut s, id, src) = session_with_dng("off", 600, 400);
    s.execute("library.smartPreviews", &json!({})).unwrap();
    assert!(!s.media.smart_editing());
    let job = s.render_job(id, 300, 300, false, true).unwrap();
    assert!(matches!(job.source, SourceRef::File { .. }));
    let _ = std::fs::remove_dir_all(src);
}

#[test]
fn the_command_sets_the_setting_and_reports_the_library() {
    let (mut s, _, src) = session_with_dng("command", 600, 400);
    let r = s.execute("library.smartPreviewsForEditing", &json!({})).unwrap();
    assert_eq!((r["enabled"].as_bool(), r["photos"].as_u64(), r["smartPreviews"].as_u64()), (Some(false), Some(1), Some(0)), "{r}");
    let r = s.execute("library.smartPreviewsForEditing", &json!({"enabled": true})).unwrap();
    assert_eq!(r["enabled"], true);
    assert!(s.media.smart_editing());
    let r = s.execute("library.smartPreviewsForEditing", &json!({"enabled": false})).unwrap();
    assert_eq!(r["enabled"], false);
    let _ = std::fs::remove_dir_all(src);
}

#[test]
fn the_levels_and_jobs_that_need_pixels_read_the_original() {
    let (mut s, id, src) = session_with_dng("levels", 1800, 1200);
    s.execute("library.smartPreviews", &json!({})).unwrap();
    s.media.set_smart_editing(true);
    // fit, fill and thumbnails prefer the proxy; zoom to 100 % (the Full level) does not
    assert!(matches!(s.thumb_job(id, 256).unwrap().source, SourceRef::Preferred { .. }));
    assert!(matches!(s.loupe_job(id, 1500, 1000, true).unwrap().source, SourceRef::Preferred { .. }));
    let zoomed = s.loupe_job(id, 4000, 2600, true).unwrap();
    assert_eq!(zoomed.level, SourceLevel::Full);
    assert!(matches!(zoomed.source, SourceRef::File { .. }), "the Full level reads the original");
    // export (and Edit In, Open as Layers, contact sheet, print, slideshow export, which render through it)
    let export = s.export_job(id, 1500, 1000, lightcraft_pipeline::OutputSpace::Srgb, lightcraft_pipeline::OutputDepth::U8).unwrap();
    assert!(matches!(export.source, SourceRef::File { .. }), "an export reads the original");
    // a source decoded from the proxy is not an export's either
    let r = s.loupe_job(id, 1500, 1000, true).unwrap().run();
    s.accept(&r);
    assert!(s.media.editing_from_smart_preview(id, &s.catalog.photo(id).unwrap().clone()), "the Info panel says so");
    let export = s.export_job(id, 1500, 1000, lightcraft_pipeline::OutputSpace::Srgb, lightcraft_pipeline::OutputDepth::U8).unwrap();
    assert!(matches!(export.source, SourceRef::File { .. }), "a kept proxy source is not used by an export");
    // turning the setting off forgets what the proxy gave
    s.media.set_smart_editing(false);
    assert!(!s.media.editing_from_smart_preview(id, &s.catalog.photo(id).unwrap().clone()));
    assert!(matches!(s.loupe_job(id, 1500, 1000, true).unwrap().source, SourceRef::File { .. }));
    let _ = std::fs::remove_dir_all(src);
}

#[test]
fn a_missing_stale_or_other_coloured_smart_preview_counts_as_missing() {
    let (mut s, id, src) = session_with_dng("missing", 600, 400);
    s.media.set_smart_editing(true);
    // none built: the original (the worker finds the proxy missing)
    let r = s.render_job(id, 300, 300, false, true).unwrap().run();
    assert!(r.rendered.is_ok());
    assert!(!r.loaded.is_some_and(|l| l.from_proxy), "no smart preview: the original");
    s.execute("library.smartPreviews", &json!({})).unwrap();
    s.media.clear_sources();
    let r = s.render_job(id, 300, 300, false, true).unwrap().run();
    assert!(r.loaded.is_some_and(|l| l.edit_proxy), "built: the smart preview");
    // an older camera look: stale, so the original is read until it is rebuilt
    let dir = s.media.smart_dir.clone().unwrap();
    let file = crate::smart::sharded_file_path(&dir, s.catalog.photo(id).unwrap());
    let bytes = std::fs::read(&file).unwrap();
    let start = b"LCSP1\n".len();
    let nl = start + bytes[start..].iter().position(|b| *b == b'\n').unwrap();
    let mut head: serde_json::Value = serde_json::from_slice(&bytes[start..nl]).unwrap();
    head["enc"] = json!(3);
    let mut stale = bytes[..start].to_vec();
    stale.extend_from_slice(head.to_string().as_bytes());
    stale.extend_from_slice(&bytes[nl..]);
    std::fs::write(&file, stale).unwrap();
    s.media.clear_sources();
    let r = s.render_job(id, 300, 300, false, true).unwrap().run();
    assert!(!r.loaded.is_some_and(|l| l.from_proxy), "a stale smart preview is read as missing");
    // No Camera Matching can't come from a proxy made for Camera Matching
    let mut raw = (**s.catalog.photo(id).unwrap()).clone();
    raw.format = "ARW".into();
    let mut d = (*raw.develop).clone();
    d.profile.camera_colour = CameraColour::Colorimetric;
    raw.develop = std::sync::Arc::new(d);
    assert!(s.media.smart_proxy(&raw, SourceLevel::Preview, LoadOptions::of(&raw.develop)).is_none(), "Colorimetric reads the original");
    let _ = std::fs::remove_dir_all(src);
}
