//! Loading and running the model sets. Without models only the error paths run; with `LIGHTCRAFT_PARSING_MODELS`
//! naming a models folder that holds a set, the parse runs on synthetic photos (no people, a 1 × 1 photo, 50 faces,
//! a face cut by the edge).

use std::path::PathBuf;

use crate::models::{FACE_MESH, ModelSet};
use crate::{Error, FaceHint, NormRect, Options, Parser, Part, RgbImage, best_device};

/// The models folder the app downloads into (shared with PhotoCraft), read only; `LIGHTCRAFT_PARSING_MODELS` names
/// another. Tests never download a model.
fn models_dir() -> Option<PathBuf> {
    match std::env::var_os("LIGHTCRAFT_PARSING_MODELS").filter(|v| !v.is_empty()) {
        Some(d) => Some(PathBuf::from(d)),
        None => std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/ArtCraft/models")),
    }
}

fn models() -> Option<(PathBuf, ModelSet)> {
    let dir = models_dir()?;
    ModelSet::ALL.into_iter().find(|s| s.installed_in(&dir)).map(|s| (dir, s))
}

#[test]
fn a_missing_model_is_an_error_that_names_it() {
    let dir = super::scratch("missing");
    for set in ModelSet::ALL {
        match Parser::load_on(set, &dir, candle_core::Device::Cpu) {
            Err(Error::Missing(what)) => assert!(what.contains(set.face_file().label), "{what}"),
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("loaded from an empty folder"),
        }
    }
}

#[test]
fn a_damaged_model_file_is_an_error_not_a_panic() {
    let dir = super::scratch("damaged");
    let set = ModelSet::BiSeNet;
    for f in set.files() {
        std::fs::create_dir_all(dir.join(f.folder)).unwrap();
        std::fs::write(f.path_in(&dir), b"this is not a model").unwrap();
    }
    let e = Parser::load_on(set, &dir, candle_core::Device::Cpu).err().unwrap();
    assert!(matches!(e, Error::Model(_)), "{e}");
    // a well-formed file of the wrong model
    let header = r#"{"x":{"dtype":"F32","shape":[1],"data_offsets":[0,4]}}"#;
    for f in set.files() {
        crate::weights::tests::write(&dir.join(f.folder), f.name, header, &[0u8; 4]);
    }
    let e = Parser::load_on(set, &dir, candle_core::Device::Cpu).err().unwrap();
    assert!(e.to_string().contains(set.face_file().label), "{e}");
    // a face mesh whose program is nonsense
    let mesh =
        r#"{"__metadata__":{"program":"{\"inputs\":[\"a\"],\"outputs\":[],\"nodes\":[{\"op\":\"conv2d\",\"out\":\"b\",\"inputs\":[\"zz\"]}]}"}}"#;
    crate::weights::tests::write(&dir.join(FACE_MESH.folder), FACE_MESH.name, mesh, &[]);
    let w = crate::weights::Weights::open(&FACE_MESH.path_in(&dir)).unwrap();
    assert!(crate::facemesh::FaceMesh::load(&w, &candle_core::Device::Cpu).is_err());
}

#[test]
fn a_model_file_of_the_right_size_with_the_wrong_bytes_is_refused() {
    let dir = super::scratch("wrong-bytes");
    let set = ModelSet::Segformer;
    for f in set.files() {
        std::fs::create_dir_all(dir.join(f.folder)).unwrap();
        std::fs::File::create(f.path_in(&dir)).unwrap().set_len(f.size).unwrap();
    }
    let e = Parser::load_on(set, &dir, candle_core::Device::Cpu).err().unwrap();
    assert!(e.to_string().contains("SHA-256") && e.to_string().contains(set.face_file().label), "{e}");
}

/// A smooth synthetic picture with no people.
fn landscape(w: usize, h: usize) -> RgbImage {
    let mut d = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            d.extend_from_slice(&[(60 + y * 120 / h) as u8, (140 + x * 60 / w) as u8, 200u8]);
        }
    }
    RgbImage::new(w, h, d).unwrap()
}

#[test]
fn odd_photos_parse_without_panicking() {
    let Some((dir, set)) = models() else {
        lightcraft_testkit::skip!(Model, "no people parsing model set in the shared models folder (or LIGHTCRAFT_PARSING_MODELS)");
        return;
    };
    let parser = Parser::load_on(set, &dir, best_device()).unwrap();
    let opts = Options::default();
    let never = || false;
    // a 1 × 1 photo
    let one = parser.parse(&RgbImage::new(1, 1, vec![9, 9, 9]).unwrap(), &[], &opts, &never).unwrap();
    assert!(one.people.is_empty());
    // no people
    let sky = parser.parse(&landscape(320, 200), &[], &opts, &never).unwrap();
    assert!(sky.people.iter().all(|p| p.area(Part::EntirePerson) < 0.02), "{:?}", crate::parser::summary(&sky));
    // 50 faces given on a picture with none in it, some cut by the edge or outside it
    let hints: Vec<FaceHint> = (0..50)
        .map(|i| {
            let (x, y) = ((i % 10) as f32 * 0.1 - 0.03, (i / 10) as f32 * 0.2 + 0.02);
            FaceHint { rect: NormRect::new(x, y, x + 0.08, y + 0.12), landmarks: None, score: 0.8, person_id: None }
        })
        .chain([FaceHint { rect: NormRect::new(1.5, 1.5, 2.0, 2.0), landmarks: None, score: 1.0, person_id: None }])
        .collect();
    let many = parser.parse(&landscape(640, 480), &hints, &opts, &never).unwrap();
    assert!(many.people.len() <= 50);
    assert!(many.people.iter().all(|p| p.face.is_some_and(|f| f.x0 >= 0.0 && f.x1 <= 1.0)));
    // cancelled
    let e = parser.parse(&landscape(64, 64), &[], &opts, &|| true).err().unwrap();
    assert!(matches!(e, Error::Cancelled));
}

#[test]
fn sapiens2_finds_no_person_in_a_landscape_or_a_one_pixel_photo() {
    let Some(dir) = models_dir().filter(|d| ModelSet::Sapiens2.installed_in(d)) else {
        lightcraft_testkit::skip!(Model, "the Sapiens2 set is not in the shared models folder (or LIGHTCRAFT_PARSING_MODELS)");
        return;
    };
    let parser = Parser::load_on(ModelSet::Sapiens2, &dir, best_device()).unwrap();
    let opts = Options::default();
    let sky = parser.parse(&landscape(320, 200), &[], &opts, &|| false).unwrap();
    assert!(sky.people.iter().all(|p| p.area(Part::EntirePerson) < 0.02), "{:?}", crate::parser::summary(&sky));
    assert_eq!(sky.model_set, "sapiens2");
    let one = parser.parse(&RgbImage::new(1, 1, vec![9, 9, 9]).unwrap(), &[], &opts, &|| false).unwrap();
    assert!(one.people.is_empty());
}
