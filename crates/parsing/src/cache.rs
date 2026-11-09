//! Parses kept on disk, one file each, named by the pixels they were made from and the model set's version, so a
//! photo is parsed once per model version and a second parse reads the file.
//!
//! A file is `LCPARSE1`, a little-endian `u32` header length, a JSON header (people, faces, landmarks and each plane's
//! rectangle, size and byte length), then each plane as an 8-bit grey PNG. Everything read back is checked; a damaged
//! file is an error the caller treats as "not cached".

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::image::RgbImage;
use crate::types::{FaceHint, Landmark, NormRect, Parsing, Part, Person, Plane};
use crate::{Error, Result};

const MAGIC: &[u8; 8] = b"LCPARSE1";
const MAX_HEADER: usize = 32 << 20;
/// No plane a parse makes is wider than the body model's largest input; a file with a wider one is damaged. Steps that
/// redraw a parse's planes (hair edges) keep to it, or their parse is never read back.
pub const MAX_PLANE_SIDE: u32 = 1536;
/// Most mask values one file may decode to (about 1 GB as floats).
const MAX_VALUES: u64 = 256 << 20;
const MAX_PEOPLE: usize = 4096;
const MAX_PLANES: usize = 65_536;
/// The file name's suffix.
pub const EXT: &str = "lcparse";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaneHead {
    rect: NormRect,
    width: u32,
    height: u32,
    bytes: u64,
    /// [`Plane::split`]
    #[serde(default)]
    split: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersonHead {
    face: Option<NormRect>,
    face_score: f32,
    landmarks: Vec<Landmark>,
    landmark_score: f32,
    person_id: Option<String>,
    bounds: Option<NormRect>,
    parts: Vec<(Part, Vec<PlaneHead>)>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Head {
    model_set: String,
    model_version: String,
    width: usize,
    height: usize,
    people: Vec<PersonHead>,
}

/// What a parse of `img` with `hints` by the model set of `version` is filed under: a SHA-256 of the pixels, the
/// size and the hints, then the version.
pub fn key(img: &RgbImage, hints: &[FaceHint], version: &str) -> String {
    let mut h = Sha256::new();
    h.update((img.width as u64).to_le_bytes());
    h.update((img.height as u64).to_le_bytes());
    h.update(&img.data);
    for f in hints {
        for v in [f.rect.x0, f.rect.y0, f.rect.x1, f.rect.y1, f.score] {
            h.update(v.to_bits().to_le_bytes());
        }
        if let Some(l) = f.landmarks {
            for (x, y) in l {
                h.update(x.to_bits().to_le_bytes());
                h.update(y.to_bits().to_le_bytes());
            }
        }
        h.update(f.person_id.as_deref().unwrap_or("").as_bytes());
        h.update([0u8]);
    }
    let digest = h.finalize();
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    let safe: String = version.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    format!("{hex}-{safe}")
}

/// The file of `key` in `dir`.
pub fn path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.{EXT}"))
}

fn encode_plane(p: &Plane) -> Result<Vec<u8>> {
    let bytes: Vec<u8> = p.data.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    let (w, h) = (
        u32::try_from(p.width).map_err(|_| Error::Cache("plane too wide".into()))?,
        u32::try_from(p.height).map_err(|_| Error::Cache("plane too tall".into()))?,
    );
    let img = image::GrayImage::from_raw(w, h, bytes).ok_or_else(|| Error::Cache("plane size does not match its data".into()))?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| Error::Cache(e.to_string()))?;
    Ok(out.into_inner())
}

/// The bytes of a parse.
pub fn encode(p: &Parsing) -> Result<Vec<u8>> {
    let mut blobs = Vec::new();
    let mut people = Vec::with_capacity(p.people.len());
    for q in &p.people {
        let mut parts = Vec::new();
        for (part, planes) in &q.parts {
            let mut heads = Vec::with_capacity(planes.len());
            for plane in planes {
                let png = encode_plane(plane)?;
                heads.push(PlaneHead {
                    rect: plane.rect,
                    width: plane.width as u32,
                    height: plane.height as u32,
                    bytes: png.len() as u64,
                    split: plane.split,
                });
                blobs.push(png);
            }
            parts.push((*part, heads));
        }
        people.push(PersonHead {
            face: q.face,
            face_score: q.face_score,
            landmarks: q.landmarks.clone(),
            landmark_score: q.landmark_score,
            person_id: q.person_id.clone(),
            bounds: q.bounds,
            parts,
        });
    }
    let head = Head { model_set: p.model_set.clone(), model_version: p.model_version.clone(), width: p.width, height: p.height, people };
    let json = serde_json::to_vec(&head).map_err(|e| Error::Cache(e.to_string()))?;
    let len = u32::try_from(json.len()).map_err(|_| Error::Cache("header too large".into()))?;
    let mut out = Vec::with_capacity(12 + json.len() + blobs.iter().map(Vec::len).sum::<usize>());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&json);
    for b in blobs {
        out.extend_from_slice(&b);
    }
    Ok(out)
}

