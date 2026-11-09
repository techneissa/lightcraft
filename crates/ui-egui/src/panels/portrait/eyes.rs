//! The Portrait tab's section of the eyes feature (`lightcraft_retouch::eyes`): Whites, Iris, Dark Circles and
//! Eye Bags, for every face or the picked one.

pub(super) fn body(cx: &mut super::Ctx<'_>) {
    for id in ["retouch.eyes.whites", "retouch.eyes.iris", "retouch.eyes.darkCircles", "retouch.eyes.eyeBags"] {
        cx.slider(id);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use crate::headless::Headless;
    use crate::{LightcraftApp, Services};

    /// Opening the Eyes section shows its four sliders.
    #[test]
    fn the_eyes_section_has_its_four_sliders() {
        let t = Duration::from_secs(20);
        let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
        let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
        assert_eq!(h.request("ui.set", json!({"view": "detail"}), t)["ok"], true);
        assert_eq!(h.request("ui.clickWidget", json!({"id": "icon:portrait"}), t)["ok"], true);
        h.step();
        let r = h.request("engine.execute", json!({"command": "portrait.section", "params": {"section": "eyes", "open": true}}), t);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        h.step();
        let shown = h.request("ui.widgets", json!({"filter": "slider:retouch.eyes."}), t);
        let ids: Vec<&str> = shown["result"].as_array().into_iter().flatten().filter_map(|w| w["id"].as_str()).collect();
        for id in ["whites", "iris", "darkCircles", "eyeBags"] {
            assert!(ids.contains(&format!("slider:retouch.eyes.{id}").as_str()), "{id} missing from {ids:?}");
        }
    }
}
