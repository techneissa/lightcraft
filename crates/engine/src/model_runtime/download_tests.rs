//! The Core ML download against a local HTTP server and a tiny zip made here; never the internet or a real model.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use sha2::{Digest, Sha256};

use super::{Release, install};
use crate::lama_fill::Download;

const PACKAGE: &str = "tiny.mlpackage";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lightcraft-coreml-download-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A stored zip of `entries` (name, bytes).
fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    let mut z = zip::ZipWriter::new(&mut out);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in entries {
        z.start_file(*name, opts).unwrap();
        z.write_all(bytes).unwrap();
    }
    z.finish().unwrap();
    out.into_inner()
}

/// Serves `body` to every request; returns the URL.
fn serve(body: Vec<u8>) -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/models-coreml-1/tiny.zip", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut c in l.incoming().flatten() {
            let mut r = BufReader::new(c.try_clone().unwrap());
            let mut line = String::new();
            while r.read_line(&mut line).unwrap_or(0) > 2 {
                line.clear();
            }
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = c.write_all(head.as_bytes());
            let _ = c.write_all(&body);
        }
    });
    url
}

fn fetch(zip: &[u8], sha: Option<String>, dir: &Path) -> Result<(), Download> {
    let sha = sha.unwrap_or_else(|| format!("{:x}", Sha256::digest(zip)));
    let r = Release { model: "tiny", file: "tiny.zip", url: "", bytes: zip.len() as u64, sha256: &sha, package: PACKAGE, license: "MIT" };
    let url = serve(zip.to_vec());
    install(&r, &url, dir, &AtomicBool::new(false), &mut |_| {})
}

#[test]
fn a_checked_zip_unpacks_into_the_model_folder() {
    let dir = scratch("good");
    let zip = zip_of(&[
        ("tiny.mlpackage/Manifest.json", b"{}"),
        ("tiny.mlpackage/Data/com.apple.CoreML/model.mlmodel", b"spec"),
        ("tiny.mlpackage/Data/com.apple.CoreML/weights/weight.bin", &[7u8; 300]),
    ]);
    std::fs::create_dir_all(dir.join(PACKAGE)).unwrap();
    std::fs::write(dir.join(PACKAGE).join("old.bin"), b"older package").unwrap();
    fetch(&zip, None, &dir).unwrap();
    let p = dir.join(PACKAGE);
    assert_eq!(std::fs::read(p.join("Manifest.json")).unwrap(), b"{}");
    assert_eq!(std::fs::read(p.join("Data/com.apple.CoreML/weights/weight.bin")).unwrap().len(), 300);
    assert!(!p.join("old.bin").exists(), "the older package is replaced whole");
    assert!(!dir.join("tiny.zip").exists() && !dir.join("tiny.mlpackage.part").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_wrong_hash_installs_nothing() {
    let dir = scratch("hash");
    let zip = zip_of(&[("tiny.mlpackage/Manifest.json", b"{}")]);
    let r = fetch(&zip, Some("0".repeat(64)), &dir);
    assert!(matches!(r, Err(Download::Failed(_))), "{r:?}");
    assert!(!dir.join(PACKAGE).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_entry_outside_the_package_installs_nothing() {
    let dir = scratch("outside");
    for bad in ["../evil.txt", "other.mlpackage/Manifest.json"] {
        let zip = zip_of(&[("tiny.mlpackage/Manifest.json", b"{}"), (bad, b"x")]);
        let r = fetch(&zip, None, &dir);
        assert!(matches!(&r, Err(Download::Failed(m)) if m.contains("outside")), "{bad}: {r:?}");
        assert!(!dir.join(PACKAGE).exists() && !dir.join("tiny.mlpackage.part").exists());
        assert!(!dir.parent().unwrap().join("evil.txt").exists());
    }
    let r = fetch(&zip_of(&[("tiny.mlpackage/readme.txt", b"no manifest")]), None, &dir);
    assert!(matches!(&r, Err(Download::Failed(_))), "{r:?}");
    assert!(!dir.join(PACKAGE).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_published_model_is_dinov2_under_its_licence() {
    let r = super::release(lightcraft_learn::ENCODER_ID).unwrap();
    assert!(r.url.starts_with("https://github.com/techneissa/lightcraft/releases/download/models-coreml-1/"));
    assert!(r.url.ends_with(r.file) && r.sha256.len() == 64 && r.bytes < 2 << 30);
    assert_eq!((r.package, r.license), ("dinov2-small.mlpackage", "Apache-2.0"));
}
