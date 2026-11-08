//! The model sets a parse can use, their files, and where those come from.
//!
//! Each model lives in its own folder under the models folder (on the Mac the one shared with the other ArtCraft apps,
//! `~/Library/Application Support/ArtCraft/models/<folder>/`), one file each, pinned by size and SHA-256 at a fixed
//! Hugging Face revision. Sets share files: the face mesh is in both.

use std::path::{Path, PathBuf};

/// One model file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    /// What the user is told it is.
    pub label: &'static str,
    /// The folder under the models folder.
    pub folder: &'static str,
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// The file at a pinned revision.
    pub url: &'static str,
    /// Its licence, as the source states it.
    pub licence: &'static str,
}

impl ModelFile {
    /// Where the file is under `models`.
    pub fn path_in(&self, models: &Path) -> PathBuf {
        models.join(self.folder).join(self.name)
    }

    /// Check the file's SHA-256 against the published one, once per file and process (again when the file's size or
    /// time changes): a file copied in by hand, or damaged on disk, is an error that names it, never a wrong model.
    pub fn verify(&self, models: &Path) -> crate::Result<()> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        static VERIFIED: std::sync::Mutex<Vec<(PathBuf, u64, Option<std::time::SystemTime>)>> = std::sync::Mutex::new(Vec::new());
        let path = self.path_in(models);
        let Ok(meta) = std::fs::metadata(&path) else { return Ok(()) };
        let stamp = (path.clone(), meta.len(), meta.modified().ok());
        if VERIFIED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(&stamp) {
            return Ok(());
        }
        let bad = |what: String| crate::Error::Model(format!("{} ({}): {what}; download it again", self.label, path.display()));
        if meta.len() != self.size {
            return Err(bad(format!("{} bytes, not {}", meta.len(), self.size)));
        }
        let mut f = std::fs::File::open(&path).map_err(|e| bad(e.to_string()))?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf).map_err(|e| bad(e.to_string()))?;
            if n == 0 {
                break;
            }
            h.update(buf.get(..n).unwrap_or(&[]));
        }
        let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if got != self.sha256 {
            return Err(bad("its SHA-256 does not match the published one".into()));
        }
        VERIFIED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(stamp);
        Ok(())
    }

    /// Whether the file is there with its published size (its hash is checked when downloaded).
    pub fn installed_in(&self, models: &Path) -> bool {
        std::fs::metadata(self.path_in(models)).is_ok_and(|m| m.is_file() && m.len() == self.size)
    }
}

pub const FACE_SEGFORMER_B0: ModelFile = ModelFile {
    label: "SegFormer-B0 face parser",
    folder: "face-parsing-segformer-b0",
    name: "model.safetensors",
    size: 14_902_364,
    sha256: "420a3e21e4ee9eea8e912c06ef3b9ca683c67570c1908aa837915751666665de",
    url: "https://huggingface.co/alexkrz/segformer-b0-celebamask_hq/resolve/e1e6109eadcedd497b771403db4c0cc88fbdd863/model.safetensors",
    licence: "other (fine-tuned from NVIDIA MiT-b0, NVIDIA Source Code Licence: non-commercial; CelebAMask-HQ: non-commercial research)",
};

pub const FACE_BISENET: ModelFile = ModelFile {
    label: "BiSeNet face parser",
    folder: "face-parsing-bisenet",
    name: "model.safetensors",
    size: 26_641_965,
    sha256: "2a8f088ca96b754b920278c87d6460bdabef8c74a161adbb82fd6b529d412e4a",
    url: "https://huggingface.co/mlx-community/face-parsing-bisenet-bf16/resolve/5f2aa216736b9a98eb366e56bcbad472548247f4/model.safetensors",
    licence: "MIT (zllrunning/face-parsing.PyTorch; trained on CelebAMask-HQ: non-commercial research)",
};

pub const BODY_SEGFORMER_B3: ModelFile = ModelFile {
    label: "SegFormer-B3 clothes parser",
    folder: "human-parsing-segformer-b3",
    name: "model.safetensors",
    size: 189_029_000,
    sha256: "f70ae566c5773fb335796ebaa8acc924ac25eb97222c2b2967d44d2fc11568e6",
    url: "https://huggingface.co/sayeed99/segformer_b3_clothes/resolve/6c12f0e4edd353fb65d4e3f9d90fdabaefea6d9e/model.safetensors",
    licence: "MIT (fine-tuned from NVIDIA MiT-b3, NVIDIA Source Code Licence: non-commercial)",
};

pub const BODY_SEGFORMER_B2: ModelFile = ModelFile {
    label: "SegFormer-B2 clothes parser",
    folder: "human-parsing-segformer-b2",
    name: "model.safetensors",
    size: 109_493_236,
    sha256: "8f86fd90c567afd4370b3cc3a7e81ed767a632b2832a738331af660acc0c4c68",
    url: "https://huggingface.co/mattmdjaga/segformer_b2_clothes/resolve/584abc1e1d260e23c0fc627c5217a09b2b461046/model.safetensors",
    licence: "MIT per the model card (fine-tuned from NVIDIA MiT-b2, NVIDIA Source Code Licence: non-commercial)",
};

