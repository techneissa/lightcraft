//! A raw is decoded once below its full size for its grid thumbnail and its loupe preview (issue #221): the grid's
//! decode makes the view preview the background build and the loupe would each have decoded the photo again for.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::Session;
use crate::media::QuickSource;

/// A library on disk with one 6000 × 4000 raw whose decodes are counted: each is the edge it was asked for.
fn raw_library(tag: &str) -> (Session, PathBuf, PhotoId, Arc<Mutex<Vec<usize>>>) {
    let root = std::env::temp_dir().join(format!("lc-decode-once-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    let id = s.catalog.alloc_photo_id();
    let path = root.join("DSC00001.ARW").to_string_lossy().to_string();
    let mut p = Photo::new(id, Source::File { path }, "DSC00001.ARW", "ARW", 6000, 4000, "2026-01-01T00:00:00");
    p.kind = MediaKind::Raw;
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    s.media.reduced_loader = None;
    let decodes = Arc::new(Mutex::new(Vec::new()));
    let seen = decodes.clone();
    s.media.file_loader = Some(Arc::new(move |_: &str, edge: usize, _, _| {
        seen.lock().unwrap().push(edge);
        let (w, h) = (edge.min(6000), (edge.min(6000) * 2 / 3).max(1));
        let image = lightcraft_raster::Rgb32f::from_fn(w, h, |x, y| [x as f32 / w as f32, y as f32 / h as f32, 0.3]);
        Ok((image, lightcraft_pipeline::SourceInfo { raw: true, ..Default::default() }))
    }));
    // (the view is rendered only once the GPU, or its absence, is known: a render never waits for the device here)
    let _ = lightcraft_gpu::available();
    (s, root, id, decodes)
}

/// #221: opening an album of raws decoded each visible one for its grid thumbnail, and preparing them for culling
/// decoded them again for their loupe previews. The grid's decode now makes the view preview too: the preview build
/// skips the photo, and the loupe shows that preview, with no second decode.
#[test]
fn a_raws_grid_thumbnail_decode_also_makes_its_loupe_preview() {
    let (mut s, root, id, decodes) = raw_library("once");
    // the loupe's canvas, as the app sets it: the build's views are this big
    s.loupe_edge = 2400;
    let job = s.grid_thumb_job(id, 512).unwrap();
    assert!(job.view_after.is_some(), "a raw whose previews are built in the background gets its view made alongside");
    let along = job.view_after.as_deref().cloned().unwrap();
    let (view, loupe) = (along.job, along.loupe);
    let (r, after) = job.run_then();
    let thumbnail = r.rendered.as_ref().expect("the thumbnail renders");
    assert_eq!(thumbnail.image.width.max(thumbnail.image.height), 512, "a grid thumbnail's size");
    // the thumbnail is handed back first; its photo's view is what is left for after it
    assert!(!crate::cmd::previews::already_built(std::slice::from_ref(&view), loupe), "the view is not made before the thumbnail is back");
    after.expect("the view is left for after the thumbnail").run();
    assert!(crate::cmd::previews::already_built(std::slice::from_ref(&view), loupe), "the view is made from the same decode");
    s.accept(&r);
    assert_eq!(*decodes.lock().unwrap(), vec![2560], "one decode, at the preview level");
    let built = s.execute("library.buildPreviews", &json!({"ids": [id.0], "wait": true})).unwrap();
    assert_eq!((built["skipped"].as_u64(), built["failed"].as_u64()), (Some(1), Some(0)), "the build finds the view made: {built}");
    let loupe = s.loupe_job(id, 2400, 1600, true).unwrap().run();
    assert_eq!(loupe.quick, Some(QuickSource::Cached), "the loupe shows the view made with the thumbnail");
    assert_eq!(decodes.lock().unwrap().len(), 1, "the raw was decoded once");
    let _ = std::fs::remove_dir_all(&root);
}

/// The grid makes no view when the background build is off for the view, or when the view is already built (then
/// the thumbnail decodes at its own size, as before).
#[test]
fn no_view_is_made_alongside_when_previews_are_off_or_built() {
    let (mut s, root, id, decodes) = raw_library("off");
    let mut off = s.prep_settings();
    off.library.previews = false;
    s.set_prep_settings(off, true).unwrap();
    assert!(s.grid_thumb_job(id, 512).unwrap().view_after.is_none(), "previews off: the thumbnail alone");
    let mut on = s.prep_settings();
    on.library.previews = true;
    s.set_prep_settings(on, true).unwrap();
    let built = s.execute("library.buildPreviews", &json!({"ids": [id.0], "wait": true})).unwrap();
    assert_eq!(built["done"].as_u64(), Some(1), "{built}");
    decodes.lock().unwrap().clear();
    s.media.forget(id);
    // a thumbnail size the build didn't make, so the thumbnail itself is rendered
    let r = s.grid_thumb_job(id, 256).unwrap().run();
    assert!(r.rendered.is_ok(), "{:?}", r.rendered.err());
    assert_eq!(*decodes.lock().unwrap(), vec![512], "the view is built: the thumbnail decodes at its own level");
    let _ = std::fs::remove_dir_all(&root);
}

/// Each channel of `img` (linear, scene-referred) as an 8-bit sRGB level, clamped: how a thumbnail difference shows.
fn levels(img: &lightcraft_raster::Rgb32f) -> Vec<u8> {
    let srgb = |v: f32| (255.0 * lightcraft_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0))).round() as u8;
    img.data.iter().flat_map(|p| p.map(srgb)).collect()
}

