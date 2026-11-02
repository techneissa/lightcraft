//! Reading LaMa's weights from a PyTorch file, without Python and without running any code in it.
//!
//! Two layouts are read:
//! - the TorchScript archive `big-lama.pt` (a zip whose `data.pkl` pickles the module tree: the
//!   generator is `model.generator.model.<index>…`);
//! - a Lightning checkpoint (`best.ckpt`, with a `state_dict` whose keys start `generator.model.`).
//!
//! Candle's pickle reader parses the pickle into a tree of plain values (it never imports or calls
//! anything); this module walks that tree, collects the tensors under `generator.model.` and reads
//! their bytes from the zip. Hostile files give an [`Error`], never a panic: the parse runs under
//! `catch_unwind` because Candle's reader indexes the arguments it expects.

use std::collections::BTreeMap;
use std::io::{BufReader, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use candle_core::pickle::{Object, Stack, TensorInfo};
use candle_core::{DType, Device, Tensor};

use crate::{Error, Result};

/// Marker every generator weight name contains; the names handed on start after it.
const GENERATOR: &str = "generator.model.";
/// Largest `data.pkl` read (the real one is 135 kB).
const MAX_PICKLE: u64 = 64 << 20;
/// Most a single tensor may hold (elements).
const MAX_TENSOR_ELEMS: usize = 1 << 28;
/// Most tensors a file may list.
const MAX_TENSORS: usize = 20_000;
/// Deepest the module tree may nest.
const MAX_DEPTH: usize = 48;

fn bad(path: &Path, why: impl std::fmt::Display) -> Error {
    Error::Model(format!("{}: {why}", path.display()))
}

/// The generator's weights from `path`, keyed `<layer index>.<module>.<param>` (so `1.ffc.convl2l.weight`),
/// on `device` as f32.
pub fn read_generator(path: &Path, device: &Device) -> Result<BTreeMap<String, Tensor>> {
    let file = std::fs::File::open(path).map_err(|e| bad(path, e))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| bad(path, format!("not a PyTorch zip file ({e})")))?;
    let pkl_name = zip.file_names().find(|n| n.ends_with("data.pkl")).map(str::to_string).ok_or_else(|| bad(path, "no data.pkl inside"))?;
    let dir = pkl_name.strip_suffix(".pkl").unwrap_or(&pkl_name).to_string();
    let pickle = {
        let entry = zip.by_name(&pkl_name).map_err(|e| bad(path, e))?;
        if entry.size() > MAX_PICKLE {
            return Err(bad(path, "data.pkl is too large"));
        }
        let mut bytes = Vec::new();
        entry.take(MAX_PICKLE).read_to_end(&mut bytes).map_err(|e| bad(path, e))?;
        bytes
    };
    let root = parse(&pickle).map_err(|e| bad(path, e))?;
    let mut found: Vec<TensorInfo> = Vec::new();
    collect(&root, &mut String::new(), &dir, &mut found, 0).map_err(|e| bad(path, e))?;
    if found.is_empty() {
        return Err(bad(path, "no generator weights found (is this a big-lama checkpoint?)"));
    }
    let mut out = BTreeMap::new();
    for info in found {
        let tensor = read_tensor(&mut zip, &info, device).map_err(|e| bad(path, format!("{}: {e}", info.name)))?;
        out.insert(info.name, tensor);
    }
    Ok(out)
}

/// Candle's pickle reader, run so a malformed file is an error.
fn parse(bytes: &[u8]) -> std::result::Result<Object, String> {
    let run = || -> std::result::Result<Object, String> {
        let mut stack = Stack::empty();
        stack.read_loop(&mut BufReader::new(bytes)).map_err(|e| e.to_string())?;
        stack.finalize().map_err(|e| e.to_string())
    };
    catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|_| Err("malformed pickle".into()))
}

/// Walks the tree, joining dict keys with dots, and keeps the tensors under [`GENERATOR`] (renamed to
/// start after it).
fn collect(obj: &Object, path: &mut String, dir: &str, out: &mut Vec<TensorInfo>, depth: usize) -> std::result::Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("module tree nests too deeply".into());
    }
    let entries = match obj {
        // a module: the state of a freshly built object is its attribute dict
        Object::Build { args, .. } => return collect(args, path, dir, out, depth + 1),
        Object::Dict(entries) => entries,
        _ => return Ok(()),
    };
    for (key, value) in entries {
        let Object::Unicode(key) = key else { continue };
        let mark = path.len();
        if !path.is_empty() {
            path.push('.');
        }
        path.push_str(key);
        match value {
            Object::Reduce { .. } => {
                if let Some(at) = path.find(GENERATOR) {
                    // a tensor leaf (anything else that is called, such as a class, is skipped)
                    if let Ok(Some(mut info)) = value.clone().into_tensor_info(Object::Unicode(path.clone()), std::path::Path::new(dir)) {
                        info.name = path[at + GENERATOR.len()..].to_string();
                        // BatchNorm's step counter and the like: not weights
                        if !matches!(info.dtype, DType::F32 | DType::F64) {
                            path.truncate(mark);
                            continue;
                        }
                        if out.len() >= MAX_TENSORS {
                            return Err("too many tensors".into());
                        }
                        out.push(info);
                    }
                }
            }
            // `state_dict` of a Lightning checkpoint, or any nested module
            Object::Dict(_) | Object::Build { .. } => collect(value, path, dir, out, depth + 1)?,
            _ => {}
        }
        path.truncate(mark);
    }
    Ok(())
}

/// One tensor's bytes from the zip, as an f32 tensor on `device`.
fn read_tensor<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, info: &TensorInfo, device: &Device) -> std::result::Result<Tensor, String> {
    let dims = info.layout.dims().to_vec();
    let elems = dims.iter().try_fold(1usize, |a, d| a.checked_mul(*d)).filter(|n| *n <= MAX_TENSOR_ELEMS).ok_or("tensor too large")?;
    if !info.layout.is_contiguous() {
        return Err("tensor is not contiguous".into());
    }
    let width = info.dtype.size_in_bytes();
    let start = info.layout.start_offset();
    let want = elems.checked_mul(width).and_then(|n| n.checked_add(start)).ok_or("tensor too large")?;
    let entry = zip.by_name(&info.path).map_err(|e| e.to_string())?;
    if (entry.size() as usize) < want || info.storage_size.checked_mul(width).is_none_or(|s| s as u64 != entry.size()) {
        return Err(format!("its storage {} holds {} bytes, expected {want}", info.path, entry.size()));
    }
    let mut bytes = vec![0u8; want];
    entry.take(want as u64).read_exact(&mut bytes).map_err(|e| e.to_string())?;
    let body = bytes.get(start..).ok_or("bad offset")?;
    let values: Vec<f32> = match info.dtype {
        DType::F32 => body.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect(),
        DType::F64 => body.as_chunks::<8>().0.iter().map(|c| f64::from_le_bytes(*c) as f32).collect(),
        other => return Err(format!("unsupported tensor type {other:?}")),
    };
    Tensor::from_vec(values, dims, device).map_err(|e| e.to_string())
}
