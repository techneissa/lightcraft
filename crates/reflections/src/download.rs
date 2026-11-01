//! Fetching and checking the DIRS-MuGI and DIRS-PAIR files, only when the user asked for it. The files are the ones
//! the authors publish (huggingface.co/huqiming513/DIRS-Models); they are used as they are, with sizes and
//! SHA-256 pinned here from Hugging Face's own file records. The downloader is `lightcraft-fetch`
//! (resume, stall timeouts, cancellation, the file is renamed into place only when it checks out).
use crate::{DIRS_PAIR, MUGI_CHECKPOINT, PAIR_CHECKPOINT, SWIN_PRIOR};
use lightcraft_fetch::{FileSpec, Options, download_url};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub const MUGI_PAGE: &str = "https://huggingface.co/huqiming513/DIRS-Models";
const BASE: &str = "https://huggingface.co/huqiming513/DIRS-Models/resolve/main/checkpoints";

struct Pinned {
    name: &'static str,
    url_path: &'static str,
    size: u64,
    sha256: &'static str,
}
const SWIN_FILE: Pinned = Pinned {
    name: SWIN_PRIOR,
    url_path: "pretrained/swin_large_o365.pth",
    size: 784_900_594,
    sha256: "4f39390961a566201eb2b691f4608644abdeaf0b4fc318f9f386d44dd0cbbaf2",
};
const MUGI_FILES: [Pinned; 2] = [
    Pinned {
        name: MUGI_CHECKPOINT,
        url_path: "dirs_mugi_lors.ckpt",
        size: 345_268_909,
        sha256: "2a67bb57231ca7d57c2b23bc2d6ecd6fd77177098e7462e129c7cb93177fe366",
    },
    SWIN_FILE,
];
const PAIR_FILES: [Pinned; 2] = [
    Pinned {
        name: PAIR_CHECKPOINT,
        url_path: "dirs_pair_lors.ckpt",
        size: 561_054_629,
        sha256: "538f89b823e19913ea561e17f9ec3f7cc85e81f79fac3ba36f5dd3f4f8fe5c99",
    },
    SWIN_FILE,
];

/// The two published files of a DIRS model (`dirs-mugi` or `dirs-pair`).
fn files(model: &str) -> &'static [Pinned; 2] {
    if model == DIRS_PAIR { &PAIR_FILES } else { &MUGI_FILES }
}

/// Total bytes of both files of `model`.
pub fn dirs_bytes(model: &str) -> u64 {
    files(model).iter().map(|f| f.size).sum()
}

/// Downloads both files of `model` into `dir` (created when missing; an interrupted download resumes).
/// `progress(done, total)` counts the bytes of both files.
pub fn download_dirs(model: &str, dir: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
    let total = dirs_bytes(model);
    let mut before = 0;
    for f in files(model) {
        let spec = FileSpec { name: f.name, size: Some(f.size), sha256: Some(f.sha256), max: f.size };
        let url = format!("{BASE}/{}", f.url_path);
        download_url(&spec, &url, dir, &Options::default(), cancel, &mut |p| progress((before + p.done).min(total), total))
            .map_err(|e| e.to_string())?;
        before += f.size;
    }
    Ok(())
}

/// Whether the two files in `dir` are exactly the published ones (size and SHA-256).
pub fn verify_dirs(model: &str, dir: &Path) -> Result<(), String> {
    for f in files(model) {
        let path = dir.join(f.name);
        let file = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", f.name))?;
        if file.metadata().map_err(|e| e.to_string())?.len() != f.size {
            return Err(format!("{} is not the published file (wrong size)", f.name));
        }
        let mut hash = Sha256::new();
        let mut reader = file.take(f.size);
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("{}: {e}", f.name))?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        if format!("{:x}", hash.finalize()) != f.sha256 {
            return Err(format!("{} is not the published file (checksum differs)", f.name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrong_sized_and_missing_files_fail_the_check() {
        let dir = std::env::temp_dir().join(format!("lc-dirs-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(verify_dirs("dirs-mugi", &dir).is_err());
        std::fs::write(dir.join(MUGI_CHECKPOINT), b"not the checkpoint").unwrap();
        assert!(verify_dirs("dirs-mugi", &dir).unwrap_err().contains("wrong size"));
        assert!(verify_dirs("dirs-pair", &dir).is_err());
        std::fs::write(dir.join(PAIR_CHECKPOINT), b"not the checkpoint").unwrap();
        assert!(verify_dirs("dirs-pair", &dir).unwrap_err().contains("wrong size"));
        assert_eq!(dirs_bytes("dirs-mugi"), 345_268_909 + 784_900_594);
        assert_eq!(dirs_bytes("dirs-pair"), 561_054_629 + 784_900_594);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn a_cancelled_download_stops_before_fetching() {
        let dir = std::env::temp_dir().join(format!("lc-dirs-cancel-{}", std::process::id()));
        let cancel = AtomicBool::new(true);
        let err = download_dirs("dirs-mugi", &dir, &cancel, &mut |_, _| {}).unwrap_err();
        assert!(err.contains("cancelled"), "{err}");
        assert!(!dir.join(MUGI_CHECKPOINT).exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
