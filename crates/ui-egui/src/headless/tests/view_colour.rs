//! Every view shows a photo with the look the loupe gives it. The grid, Compare and Survey drew a rejected
//! photo under a dark veil: on an A7 V portrait on macOS Colour the wall went from L* 49.6 to 28.7 (median ΔE00
//! 19 against the loupe), so the same photo looked darker, flatter and greyer there than in the loupe and the
//! filmstrip. Compare and Survey, where photos are compared, never dim it; the grid dims it only with Settings ▸
//! Interface ▸ Dim Rejected Photos (on by default, as in Lightroom Classic).
use super::*;
use crate::state::ViewMode;
use lightcraft_catalog::PhotoId;

/// The views a photo is judged in, the loupe first (the reference).
const VIEWS: [ViewMode; 4] = [ViewMode::Detail, ViewMode::PhotoGrid, ViewMode::Compare, ViewMode::Survey];

/// Where the last frame drew photo `id`: the largest image painted from one of its textures (the loupe's render
/// rather than its filmstrip thumbnail, the Compare tile rather than its filmstrip cell).
fn drawn(h: &Headless, id: PhotoId) -> Option<egui::Rect> {
    let mine: Vec<egui::TextureId> = h.app.renderer.textures.values().filter(|t| t.photo == id).map(|t| t.tex.id()).collect();
    h.view
        .painted_shapes()
        .into_iter()
        .filter_map(|s| match s {
            egui::epaint::Shape::Mesh(m) if mine.contains(&m.texture_id) => {
                Some(m.vertices.iter().fold(egui::Rect::NOTHING, |r, v| r.union(egui::Rect::from_min_max(v.pos, v.pos))))
            }
            _ => None,
        })
        .max_by(|a, b| a.area().total_cmp(&b.area()))
}

