use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::SourceInfo;
use lightcraft_raster::Rgb32f;
use lightcraft_scenes::{Kind, render_srgb_linear};

use super::reference::{self, Source};
use super::*;

/// A procedural photo in the working space (linear Rec.2020), as an imported JPEG's source.
fn scene(kind: Kind, seed: u32) -> Rgb32f {
    let m = lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020).to_f32();
    render_srgb_linear(kind, seed, 300, 200).map(|p| {
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2],
        ]
    })
}

fn fit(src: &Rgb32f, settings: &DevelopSettings, reference: &Rgba8) -> MatchResult {
    let info = SourceInfo::default();
    let never = AtomicBool::new(false);
    match_look(&Target { src, info: &info, settings }, reference, &[RegionPair::whole()], &MatchOptions::default(), &never, &|_, _| {}).unwrap()
}

fn render(src: &Rgb32f, d: &DevelopSettings) -> Rgba8 {
    render_reference(src, &SourceInfo::default(), d, 256)
}

fn value(r: &MatchResult, id: &str) -> f64 {
    if id == "wb.temp" {
        return r.settings.wb.temp;
    }
    controls::get(&r.settings, id).unwrap_or(f64::NAN)
}

fn edited(f: impl FnOnce(&mut DevelopSettings)) -> DevelopSettings {
    let mut d = DevelopSettings::default();
    f(&mut d);
    d
}

#[test]
fn matching_a_photo_to_itself_changes_nothing() {
    let src = scene(Kind::AlpineLake, 11);
    let d = edited(|d| {
        d.light.exposure = 0.3;
        d.light.contrast = 15.0;
        d.color.vibrance = 10.0;
    });
    let r = fit(&src, &d, &render(&src, &d));
    eprintln!("{:#?} loss {} → {}", r.changes, r.loss_before, r.loss_after);
    for c in &r.changes {
        let tol = match c.id.as_str() {
            "light.exposure" => 0.05,
            "wb.temp" => 100.0,
            _ => 3.0,
        };
        assert!((c.to - c.from).abs() <= tol, "{} moved {} → {}", c.id, c.from, c.to);
    }
    assert!(r.curve.is_none(), "no curve for the same look: {:?}", r.curve);
    assert!(r.loss_after <= r.loss_before);
}

#[test]
fn recovers_known_slider_changes() {
    let src = scene(Kind::Canyon, 17);
    let a = DevelopSettings::default();
    let b = edited(|d| {
        d.wb.mode = lightcraft_develop::WbMode::Custom;
        d.wb.temp = 7600.0;
        d.light.exposure = 0.5;
        d.color.saturation = 20.0;
    });
    let r = fit(&src, &a, &render(&src, &b));
    eprintln!("{:#?} loss {} → {}", r.changes, r.loss_before, r.loss_after);
    assert!((value(&r, "wb.temp") - 7600.0).abs() < 400.0, "temp {}", value(&r, "wb.temp"));
    assert!((value(&r, "light.exposure") - 0.5).abs() < 0.2, "exposure {}", value(&r, "light.exposure"));
    let colour = value(&r, "color.saturation") + 0.5 * value(&r, "color.vibrance");
    assert!((colour - 20.0).abs() < 10.0, "saturation {colour}");
    assert!(r.loss_after < r.loss_before * 0.1, "loss {} → {}", r.loss_before, r.loss_after);
}

#[test]
fn a_warmer_reference_raises_temp() {
    let target = scene(Kind::AlpineLake, 41);
    // another photo of the same kind of scene (a reference is usually picked for being alike)
    let other = scene(Kind::AlpineLake, 11);
    let neutral = fit(&target, &DevelopSettings::default(), &render(&other, &DevelopSettings::default()));
    let warm_ref = edited(|d| {
        d.wb.mode = lightcraft_develop::WbMode::Custom;
        d.wb.temp = 9000.0;
    });
    let warm = fit(&target, &DevelopSettings::default(), &render(&other, &warm_ref));
    assert!(value(&warm, "wb.temp") > value(&neutral, "wb.temp") + 300.0, "warm {} neutral {}", value(&warm, "wb.temp"), value(&neutral, "wb.temp"));
    assert!(value(&warm, "wb.temp") > 6500.0);
}

