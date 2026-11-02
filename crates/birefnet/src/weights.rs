//! Safetensors weights read on demand, any float width (BiRefNet's published files are F16, the
//! matting model's F32), converted to F32 on the CPU.
//!
//! Positional reads, no memory map: `unsafe` stays in `lightcraft-sysmem`. The header is checked
//! against the file before anything is read, so a damaged download fails with a message.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

use candle_core::{DType, Device, Tensor};

use crate::{Error, Result};

/// Largest header accepted (the real ones are under 100 kB).
const MAX_HEADER: u64 = 16 << 20;

struct Entry {
    /// `None`: a dtype BiRefNet doesn't read (the `I64` index buffers and counters stored beside the weights).
    dtype: Option<DType>,
    shape: Vec<usize>,
    start: u64,
    end: u64,
}

/// An open safetensors file: its header, and the file to read tensor bytes from.
pub struct Weights {
    file: Mutex<File>,
    entries: HashMap<String, Entry>,
    data_start: u64,
}

fn model_err(path: &Path, what: impl std::fmt::Display) -> Error {
    Error::Model(format!("{}: {what}", path.display()))
}

impl Weights {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path).map_err(|e| model_err(path, e))?;
        let file_len = file.metadata().map_err(|e| model_err(path, e))?.len();
        let mut len = [0u8; 8];
        file.read_exact(&mut len).map_err(|e| model_err(path, e))?;
        let n = u64::from_le_bytes(len);
        if n > MAX_HEADER || n.saturating_add(8) > file_len {
            return Err(model_err(path, format!("not a safetensors file (header of {n} bytes)")));
        }
        let mut header = vec![0u8; usize::try_from(n).map_err(|_| model_err(path, "bad header"))?];
        file.read_exact(&mut header).map_err(|e| model_err(path, e))?;
        let header: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&header).map_err(|e| model_err(path, format!("bad header: {e}")))?;
        let mut entries = HashMap::new();
        for (name, v) in header {
            if name == "__metadata__" {
                continue;
            }
            let dtype = match v.get("dtype").and_then(|d| d.as_str()) {
                Some("F32") => Some(DType::F32),
                Some("F16") => Some(DType::F16),
                Some("BF16") => Some(DType::BF16),
                _ => None,
            };
            let shape: Vec<usize> = v
                .get("shape")
                .and_then(|s| s.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_u64().and_then(|x| usize::try_from(x).ok())).collect())
                .unwrap_or_default();
            let offsets: Vec<u64> =
                v.get("data_offsets").and_then(|s| s.as_array()).map(|a| a.iter().filter_map(serde_json::Value::as_u64).collect()).unwrap_or_default();
            let [start, end] = offsets[..] else { return Err(model_err(path, format!("{name}: bad data_offsets"))) };
            if let Some(dtype) = dtype {
                let want = shape.iter().try_fold(dtype.size_in_bytes(), |a, d| a.checked_mul(*d));
                if end.checked_sub(start) != want.and_then(|w| u64::try_from(w).ok()) {
                    return Err(model_err(path, format!("{name}: size does not match its shape")));
                }
            }
            // the bytes must be in the file (a truncated download fails here, not in the middle of a render)
            if (8 + n).checked_add(end).is_none_or(|e| e > file_len) {
                return Err(model_err(path, format!("tensor {name} lies outside the file (incomplete or damaged download?)")));
            }
            entries.insert(name, Entry { dtype, shape, start, end });
        }
        Ok(Self { file: Mutex::new(file), entries, data_start: 8 + n })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// The shape stored in the header for `name`, without reading its data.
    pub fn shape(&self, name: &str) -> Option<&[usize]> {
        self.entries.get(name).map(|e| e.shape.as_slice())
    }

    /// Tensor `name` as F32 on the CPU.
    pub fn read(&self, name: &str) -> Result<Tensor> {
        let e = self.entries.get(name).ok_or_else(|| Error::Model(format!("missing tensor `{name}`")))?;
        let Some(dtype) = e.dtype else { return Err(Error::Model(format!("{name}: unsupported dtype (BiRefNet weights are F16, BF16 or F32)"))) };
        let len = usize::try_from(e.end.saturating_sub(e.start)).map_err(|_| Error::Model(format!("{name}: too large")))?;
        let mut bytes = vec![0u8; len];
        {
            let mut f = self.file.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            f.seek(SeekFrom::Start(self.data_start + e.start)).map_err(|err| Error::Model(format!("{name}: {err}")))?;
            f.read_exact(&mut bytes).map_err(|err| Error::Model(format!("{name}: {err}")))?;
        }
        let t = Tensor::from_raw_buffer(&bytes, dtype, e.shape.as_slice(), &Device::Cpu)?;
        Ok(t.to_dtype(DType::F32)?)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Writes a safetensors file with `header` and `data` bytes after it.
    pub(crate) fn write(name: &str, header: &str, data: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("lc-birefnet-{name}-{}.safetensors", std::process::id()));
        let mut b = (header.len() as u64).to_le_bytes().to_vec();
        b.extend_from_slice(header.as_bytes());
        b.extend_from_slice(data);
        std::fs::write(&p, b).unwrap();
        p
    }

    #[test]
    fn f16_and_f32_tensors_read_as_f32() {
        let mut data = Vec::new();
        for v in [1.0f32, 2.0, 3.0, 4.0] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        for v in [0.5f32, -2.0] {
            data.extend_from_slice(&half_bits(v).to_le_bytes());
        }
        let p = write(
            "mixed",
            r#"{"a":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]},"b":{"dtype":"F16","shape":[2],"data_offsets":[16,20]},"i":{"dtype":"I64","shape":[],"data_offsets":[20,28]}}"#,
            &[data, vec![0u8; 8]].concat(),
        );
        let w = Weights::open(&p).unwrap();
        assert_eq!(w.read("a").unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap(), vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(w.read("b").unwrap().to_vec1::<f32>().unwrap(), vec![0.5, -2.0]);
        assert!(w.read("i").unwrap_err().to_string().contains("unsupported dtype"));
        assert!(w.read("nope").unwrap_err().to_string().contains("missing tensor"));
        let _ = std::fs::remove_file(p);
    }

    /// IEEE half bits of the exactly representable values the tests use.
    fn half_bits(v: f32) -> u16 {
        match v.to_bits() {
            b if b == 0.5f32.to_bits() => 0x3800,
            b if b == (-2.0f32).to_bits() => 0xC000,
            _ => 0,
        }
    }

    #[test]
    fn hostile_headers_are_errors() {
        let ok = write("ok", r#"{"a":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]}}"#, &[0u8; 16]);
        assert!(Weights::open(&ok).is_ok());
        let short = write("short", r#"{"a":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]}}"#, &[0u8; 8]);
        assert!(Weights::open(&short).err().unwrap().to_string().contains("outside the file"));
        let huge = write("huge", r#"{"a":{"dtype":"F32","shape":[1073741824,1073741824],"data_offsets":[0,4611686018427387904]}}"#, &[0u8; 16]);
        assert!(Weights::open(&huge).is_err());
        let wrap = write("wrap", r#"{"a":{"dtype":"F32","shape":[1],"data_offsets":[18446744073709551612,18446744073709551616]}}"#, &[0u8; 16]);
        assert!(Weights::open(&wrap).is_err());
        let mismatch = write("mismatch", r#"{"a":{"dtype":"F16","shape":[3],"data_offsets":[0,4]}}"#, &[0u8; 8]);
        assert!(Weights::open(&mismatch).err().unwrap().to_string().contains("does not match"));
        let p = std::env::temp_dir().join(format!("lc-birefnet-hdr-{}.safetensors", std::process::id()));
        std::fs::write(&p, 1000u64.to_le_bytes()).unwrap();
        assert!(Weights::open(&p).is_err());
        let not_json = write("json", "not json at all", &[]);
        assert!(Weights::open(&not_json).is_err());
        for f in [ok, short, huge, wrap, mismatch, p, not_json] {
            let _ = std::fs::remove_file(f);
        }
    }
}
