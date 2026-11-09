use super::*;
use crate::display::{Display, finish_render};
use crate::media::fit_for_display;
use lightcraft_codecs::icc;
use lightcraft_codecs::space::{NamedSpace, Trc};

/// A `w × h` picture with detail everywhere, and the same picture with `rects` painted over.
fn pictures(w: usize, h: usize, rects: &[Rect]) -> (Rgba8, Rgba8) {
    let px = |x: usize, y: usize, k: usize| ((x * 7 + y * 13 + k * 31 + (x * y) % 97) % 256) as u8;
    let a = Rgba8 { width: w, height: h, data: (0..w * h).map(|i| [px(i % w, i / w, 0), px(i % w, i / w, 1), px(i % w, i / w, 2), 255]).collect() };
    let mut b = a.clone();
    for r in rects {
        for y in r[1]..r[3] {
            for x in r[0]..r[2] {
                b.data[y * w + x] = [px(x, y, 5), 255 - px(x, y, 3), px(y, x, 1), 255];
            }
        }
    }
    (a, b)
}

#[test]
fn fitting_the_changed_rectangles_gives_the_whole_fit() {
    for ((w, h), fit, rects) in [
        ((2560, 1707), (2463, 1642), vec![[640, 320, 768, 448], [2496, 1664, 2560, 1707]]),
        ((801, 533), (640, 640), vec![[0, 0, 64, 64], [300, 200, 301, 201]]),
    ] {
        let (a, b) = pictures(w, h, &rects);
        let mut out = fit_for_display(&a, fit.0, fit.1);
        let made = fit_rects(&b, &mut out, &rects).expect("across first");
        let want = fit_for_display(&b, fit.0, fit.1);
        let differ = out.data.iter().zip(&want.data).filter(|(p, q)| p != q).count();
        assert_eq!(differ, 0, "{w}×{h}: {differ} pixels differ from the whole fit");
        assert_eq!(made, fitted_rects((w, h), (out.width, out.height), &rects));
        // outside what it made, the fit of the old picture already was the new one's
        let inside = |x: usize, y: usize| made.iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3]);
        let before = fit_for_display(&a, fit.0, fit.1);
        let outside = (0..want.data.len()).filter(|i| !inside(i % want.width, i / want.width) && want.data[*i] != before.data[*i]).count();
        assert_eq!(outside, 0, "{w}×{h}: the fit changed outside the rectangles it reports");
    }
}

#[test]
fn converting_the_changed_rectangles_gives_the_whole_conversion() {
    let displays = [icc::write_named(NamedSpace::DisplayP3), icc::write_matrix_trc(&lightcraft_color::ADOBE_RGB, &Trc::Gamma(2.2))];
    let rects = vec![[100, 60, 228, 188], [700, 0, 801, 64]];
    for bytes in displays {
        let d = Display::from_bytes("test.icc", &bytes).expect("profile");
        for full_srgb in [true, false] {
            let (a, b) = pictures(801, 533, &rects);
            let mut kept = a.clone();
            let (srgb, _) = finish_render(&mut kept, &d, full_srgb);
            let from = Kept {
                display: Some(d.id()),
                size: (kept.width, kept.height),
                srgb: srgb.map(Arc::new),
                converted: Some(Arc::new(kept)),
                fitted: false,
                fit: None,
                device_fit: false,
            };
            let patch = Patch { note: None, from: Some(Arc::new(from)), rects: Some(rects.clone()) };
            let mut got = b.clone();
            let (got_srgb, got_hist) = patch.convert(&mut got, &d, full_srgb).expect("patched");
            let mut want = b.clone();
            let (want_srgb, want_hist) = finish_render(&mut want, &d, full_srgb);
            assert!(got.data == want.data, "{}: converted pixels differ", d.profile.description);
            assert!(got_srgb.map(|s| s.data) == want_srgb.map(|s| s.data), "{}: sRGB twin differs", d.profile.description);
            assert!(got_hist == want_hist, "{}: histogram differs", d.profile.description);
        }
    }
}

/// A change over most of the picture is fitted whole (on every core, not rectangle by rectangle on one), with the same
/// bits and the same changed rectangles.
#[test]
fn a_change_over_most_of_the_picture_is_fitted_whole() {
    let (w, h, fit) = (801, 533, (640, 640));
    assert!(!fit_whole(&[[0, 0, 64, 64]], (w, h)));
    assert!(fit_whole(&[[0, 0, 801, 300]], (w, h)));
    for rects in [vec![[0, 0, 801, 300]], vec![[0, 0, 64, 64], [300, 200, 301, 201]]] {
        let (a, b) = pictures(w, h, &rects);
        let old = fit_for_display(&a, fit.0, fit.1);
        let from = Kept { display: None, size: (w, h), srgb: None, converted: None, fitted: true, fit: Some(Arc::new(old)), device_fit: false };
        let patch = Patch { note: None, from: Some(Arc::new(from)), rects: Some(rects.clone()) };
        let (got, changed) = patch.fit(&b, fit).expect("patched");
        let want = fit_for_display(&b, fit.0, fit.1);
        assert!(got.data == want.data, "{rects:?}: the fit differs from the whole fit");
        assert_eq!(changed, fitted_rects((w, h), (got.width, got.height), &rects));
    }
}

