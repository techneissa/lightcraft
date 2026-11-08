//! Fork feature (`fork/learn-my-edits.md`): Learn My Edits through the commands, on procedural photos in a temporary
//! library, with features from the photo statistics only (the image model is not needed).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-learn-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A landscape-like PNG at brightness `light` (0..1).
fn png(dir: &Path, name: &str, light: f32, seed: usize) -> String {
    let (w, h) = (160usize, 120usize);
    let data: Vec<[u8; 4]> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let sky = y < h * 2 / 5;
            let blob = ((x as i64 - (seed * 37 % w) as i64).pow(2) + (y as i64 - (seed * 53 % h) as i64).pow(2)) < 400;
            let c = if blob {
                [0.8, 0.3, 0.2]
            } else if sky {
                [0.5, 0.65, 0.9]
            } else {
                [0.35, 0.5, 0.25]
            };
            let t = 0.85 + 0.3 * (((x * 7 + y * 13 + seed) % 11) as f32 / 11.0);
            let px = |v: f32| ((v * t * light * 1.5).clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
            [px(c[0]), px(c[1]), px(c[2]), 255]
        })
        .collect();
    let bytes = lightcraft_codecs::encode_png(
        &lightcraft_codecs::EncodeImage::rgba8(&lightcraft_raster::Rgba8 { width: w, height: h, data }),
        &Default::default(),
    )
    .unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

/// A session whose library holds `n` photos edited by the rule "darker photos got more Exposure", plus two
/// unedited photos (one dark, one bright) at the end. Learn My Edits uses `dir/profiles`.
fn session(tag: &str, n: usize) -> (Session, PathBuf, Vec<u64>) {
    let dir = temp_dir(tag);
    let mut paths: Vec<(String, f32)> = (0..n).map(|i| (format!("p{i}.png"), 0.15 + 0.75 * ((i * 37) % n) as f32 / n as f32)).collect();
    paths.push(("dark.png".into(), 0.2));
    paths.push(("bright.png".into(), 0.85));
    let files: Vec<String> = paths.iter().enumerate().map(|(i, (name, light))| png(&dir, name, *light, i)).collect();
    let mut s = Session::new().with_fs();
    s.learn.set_dirs(Some(dir.join("models")), Some(dir.join("profiles")));
    s.learn.set_statistics_only(true);
    s.execute("library.import", &json!({"paths": files})).unwrap();
    let mut ids: Vec<(String, u64)> = s.catalog.photos().map(|p| (p.file_name.clone(), p.id.0)).collect();
    ids.sort_by_key(|(name, _)| paths.iter().position(|(n, _)| n == name));
    let ids: Vec<u64> = ids.into_iter().map(|(_, id)| id).collect();
    for (i, (_, light)) in paths.iter().enumerate().take(n) {
        let exposure = (-(light / 0.5).log2()) as f64;
        s.execute("develop.set", &json!({"ids": [ids[i]], "values": {"light.exposure": exposure, "light.shadows": 25}})).unwrap();
    }
    (s, dir, ids)
}

fn history(s: &Session, id: u64) -> Vec<String> {
    s.catalog.photo(lightcraft_catalog::PhotoId(id)).unwrap().history.iter().map(|h| h.label.clone()).collect()
}

