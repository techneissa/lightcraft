//! Brush painting through the whole app: the real session, renderer, job pool and GPU (when there
//! is one), driven by a headless host at 60 frames a second, as a window with vsync would be.
//!
//! `cargo run --release -p lightcraft-ui-egui --example brush_live_bench -- [LIBRARY_DIR]`
//! Env: `MP=24` (megapixels of the generated demo photo), `SIZE=1920x1200` (points), `SCALE=1`,
//! `GPU=0` for the CPU pipeline, `STROKES=3`, `ERASE=1` to also paint an erase stroke.
//!
//! Prints, per stroke: the UI thread's frame time while painting (median / p90 / max), the time
//! from the pointer move to the frame that drew the adjustment under the brush (`live_ms`, NaN
//! when the build has no live adjustment preview), and after release the time until the settled
//! full-quality render is the texture.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use lightcraft_codecs::{ChromaSubsampling, EncodeImage, EncodeMeta, encode_jpeg};
use lightcraft_engine::Session;
use lightcraft_ui_egui::headless::Headless;
use lightcraft_ui_egui::render::Slot;
use lightcraft_ui_egui::{LightcraftApp, Services};
use serde_json::json;

const FRAME: Duration = Duration::from_micros(16_667);

/// A detailed `mp`-megapixel photo (smooth sky, textured ground, edges) as a JPEG.
fn demo_jpeg(mp: f64) -> Vec<u8> {
    let (w, h) = (((mp * 1e6 * 1.5).sqrt()) as usize, ((mp * 1e6 / 1.5).sqrt()) as usize);
    let mut px = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let edge = if v > 0.55 + 0.05 * (u * 12.0).sin() { 1.0 } else { 0.0 };
            let tex = ((x * 7 + y * 13) % 29) as f32 / 29.0;
            let sky = [0.25 + 0.4 * v, 0.45 + 0.3 * v, 0.9 - 0.2 * v];
            let ground = [0.30 + 0.25 * tex, 0.22 + 0.2 * tex, 0.12 + 0.1 * tex];
            let i = (y * w + x) * 3;
            for c in 0..3 {
                px[i + c] = ((sky[c] * (1.0 - edge) + ground[c] * edge) * 255.0) as u8;
            }
        }
    }
    let img = EncodeImage::new(w as u32, h as u32, 3, lightcraft_codecs::Samples::U8(&px));
    encode_jpeg(&img, 90, ChromaSubsampling::S444, &EncodeMeta::default()).unwrap()
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn settle(h: &mut Headless) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(120) {
        h.step();
        let r = &h.app.renderer;
        if r.textures.contains_key(&Slot::Main) && !r.is_pending(Slot::Main) && !h.busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    eprintln!("settle: timed out");
}

fn pointer(h: &mut Headless, ev: serde_json::Value) {
    let r = h.request("ui.pointer", json!({"events": [ev]}), Duration::from_secs(60));
    assert_eq!(r["ok"], true, "{r}");
}

