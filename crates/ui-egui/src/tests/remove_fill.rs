//! Fork feature (`fork/lama-fill.md`): the Remove panel's Fill picker (headless).

use std::time::Duration;

use lightcraft_develop::SpotFill;
use lightcraft_engine::find_removals::Entry;
use lightcraft_pipeline::find::{Candidate, Kind, cover, shape::Mask};
use serde_json::json;

use crate::headless::Headless;
use crate::state::RightPanel;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn person() -> Candidate {
    let (w, h) = (300usize, 200usize);
    let m = Mask::from_fn(w, h, |x, y| {
        let (u, v) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
        (0.1..0.2).contains(&u) && (0.2..0.8).contains(&v)
    });
    Candidate {
        kind: Kind::Person,
        source: "sam3",
        outline: vec![(0.1, 0.2), (0.2, 0.2), (0.2, 0.8), (0.1, 0.8)],
        closed: true,
        bbox: [0.1, 0.2, 0.2, 0.8],
        area: 0.06,
        score: 0.9,
        main: false,
        spots: cover::cover_mask(&m),
    }
}

/// The Remove panel with two found people; `model` puts a (fake) model file where LaMa looks.
fn panel(model: bool, tag: &str) -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    let dir = std::env::temp_dir().join(format!("lc-remove-fill-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    if model {
        std::fs::write(dir.join(lightcraft_engine::lama_fill::MODEL_FILE), b"not a model, but the file is there").unwrap();
    }
    h.app.session.lama.set_dir(Some(dir));
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    assert_eq!(h.request("engine.execute", json!({"command": "panel.remove"}), T)["ok"], true);
    assert_eq!(h.app.ui.right, RightPanel::Remove);
    let photo = h.app.session.active();
    let entries = [person(), person()].into_iter().enumerate().map(|(i, candidate)| Entry { id: i as u32 + 1, candidate, selected: false }).collect();
    h.app.session.finds.restore(photo, entries);
    h.settle(Duration::from_secs(10));
    h
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
}

fn fills(h: &Headless) -> Vec<SpotFill> {
    h.app.session.develop_of(h.app.session.active().unwrap()).unwrap().spots.iter().map(|s| s.fill).collect()
}

fn pick(h: &mut Headless, entry: &str) {
    click(h, "dropdown:removeFill");
    h.step();
    click(h, &format!("removeFill:{entry}"));
    h.step();
}

#[test]
fn the_picker_sets_the_fill_of_new_spots_only() {
    let mut h = panel(true, "new-only");
    assert_eq!(h.app.ui.remove_fill, SpotFill::Classic);
    // Classic: Remove takes the first one with Classic spots
    click(&mut h, "findCandidate:1");
    click(&mut h, "button:findRemove");
    let classic = fills(&h);
    assert!(!classic.is_empty() && classic.iter().all(|f| *f == SpotFill::Classic), "{classic:?}");
    // LaMa: the next ones are LaMa; the earlier spots keep theirs
    pick(&mut h, "lama");
    assert_eq!(h.app.ui.remove_fill, SpotFill::Lama);
    click(&mut h, "button:findRemoveAll");
    let after = fills(&h);
    assert_eq!(after[..classic.len()], classic[..], "earlier spots keep their fill");
    assert!(after.len() > classic.len() && after[classic.len()..].iter().all(|f| *f == SpotFill::Lama), "{after:?}");
    // and back to Classic: nothing already made changes
    pick(&mut h, "classic");
    assert_eq!(h.app.ui.remove_fill, SpotFill::Classic);
    assert_eq!(fills(&h), after);
}

#[test]
fn without_the_model_lama_offers_a_download_and_changes_nothing_until_asked() {
    let mut h = panel(false, "offer");
    pick(&mut h, "lama");
    assert_eq!(h.app.ui.remove_fill, SpotFill::Classic, "choosing LaMa without it only asks");
    // the question is up; Cancel closes it and downloads nothing
    click(&mut h, "button:lamaCancel");
    h.step();
    assert!(h.app.widgets.iter().all(|(id, _)| id != "button:lamaDownload"), "the question is gone");
    assert_eq!(h.app.session.lama.download_state(), lightcraft_engine::lama_fill::Download::Idle);
    assert_eq!(h.app.ui.remove_fill, SpotFill::Classic);
    // Remove All still works, with Classic spots
    click(&mut h, "button:findRemoveAll");
    assert!(fills(&h).iter().all(|f| *f == SpotFill::Classic));
}