#[test]
fn a_profile_trained_in_the_library_applies_as_one_undo_step() {
    let (mut s, dir, ids) = session("apply", 16);
    let (edited, rest) = ids.split_at(16);
    let r =
        s.execute("learn.create", &json!({"name": "Evening Light", "kind": "Landscapes", "groups": ["tone"], "ids": edited, "wait": true})).unwrap();
    assert_eq!(r["photos"], 16, "{r}");
    let profile = &r["profile"];
    assert_eq!(profile["trained"], true, "{r}");
    assert_eq!(profile["learned"], json!(["tone"]));
    assert!(profile["report"]["beatsDefaults"].as_u64().unwrap() >= 13, "{r}");
    let id = r["id"].as_str().unwrap().to_string();
    assert_eq!(id, "evening-light");
    assert!(dir.join("profiles/evening-light.lclearn").is_file());

    // the two new photos: give them something outside Tone that must survive
    let (dark, bright) = (rest[0], rest[1]);
    s.execute("develop.set", &json!({"ids": [dark, bright], "values": {"mixer.blue.sat": -20, "color.vibrance": 10}})).unwrap();
    let before: Vec<lightcraft_develop::DevelopSettings> =
        [dark, bright].iter().map(|i| (*s.develop_of(lightcraft_catalog::PhotoId(*i)).unwrap()).clone()).collect();
    let undo_depth = s.undo.len();
    let r = s.execute("learn.apply", &json!({"profile": id, "ids": [dark, bright], "wait": true})).unwrap();
    assert_eq!(r["applied"], 2, "{r}");
    assert_eq!(s.undo.len(), undo_depth + 1, "one undo step for the whole apply");
    let after: Vec<lightcraft_develop::DevelopSettings> =
        [dark, bright].iter().map(|i| (*s.develop_of(lightcraft_catalog::PhotoId(*i)).unwrap()).clone()).collect();
    assert!(after[0].light.exposure > after[1].light.exposure + 0.8, "dark {} bright {}", after[0].light.exposure, after[1].light.exposure);
    assert!(after[0].light.exposure > 0.6, "{}", after[0].light.exposure);
    assert!((after[0].light.shadows - 25.0).abs() < 3.0, "{}", after[0].light.shadows);
    for (a, b) in after.iter().zip(&before) {
        let mut back = a.clone();
        back.light = b.light;
        assert_eq!(&back, b, "only the Tone sliders change");
    }
    for id in [dark, bright] {
        assert_eq!(history(&s, id).last().map(String::as_str), Some("Learned: Evening Light"));
        assert_eq!(history(&s, id).iter().filter(|l| l.starts_with("Learned")).count(), 1);
    }
    s.execute("edit.undo", &json!({})).unwrap();
    let undone: Vec<lightcraft_develop::DevelopSettings> =
        [dark, bright].iter().map(|i| (*s.develop_of(lightcraft_catalog::PhotoId(*i)).unwrap()).clone()).collect();
    assert_eq!(undone, before);

    // Add This Photo as Example: an adjusted photo goes back in and the profile retrains
    s.execute("develop.set", &json!({"ids": [dark], "values": {"light.exposure": 1.3}})).unwrap();
    let r = s.execute("learn.addExamples", &json!({"profile": id, "ids": [dark], "wait": true})).unwrap();
    assert_eq!(r["profile"]["examples"], 17, "{r}");
    // status lists it
    let st = s.execute("learn.status", &json!({})).unwrap();
    assert_eq!(st["profiles"][0]["name"], "Evening Light", "{st}");
    // export, import a copy, delete
    let out = dir.join("export.lclearn");
    s.execute("learn.export", &json!({"profile": id, "path": out.to_string_lossy()})).unwrap();
    let r = s.execute("learn.import", &json!({"path": out.to_string_lossy()})).unwrap();
    assert_eq!(r["profile"]["id"], "evening-light-2");
    s.execute("learn.delete", &json!({"profile": "evening-light-2"})).unwrap();
    assert_eq!(s.execute("learn.status", &json!({})).unwrap()["profiles"].as_array().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_job_shows_a_row_in_the_activity_stack_until_it_ends() {
    let (mut s, dir, ids) = session("row", 12);
    s.execute("learn.create", &json!({"name": "Rows", "groups": ["tone"], "ids": &ids[..12]})).unwrap();
    let rows = s.activity.list();
    assert!(rows.iter().any(|t| t.kind == "learn" && t.label.contains("Learning")), "{rows:?}");
    assert!(s.execute("learn.train", &json!({"profile": "rows"})).unwrap_err().to_string().contains("busy"));
    let mut done = Value::Null;
    for _ in 0..600 {
        done = s.execute("learn.pump", &json!({})).unwrap();
        if !done["trained"].is_null() || !done["error"].is_null() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(done["trained"], "rows", "{done}");
    assert!(!s.activity.list().iter().any(|t| t.kind == "learn"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn too_few_examples_and_a_missing_model_are_errors() {
    let (mut s, dir, ids) = session("few", 6);
    let e = s.execute("learn.create", &json!({"name": "Few", "groups": ["tone"], "ids": &ids[..6], "wait": true})).unwrap_err().to_string();
    assert!(e.contains("at least 10"), "{e}");
    // the profile keeps its examples and says why it has no model
    let st = s.execute("learn.status", &json!({})).unwrap();
    assert!(st["profiles"][0]["problem"].as_str().unwrap().contains("at least 10"), "{st}");
    assert!(s.execute("learn.apply", &json!({"profile": "few", "ids": [ids[6]]})).unwrap_err().to_string().contains("no model"));
    // unedited photos only: nothing to learn from
    let e = s.execute("learn.create", &json!({"name": "None", "ids": [ids[6], ids[7]]})).unwrap_err().to_string();
    assert!(e.contains("no edits"), "{e}");
    // without the image model, training and applying say so
    s.learn.set_statistics_only(false);
    let e = s.execute("learn.create", &json!({"name": "Model", "ids": &ids[..6]})).unwrap_err().to_string();
    assert!(e.contains("isn't installed"), "{e}");
    assert!(s.execute("learn.downloadModel", &json!({})).unwrap_err().to_string().contains("ask the user first"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_damaged_profile_file_is_an_error_not_a_panic() {
    let (mut s, dir, ids) = session("damaged", 2);
    let profiles = dir.join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(profiles.join("broken.lclearn"), b"{\"format\": \"lightcraft-learn-profile\", \"version\": 1, \"name\": ").unwrap();
    std::fs::write(profiles.join("huge.lclearn"), vec![b'['; 1 << 16]).unwrap();
    let st = s.execute("learn.status", &json!({})).unwrap();
    let listed = st["profiles"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|p| p["error"].is_string()), "{st}");
    assert!(s.execute("learn.apply", &json!({"profile": "broken", "ids": [ids[0]]})).is_err());
    assert!(s.execute("learn.train", &json!({"profile": "huge"})).is_err());
    assert!(s.execute("learn.import", &json!({"path": profiles.join("broken.lclearn").to_string_lossy()})).is_err());
    assert!(s.execute("learn.apply", &json!({"profile": "../../etc/passwd", "ids": [ids[0]]})).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_wait_the_next_learn_command_takes_the_job_in() {
    // no frame loop here (as over MCP or the CLI): `learn.status` itself writes the apply's sliders
    let (mut s, dir, ids) = session("nowait", 12);
    s.execute("learn.create", &json!({"name": "No Wait", "groups": ["tone"], "ids": &ids[..12], "wait": true})).unwrap();
    let dark = lightcraft_catalog::PhotoId(ids[12]);
    let r = s.execute("learn.apply", &json!({"profile": "no-wait", "ids": [ids[12]]})).unwrap();
    assert_eq!(r["started"], true, "{r}");
    let mut st = Value::Null;
    for _ in 0..600 {
        st = s.execute("learn.status", &json!({})).unwrap();
        if st["job"].is_null() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(st["job"].is_null(), "still busy: {st}");
    assert!(s.develop_of(dark).unwrap().light.exposure > 0.5, "the sliders were written");
    assert_eq!(history(&s, ids[12]).last().map(String::as_str), Some("Learned: No Wait"));
    // and a later job command is not refused as busy
    s.execute("learn.train", &json!({"profile": "no-wait", "wait": true})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_new_profile_while_a_job_runs_is_refused_and_leaves_no_file() {
    let (mut s, dir, ids) = session("busy", 12);
    s.learn.hold_jobs(true);
    s.execute("learn.create", &json!({"name": "First", "groups": ["tone"], "ids": &ids[..12]})).unwrap();
    let e = s.execute("learn.create", &json!({"name": "Second", "groups": ["tone"], "ids": &ids[..12]})).unwrap_err().to_string();
    assert!(e.contains("busy"), "{e}");
    assert!(!dir.join("profiles/second.lclearn").exists(), "no empty profile left behind");
    s.learn.hold_jobs(false);
    let _ = std::fs::remove_dir_all(dir);
}