fn decode_plane(head: &PlaneHead, bytes: &[u8]) -> Result<Plane> {
    let bad = |what: &str| Error::Cache(format!("damaged plane: {what}"));
    if head.width == 0 || head.height == 0 || head.width > MAX_PLANE_SIDE || head.height > MAX_PLANE_SIDE || !head.rect.is_valid() {
        return Err(bad("size"));
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PLANE_SIDE);
    limits.max_image_height = Some(MAX_PLANE_SIDE);
    limits.max_alloc = Some(256 << 20);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| Error::Cache(format!("damaged plane: {e}")))?.into_luma8();
    if img.width() != head.width || img.height() != head.height {
        return Err(bad("size differs from its header"));
    }
    let data = img.into_raw().into_iter().map(|b| f32::from(b) / 255.0).collect();
    Ok(Plane { rect: head.rect, width: head.width as usize, height: head.height as usize, data, split: head.split })
}

/// A parse from its bytes.
pub fn decode(bytes: &[u8]) -> Result<Parsing> {
    decode_capped(bytes, MAX_VALUES)
}

/// [`decode`], refusing a file whose planes add up to more than `max_values` values.
fn decode_capped(bytes: &[u8], max_values: u64) -> Result<Parsing> {
    let bad = |what: &str| Error::Cache(format!("not a parse file: {what}"));
    if bytes.get(..8) != Some(&MAGIC[..]) {
        return Err(bad("wrong start"));
    }
    let len = bytes.get(8..12).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes).ok_or_else(|| bad("no header"))?;
    let len = usize::try_from(len).map_err(|_| bad("header length"))?;
    if len > MAX_HEADER {
        return Err(bad("header too large"));
    }
    let json = bytes.get(12..12usize.saturating_add(len)).ok_or_else(|| bad("short header"))?;
    let head: Head = serde_json::from_slice(json).map_err(|e| Error::Cache(format!("bad header: {e}")))?;
    if head.people.len() > MAX_PEOPLE || head.people.iter().map(|p| p.parts.iter().map(|q| q.1.len()).sum::<usize>()).sum::<usize>() > MAX_PLANES {
        return Err(bad("too many people or planes"));
    }
    let values = head
        .people
        .iter()
        .flat_map(|p| p.parts.iter().flat_map(|q| q.1.iter()))
        .fold(0u64, |a, pl| a.saturating_add(u64::from(pl.width).saturating_mul(u64::from(pl.height))));
    if values > max_values {
        return Err(bad("its masks are larger than any parse makes"));
    }
    let mut at = 12 + len;
    let mut people = Vec::with_capacity(head.people.len());
    for ph in head.people {
        let mut parts = BTreeMap::new();
        for (part, planes) in ph.parts {
            let mut out = Vec::with_capacity(planes.len());
            for pl in planes {
                let n = usize::try_from(pl.bytes).map_err(|_| bad("plane length"))?;
                let end = at.checked_add(n).ok_or_else(|| bad("plane length"))?;
                let blob = bytes.get(at..end).ok_or_else(|| bad("short file"))?;
                out.push(decode_plane(&pl, blob)?);
                at = end;
            }
            parts.insert(part, out);
        }
        let finite = |l: &Landmark| l.x.is_finite() && l.y.is_finite() && l.z.is_finite();
        if !ph.landmarks.iter().all(finite) || ph.face.is_some_and(|f| !f.is_valid()) {
            return Err(bad("bad face"));
        }
        people.push(Person {
            face: ph.face,
            face_score: ph.face_score,
            landmarks: ph.landmarks,
            landmark_score: ph.landmark_score,
            person_id: ph.person_id,
            bounds: ph.bounds,
            parts,
        });
    }
    if at != bytes.len() {
        return Err(bad("extra bytes"));
    }
    Ok(Parsing { model_set: head.model_set, model_version: head.model_version, width: head.width, height: head.height, people })
}

/// The parse filed under `key` in `dir`: `Ok(None)` when there is none, an error when the file is damaged.
pub fn load(dir: &Path, key: &str) -> Result<Option<Parsing>> {
    let p = path(dir, key);
    let bytes = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Cache(format!("{}: {e}", p.display()))),
    };
    decode(&bytes).map(Some)
}

