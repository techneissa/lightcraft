//! The depth model: where it lives and where it downloads from. The downloader itself (resuming, checks, the
//! pure-Rust HTTPS client) is the `lightcraft-fetch` crate.
//!
//! The weights are the Apache-2.0 safetensors file of `depth-anything/DA3MONO-LARGE` on Hugging Face, pinned by
//! size and SHA-256 at revision `f465978`.

pub use lightcraft_fetch::*;

use std::path::{Path, PathBuf};

/// The folder name under `<models>/`.
pub const DIR_NAME: &str = "da3-mono-large";
/// What the user is told this model is.
pub const LABEL: &str = "Depth Anything 3 Mono Large";
/// The default download location.
pub const MIRROR: &str = "https://huggingface.co/depth-anything/DA3MONO-LARGE/resolve/f465978e618db8cc79c83b8bbf24964857db1875";
/// Environment variable with extra mirrors (base URLs separated by commas, spaces or newlines), tried before the
/// default.
pub const MIRRORS_ENV: &str = "LIGHTCRAFT_DEPTH_MIRRORS";

const SIZE: u64 = 1_336_734_448;

/// The file to download.
pub static FILES: &[FileSpec<'static>] = &[FileSpec {
    name: crate::WEIGHTS_FILE,
    size: Some(SIZE),
    sha256: Some("7a799a7f95eb8d4c404c2ca8be3dc3276b350a417ddc4420db72ba850cc0e960"),
    max: SIZE,
}];

/// Bytes to download.
pub fn size() -> u64 {
    FILES.iter().filter_map(|f| f.size).sum()
}

/// The model's folder under the model folder `models`.
pub fn dir_in(models: &Path) -> PathBuf {
    models.join(DIR_NAME)
}

/// The mirrors to try, in order: the environment variable's, then the default.
pub fn mirrors(env: Option<&str>) -> Vec<String> {
    lightcraft_fetch::mirrors(env, None, &[MIRROR])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_is_one_pinned_file() {
        assert_eq!(FILES.len(), 1);
        assert_eq!(FILES[0].sha256.map(str::len), Some(64));
        let m = mirrors(Some("https://a.example/x/"));
        assert_eq!(m.len(), 2);
        assert!(m[0].starts_with("https://a.example") && m[1].starts_with("https://huggingface.co/depth-anything/"));
    }
}
