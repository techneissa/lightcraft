//! Fork feature (fork/birefnet-masks.md, issue #48): the model dropdown for Subject and Background parts, the model
//! stored with each part, the BiRefNet commands, and a damaged model file. No test needs a real model or the network.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lightcraft_develop::{MaskShape, SegMask};
use serde_json::{Value, json};

use crate::Session;
use crate::segment::birefnet::SelectionModel;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-birefnet-engine-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Put a (fake) SAM 3 model folder in `root/sam3`.
fn fake_sam3(root: &std::path::Path) -> PathBuf {
    let dir = root.join("sam3");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.safetensors"), b"\x10\x00\x00\x00\x00\x00\x00\x00{}").unwrap();
    std::fs::write(dir.join("vocab.json"), b"{}").unwrap();
    std::fs::write(dir.join("merges.txt"), b"#version: 0.2\n").unwrap();
    dir
}

/// Put a (fake, unusable) BiRefNet model file in `root/birefnet`.
fn fake_birefnet(root: &std::path::Path, bytes: &[u8]) {
    std::fs::create_dir_all(root.join("birefnet")).unwrap();
    std::fs::write(root.join("birefnet").join("model.safetensors"), bytes).unwrap();
}

/// A session in background mode with the models in `root` (`sam3`, `birefnet`) as asked.
fn session(root: &std::path::Path, sam: bool, bire: bool) -> Session {
    let mut s = Session::with_demo();
    s.segmenter.background = true;
    if sam {
        s.segmenter.dir = Some(fake_sam3(root));
    }
    if bire {
        fake_birefnet(root, b"\x10\x00\x00\x00\x00\x00\x00\x00{}");
    }
    s.segmenter.birefnet.root = Some(root.to_path_buf());
    s.segmenter.birefnet.file = Some(root.join("config").join("selection-model.json"));
    s
}

fn model_of(s: &Session, mask: usize) -> Option<String> {
    let d = s.develop_of(s.active().unwrap()).unwrap();
    match &d.masks[mask].components[0].shape {
        MaskShape::Subject { seg: Some(seg), .. } | MaskShape::Background { seg: Some(seg), .. } => seg.model.clone(),
        other => panic!("not a stored Subject or Background: {other:?}"),
    }
}

fn select(s: &mut Session, model: &str) -> Value {
    s.execute("segment.selectionModel", &json!({"model": model})).unwrap()
}

/// What the activity stack says the running segmentation jobs are.
fn rows(s: &Session) -> Vec<String> {
    s.activity.list().into_iter().filter(|t| t.kind == "segment").map(|t| t.label).collect()
}