/// File a parse under `key` in `dir` (written to a temporary file first, then moved, so a reader never sees half).
pub fn store(dir: &Path, key: &str, p: &Parsing) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| Error::Cache(format!("{}: {e}", dir.display())))?;
    let bytes = encode(p)?;
    let dest = path(dir, key);
    let tmp = dir.join(format!(".{key}.{}.tmp", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, &bytes) {
        // a half-written file (a full disk) is not left behind
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Cache(format!("{}: {e}", tmp.display())));
    }
    std::fs::rename(&tmp, &dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::Cache(format!("{}: {e}", dest.display()))
    })
}

/// Remove the oldest parse files until the folder holds at most `max_bytes` of them. Returns how many went.
pub fn prune(dir: &Path, max_bytes: u64) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == EXT))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len(), e.path()))
        })
        .collect();
    files.sort();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    let mut removed = 0;
    for (_, len, p) in files {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total = total.saturating_sub(len);
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Parsing {
        let mut person = Person { face: Some(NormRect::new(0.1, 0.1, 0.3, 0.3)), face_score: 0.9, person_id: Some("p1".into()), ..Person::default() };
        person.landmarks = vec![Landmark { x: 0.2, y: 0.2, z: -0.01 }; 478];
        person.parts.insert(
            Part::Hair,
            vec![Plane { rect: NormRect::new(0.0, 0.0, 0.5, 0.5), width: 3, height: 2, data: vec![0.0, 0.5, 1.0, 1.0, 0.25, 0.0], split: true }],
        );
        Parsing { model_set: "bisenet".into(), model_version: "v".into(), width: 40, height: 30, people: vec![person] }
    }

    #[test]
    fn a_parse_comes_back_from_its_bytes() {
        let p = sample();
        let back = decode(&encode(&p).unwrap()).unwrap();
        assert_eq!(back.people.len(), 1);
        assert_eq!(back.people[0].landmarks.len(), 478);
        assert_eq!(back.people[0].person_id.as_deref(), Some("p1"));
        let (a, b) = (&p.people[0].parts[&Part::Hair][0], &back.people[0].parts[&Part::Hair][0]);
        assert_eq!((a.width, a.height, a.rect, a.split), (b.width, b.height, b.rect, b.split));
        assert!(a.data.iter().zip(&b.data).all(|(x, y)| (x - y).abs() <= 0.5 / 255.0 + 1e-6));
    }

    #[test]
    fn damaged_files_are_errors_not_panics() {
        let good = encode(&sample()).unwrap();
        assert!(decode(&[]).is_err());
        assert!(decode(b"LCPARSE1").is_err());
        for cut in [9, 12, 20, good.len() / 2, good.len() - 1] {
            assert!(decode(&good[..cut]).is_err(), "{cut}");
        }
        let mut extra = good.clone();
        extra.push(0);
        assert!(decode(&extra).is_err());
        let mut huge = good.clone();
        huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&huge).is_err());
        let mut flipped = good;
        let last = flipped.len() - 20;
        flipped[last] ^= 0xff;
        let _ = decode(&flipped);
    }

    #[test]
    fn planes_larger_than_a_parse_makes_are_refused() {
        let mut wide = sample();
        wide.people[0]
            .parts
            .insert(Part::Hat, vec![Plane { rect: NormRect::new(0.0, 0.0, 1.0, 1.0), width: 1600, height: 1, data: vec![0.5; 1600], split: false }]);
        assert!(decode(&encode(&wide).unwrap()).is_err());
        let ok = encode(&sample()).unwrap();
        assert!(decode_capped(&ok, 6).is_ok());
        assert!(decode_capped(&ok, 5).is_err());
    }

    #[test]
    fn store_load_and_prune() {
        let dir = crate::tests::scratch("cache");
        let img = RgbImage::new(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap();
        let k = key(&img, &[], "set/1");
        assert!(!k.contains('/'));
        assert_ne!(k, key(&img, &[], "set/2"));
        assert!(load(&dir, &k).unwrap().is_none());
        store(&dir, &k, &sample()).unwrap();
        assert_eq!(load(&dir, &k).unwrap().unwrap().people.len(), 1);
        std::fs::write(path(&dir, "broken"), b"LCPARSE1junk").unwrap();
        assert!(load(&dir, "broken").is_err());
        assert_eq!(prune(&dir, 0), 2);
        assert!(load(&dir, &k).unwrap().is_none());
    }
}
