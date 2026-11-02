//! The BiRefNet models: which exist, where they live, where they download from. The downloader
//! itself (resuming, checks, the pure-Rust HTTPS client) is the `lightcraft-fetch` crate.
//!
//! The weights are the MIT-licensed safetensors files of `ZhengPeng7/BiRefNet*` on Hugging Face,
//! pinned by size and SHA-256 at the revision named in each [`Variant`].

pub use lightcraft_fetch::*;

use std::path::{Path, PathBuf};

/// Which BiRefNet checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Variant {
    /// `BiRefNet`: the general model (1024 × 1024 input, 444 MB).
    General,
    /// `BiRefNet-matting`: trained for soft alpha mattes, hair and fur (1024 × 1024, 885 MB).
    Matting,
}

impl Variant {
    pub const ALL: [Variant; 2] = [Variant::General, Variant::Matting];

    /// The folder name under `<models>/`.
    pub fn dir_name(self) -> &'static str {
        match self {
            Variant::General => "birefnet",
            Variant::Matting => "birefnet-matting",
        }
    }

    /// Side of the square the photo is stretched to.
    pub fn input(self) -> usize {
        1024
    }

    /// What the user is told this model is.
    pub fn label(self) -> &'static str {
        match self {
            Variant::General => "BiRefNet",
            Variant::Matting => "BiRefNet Matting",
        }
    }

    /// The file to download.
    pub fn files(self) -> &'static [FileSpec<'static>] {
        match self {
            Variant::General => GENERAL_FILES,
            Variant::Matting => MATTING_FILES,
        }
    }

    pub fn size(self) -> u64 {
        self.files().iter().filter_map(|f| f.size).sum()
    }

    /// The default download location of this model.
    pub fn mirror(self) -> &'static str {
        match self {
            Variant::General => "https://huggingface.co/ZhengPeng7/BiRefNet/resolve/e2bf8e4460fc8fa32bba5ea4d94b3233d367b0e4",
            Variant::Matting => "https://huggingface.co/ZhengPeng7/BiRefNet-matting/resolve/eccde0a8cbdce7ac5fecfeb06340fe7b949e85d9",
        }
    }

    /// Where `dir_name` is under the model folder `models`.
    pub fn dir_in(self, models: &Path) -> PathBuf {
        models.join(self.dir_name())
    }
}

const GENERAL_SIZE: u64 = 444_473_596;
const MATTING_SIZE: u64 = 884_878_856;

static GENERAL_FILES: &[FileSpec<'static>] = &[FileSpec {
    name: crate::WEIGHTS_FILE,
    size: Some(GENERAL_SIZE),
    sha256: Some("9ab37426bf4de0567af6b5d21b16151357149139362e6e8992021b8ce356a154"),
    max: GENERAL_SIZE,
}];

static MATTING_FILES: &[FileSpec<'static>] = &[FileSpec {
    name: crate::WEIGHTS_FILE,
    size: Some(MATTING_SIZE),
    sha256: Some("a9875de5b1e6c8eb5fdaa8c727a82927ce442cdc87ba3abee6a77e6fa46c25bb"),
    max: MATTING_SIZE,
}];

/// Environment variable with extra mirrors (base URLs separated by commas, spaces or newlines),
/// tried before the default.
pub const MIRRORS_ENV: &str = "LIGHTCRAFT_BIREFNET_MIRRORS";

/// The mirrors to try for `variant`, in order: the environment variable's, then the default.
pub fn mirrors(variant: Variant, env: Option<&str>) -> Vec<String> {
    lightcraft_fetch::mirrors(env, None, &[variant.mirror()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_names_one_pinned_file() {
        for v in Variant::ALL {
            let f = v.files();
            assert_eq!(f.len(), 1);
            assert_eq!(f[0].sha256.map(str::len), Some(64), "{v:?}");
            assert_eq!(f[0].size, Some(v.size()));
            assert!(v.dir_name().starts_with("birefnet"));
            assert!(v.mirror().starts_with("https://huggingface.co/ZhengPeng7/"));
        }
    }

    #[test]
    fn the_environment_mirror_comes_first() {
        let m = mirrors(Variant::General, Some("https://a.example/x/"));
        assert_eq!(m.len(), 2);
        assert!(m[0].starts_with("https://a.example"));
    }
}
