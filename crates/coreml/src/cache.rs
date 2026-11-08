//! The compiled copy of a package, kept next to it and keyed by the package's manifest: a changed model compiles again,
//! and a cached copy that no longer loads compiles again once.

use std::path::{Path, PathBuf};

use crate::{ComputeUnits, Error, Model, Result, compile};

/// Largest manifest (or single `.mlmodel`) read for the key.
const MAX_KEY_FILE: u64 = 4 << 20;

/// FNV-1a 64: a stable cache key, not a security check (downloads are checked by SHA-256).
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// The key of `package`: its `Manifest.json` (coremltools writes new identifiers into it at every save, so a new
/// conversion has a new key), or the bytes of a single `.mlmodel` file.
pub(crate) fn package_key(package: &Path) -> Result<String> {
    let file = if package.is_dir() { package.join("Manifest.json") } else { package.to_path_buf() };
    let meta = std::fs::metadata(&file).map_err(|e| Error::Io(format!("{}: {e}", file.display())))?;
    if meta.len() > MAX_KEY_FILE {
        return Err(Error::Io(format!("{}: too large for a model manifest", file.display())));
    }
    let bytes = std::fs::read(&file).map_err(|e| Error::Io(format!("{}: {e}", file.display())))?;
    Ok(format!("{:016x}-{}", fnv(&bytes), bytes.len()))
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
    if !crate::available() {
        return Err(Error::Unavailable);
    }
    if !package.exists() {
        return if compiled.is_dir() { Model::load(compiled, units) } else { Err(Error::Missing(package.to_path_buf())) };
    }
    let key = package_key(package)?;
    let cached = compiled.is_dir() && std::fs::read_to_string(key_path(compiled)).is_ok_and(|k| k == key);
    if !cached {
        compile_keyed(package, compiled, &key)?;
        return Model::load(compiled, units);
    }
    match Model::load(compiled, units) {
        Err(Error::Load(why)) => {
            log::warn!("coreml: the compiled copy of {} does not load ({why}); compiling it again", package.display());
            compile_keyed(package, compiled, &key)?;
            Model::load(compiled, units)
        }
        other => other,
    }
}
