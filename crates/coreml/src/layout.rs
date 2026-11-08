//! Pure helpers: shape checks, reading a strided tensor out of Core ML's bytes, and moving the compiled folder.
//! Used only by the Core ML backend; their tests run everywhere.

use std::path::Path;

use crate::{Error, MAX_ELEMENTS, MAX_RANK, Result};

/// How the numbers of a Core ML tensor are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Elem {
    F16,
    F32,
    F64,
}

impl Elem {
    pub(crate) fn bytes(self) -> usize {
        match self {
            Elem::F16 => 2,
            Elem::F32 => 4,
            Elem::F64 => 8,
        }
    }
}

/// The number of elements of `shape`, checked against `len` and the caps.
pub(crate) fn check_shape(shape: &[usize], len: usize) -> Result<usize> {
    if shape.is_empty() || shape.len() > MAX_RANK {
        return Err(Error::Input(format!("a tensor has 1 to {MAX_RANK} dimensions, not {}", shape.len())));
    }
    let count = shape.iter().try_fold(1usize, |n, &d| if d == 0 { None } else { n.checked_mul(d) });
    match count {
        Some(n) if n <= MAX_ELEMENTS && n == len => Ok(n),
        Some(n) if n <= MAX_ELEMENTS => Err(Error::Input(format!("shape {shape:?} holds {n} numbers, {len} given"))),
        _ => Err(Error::Input(format!("shape {shape:?} is empty or too large"))),
    }
}

/// Row-major strides of `shape`, in elements.
pub(crate) fn contiguous(shape: &[usize]) -> Vec<usize> {
    let mut out = vec![1usize; shape.len()];
    for i in (0..shape.len().saturating_sub(1)).rev() {
        let next = out.get(i + 1).copied().unwrap_or(1).saturating_mul(shape.get(i + 1).copied().unwrap_or(1));
        if let Some(s) = out.get_mut(i) {
            *s = next;
        }
    }
    out
}

/// IEEE half to single precision.
pub(crate) fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1f);
    let frac = f32::from(h & 0x3ff);
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        31 if frac == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        e => sign * (1.0 + frac / 1024.0) * 2f32.powi(e - 15),
    }
}

/// The numbers of a tensor of `shape` stored with `strides` (in elements, as Core ML reports them; the Neural Engine
/// pads rows) in `bytes`, row-major. Any offset past the end of `bytes` is an error, never a read out of bounds.
pub(crate) fn gather(shape: &[usize], strides: &[usize], elem: Elem, bytes: &[u8]) -> Result<Vec<f32>> {
    if strides.len() != shape.len() {
        return Err(Error::Output(format!("shape {shape:?} with strides {strides:?}")));
    }
    // counted with checked multiplies: an overflow here would panic inside Core ML's byte handler
    let count = shape.iter().try_fold(1usize, |n, &d| n.checked_mul(d)).ok_or_else(|| Error::Output(format!("shape {shape:?} is too large")))?;
    let count = check_shape(shape, count).map_err(|e| Error::Output(e.to_string()))?;
    let size = elem.bytes();
    let mut out = Vec::with_capacity(count);
    let mut index = vec![0usize; shape.len()];
    for _ in 0..count {
        let offset = index.iter().zip(strides).try_fold(0usize, |o, (&i, &s)| i.checked_mul(s).and_then(|v| o.checked_add(v)));
        let at = offset.and_then(|o| o.checked_mul(size));
        let Some(raw) = at.and_then(|a| bytes.get(a..a.checked_add(size)?)) else {
            return Err(Error::Output(format!("shape {shape:?} with strides {strides:?} reads past {} bytes", bytes.len())));
        };
        out.push(match (elem, raw) {
            (Elem::F16, &[a, b]) => f16_to_f32(u16::from_ne_bytes([a, b])),
            (Elem::F32, &[a, b, c, d]) => f32::from_ne_bytes([a, b, c, d]),
            (Elem::F64, &[a, b, c, d, e, f, g, h]) => f64::from_ne_bytes([a, b, c, d, e, f, g, h]) as f32,
            _ => return Err(Error::Output("bad element size".into())),
        });
        // the next index, last dimension fastest
        for (i, d) in index.iter_mut().zip(shape).rev() {
            *i += 1;
            if *i < *d {
                break;
            }
            *i = 0;
        }
    }
    Ok(out)
}

/// Move folder `from` to `dest`, replacing `dest`: through `dest` + `.part`, so a crash leaves either the old folder
/// or the new one, never half of one. Copies when `from` is on another volume.
pub(crate) fn replace_dir(from: &Path, dest: &Path) -> Result<()> {
    let io = |what: &str, p: &Path, e: std::io::Error| Error::Io(format!("{what} {}: {e}", p.display()));
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = std::path::PathBuf::from(part);
    if part.exists() {
        std::fs::remove_dir_all(&part).map_err(|e| io("could not clear", &part, e))?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io("could not create", parent, e))?;
    }
    if std::fs::rename(from, &part).is_err() {
        copy_dir(from, &part, 0).map_err(|e| io("could not copy to", &part, e))?;
    }
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| io("could not replace", dest, e))?;
    }
    std::fs::rename(&part, dest).map_err(|e| io("could not move to", dest, e))
}

/// A compiled model is a few folders deep; anything deeper is not one.
const MAX_DEPTH: usize = 8;

fn copy_dir(from: &Path, to: &Path, depth: usize) -> std::io::Result<()> {
    if depth > MAX_DEPTH {
        return Err(std::io::Error::other("folders nested too deep"));
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target, depth + 1)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
