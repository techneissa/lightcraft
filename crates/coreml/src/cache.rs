//! The compiled copy of a package, kept next to it and keyed by the package's manifest and files: a changed model
//! compiles again, and a cached copy that no longer loads compiles again once.

use std::path::{Path, PathBuf};

use crate::{ComputeUnits, Error, Model, Result, compile};

/// Largest manifest (or single `.mlmodel`) read for the key.
const MAX_KEY_FILE: u64 = 4 << 20;

/// FNV-1a 64: a stable cache key, not a security check (downloads are checked by SHA-256).
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Most files of a package that go into its key, and how deep its folders are walked.
const MAX_KEY_ENTRIES: usize = 256;
const MAX_KEY_DEPTH: usize = 8;

/// The key of `package`: its `Manifest.json` (coremltools writes new identifiers into it at every save, so a new
/// conversion has a new key) and the path, size and time of each of its files (the model and its weights, so a file
/// replaced inside the package compiles again too), or the bytes of a single `.mlmodel` file.
pub(crate) fn package_key(package: &Path) -> Result<String> {
    let file = if package.is_dir() { package.join("Manifest.json") } else { package.to_path_buf() };
    let meta = std::fs::metadata(&file).map_err(|e| Error::Io(format!("{}: {e}", file.display())))?;
    if meta.len() > MAX_KEY_FILE {
        return Err(Error::Io(format!("{}: too large for a model manifest", file.display())));
    }
    let mut bytes = std::fs::read(&file).map_err(|e| Error::Io(format!("{}: {e}", file.display())))?;
    let len = bytes.len();
    if package.is_dir() {
        let mut files = Vec::new();
        walk(package, Path::new(""), 0, &mut files)?;
        files.sort();
        for f in files {
            bytes.extend_from_slice(f.as_bytes());
            bytes.push(0);
        }
    }
    Ok(format!("{:016x}-{len}", fnv(&bytes)))
}

/// Each file under `dir` (`rel` inside the package) as `path size time`, sorted later.
fn walk(dir: &Path, rel: &Path, depth: usize, out: &mut Vec<String>) -> Result<()> {
    let io = |e: std::io::Error| Error::Io(format!("{}: {e}", dir.display()));
    if depth > MAX_KEY_DEPTH {
        return Err(Error::Io(format!("{}: folders too deep for a model package", dir.display())));
    }
    for entry in std::fs::read_dir(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        let path = entry.path();
        let name = rel.join(entry.file_name());
        let meta = std::fs::metadata(&path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
        if meta.is_dir() {
            walk(&path, &name, depth + 1, out)?;
        } else {
            let time = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            out.push(format!("{} {} {time}", name.display(), meta.len()));
        }
        if out.len() > MAX_KEY_ENTRIES {
            return Err(Error::Io(format!("{}: too many files for a model package", dir.display())));
        }
    }
    Ok(())
}

/// Where the key of the compiled copy `compiled` is kept: `<compiled>.key`.
pub(crate) fn key_path(compiled: &Path) -> PathBuf {
    let mut p = compiled.as_os_str().to_owned();
    p.push(".key");
    PathBuf::from(p)
}

fn compile_keyed(package: &Path, compiled: &Path, key: &str) -> Result<()> {
    let _ = std::fs::remove_file(key_path(compiled));
    compile(package, compiled)?;
    std::fs::write(key_path(compiled), key).map_err(|e| Error::Io(format!("{}: {e}", key_path(compiled).display())))
}

/// Load `package` through its compiled copy at `compiled`: compiled first when the copy is missing or was made from
/// another package, and compiled again once when a cached copy fails to load. Without the package, the compiled copy
/// alone is loaded.
pub fn load_package(package: &Path, compiled: &Path, units: ComputeUnits) -> Result<Model> {
    load_package_function(package, compiled, units, None)
}

/// [`load_package`] for `function` of a model with several ([`Model::load_function`]).
pub fn load_package_function(package: &Path, compiled: &Path, units: ComputeUnits, function: Option<&str>) -> Result<Model> {
    if !crate::available() {
        return Err(Error::Unavailable);
    }
    let load = || Model::load_function(compiled, units, function);
    if !package.exists() {
        return if compiled.is_dir() { load() } else { Err(Error::Missing(package.to_path_buf())) };
    }
    let key = package_key(package)?;
    let cached = compiled.is_dir() && std::fs::read_to_string(key_path(compiled)).is_ok_and(|k| k == key);
    if !cached {
        compile_keyed(package, compiled, &key)?;
        return load();
    }
    match load() {
        Err(Error::Load(why)) => {
            log::warn!("coreml: the compiled copy of {} does not load ({why}); compiling it again", package.display());
            compile_keyed(package, compiled, &key)?;
            load()
        }
        other => other,
    }
}