#[test]
fn a_darker_reference_lowers_exposure() {
    let target = scene(Kind::Beach, 19);
    let other = scene(Kind::Dunes, 7);
    let dark = fit(&target, &DevelopSettings::default(), &render(&other, &edited(|d| d.light.exposure = -1.2)));
    let plain = fit(&target, &DevelopSettings::default(), &render(&other, &DevelopSettings::default()));
    assert!(
        value(&dark, "light.exposure") < value(&plain, "light.exposure") - 0.3,
        "dark {} plain {}",
        value(&dark, "light.exposure"),
        value(&plain, "light.exposure")
    );
    assert!(value(&dark, "light.exposure") < 0.0);
}

#[test]
fn cancelling_stops_the_match() {
    let src = scene(Kind::Lavender, 9);
    let reference = render(&src, &edited(|d| d.light.exposure = 1.0));
    let cancel = AtomicBool::new(false);
    let info = SourceInfo::default();
    let d = DevelopSettings::default();
    let r = match_look(
        &Target { src: &src, info: &info, settings: &d },
        &reference,
        &[RegionPair::whole()],
        &MatchOptions::default(),
        &cancel,
        &|n, _| {
            if n >= 3 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    );
    assert_eq!(r.unwrap_err(), LookError::Cancelled);
}

#[test]
fn applying_writes_only_the_moved_sliders() {
    let src = scene(Kind::Canyon, 17);
    let r = fit(&src, &DevelopSettings::default(), &render(&src, &edited(|d| d.light.exposure = 0.8)));
    // the photo moved on meanwhile: its other sliders stay
    let mut now = edited(|d| d.effects.clarity = 40.0);
    r.apply_to(&mut now, &SourceInfo::default());
    assert_eq!(now.effects.clarity, 40.0);
    assert!((now.light.exposure - value(&r, "light.exposure")).abs() < 1e-9);
}

#[test]
fn empty_inputs_are_errors() {
    let src = scene(Kind::Canyon, 17);
    let info = SourceInfo::default();
    let d = DevelopSettings::default();
    let never = AtomicBool::new(false);
    let t = Target { src: &src, info: &info, settings: &d };
    let empty = Rgba8::new(0, 0);
    assert!(matches!(match_look(&t, &empty, &[RegionPair::whole()], &MatchOptions::default(), &never, &|_, _| {}), Err(LookError::Invalid(_))));
    let one = Rgba8::filled(4, 4, [128, 128, 128, 255]);
    assert!(matches!(match_look(&t, &one, &[], &MatchOptions::default(), &never, &|_, _| {}), Err(LookError::Invalid(_))));
    let blank = RegionPair { reference: Some(lightcraft_raster::Plane::filled(4, 4, 0.0)), ..RegionPair::whole() };
    assert!(matches!(match_look(&t, &one, &[blank], &MatchOptions::default(), &never, &|_, _| {}), Err(LookError::Invalid(_))));
    let nopixels = Rgb32f::new(0, 0);
    let t0 = Target { src: &nopixels, info: &info, settings: &d };
    assert!(matches!(match_look(&t0, &one, &[RegionPair::whole()], &MatchOptions::default(), &never, &|_, _| {}), Err(LookError::Invalid(_))));
}

#[test]
fn bad_web_addresses_are_errors() {
    let never = AtomicBool::new(false);
    for url in ["http://example.com/a.jpg", "ftp://example.com/a.jpg", "not an address", "https://", "https://user:pw@example.com/a.jpg", ""] {
        let r = reference::load(&Source::Url(url.into()), None, &never);
        assert!(matches!(r, Err(LookError::Download(_))), "{url}: {r:?}");
    }
}

#[test]
fn a_huge_download_is_refused() {
    // announced larger than the cap: refused before reading
    let mut reads = 0;
    let r = reference::read_capped(Some(reference::MAX_BYTES + 1), reference::MAX_BYTES, |_| {
        reads += 1;
        Ok(0)
    });
    assert!(matches!(r, Err(LookError::TooLarge(_))));
    assert_eq!(reads, 0);
    // no length announced, more sent than the cap: refused once past it
    let mut sent = 0u64;
    let r = reference::read_capped(None, 1 << 20, |buf| {
        sent += buf.len() as u64;
        buf.fill(0);
        Ok(buf.len())
    });
    assert!(matches!(r, Err(LookError::TooLarge(_))));
    assert!(sent <= (1 << 20) + 64 * 1024);
    // within the cap: all of it
    let mut left = 100_000usize;
    let r = reference::read_capped(Some(100_000), reference::MAX_BYTES, |buf| {
        let n = left.min(buf.len());
        left -= n;
        Ok(n)
    });
    assert_eq!(r.map(|b| b.len()), Ok(100_000));
}

#[test]
fn corrupt_or_missing_images_are_errors() {
    let never = AtomicBool::new(false);
    assert!(matches!(reference::load(&Source::Bytes(b"not an image at all".to_vec()), None, &never), Err(LookError::Decode(_))));
    assert!(matches!(reference::load(&Source::Bytes(Vec::new()), None, &never), Err(LookError::Decode(_))));
    // a real JPEG cut short, and one with its data scrambled
    let img = Rgba8::from_fn(64, 48, |x, y| [(x * 4) as u8, (y * 5) as u8, 90, 255]);
    let jpeg = lightcraft_codecs::encode_jpeg(&lightcraft_codecs::EncodeImage::rgba8(&img), 90, Default::default(), &Default::default()).unwrap();
    assert!(reference::decode(&jpeg).is_ok());
    // half a JPEG may still decode (the rest grey): never a panic; with no image data left, an error
    let _ = reference::load(&Source::Bytes(jpeg[..jpeg.len() / 2].to_vec()), None, &never);
    for cut in [40, 3] {
        assert!(reference::load(&Source::Bytes(jpeg[..cut].to_vec()), None, &never).is_err(), "cut at {cut}");
    }
    let mut scrambled = jpeg.clone();
    for (i, b) in scrambled.iter_mut().enumerate().skip(200) {
        *b = b.wrapping_mul(31).wrapping_add(i as u8);
    }
    let _ = reference::load(&Source::Bytes(scrambled), None, &never);
    assert!(matches!(reference::load(&Source::File("/no/such/reference.jpg".into()), None, &never), Err(LookError::Io(_))));
}

#[test]
fn a_downloaded_reference_is_read_from_the_cache() {
    let dir = std::env::temp_dir().join(format!("lightcraft-look-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let img = Rgba8::from_fn(32, 32, |x, _| [(x * 8) as u8, 120, 60, 255]);
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &Default::default()).unwrap();
    let url = "https://example.invalid/reference.png";
    std::fs::write(dir.join(reference::cache_name(url)), &png).unwrap();
    let never = AtomicBool::new(false);
    let got = reference::load(&Source::Url(url.into()), Some(&dir), &never).unwrap();
    assert_eq!((got.width, got.height), (32, 32));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn amount_scales_the_changes() {
    let src = scene(Kind::Canyon, 17);
    let r = fit(&src, &DevelopSettings::default(), &render(&src, &edited(|d| d.light.exposure = 0.8)));
    let half = r.scaled(0.5);
    let ex = |r: &MatchResult| r.changes.iter().find(|c| c.id == "light.exposure").map(|c| c.to).unwrap();
    assert!((ex(&half) - ex(&r) / 2.0).abs() < 1e-9);
    assert!(r.scaled(0.0).changes.iter().filter(|c| !c.id.ends_with(".hue")).all(|c| c.to == c.from));
    assert_eq!(r.scaled(f64::NAN).changes, r.changes);
}

/// A grey wall (working space, linear) with a small muted yellow patch on it.
fn grey_photo() -> Rgb32f {
    Rgb32f::from_fn(240, 160, |x, y| {
        let v = 0.04 + 0.7 * (x as f32 / 240.0) * (0.6 + 0.4 * y as f32 / 160.0);
        if (100..140).contains(&x) && (60..110).contains(&y) { [0.42, 0.36, 0.12] } else { [v, v, v] }
    })
}

#[test]
fn white_balance_follows_the_neutrals_not_a_big_coloured_object() {
    // the reference: a cool grey wall, and a large saturated yellow shirt covering half of it
    let reference = Rgba8::from_fn(240, 160, |x, y| {
        let g = (60.0 + 150.0 * x as f32 / 240.0) as u8;
        if (60..180).contains(&x) && y > 20 { [235, 205, 25, 255] } else { [g.saturating_sub(10), g.saturating_sub(2), g.saturating_add(14), 255] }
    });
    let r = fit(&grey_photo(), &DevelopSettings::default(), &reference);
    assert!(value(&r, "wb.temp") < 6500.0, "cooler like the reference's wall, not warmer like its shirt: {}", value(&r, "wb.temp"));
}