/// CIELAB (D65) of an sRGB pixel.
fn lab(c: [f64; 3]) -> [f64; 3] {
    let lin = c.map(|v| {
        let v = v / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    });
    let xyz = [
        (0.412_456_4 * lin[0] + 0.357_576_1 * lin[1] + 0.180_437_5 * lin[2]) / 0.950_47,
        0.212_672_9 * lin[0] + 0.715_152_2 * lin[1] + 0.072_175 * lin[2],
        (0.019_333_9 * lin[0] + 0.119_192 * lin[1] + 0.950_304_1 * lin[2]) / 1.088_83,
    ];
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let [fx, fy, fz] = xyz.map(f);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 colour difference.
fn de2000(a: [f64; 3], b: [f64; 3]) -> f64 {
    use std::f64::consts::PI;
    let ([l1, a1, b1], [l2, a2, b2]) = (a, b);
    let c_bar = (a1.hypot(b1) + a2.hypot(b2)) / 2.0;
    let g = 0.5 * (1.0 - (c_bar.powi(7) / (c_bar.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = (a1 * (1.0 + g), a2 * (1.0 + g));
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| if b == 0.0 && a == 0.0 { 0.0 } else { b.atan2(a).rem_euclid(2.0 * PI) };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let d = h2p - h1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else if d > PI {
        d - 2.0 * PI
    } else if d < -PI {
        d + 2.0 * PI
    } else {
        d
    };
    let dh_big = 2.0 * (c1p * c2p).sqrt() * (dh / 2.0).sin();
    let (l_bar, c_bar_p) = ((l1 + l2) / 2.0, (c1p + c2p) / 2.0);
    let h_bar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= PI {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 2.0 * PI {
        (h1p + h2p + 2.0 * PI) / 2.0
    } else {
        (h1p + h2p - 2.0 * PI) / 2.0
    };
    let t = 1.0 - 0.17 * (h_bar - PI / 6.0).cos() + 0.24 * (2.0 * h_bar).cos() + 0.32 * (3.0 * h_bar + PI / 30.0).cos()
        - 0.20 * (4.0 * h_bar - 63.0 * PI / 180.0).cos();
    let d_theta = (30.0 * PI / 180.0) * (-((h_bar * 180.0 / PI - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (c_bar_p.powi(7) / (c_bar_p.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (l_bar - 50.0).powi(2) / (20.0 + (l_bar - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * c_bar_p;
    let sh = 1.0 + 0.015 * c_bar_p * t;
    let rt = -(2.0 * d_theta).sin() * rc;
    let (x, y, z) = ((l2 - l1) / sl, (c2p - c1p) / sc, dh_big / sh);
    (x * x + y * y + z * z + rt * y * z).max(0.0).sqrt()
}

/// The photo inside `r` of the screen, averaged down to a 48 × 24 grid of CIELAB values. Only the top three
/// quarters: the grid's badge bar (stars, flag) sits on the bottom edge of a justified cell.
fn cut(img: &egui::ColorImage, r: egui::Rect, ppp: f32) -> Vec<[f64; 3]> {
    const W: usize = 48;
    const H: usize = 24;
    let r = r.shrink(2.0);
    let (x0, y0) = ((r.left() * ppp).max(0.0) as usize, (r.top() * ppp).max(0.0) as usize);
    let (w, h) = ((r.width() * ppp) as usize, (r.height() * ppp * 0.75) as usize);
    assert!(w >= W && h >= H, "the photo is drawn large enough to measure: {r:?}");
    let mut out = Vec::with_capacity(W * H);
    for gy in 0..H {
        for gx in 0..W {
            let (mut sum, mut n) = ([0.0; 3], 0.0_f64);
            for y in y0 + gy * h / H..y0 + (gy + 1) * h / H {
                for x in x0 + gx * w / W..x0 + (gx + 1) * w / W {
                    if let Some(c) = img.pixels.get(y * img.size[0] + x) {
                        sum = [sum[0] + f64::from(c.r()), sum[1] + f64::from(c.g()), sum[2] + f64::from(c.b())];
                        n += 1.0;
                    }
                }
            }
            out.push(lab(sum.map(|v| v / n.max(1.0))));
        }
    }
    out
}

fn median_de(a: &[[f64; 3]], b: &[[f64; 3]]) -> f64 {
    let mut d: Vec<f64> = a.iter().zip(b).map(|(p, q)| de2000(*p, *q)).collect();
    d.sort_by(f64::total_cmp);
    d[d.len() / 2]
}

/// Flags `ids` (two photos) as rejected, sets Dim Rejected Photos to `dim`, opens each view on them, and returns
/// each view's median ΔE00 from the loupe's picture of the first one.
fn rejected_views(h: &mut Headless, ids: [PhotoId; 2], dim: bool) -> Vec<(ViewMode, f64)> {
    let t = Duration::from_secs(10);
    let set = h.request("ui.set", json!({"settings": {"dimRejected": dim}}), t);
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(h.app.ui.settings.dim_rejected, dim, "ui.set reaches the setting");
    let flag = h.request("engine.execute", json!({"command": "photo.flag", "params": {"flag": "reject", "ids": [ids[0].0, ids[1].0]}}), t);
    assert_eq!(flag["ok"], true, "{flag}");
    let select = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [ids[0].0, ids[1].0], "active": ids[0].0}}), t);
    assert_eq!(select["ok"], true, "{select}");
    let mut loupe = Vec::new();
    let mut out = Vec::new();
    for view in VIEWS {
        h.app.ui.view = view;
        // (a few more frames once the renders are in: Compare asks for its tile's render after it has drawn the thumbnail)
        let img = (0..3).map(|_| h.snapshot(SETTLE)).last().expect("a frame");
        let r = drawn(h, ids[0]).unwrap_or_else(|| panic!("{view:?} draws the photo"));
        let lab = cut(&img, r, h.pixels_per_point);
        if view == ViewMode::Detail {
            loupe = lab;
        } else {
            out.push((view, median_de(&lab, &loupe)));
        }
    }
    out
}

/// Compare and Survey show the loupe's look whatever the setting; the grid is dimmed (far from it) only with it on.
fn assert_views(views: &[(ViewMode, f64)], dim: bool, what: &str) {
    for (view, de) in views {
        if *view == ViewMode::PhotoGrid && dim {
            assert!(*de > 10.0, "{what}, dimming on: the grid dims the rejected photo: {views:.2?}");
        } else {
            assert!(*de < 2.0, "{what}, dimming {}: {view:?} shows the loupe's look: {views:.2?}", if dim { "on" } else { "off" });
        }
    }
}

#[test]
fn a_rejected_photo_is_dimmed_only_in_the_grid_and_only_with_the_setting_on() {
    let mut h = demo([1200.0, 800.0]);
    let visible = h.app.session.visible_cloned();
    let ids = [visible[0], visible[1]];
    for dim in [true, false] {
        let views = rejected_views(&mut h, ids, dim);
        eprintln!("dimming {dim}: median ΔE00 from the loupe {views:.2?}");
        assert_views(&views, dim, "demo photo");
    }
}

/// Dim Rejected Photos is on for new users and for a `ui.json` saved before it existed, survives a save, and is the
/// Interface page's check box.
#[test]
fn dim_rejected_photos_is_on_by_default_saved_and_in_settings() {
    assert!(crate::state::AppSettings::default().dim_rejected, "on by default, as in Lightroom Classic");
    let old: crate::UiState = serde_json::from_str(r#"{"settings": {"gridBadges": "always"}}"#).unwrap();
    assert!(old.settings.dim_rejected, "a ui.json from before the setting dims");
    let mut off = old.clone();
    off.settings.dim_rejected = false;
    let saved = serde_json::to_string(&off).unwrap();
    assert!(saved.contains("\"dimRejected\":false"), "saved under its name");
    let read: crate::UiState = serde_json::from_str(&saved).unwrap();
    assert!(!read.settings.dim_rejected, "off survives a restart");
    // the check box on Settings ▸ Interface switches it
    let mut h = demo([1200.0, 800.0]);
    let t = Duration::from_secs(10);
    h.app.ui.dialog = Some(crate::state::Dialog::Settings { tab: "interface".into() });
    h.settle(SETTLE);
    let r = h.request("ui.clickWidget", json!({"id": "check:settings.dimRejected"}), t);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(!h.app.ui.settings.dim_rejected, "the check box turns it off");
}

/// The same on a real A7 V raw on its starting colour (macOS Colour on a Mac whose decoder reads it, else Camera
/// Matching), browsed in Local and added to the library. `LIGHTCRAFT_MACRAW_ARW` names the file; it is only read
/// (linked or copied twice into a temporary folder: Compare and Survey need two photos; the library takes the
/// second as a duplicate, so there the second photo is a virtual copy).
#[test]
fn a_rejected_a7v_raw_is_dimmed_only_in_the_grid_in_local_and_in_the_library() {
    let Some(arw) = std::env::var_os("LIGHTCRAFT_MACRAW_ARW") else {
        lightcraft_testkit::skip!(Input, "set LIGHTCRAFT_MACRAW_ARW to an A7 V ARW file");
        return;
    };
    let dir = std::env::temp_dir().join(format!("lc-ui-view-colour-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<String> = ["A.ARW", "B.ARW"]
        .iter()
        .map(|name| {
            let to = dir.join(name);
            if std::fs::hard_link(&arw, &to).is_err() {
                std::fs::copy(&arw, &to).unwrap();
            }
            to.to_string_lossy().into_owned()
        })
        .collect();
    let t = Duration::from_secs(120);
    for local in [true, false] {
        let services = crate::Services { png: None, ..Default::default() };
        let mut h = Headless::new(LightcraftApp::new(lightcraft_engine::Session::new().with_fs(), services), [1200.0, 800.0], 1.0);
        let r = if local {
            h.request("engine.execute", json!({"command": "library.browse", "params": {"path": dir.to_string_lossy()}}), t)
        } else {
            let r = h.request("engine.execute", json!({"command": "library.import", "params": {"paths": [files[0]]}}), t);
            let first = r["result"]["imported"][0].as_u64().unwrap_or_default();
            h.request("engine.execute", json!({"command": "photo.virtualCopy", "params": {"ids": [first]}}), t)
        };
        assert_eq!(r["ok"], true, "{r}");
        // (the window's browse lists the folder on a worker)
        assert!(h.step_until(SETTLE, |h| h.app.session.catalog.photos().count() == 2), "both copies are added: {r}");
        h.settle(SETTLE);
        let visible = h.app.session.visible_cloned();
        assert_eq!(visible.len(), 2, "both copies are listed");
        let colour = h.app.session.catalog.photo(visible[0]).map(|p| p.develop.profile.camera_colour);
        eprintln!("local {local}: starting colour {colour:?}");
        for dim in [true, false] {
            let views = rejected_views(&mut h, [visible[0], visible[1]], dim);
            eprintln!("local {local}, dimming {dim}: median ΔE00 from the loupe {views:.2?}");
            assert_views(&views, dim, &format!("A7 V raw, local {local}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
