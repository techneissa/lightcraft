//! Fork feature (`fork/parse-ahead.md`): the switch in Settings ▸ Performance turns people parsing ahead of time off
//! in the engine at the next frame, and on again (headless).

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

#[test]
fn the_performance_switch_turns_parsing_ahead_off_and_on() {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default());
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    assert!(h.app.ui.settings.parse_ahead, "on by default");
    h.step();
    assert!(h.app.session.parse_ahead.enabled());
    let r = h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "performance"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(Duration::from_secs(30));
    h.request("ui.widgets", json!({}), T);
    h.step();
    assert_eq!(h.request("ui.move", json!({"x": 600.0, "y": 450.0}), T)["ok"], true);
    assert_eq!(h.request("ui.scroll", json!({"dx": 0.0, "dy": -1200.0}), T)["ok"], true);
    h.step();
    h.step();
    let r = h.request("ui.clickWidget", json!({"id": "check:settings.parseAhead"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(!h.app.ui.settings.parse_ahead);
    assert!(!h.app.session.parse_ahead.enabled(), "the frame's pump passed the switch on");
    let st = h.app.session.execute("parsing.ahead.status", &json!({})).unwrap();
    assert_eq!(st["state"], "off", "{st}");
    let r = h.request("ui.clickWidget", json!({"id": "check:settings.parseAhead"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.app.ui.settings.parse_ahead && h.app.session.parse_ahead.enabled());
}

/// The picture for the PR (`LIGHTCRAFT_TEST_SHOTS=<dir>`): Settings ▸ Performance scrolled to the switch.
#[test]
fn screenshot_of_the_switch() {
    let Some(dir) = std::env::var_os("LIGHTCRAFT_TEST_SHOTS") else { return };
    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    let services = Services {
        png: Some(Box::new(png)),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        write_new: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        ..Default::default()
    };
    let mut h = Headless::new(LightcraftApp::new(lightcraft_engine::Session::with_demo(), services), [1200.0, 900.0], 1.0);
    h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "performance"}}), T);
    h.settle(Duration::from_secs(30));
    h.request("ui.move", json!({"x": 600.0, "y": 450.0}), T);
    h.request("ui.scroll", json!({"dx": 0.0, "dy": -900.0}), T);
    h.step();
    h.step();
    let path = std::path::Path::new(&dir).join("parse-ahead-setting.png");
    let r = h.request("ui.screenshot", json!({"path": path.to_string_lossy(), "headless": true}), T);
    assert_eq!(r["ok"], true, "{r}");
}