/// Mean absolute and per-channel signed (b − a) difference of two level images.
fn difference(a: &[u8], b: &[u8]) -> (f64, [f64; 3]) {
    let n = a.len().max(1) as f64;
    let mean = a.iter().zip(b).map(|(x, y)| f64::from(x.abs_diff(*y))).sum::<f64>() / n;
    let signed = std::array::from_fn(|c| a.iter().zip(b).skip(c).step_by(3).map(|(x, y)| f64::from(*y) - f64::from(*x)).sum::<f64>() / (n / 3.0));
    (mean, signed)
}

/// The thumbnail a grid cell now gets (#221) against the one it got before, on real raws: cut with Lanczos from the
/// preview level's decode (as a preview build has always cut its thumbnails, and the source cache one from a preview
/// source in memory), against the raw decoded at the thumbnail's own size (block means, or the Mac's decoder at 512
/// px). The tone and colour are the same: per channel the mean difference is under one 8-bit level, and with both
/// smoothed to a fifth of their size, under two. What differs is detail: the cut thumbnail keeps finer detail than
/// the block means (on four test raws a mean 0.8–4.4 levels at full thumbnail size, largest on a 33 MP ARW).
/// Corpus files (`cargo xtask corpus --download`), with macOS Colour where this Mac has it.
#[test]
fn corpus_a_thumbnail_cut_from_the_preview_decode_has_the_tone_and_colour_of_the_thumbnail_decode() {
    use lightcraft_develop::CameraColour;
    use lightcraft_raster::resample::{Filter, fit};
    let root = std::env::var_os("LIGHTCRAFT_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"));
    let names = ["arw-sony-a7m4-14bit.arw", "coverage-2789.nef", "cr2-canon-5d3.cr2", "cr3-canon-r8-raw.cr3"];
    let mut compared = 0;
    for name in names {
        let Ok(bytes) = std::fs::read(root.join("raw").join(name)) else { continue };
        let mut sources = vec![("Camera Matching", crate::media::LoadOptions::default())];
        if crate::files::macos_available() {
            let options = crate::media::LoadOptions { colour: CameraColour::Macos, profile_key: Some(u128::from(crate::files::macos_cache_key())) };
            sources.push(("macOS Colour", options));
        }
        for (what, options) in sources {
            let decode = |edge| crate::files::load_bytes_decoded(&bytes, edge, options, crate::media::RawDecode::default()).unwrap().0;
            let before = decode(512);
            let after = fit(&decode(2560), 512, 512, Filter::Lanczos3);
            assert_eq!((after.width, after.height), (before.width, before.height), "{name} {what}: the same size");
            let (mean, signed) = difference(&levels(&before), &levels(&after));
            let small = |img: &lightcraft_raster::Rgb32f| levels(&fit(img, 103, 103, Filter::Box));
            let (smoothed, _) = difference(&small(&before), &small(&after));
            eprintln!("{name} {what}: mean {mean:.2} levels, per channel {signed:.2?}, smoothed {smoothed:.2}");
            assert!(signed.iter().all(|d| d.abs() < 1.0), "{name} {what}: the tone or colour moved {signed:.2?}");
            assert!(smoothed < 2.0, "{name} {what}: smoothed, {smoothed:.2} levels apart");
            compared += 1;
        }
    }
    if compared == 0 {
        lightcraft_testkit::skip!(Input, "no corpus raws in {}", root.display());
    }
}