pub const FACE_MESH: ModelFile = ModelFile {
    label: "MediaPipe face mesh v2",
    folder: "facemesh-v2",
    name: "face_landmarks_detector.safetensors",
    size: 4_997_792,
    sha256: "94e0dda4bb58733fc7454997868232bdcc2b11531a18d2bbfec5479c75f574f4",
    url: "https://huggingface.co/SceneWorks/mp-facemesh-v2/resolve/4e65de94fd007bde5eccd557bd83f067cbd3d596/face_landmarks_detector.safetensors",
    licence: "Apache-2.0 (Google MediaPipe)",
};

/// Which face parser a set uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceModel {
    /// 19 CelebAMask-HQ classes in the SegFormer fine-tune's order.
    Segformer,
    /// 19 CelebAMask-HQ classes in face-parsing.PyTorch's order.
    BiSeNet,
}

/// A set of models that parses together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelSet {
    /// SegFormer-B0 faces, SegFormer-B3 bodies, face mesh.
    Segformer,
    /// BiSeNet faces, SegFormer-B2 bodies, face mesh.
    BiSeNet,
}

impl ModelSet {
    pub const ALL: [ModelSet; 2] = [ModelSet::Segformer, ModelSet::BiSeNet];

    /// The id commands and settings use.
    pub fn id(self) -> &'static str {
        match self {
            ModelSet::Segformer => "segformer",
            ModelSet::BiSeNet => "bisenet",
        }
    }

    pub fn from_id(id: &str) -> Option<ModelSet> {
        ModelSet::ALL.into_iter().find(|s| s.id() == id)
    }

    /// What the user sees.
    pub fn label(self) -> &'static str {
        match self {
            ModelSet::Segformer => "SegFormer",
            ModelSet::BiSeNet => "BiSeNet",
        }
    }

    pub fn face_file(self) -> ModelFile {
        match self {
            ModelSet::Segformer => FACE_SEGFORMER_B0,
            ModelSet::BiSeNet => FACE_BISENET,
        }
    }

    pub fn face_model(self) -> FaceModel {
        match self {
            ModelSet::Segformer => FaceModel::Segformer,
            ModelSet::BiSeNet => FaceModel::BiSeNet,
        }
    }

    pub fn body_file(self) -> ModelFile {
        match self {
            ModelSet::Segformer => BODY_SEGFORMER_B3,
            ModelSet::BiSeNet => BODY_SEGFORMER_B2,
        }
    }

    /// Every file the set needs.
    pub fn files(self) -> [ModelFile; 3] {
        [self.face_file(), self.body_file(), FACE_MESH]
    }

    /// Bytes of all its files.
    pub fn size(self) -> u64 {
        self.files().iter().map(|f| f.size).sum()
    }

    /// Bytes still to download into `models`.
    pub fn missing_size(self, models: &Path) -> u64 {
        self.files().iter().filter(|f| !f.installed_in(models)).map(|f| f.size).sum()
    }

    pub fn installed_in(self, models: &Path) -> bool {
        self.files().iter().all(|f| f.installed_in(models))
    }

    /// What a cached parse is filed under: the set, its files' hashes and the parsing code's version, so a new model
    /// or a change in how parts are made never serves an old parse.
    pub fn version(self) -> String {
        let hashes: Vec<&str> = self.files().iter().map(|f| f.sha256.get(..8).unwrap_or(f.sha256)).collect();
        format!("{}-{}-v{}", self.id(), hashes.join("."), crate::PARSE_VERSION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_is_pinned_and_under_300_mb() {
        for set in ModelSet::ALL {
            for f in set.files() {
                assert_eq!(f.sha256.len(), 64, "{}", f.label);
                assert!(f.size > 1_000_000 && f.size < 300_000_000, "{}", f.label);
                assert!(f.url.starts_with("https://huggingface.co/") && f.url.contains("/resolve/") && f.url.ends_with(f.name), "{}", f.label);
            }
            assert_eq!(ModelSet::from_id(set.id()), Some(set));
            assert!(set.version().starts_with(set.id()));
        }
        assert_ne!(ModelSet::Segformer.version(), ModelSet::BiSeNet.version());
    }

    #[test]
    fn a_set_is_installed_only_with_every_file_at_its_size() {
        let dir = crate::tests::scratch("models-installed");
        let set = ModelSet::BiSeNet;
        assert!(!set.installed_in(&dir));
        assert_eq!(set.missing_size(&dir), set.size());
        let f = FACE_MESH;
        std::fs::create_dir_all(dir.join(f.folder)).unwrap();
        std::fs::write(f.path_in(&dir), b"short").unwrap();
        assert!(!f.installed_in(&dir));
        assert_eq!(set.missing_size(&dir), set.size());
    }
}
