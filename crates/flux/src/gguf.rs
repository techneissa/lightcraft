//! A hostile-file check of a GGUF's header, before candle reads it.
//!
//! Candle's GGUF reader allocates what the header says (a tensor count, a string length, an element count) before it
//! looks at the file's size, so a damaged or malicious file could ask for gigabytes. [`check`] walks the same header
//! with every count and size held against the file's length and a cap, and refuses with an error instead. It only
//! reads the header (never the tensor data) and checks that the names the FLUX loader needs are there.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::{Error, Result};

const MAGIC: &[u8; 4] = b"GGUF";
/// FLUX.1 has about 780 tensors; nothing legitimate has more than this.
const MAX_TENSORS: u64 = 20_000;
const MAX_METADATA: u64 = 100_000;
/// A key or tensor name longer than this is damage.
const MAX_NAME: u64 = 4096;
const MAX_DIMS: u32 = 4;
/// Elements in one tensor (FLUX's largest is ~ 10⁸).
const MAX_ELEMENTS: u64 = 1 << 34;
/// Nested metadata arrays deeper than this are damage.
const MAX_ARRAY_DEPTH: u32 = 3;
const DEFAULT_ALIGNMENT: u64 = 32;
/// The tensors the FLUX loader asks for first (it refuses a GGUF without them).
const REQUIRED: [&str; 2] = ["img_in.weight", "double_blocks.0.img_attn.qkv.weight"];

/// Block size (elements) and bytes per block of a GGML type, for the types candle can load.
fn block_of(ty: u32) -> Option<(u64, u64)> {
    Some(match ty {
        0 => (1, 4),      // F32
        1 => (1, 2),      // F16
        2 => (32, 18),    // Q4_0
        3 => (32, 20),    // Q4_1
        6 => (32, 22),    // Q5_0
        7 => (32, 24),    // Q5_1
        8 => (32, 34),    // Q8_0
        9 => (32, 36),    // Q8_1
        10 => (256, 84),  // Q2_K
        11 => (256, 110), // Q3_K
        12 => (256, 144), // Q4_K
        13 => (256, 176), // Q5_K
        14 => (256, 210), // Q6_K
        15 => (256, 292), // Q8_K
        30 => (1, 2),     // BF16
        _ => return None,
    })
}

/// A reader that knows where it is and refuses to go past the file's end.
struct Head<R> {
    r: R,
    pos: u64,
    len: u64,
}

impl<R: Read + Seek> Head<R> {
    fn bad(what: &str) -> Error {
        Error::Model(format!("not a usable GGUF file: {what}"))
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self.pos.checked_add(N as u64).filter(|e| *e <= self.len).ok_or_else(|| Self::bad("it ends inside its header"))?;
        let mut b = [0u8; N];
        self.r.read_exact(&mut b).map_err(|e| Error::Model(format!("GGUF header: {e}")))?;
        self.pos = end;
        Ok(b)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take()?))
    }

    fn skip(&mut self, n: u64) -> Result<()> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.len).ok_or_else(|| Self::bad("a value runs past its end"))?;
        let by = i64::try_from(n).map_err(|_| Self::bad("a value is too large"))?;
        self.r.seek(SeekFrom::Current(by)).map_err(|e| Error::Model(format!("GGUF header: {e}")))?;
        self.pos = end;
        Ok(())
    }

    /// A length-prefixed string of at most `cap` bytes, read.
    fn string(&mut self, cap: u64) -> Result<String> {
        let n = self.u64()?;
        if n > cap || self.pos.checked_add(n).is_none_or(|e| e > self.len) {
            return Err(Self::bad("a name is longer than the file or the allowed size"));
        }
        let mut b = vec![0u8; n as usize];
        self.r.read_exact(&mut b).map_err(|e| Error::Model(format!("GGUF header: {e}")))?;
        self.pos += n;
        Ok(String::from_utf8_lossy(&b).into_owned())
    }

    /// Steps over a metadata value of type `ty`; gives the value when it is a `u32` (for the alignment).
    fn value(&mut self, ty: u32, depth: u32) -> Result<Option<u32>> {
        match ty {
            0 | 1 | 7 => self.skip(1)?,
            2 | 3 => self.skip(2)?,
            4 => return Ok(Some(self.u32()?)),
            5 | 6 => self.skip(4)?,
            10..=12 => self.skip(8)?,
            8 => {
                let n = self.u64()?;
                self.skip(n)?;
            }
            9 => {
                if depth >= MAX_ARRAY_DEPTH {
                    return Err(Self::bad("arrays nested too deep"));
                }
                let elem = self.u32()?;
                let n = self.u64()?;
                // every element takes at least a byte: a count past the file's end is damage
                if n > self.len.saturating_sub(self.pos) {
                    return Err(Self::bad("an array is longer than the file"));
                }
                match elem {
                    0 | 1 | 7 => self.skip(n)?,
                    2 | 3 => self.skip(n.saturating_mul(2))?,
                    4..=6 => self.skip(n.saturating_mul(4))?,
                    10..=12 => self.skip(n.saturating_mul(8))?,
                    8 | 9 => {
                        for _ in 0..n {
                            self.value(elem, depth + 1)?;
                        }
                    }
                    _ => return Err(Self::bad("an array of an unknown type")),
                }
            }
            _ => return Err(Self::bad("a value of an unknown type")),
        }
        Ok(None)
    }
}