/// What a job keeps for the next one is counted with the view's stages, and holds a copy of the picture only where the
/// next job starts from it: nothing converted and a fit made on the device keep no image.
#[test]
fn a_job_keeps_only_the_images_the_next_one_starts_from() {
    let stages = lightcraft_pipeline::StageCache::default();
    let note = || Some(lightcraft_gpu::FrameNote { frame: 7, changed: None });
    let img = |w: usize, h: usize| Arc::new(Rgba8 { width: w, height: h, data: vec![[0, 0, 0, 255]; w * h] });
    let patch = Patch { note: note(), from: None, rects: None };
    assert!(patch.keeps(Some(&stages)) && !patch.keeps(None));
    let made = Made { srgb: Some(img(100, 50)), converted: Some(img(100, 50)), fit: Some(img(40, 20)) };
    patch.finish(Some(&stages), (Some(1), (100, 50), Some(false)), Some(made), None).expect("numbered");
    assert_eq!(lightcraft_gpu::stage_bytes(&stages), (2 * 100 * 50 + 40 * 20) * 4, "converted, sRGB and the host fit");
    let patch = Patch { note: note(), from: None, rects: None };
    let made = Made { srgb: None, converted: Some(img(100, 50)), fit: Some(img(40, 20)) };
    patch.finish(Some(&stages), (None, (100, 50), Some(true)), Some(made), None).expect("numbered");
    assert_eq!(lightcraft_gpu::stage_bytes(&stages), 0, "nothing converted, fitted on the device: no image kept");
    assert!(kept_of(Some(&stages), 7).is_some(), "what the next job checks against is kept");
    assert!(!Patch { note: None, from: None, rects: None }.keeps(Some(&stages)), "a render that is not numbered keeps nothing");
}

/// Loupe jobs with the view's stages, a display profile and a fit to the drawn size: after a spot edit the job converts
/// and fits only what the render changed, its picture has the bits of a job that kept nothing, and it says which
/// rectangles changed (the picture equals the last one outside them).
#[test]
fn a_loupe_job_after_a_spot_edit_gives_a_whole_job_s_picture_and_says_what_changed() {
    if !lightcraft_gpu::available() {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    let mut s = crate::Session::with_demo();
    let id = s.active().expect("a photo");
    let d = Arc::new(Display::from_bytes("p3.icc", &icc::write_named(NamedSpace::DisplayP3)).expect("profile"));
    let stages = Arc::new(lightcraft_pipeline::StageCache::default());
    let job = |s: &mut crate::Session, kept: bool| {
        let j = s.loupe_job(id, 1600, 1600, true).expect("job").with_display(Some(d.clone())).with_display_fit(1300, 1300);
        // (the job that keeps nothing renders: the view cache would hand it the kept job's picture of these settings)
        let j = if kept { j.with_stages(stages.clone()) } else { crate::media::RenderJob { view_cache: None, ..j } };
        let r = j.run();
        (r.rendered.expect("rendered").image, r.changed)
    };
    s.execute("spot.add", &serde_json::json!({"mode": "heal", "points": [[0.3, 0.4]], "size": 0.02, "source": [0.05, 0.0]})).expect("spot");
    let (first, note) = job(&mut s, true);
    let first_frame = note.expect("a render with stages is numbered").frame;
    for (k, (x, y)) in [(0.6, 0.5), (0.2, 0.8), (0.75, 0.25)].into_iter().enumerate() {
        s.execute("spot.add", &serde_json::json!({"mode": "heal", "points": [[x, y]], "size": 0.02, "source": [0.05, 0.0]})).expect("spot");
        let (img, changed) = job(&mut s, true);
        let (whole, _) = job(&mut s, false);
        let differ = img.data.iter().zip(&whole.data).filter(|(p, q)| p != q).count();
        assert_eq!(differ, 0, "spot {k}: {differ} pixels differ from a job that kept nothing");
        let c = changed.expect("numbered");
        assert!(c.from.is_some() && !c.rects.is_empty(), "spot {k}: {c:?}");
        if k == 0 {
            assert_eq!(c.from, Some(first_frame));
            let inside = |x: usize, y: usize| c.rects.iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3]);
            let outside = (0..img.data.len()).filter(|i| !inside(i % img.width, i / img.width) && img.data[*i] != first.data[*i]).count();
            assert_eq!(outside, 0, "pixels outside the changed rectangles differ from the last picture");
            let area: usize = c.rects.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1])).sum();
            assert!(area * 2 < img.data.len(), "{area} of {} pixels reported", img.data.len());
        }
    }
}