#[test]
fn the_dropdown_lists_only_installed_models_and_refuses_the_others() {
    let root = scratch("list");
    let mut s = session(&root, true, false);
    let st = s.execute("segment.selectionModel", &json!({})).unwrap();
    assert_eq!(st["model"], "sam3", "SAM 3 is all there is");
    let installed: Vec<_> = st["models"].as_array().unwrap().iter().filter(|m| m["installed"] == true).map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(installed, ["sam3"], "only SAM 3 is installed: the dropdown is hidden");
    let e = s.execute("segment.selectionModel", &json!({"model": "birefnet"})).unwrap_err().to_string();
    assert!(e.contains("not installed"), "{e}");
    assert!(s.execute("segment.selectionModel", &json!({"model": "sam3+birefnet"})).is_err());

    fake_birefnet(&root, b"x");
    let st = select(&mut s, "birefnet");
    assert_eq!(st["model"], "birefnet");
    let installed: Vec<_> = st["models"].as_array().unwrap().iter().filter(|m| m["installed"] == true).map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(installed, ["sam3", "birefnet", "sam3+birefnet"]);
    // the choice is kept across restarts
    assert!(root.join("config").join("selection-model.json").is_file());
    let mut again = session(&root, true, true);
    again.segmenter.birefnet.selection = Some(SelectionModel::Combined);
    assert_eq!(again.segmenter.selection(), SelectionModel::Combined);
    // never chosen: BiRefNet when it is installed
    again.segmenter.birefnet.selection = None;
    assert_eq!(again.segmenter.selection(), SelectionModel::BiRefNet);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_model_that_went_away_falls_back_to_sam3() {
    let root = scratch("gone");
    let mut s = session(&root, true, true);
    select(&mut s, "birefnet");
    std::fs::remove_file(root.join("birefnet").join("model.safetensors")).unwrap();
    assert_eq!(s.segmenter.selection(), SelectionModel::Sam3, "the saved choice is not installed any more");
    // only BiRefNet installed: it is the model
    let root2 = scratch("only-bire");
    let s = session(&root2, false, true);
    assert_eq!(s.segmenter.selection(), SelectionModel::BiRefNet);
    assert!(s.segmenter.can_select("subject") && !s.segmenter.can_select("sky"), "Sky needs SAM 3");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

/// Changing the dropdown touches only parts made afterwards: each job carries the model chosen when it was queued, and
/// each stored part keeps the id of the model that made it.
#[test]
fn the_dropdown_sets_the_model_of_new_parts_only() {
    let root = scratch("new-only");
    let mut s = session(&root, true, true);
    let photo = s.active().unwrap();
    let seg = SegMask::from_logits(4, &[5.0; 16]);

    assert_eq!(s.segmenter.selection(), SelectionModel::BiRefNet, "never chosen, BiRefNet installed: BiRefNet");
    select(&mut s, "sam3");
    s.execute("mask.add", &json!({"kind": "subject"})).unwrap();
    assert_eq!(rows(&s), ["Selecting with SAM 3"]);
    select(&mut s, "birefnet");
    s.execute("mask.add", &json!({"kind": "subject"})).unwrap();
    select(&mut s, "sam3+birefnet");
    s.execute("mask.add", &json!({"kind": "background"})).unwrap();
    select(&mut s, "sam3");
    s.execute("mask.add", &json!({"kind": "sky"})).unwrap();
    let labels = rows(&s);
    assert_eq!(labels, ["Selecting with SAM 3", "Selecting with BiRefNet", "Selecting with SAM 3 + BiRefNet", "Selecting with SAM 3"], "{labels:?}");

    // the (fake) worker's answers: the ids the tags name
    let tags: Vec<_> = s.segmenter.in_flight_tags();
    assert_eq!(tags.len(), 4);
    for (tag, id) in tags.into_iter().zip([None, Some("birefnet"), Some("sam3+birefnet"), None]) {
        let made = match id {
            Some(id) => seg.clone().made_by(id),
            None => seg.clone(),
        };
        s.segmenter.deliver(tag, Ok(Some(made)));
    }
    let polled = s.segment_poll();
    assert!(polled.changed);
    let d = s.develop_of(photo).unwrap();
    assert_eq!(d.masks.len(), 4);
    assert_eq!(model_of(&s, 0), None, "SAM 3 parts store no id");
    assert_eq!(model_of(&s, 1).as_deref(), Some("birefnet"));
    assert_eq!(model_of(&s, 2).as_deref(), Some("sam3+birefnet"));
    assert!(matches!(d.masks[3].components[0].shape, MaskShape::Sky { seg: Some(_), .. }));

    // changing the dropdown later changes none of them, and the ids survive saving and reading the document
    select(&mut s, "birefnet");
    assert_eq!(model_of(&s, 0), None);
    assert_eq!(model_of(&s, 1).as_deref(), Some("birefnet"));
    let json = serde_json::to_string(&*s.develop_of(photo).unwrap()).unwrap();
    let back: lightcraft_develop::DevelopSettings = serde_json::from_str(&json).unwrap();
    let ids: Vec<_> = back
        .masks
        .iter()
        .filter_map(|m| match &m.components[0].shape {
            MaskShape::Subject { seg: Some(g), .. } | MaskShape::Background { seg: Some(g), .. } => Some(g.model.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, [None, Some("birefnet".to_string()), Some("sam3+birefnet".to_string())]);
    let _ = std::fs::remove_dir_all(&root);
}

/// A BiRefNet part gets no SAM 3 detail pass (that pass would replace its fine edge); a SAM 3 part still does.
#[test]
fn a_birefnet_part_keeps_its_edges() {
    let root = scratch("detail");
    let mut s = session(&root, true, true);
    s.segmenter.background = false;
    let photo = s.active().unwrap();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    let mid = s.active_mask.unwrap();
    let mut small = vec![-8.0f32; 64 * 64];
    for y in 20..30 {
        for x in 20..30 {
            small[y * 64 + x] = 8.0;
        }
    }
    for (n, made) in [None, Some("birefnet"), Some("sam3+birefnet")].into_iter().enumerate() {
        let g = SegMask::from_logits(64, &small);
        let g = made.map_or(g.clone(), |id| g.made_by(id));
        s.execute("mask.addComponent", &json!({"id": mid, "op": "add", "kind": "subject", "seg": serde_json::to_value(g).unwrap()})).unwrap();
        let r = s.segment_detail(photo, mid, n + 1);
        if made.is_none() {
            // (asked, and the fake SAM 3 model fails)
            assert!(r.is_err(), "a SAM 3 part still gets its detail pass: {r:?}");
        } else {
            assert_eq!(r, Ok(false), "{made:?} gets none");
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A damaged model file is a message in the activity of the user, not a panic, and the worker carries on.
#[test]
fn a_damaged_birefnet_file_is_a_message_not_a_panic() {
    let root = scratch("damaged");
    for (name, bytes) in [
        ("not-safetensors", b"definitely not a model".to_vec()),
        ("empty-header", b"\x02\x00\x00\x00\x00\x00\x00\x00{}".to_vec()),
        ("huge-header", vec![0xff; 16]),
    ] {
        let root = root.join(name);
        std::fs::create_dir_all(&root).unwrap();
        let mut s = session(&root, false, false);
        fake_birefnet(&root, &bytes);
        assert_eq!(s.segmenter.selection(), SelectionModel::BiRefNet, "{name}");
        s.execute("mask.add", &json!({"kind": "subject"})).unwrap();
        let t = Instant::now();
        let mut messages = Vec::new();
        while messages.is_empty() {
            messages.extend(s.segment_poll().messages);
            assert!(t.elapsed() < Duration::from_secs(60), "{name}: no answer");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(messages[0].to_lowercase().contains("birefnet"), "{name}: {messages:?}");
        assert!(s.develop_of(s.active().unwrap()).unwrap().masks.is_empty(), "{name}: no rough mask is left behind");
        assert!(rows(&s).is_empty(), "{name}: the row is gone");
        // the worker is still there for the next request
        s.execute("mask.add", &json!({"kind": "subject"})).unwrap();
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hostile_command_input_is_an_error() {
    let root = scratch("hostile");
    let mut s = session(&root, true, true);
    let long = "x".repeat(100_000);
    for bad in
        [json!(7), json!(true), json!([]), json!({}), json!(""), json!("   "), json!("SAM3"), json!("sam3;birefnet"), json!(long), json!("../../etc")]
    {
        let r = s.execute("segment.selectionModel", &json!({"model": bad}));
        let e = r.unwrap_err().to_string();
        assert!(e.len() < 400, "an error that does not echo the input back whole: {}", e.len());
    }
    assert_eq!(s.segmenter.selection(), SelectionModel::BiRefNet, "nothing changed (never chosen: BiRefNet)");
    // null and a missing model only read
    assert_eq!(s.execute("segment.selectionModel", &json!({"model": null})).unwrap()["model"], "birefnet");
    // the download needs `confirm: true` exactly
    for bad in [json!({}), json!({"confirm": "yes"}), json!({"confirm": 1}), json!({"confirm": false}), json!({"acknowledged": true})] {
        let e = s.execute("birefnet.model.download", &bad).unwrap_err().to_string();
        assert!(e.contains("confirm") && e.contains("MB"), "{e}");
    }
    assert!(!s.segmenter.birefnet_download_status().running);
    assert_eq!(s.execute("birefnet.model.cancel", &json!({})).unwrap()["cancelled"], false);
    let st = s.execute("birefnet.model.status", &json!({})).unwrap();
    assert_eq!(st["installed"], true);
    assert_eq!(st["selection"], "birefnet");
    assert_eq!(st["sizeBytes"], 444_473_596u64);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_download_is_refused_without_a_folder_and_does_nothing_when_installed() {
    let mut s = Session::with_demo();
    let e = s.execute("birefnet.model.download", &json!({"confirm": true})).unwrap_err().to_string();
    assert!(e.contains("no folder") || e.contains("not available"), "{e}");
    let root = scratch("installed");
    let mut s = session(&root, false, true);
    let r = s.execute("birefnet.model.download", &json!({"confirm": true})).unwrap();
    assert_eq!(r["started"], false);
    assert_eq!(r["installed"], true);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_damaged_selection_file_is_the_default() {
    let root = scratch("file");
    let mut s = session(&root, true, true);
    let file = root.join("config").join("selection-model.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    for body in ["", "{", "null", r#"{"model":"nonsense"}"#, "\u{0}"] {
        std::fs::write(&file, body).unwrap();
        s.segmenter.birefnet.selection = None;
        let file_choice = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        assert!(file_choice.and_then(|v| v["model"].as_str().and_then(SelectionModel::parse)).is_none(), "{body:?}");
    }
    let _ = std::fs::remove_dir_all(&root);
}