fn main() {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let mp: f64 = env("MP", "24").parse().unwrap();
    let (w, hgt) = env("SIZE", "1920x1200").split_once('x').map(|(a, b)| (a.parse::<f32>().unwrap(), b.parse::<f32>().unwrap())).unwrap();
    let scale: f32 = env("SCALE", "1").parse().unwrap();
    let gpu = env("GPU", "1") != "0";
    let strokes: usize = env("STROKES", "3").parse().unwrap();
    let dir = std::env::args().nth(1).unwrap_or_else(|| std::env::temp_dir().join("brush_live_bench").to_string_lossy().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    let file = format!("{dir}/demo_{mp}mp.jpg");
    if !std::path::Path::new(&file).exists() {
        std::fs::write(&file, demo_jpeg(mp)).unwrap();
    }

    let mut s = Session::new().with_fs().with_system_clock();
    s.open_library(format!("{dir}/lib"), false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [file]})).unwrap();
    let id = r["imported"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_u64())
        .map(lightcraft_catalog::PhotoId)
        .unwrap_or_else(|| s.catalog.photos().find(|p| p.file_name.ends_with(".jpg")).map(|p| p.id).expect("photo in library"));
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    let mut app = LightcraftApp::new(s, Services::default());
    app.ui.settings.gpu = gpu;
    let mut h = Headless::new(app, [w, hgt], scale);
    let t = Duration::from_secs(60);
    h.request("ui.set", json!({"view": "detail", "right": "edit", "filmstrip": true}), t);
    let t_open = Instant::now();
    settle(&mut h);
    eprintln!(
        "opened in {:.0} ms, loupe texture {:?}, gpu {gpu}, {mp} MP",
        t_open.elapsed().as_secs_f64() * 1e3,
        h.app.renderer.textures.get(&Slot::Main).map(|t| t.size)
    );
    h.request("engine.execute", json!({"command": "tool.brush", "params": {"new": true}}), t);
    h.request("ui.set", json!({"brushSize": 0.06, "brushFeather": 60.0, "brushFlow": 100.0}), t);
    // a mask with an adjustment (a new brush mask starts at zero, which shows nothing) and a first stroke
    let r = h.request("engine.execute", json!({"command": "mask.brushStroke", "params": {"points": [[0.5, 0.9], [0.52, 0.9]], "size": 0.03}}), t);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "mask.adjust", "params": {"values": {"exposure": 1.5}}}), t);
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);

    println!("stroke,frames,frame_ms_med,frame_ms_p90,frame_ms_max,live_ms_med,live_ms_max,main_ms,settle_ms");
    let t0 = Instant::now();
    let mut n = 0u32;
    for k in 0..strokes {
        let y0 = 0.3 + 0.12 * k as f64;
        let mut frame_ms = Vec::new();
        let mut live_ms = Vec::new();
        let steps = 60;
        for i in 0..=steps {
            let x = 0.15 + 0.7 * i as f64 / steps as f64;
            let y = y0 + 0.03 * (i as f64 * 0.3).sin();
            let kind = if i == 0 { "down" } else { "drag" };
            let due = t0 + FRAME * (n + 1);
            if let Some(d) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
            n += 1;
            let start = Instant::now();
            pointer(&mut h, json!({"kind": kind, "x": x, "y": y}));
            frame_ms.push(start.elapsed().as_secs_f64() * 1e3);
            if let Some(ms) = live_adjustment_ms(&h) {
                live_ms.push(ms);
            }
            if k == 0 && i == steps / 2 {
                snap(&h, "mid-stroke");
            }
        }
        let release = Instant::now();
        let last = 0.15 + 0.7;
        pointer(&mut h, json!({"kind": "up", "x": last, "y": y0}));
        // the loupe with the committed stroke, then everything (the brush preview renders after it)
        let main_ms = loop {
            h.step();
            let r = &h.app.renderer;
            if h.app.session.active_mask.is_some()
                && !r.is_pending(Slot::Main)
                && r.textures.get(&Slot::Main).is_some_and(|t| Some(t.key) == r.wanted(Slot::Main))
                && h.app.brush_live.is_none()
            {
                break release.elapsed().as_secs_f64() * 1e3;
            }
            if release.elapsed() > Duration::from_secs(60) {
                break f64::NAN;
            }
        };
        settle(&mut h);
        if k == 0 {
            snap(&h, "settled");
        }
        let settle_ms = release.elapsed().as_secs_f64() * 1e3;
        let fmax = frame_ms.iter().cloned().fold(0.0, f64::max);
        let lmax = live_ms.iter().cloned().fold(f64::NAN, f64::max);
        println!(
            "{k},{},{:.1},{:.1},{fmax:.1},{:.1},{lmax:.1},{main_ms:.0},{settle_ms:.0}",
            frame_ms.len(),
            pct(&mut frame_ms, 0.5),
            pct(&mut frame_ms, 0.9),
            pct(&mut live_ms, 0.5)
        );
    }
}

/// Milliseconds the last frame took to bring the adjustment under the brush on screen (`None`:
/// this build draws no live adjustment).
fn live_adjustment_ms(h: &Headless) -> Option<f64> {
    h.app.brush_live_ms()
}

/// With `SNAP=DIR`: what the window shows now, as `DIR/NAME.png`.
fn snap(h: &Headless, name: &str) {
    let Ok(dir) = std::env::var("SNAP") else { return };
    let img = h.paint();
    let rgba: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
    let enc = EncodeImage::new(img.size[0] as u32, img.size[1] as u32, 4, lightcraft_codecs::Samples::U8(&rgba));
    let png = lightcraft_codecs::encode_png(&enc, &EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(format!("{dir}/{name}.png"), png).unwrap();
}
