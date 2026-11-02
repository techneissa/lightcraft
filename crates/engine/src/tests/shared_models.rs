//! Face and denoise models in the folder shared with the other ArtCraft apps (fork/shared-models.md): new models go
//! there, the old folder is still read, and removing a shared model needs `everyApp`. Temporary folders only.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use serde_json::{Value, json};

use crate::Session;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-shared-models-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn face_session(d: &Path) -> Session {
    let mut s = Session::new();
    s.face_models_dir = Some(d.join("old"));
    s.face_shared_dir = Some(d.join("shared"));
    s
}

fn denoise_session(d: &Path) -> Session {
    let mut s = Session::new();
    s.set_denoise_models_dir(Some(d.join("old")));
    s.denoise.shared_dir = Some(d.join("shared"));
    s.denoise.loader = super::denoise::dim_loader(&Arc::new(AtomicUsize::new(0)));
    s
}

fn listed<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["models"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or(&Value::Null)
}

#[test]
fn a_new_face_model_lands_in_the_shared_folder_and_removing_it_needs_every_app() {
    let d = temp("faces");
    let mut s = face_session(&d);
    let f = d.join("Mine.onnx");
    std::fs::write(&f, lightcraft_faces::synthetic::tiny_embedder_model(512)).unwrap();
    let r = s.execute("faces.models.install", &json!({"path": f.to_string_lossy(), "acknowledged": true})).unwrap();
    let id = r["installed"]["id"].as_str().unwrap().to_string();
    let shared = d.join("shared").join(&id);
    assert!(shared.join("model.onnx").is_file() && shared.join("face-model.json").is_file());
    assert!(!d.join("old").join(&id).exists(), "nothing lands in the old folder");
    let l = s.execute("faces.models.list", &json!({})).unwrap();
    assert_eq!(listed(&l, &id)["shared"], true);
    // the settings stay in the old folder
    assert!(d.join("old").join("settings.json").is_file());
    // refused without everyApp, and nothing is touched
    let e = s.execute("faces.models.remove", &json!({"id": id})).unwrap_err().to_string();
    assert!(e.contains("everyApp"), "{e}");
    assert!(s.execute("faces.models.remove", &json!({"id": id, "everyApp": false})).is_err());
    assert!(shared.join("model.onnx").is_file());
    assert_eq!(s.execute("faces.models.remove", &json!({"id": id, "everyApp": true})).unwrap()["removed"], id);
    assert!(!shared.exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_face_model_in_the_old_folder_is_found_used_and_removed_with_one_click() {
    let d = temp("faces-old");
    let mut s = face_session(&d);
    // install into the old folder only (a host without the shared one), then add the shared one
    s.face_shared_dir = None;
    let f = d.join("Mine.onnx");
    std::fs::write(&f, lightcraft_faces::synthetic::tiny_embedder_model(512)).unwrap();
    let id = s.execute("faces.models.install", &json!({"path": f.to_string_lossy(), "acknowledged": true})).unwrap()["installed"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.face_shared_dir = Some(d.join("shared"));
    let l = s.execute("faces.models.list", &json!({})).unwrap();
    assert_eq!(listed(&l, &id)["installed"], true);
    assert_eq!(listed(&l, &id)["shared"], false);
    assert_eq!(s.execute("faces.models.test", &json!({"id": id})).unwrap()["ok"], true);
    assert!(s.execute("faces.models.remove", &json!({"id": id})).is_ok(), "an old-folder model is only LightCraft's");
    assert!(!d.join("old").join(&id).exists());
    assert!(!d.join("shared").exists(), "the shared folder is never made by a removal");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_denoise_model_is_looked_up_in_both_folders_and_a_new_one_goes_to_the_shared_one() {
    let d = temp("denoise");
    let mut s = denoise_session(&d);
    let onnx = super::denoise::model_files(&d, "stand-in");
    let r = s.execute("denoise.models.install", &json!({"path": onnx.to_string_lossy(), "acknowledged": true})).unwrap();
    assert_eq!(r["model"], "stand-in");
    assert!(d.join("shared").join("stand-in").join("model.onnx").is_file());
    assert!(!d.join("old").join("stand-in").exists());
    let l = s.execute("denoise.models.list", &json!({})).unwrap();
    assert_eq!(listed(&l, "stand-in")["shared"], true);
    // installing again is refused: it is there already, nothing is downloaded or copied twice
    let e = s.execute("denoise.models.install", &json!({"path": onnx.to_string_lossy(), "acknowledged": true})).unwrap_err().to_string();
    assert!(e.contains("already installed"), "{e}");
    let e = s.execute("denoise.models.remove", &json!({"id": "stand-in"})).unwrap_err().to_string();
    assert!(e.contains("everyApp"), "{e}");
    assert!(d.join("shared").join("stand-in").join("model.onnx").is_file());
    s.execute("denoise.models.remove", &json!({"id": "stand-in", "everyApp": true})).unwrap();
    assert!(!d.join("shared").join("stand-in").exists());

    // a model in the old folder only is found, chosen and removed without everyApp
    let old_home = d.join("old").join("stand-in");
    std::fs::create_dir_all(&old_home).unwrap();
    std::fs::copy(onnx.with_file_name("denoise-model.json"), old_home.join("denoise-model.json")).unwrap();
    std::fs::copy(&onnx, old_home.join("model.onnx")).unwrap();
    let l = s.execute("denoise.models.list", &json!({})).unwrap();
    assert_eq!((listed(&l, "stand-in")["installed"].clone(), listed(&l, "stand-in")["shared"].clone()), (json!(true), json!(false)));
    s.execute("denoise.models.select", &json!({"id": "stand-in"})).unwrap();
    s.execute("denoise.models.remove", &json!({"id": "stand-in"})).unwrap();
    assert!(!old_home.exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_session_built_with_the_two_face_folders_finds_a_model_that_is_only_in_the_shared_one() {
    let d = temp("faces-built");
    let f = d.join("Mine.onnx");
    std::fs::write(&f, lightcraft_faces::synthetic::tiny_embedder_model(512)).unwrap();
    let id =
        face_session(&d).execute("faces.models.install", &json!({"path": f.to_string_lossy(), "acknowledged": true})).unwrap()["installed"]["id"]
            .as_str()
            .unwrap()
            .to_string();
    assert!(!d.join("old").join(&id).exists(), "the model is only in the shared folder");
    // the way the desktop app, the CLI and MCP build it (`with_default_face_models` passes the default folders here)
    let mut s = Session::new().with_face_models(Some(d.join("old")), Some(d.join("shared")));
    let l = s.execute("faces.models.list", &json!({})).unwrap();
    assert_eq!(listed(&l, &id)["installed"], true);
    assert_eq!(s.execute("faces.models.test", &json!({"id": id})).unwrap()["ok"], true);
    // without the shared folder (what the desktop app did before) the model is "not installed"
    let mut old_only = Session::new().with_face_models(Some(d.join("old")), None);
    assert_eq!(listed(&old_only.execute("faces.models.list", &json!({})).unwrap(), &id)["installed"], Value::Null);
    let _ = std::fs::remove_dir_all(&d);
}