/// Checks the GGUF header of `path`: the counts, names and sizes against the file's length and the caps above.
pub fn check(path: &Path) -> Result<()> {
    let f = std::fs::File::open(path).map_err(|e| Error::Model(format!("{}: {e}", path.display())))?;
    let len = f.metadata().map_err(|e| Error::Model(format!("{}: {e}", path.display())))?.len();
    check_reader(std::io::BufReader::new(f), len).map_err(|e| match e {
        Error::Model(m) => Error::Model(format!("{}: {m}", path.display())),
        e => e,
    })
}

/// [`check`] over any reader of `len` bytes.
pub(crate) fn check_reader<R: Read + Seek>(r: R, len: u64) -> Result<()> {
    let mut h = Head { r, pos: 0, len };
    if &h.take::<4>()? != MAGIC {
        return Err(Head::<R>::bad("it does not start with GGUF"));
    }
    let version = h.u32()?;
    if !(2..=3).contains(&version) {
        return Err(Head::<R>::bad(&format!("version {version}")));
    }
    let tensors = h.u64()?;
    let metadata = h.u64()?;
    if tensors == 0 || tensors > MAX_TENSORS || metadata > MAX_METADATA {
        return Err(Head::<R>::bad(&format!("{tensors} tensors and {metadata} metadata entries")));
    }
    let mut alignment = DEFAULT_ALIGNMENT;
    for _ in 0..metadata {
        let key = h.string(MAX_NAME)?;
        let ty = h.u32()?;
        let v = h.value(ty, 0)?;
        if key == "general.alignment" {
            alignment = u64::from(v.ok_or_else(|| Head::<R>::bad("a general.alignment that is not a number"))?);
            if alignment == 0 || !alignment.is_power_of_two() || alignment > 1 << 20 {
                return Err(Head::<R>::bad(&format!("alignment {alignment}")));
            }
        }
    }
    // the tensors' placement, held for after the header ends (their offsets are from the aligned data start)
    let mut placed: Vec<(u64, u64)> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut total: u64 = 0;
    for _ in 0..tensors {
        let name = h.string(MAX_NAME)?;
        let dims = h.u32()?;
        if dims == 0 || dims > MAX_DIMS {
            return Err(Head::<R>::bad(&format!("tensor {name} has {dims} dimensions")));
        }
        let mut elements: u64 = 1;
        let mut first = 0;
        for i in 0..dims {
            let d = h.u64()?;
            if i == 0 {
                first = d;
            }
            elements =
                elements.checked_mul(d).filter(|e| *e <= MAX_ELEMENTS).ok_or_else(|| Head::<R>::bad(&format!("tensor {name} is too large")))?;
        }
        let ty = h.u32()?;
        let (block, bytes) = block_of(ty).ok_or_else(|| Head::<R>::bad(&format!("tensor {name} has the unsupported type {ty}")))?;
        let offset = h.u64()?;
        if elements == 0 || first % block != 0 {
            return Err(Head::<R>::bad(&format!("tensor {name} has a size its type can't hold")));
        }
        let size = elements / block * bytes;
        total = total.checked_add(size).ok_or_else(|| Head::<R>::bad("the tensors are too large"))?;
        placed.push((offset, size));
        names.push(name);
    }
    let data = h.pos.div_ceil(alignment).checked_mul(alignment).ok_or_else(|| Head::<R>::bad("the header is too large"))?;
    let room = len.saturating_sub(data);
    // the tensors together can't be larger than what follows the header, and each lies inside it
    if total > room {
        return Err(Head::<R>::bad(&format!("its tensors need {total} bytes and the file has {room} after the header")));
    }
    for ((offset, size), name) in placed.iter().zip(&names) {
        if offset.checked_add(*size).is_none_or(|e| e > room) {
            return Err(Head::<R>::bad(&format!("tensor {name} lies past the end of the file")));
        }
    }
    for need in REQUIRED {
        if !names.iter().any(|n| n == need) {
            return Err(Error::Model(format!("not a FLUX.1 GGUF: no tensor {need}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn string(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u64).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    /// A GGUF: `tensors` as (name, F32 elements); data padded to the file's real size.
    fn file(tensors: &[(&str, u64)], pad_data: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&(tensors.len() as u64).to_le_bytes());
        out.extend_from_slice(&1u64.to_le_bytes());
        string(&mut out, "general.architecture");
        out.extend_from_slice(&8u32.to_le_bytes());
        string(&mut out, "flux");
        let mut offset = 0u64;
        for (name, n) in tensors {
            string(&mut out, name);
            out.extend_from_slice(&1u32.to_le_bytes());
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            offset += n * 4;
        }
        while out.len() % 32 != 0 {
            out.push(0);
        }
        if pad_data {
            out.resize(out.len() + offset as usize, 0);
        }
        out
    }

    fn run(b: &[u8]) -> Result<()> {
        check_reader(Cursor::new(b.to_vec()), b.len() as u64)
    }

    const FLUX: [(&str, u64); 2] = [("img_in.weight", 8), ("double_blocks.0.img_attn.qkv.weight", 16)];

    #[test]
    fn a_whole_flux_header_passes() {
        run(&file(&FLUX, true)).unwrap();
    }

    #[test]
    fn a_file_missing_the_flux_names_is_refused() {
        let e = run(&file(&[("tok_embeddings.weight", 8)], true)).unwrap_err().to_string();
        assert!(e.contains("not a FLUX.1 GGUF"), "{e}");
    }

    #[test]
    fn a_truncated_or_garbage_file_is_an_error_not_an_allocation() {
        let good = file(&FLUX, true);
        // cut inside the header, inside the data, and nothing at all
        for cut in [0, 3, 10, 30, 60, good.len() / 2, good.len() - 1] {
            assert!(run(&good[..cut]).is_err(), "cut at {cut}");
        }
        assert!(run(&[0xA5; 4096]).is_err());
        assert!(run(b"GGUF").is_err());
    }

    #[test]
    fn huge_counts_and_sizes_are_refused_before_anything_is_allocated() {
        let good = file(&FLUX, true);
        // the tensor count (offset 8) and the metadata count (offset 16)
        for at in [8, 16] {
            let mut b = good.clone();
            b[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
            assert!(run(&b).is_err(), "count at {at}");
        }
        // the first key's length
        let mut b = good.clone();
        b[24..32].copy_from_slice(&(1u64 << 60).to_le_bytes());
        assert!(run(&b).is_err());
        // a tensor that claims 2^33 elements in a small file: its size is larger than the file
        let huge = file(&[("img_in.weight", 1 << 33), ("double_blocks.0.img_attn.qkv.weight", 16)], false);
        let e = run(&huge).unwrap_err().to_string();
        assert!(e.contains("file has"), "{e}");
        // an element count past the cap, or one that overflows
        let past = file(&[("img_in.weight", u64::MAX / 8), ("double_blocks.0.img_attn.qkv.weight", 16)], false);
        assert!(run(&past).is_err());
    }

    #[test]
    fn a_tensor_past_the_end_of_the_file_is_refused() {
        let mut b = file(&FLUX, true);
        // the first tensor's offset: after magic, version, counts (24), the metadata entry, the name, dims, type
        let at = 24 + (8 + 20 + 4 + 8 + 4) + (8 + 13) + 4 + 8 + 4;
        b[at..at + 8].copy_from_slice(&(1u64 << 40).to_le_bytes());
        let e = run(&b).unwrap_err().to_string();
        assert!(e.contains("past the end"), "{e}");
    }
}
