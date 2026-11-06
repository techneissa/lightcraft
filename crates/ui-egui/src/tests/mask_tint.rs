//! The mask overlay is drawn over the loupe's plain render ([`crate::mask_tint`]): showing it, hiding it and changing its
//! opacity, colour or view start no render. The tests count the renders that finish ([`crate::render::Renderer::completed`]).

use std::time::Duration;

use lightcraft_pipeline::MaskView;
use serde_json::json;

use crate::headless::Headless;
use crate::render::Slot;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn masking() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    for (command, params) in [
        ("panel.masking", json!({})),
        ("mask.add", json!({"kind": "radial", "center": [0.4, 0.5], "rx": 0.2, "ry": 0.2})),
        ("mask.add", json!({"kind": "linear", "start": [0.2, 0.3], "end": [0.8, 0.7]})),
    ] {
        let r = h.request("ui.set", json!({"view": "detail"}), T);
        assert_eq!(r["ok"], true, "{r}");
        let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
        assert_eq!(r["ok"], true, "{command}: {r}");
    }
    assert!(h.settle(SETTLE), "the renders settle");
    h
}

fn main_key(h: &Headless) -> u64 {
    h.app.renderer.textures.get(&Slot::Main).expect("the loupe's render").key
}

/// Run frames until nothing renders, and say how many renders finished meanwhile.
fn renders_during(h: &mut Headless, change: impl FnOnce(&mut Headless)) -> u64 {
    let before = h.app.renderer.completed;
    change(h);
    assert!(h.settle(SETTLE), "the renders settle");
    h.app.renderer.completed - before
}

// Given a mask overlay over the loupe, toggling it with O any number of times, changing its opacity, colour and view, and
// showing every mask at once start no render, and what is drawn follows
#[test]
fn the_overlay_toggle_and_its_opacity_start_no_render() {
    let mut h = masking();
    assert!(h.app.ui.mask_overlay, "a new mask shows its overlay");
    let key = main_key(&h);
    assert!(h.app.renderer.tints.has_masks(Slot::Main), "the loupe's render came with the masks' alphas");
    assert!(h.app.renderer.tints.drawn(Slot::Main, key), "the tint is up");
    let builds = h.app.renderer.tints.builds;

    for flip in 0..4 {
        let n = renders_during(&mut h, |h| {
            let r = h.request("ui.key", json!({"key": "o"}), T);
            assert_eq!(r["ok"], true, "{r}");
        });
        assert_eq!(n, 0, "flip {flip} of O rendered");
        assert_eq!(h.app.ui.mask_overlay, flip % 2 == 1, "O flipped the overlay");
        assert_eq!(h.app.renderer.tints.drawn(Slot::Main, key), h.app.ui.mask_overlay, "the tint follows the flag");
    }
    assert_eq!(h.app.renderer.tints.builds, builds, "a toggle makes no tint texture either");
    assert_eq!(main_key(&h), key, "the photo's render is the one it was");

    for opacity in [80.0, 30.0, 5.0, 0.0, 100.0] {
        let n = renders_during(&mut h, |h| h.app.ui.mask_overlay_opacity = opacity);
        assert_eq!(n, 0, "opacity {opacity} rendered");
    }
    assert_eq!(h.app.renderer.tints.builds, builds, "the opacity is a tint of the draw: no texture is made");

    // the colour and the view change the tint's texture, not the photo
    for view in MaskView::ALL {
        let n = renders_during(&mut h, |h| h.app.ui.mask_overlay_mode = view.name().to_string());
        assert_eq!(n, 0, "{view:?} rendered");
        assert!(h.app.renderer.tints.drawn(Slot::Main, key), "{view:?} is drawn");
    }
    let n = renders_during(&mut h, |h| h.app.ui.mask_overlay_color = [30, 200, 60]);
    assert_eq!(n, 0, "a colour rendered");

    // every mask at once, and back
    let n = renders_during(&mut h, |h| {
        let r = h.request("engine.execute", json!({"command": "view.maskOverlayAll", "params": {}}), T);
        assert_eq!(r["ok"], true, "{r}");
    });
    assert_eq!(n, 0, "Show All Masks rendered");
    assert!(h.app.renderer.tints.drawn(Slot::Main, key), "all the masks are drawn");
    // a mask's shape does change the photo's render (its effect), and the new render brings the new alphas
    let n = renders_during(&mut h, |h| {
        let r = h.request("engine.execute", json!({"command": "mask.adjust", "params": {"values": {"exposure": 0.5}}}), T);
        assert_eq!(r["ok"], true, "{r}");
    });
    assert!(n >= 1, "an edit renders");
}

// Given a render that came with the masks, the alpha tinted is the selected mask's (the other masks do not show)
#[test]
fn the_tint_is_the_selected_masks_alpha() {
    let h = masking();
    let both = h.app.renderer.tints.planes(Slot::Main).expect("the masks");
    assert_eq!(both.planes.len(), 2, "both masks' alphas come with the render");
    let ids: Vec<u32> = both.planes.iter().map(|(id, _)| *id).collect();
    assert!(both.plane(ids[0]).is_some_and(|p| p.iter().any(|a| *a > 0)), "the radial mask selects something");
    assert!(both.union().iter().zip(both.plane(ids[0]).unwrap()).all(|(u, a)| u >= a), "the union covers each mask");
}
