//! The repairs made so far, per photo: in memory, and on disk (`<cache>/glare-repair/<hash>.lcgr`) so they survive a
//! restart. Keyed by the photo's faces key (`Session::retouch_key`: its content, orientation and parse version), and
//! within it by the face's key box, so faces found again get their repair back.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_develop::retouch::input::Patch;
use lightcraft_develop::retouch::{Face, Input, NormRect};
use lightcraft_raster::{Plane, Rgb32f};

const MAGIC: &[u8; 8] = b"LCGLARE1";
/// Largest side of a stored patch (a damaged file can't ask for more).
const MAX_SIDE: usize = 2048;
/// Photos whose repairs stay in memory (each about 10 MB per face).
const KEEP: usize = 16;

/// How a repair was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// FLUX.1 Fill over the lenses glare hides (minutes).
    Flux,
    /// LaMa over the glints (seconds).
    Lama,
}

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::Flux => "flux",
            Method::Lama => "lama",
        }
    }

    pub fn from_name(s: &str) -> Option<Method> {
        match s {
            "flux" => Some(Method::Flux),
            "lama" => Some(Method::Lama),
            _ => None,
        }
    }
}

/// One face's repair.
#[derive(Clone, Debug)]
pub struct Saved {
    /// The face's key box ([`Face::key_bounds`]) when it was made.
    pub key_box: NormRect,
    pub method: Method,
    pub patch: Arc<Patch>,
}

/// The repairs of every photo, shared by the face store (which hands them to renders) and the repair jobs.
#[derive(Clone, Default)]
pub struct Repairs {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    dir: Option<PathBuf>,
    /// (faces key, repairs), used least recently first.
    kept: Vec<(String, Vec<Saved>)>,
}

pub(crate) fn iou(a: &NormRect, b: &NormRect) -> f64 {
    let ix = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
    let iy = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
    let inter = ix * iy;
    let union = a.width() * a.height() + b.width() * b.height() - inter;
    if union > 0.0 && inter.is_finite() { inter / union } else { 0.0 }
}

/// FNV-1a of `s`, for file names.
fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

impl Repairs {
    /// Keep repairs on disk in `dir` (`None`: memory only).
    pub fn set_dir(&self, dir: Option<PathBuf>) {
        let mut st = self.lock();
        st.dir = dir;
        st.kept.clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn file(dir: &Path, key: &str) -> PathBuf {
        dir.join(format!("{:016x}.lcgr", hash(key)))
    }

    /// The repairs of faces key `key` (read from disk the first time).
    pub fn of(&self, key: &str) -> Vec<Saved> {
        Self::of_in(&mut self.lock(), key)
    }

    fn of_in(st: &mut Inner, key: &str) -> Vec<Saved> {
        if let Some(i) = st.kept.iter().position(|(k, _)| k == key) {
            let hit = st.kept.remove(i);
            let out = hit.1.clone();
            st.kept.push(hit);
            return out;
        }
        let read = st.dir.as_ref().and_then(|d| std::fs::read(Self::file(d, key)).ok()).and_then(|b| decode(&b, key)).unwrap_or_default();
        st.kept.push((key.to_string(), read.clone()));
        let extra = st.kept.len().saturating_sub(KEEP);
        st.kept.drain(..extra);
        read
    }

    /// Keep `saved` for `key`, replacing the repair of the same face; written to disk. An error says why it could
    /// not be written (it is kept in memory anyway). Read, change and write happen under one lock, so two changes
    /// at once (a repair kept while another face's is cleared) never lose one another.
    pub fn add(&self, key: &str, saved: Saved) -> Result<(), String> {
        let mut st = self.lock();
        let mut list = Self::of_in(&mut st, key);
        list.retain(|s| iou(&s.key_box, &saved.key_box) < 0.5);
        list.push(saved);
        Self::put_in(&mut st, key, list)
    }

    /// Drop the repair of the face whose key box is `key_box` (`None`: every face of the photo).
    pub fn clear(&self, key: &str, key_box: Option<&NormRect>) -> Result<(), String> {
        let mut st = self.lock();
        let mut list = Self::of_in(&mut st, key);
        match key_box {
            Some(b) => list.retain(|s| iou(&s.key_box, b) < 0.5),
            None => list.clear(),
        }
        Self::put_in(&mut st, key, list)
    }

    /// Replaces the repairs of `key` in memory and on disk (the lock is held by the caller across the write, so the
    /// file always holds the latest list).
    fn put_in(st: &mut Inner, key: &str, list: Vec<Saved>) -> Result<(), String> {
        st.kept.retain(|(k, _)| k != key);
        st.kept.push((key.to_string(), list.clone()));
        let Some(dir) = st.dir.clone() else { return Ok(()) };
        let path = Self::file(&dir, key);
        if list.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
                _ => Ok(()),
            };
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, encode(key, &list)).and_then(|()| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// `input` with each face's repair from `key` attached (the face whose key box overlaps a repair's most);
    /// `None` when nothing changes.
    pub fn attach(&self, key: &str, input: &Input) -> Option<Input> {
        let saved = self.of(key);
        let mut changed = false;
        let faces: Vec<Face> = input
            .faces
            .iter()
            .map(|f| {
                let kb = f.key_bounds();
                let best = saved.iter().map(|s| (iou(&s.key_box, &kb), s)).filter(|(o, _)| *o >= 0.5).max_by(|a, b| a.0.total_cmp(&b.0));
                let patch = best.map(|(_, s)| s.patch.clone());
                if patch.as_ref().map(|p| p.stamp) != f.patch.as_ref().map(|p| p.stamp) {
                    changed = true;
                }
                Face { patch, ..f.clone() }
            })
            .collect();
        changed.then(|| if input.on_photo { Input::new_on_photo(faces) } else { Input::new(faces) })
    }

    /// The method of the repair attached to each face of `input`, by face.
    pub fn methods(&self, key: &str, input: &Input) -> Vec<Option<Method>> {
        let saved = self.of(key);
        input
            .faces
            .iter()
            .map(|f| {
                let stamp = f.patch.as_ref()?.stamp;
                saved.iter().find(|s| s.patch.stamp == stamp).map(|s| s.method)
            })
            .collect()
    }
}

fn put_f64(out: &mut Vec<u8>, v: f64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn encode(key: &str, list: &[Saved]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(key.len() as u64).to_le_bytes());
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(&(list.len() as u64).to_le_bytes());
    for s in list {
        for r in [&s.key_box, &s.patch.area] {
            for v in [r.x0, r.y0, r.x1, r.y1] {
                put_f64(&mut out, v);
            }
        }
        out.extend_from_slice(&s.patch.stamp.to_le_bytes());
        out.push(match s.method {
            Method::Flux => 0,
            Method::Lama => 1,
        });
        out.extend_from_slice(&(s.patch.alpha.width as u64).to_le_bytes());
        out.extend_from_slice(&(s.patch.alpha.height as u64).to_le_bytes());
        for v in s.patch.gain.data.iter().flatten().chain(&s.patch.alpha.data) {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// Reads [`encode`]; `None` for another key's file, a damaged one, or an old format.
fn decode(b: &[u8], key: &str) -> Option<Vec<Saved>> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = b.get(at..at.checked_add(n)?)?;
        at += n;
        Some(s)
    };
    if take(8)? != MAGIC {
        return None;
    }
    let u64_ = |s: &[u8]| -> Option<u64> { Some(u64::from_le_bytes(s.try_into().ok()?)) };
    let klen = usize::try_from(u64_(take(8)?)?).ok()?;
    if take(klen)? != key.as_bytes() {
        return None;
    }
    let n = usize::try_from(u64_(take(8)?)?).ok()?;
    if n > 1000 {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let rect = |take: &mut dyn FnMut(usize) -> Option<Vec<u8>>| -> Option<NormRect> {
            let mut v = [0f64; 4];
            for x in &mut v {
                *x = f64::from_le_bytes(take(8)?.try_into().ok()?);
            }
            let r = NormRect::new(v[0], v[1], v[2], v[3]);
            r.is_valid().then_some(r)
        };
        let mut owned = |n: usize| take(n).map(<[u8]>::to_vec);
        let key_box = rect(&mut owned)?;
        let area = rect(&mut owned)?;
        let stamp = u64_(&owned(8)?)?;
        let method = match owned(1)?.first()? {
            0 => Method::Flux,
            1 => Method::Lama,
            _ => return None,
        };
        let w = usize::try_from(u64_(&owned(8)?)?).ok()?;
        let h = usize::try_from(u64_(&owned(8)?)?).ok()?;
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
            return None;
        }
        let px = w * h;
        let floats = owned(px.checked_mul(16)?)?;
        let vals: Vec<f32> = floats.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
        let (g, a) = vals.split_at_checked(px * 3)?;
        let gain = Rgb32f { width: w, height: h, data: g.as_chunks::<3>().0.to_vec() };
        let alpha = Plane { width: w, height: h, data: a.to_vec() };
        out.push(Saved { key_box, method, patch: Arc::new(Patch { area, gain, alpha, stamp }) });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(x: f64, stamp: u64) -> Saved {
        let patch = Patch {
            area: NormRect::new(x, 0.2, x + 0.2, 0.4),
            gain: Rgb32f::from_fn(3, 2, |x, y| [x as f32, y as f32, 0.5]),
            alpha: Plane::from_fn(3, 2, |x, _| x as f32 / 2.0),
            stamp,
        };
        Saved { key_box: NormRect::new(x, 0.1, x + 0.2, 0.5), method: Method::Flux, patch: Arc::new(patch) }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-glare-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn repairs_survive_a_restart_and_follow_their_face() {
        let d = dir("restart");
        let r = Repairs::default();
        r.set_dir(Some(d.clone()));
        r.add("photo", saved(0.1, 1)).unwrap();
        r.add("photo", saved(0.6, 2)).unwrap();
        // the same face again replaces its repair
        r.add("photo", saved(0.1, 3)).unwrap();
        let again = Repairs::default();
        again.set_dir(Some(d.clone()));
        let read = again.of("photo");
        assert_eq!(read.len(), 2);
        assert_eq!(read.iter().map(|s| s.patch.stamp).collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(read[1].patch.gain, saved(0.1, 3).patch.gain);
        assert_eq!(read[1].patch.alpha, saved(0.1, 3).patch.alpha);
        // faces found in another order get theirs; a face with none gets none
        let faces = vec![
            Face::boxed(NormRect::new(0.61, 0.1, 0.8, 0.5)),
            Face::boxed(NormRect::new(0.3, 0.6, 0.4, 0.9)),
            Face::boxed(NormRect::new(0.1, 0.1, 0.3, 0.5)),
        ];
        let input = again.attach("photo", &Input::new(faces)).unwrap();
        let stamps: Vec<Option<u64>> = input.faces.iter().map(|f| f.patch.as_ref().map(|p| p.stamp)).collect();
        assert_eq!(stamps, vec![Some(2), None, Some(3)]);
        assert_eq!(again.methods("photo", &input), vec![Some(Method::Flux), None, Some(Method::Flux)]);
        // nothing new: no new input
        assert!(again.attach("photo", &input).is_none());
        again.clear("photo", Some(&NormRect::new(0.6, 0.1, 0.8, 0.5))).unwrap();
        assert_eq!(again.of("photo").len(), 1);
        again.clear("photo", None).unwrap();
        assert!(!Repairs::file(&d, "photo").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Faces' repairs kept at once (and a clear running beside them) never lose one another: every face stays.
    #[test]
    fn adds_and_clears_at_once_lose_nothing() {
        let r = Repairs::default();
        r.set_dir(None);
        let n = 24;
        std::thread::scope(|s| {
            for i in 0..n {
                let r = &r;
                s.spawn(move || {
                    let mut one = saved(0.0, i as u64 + 1);
                    // narrow boxes side by side: no face is another's
                    one.key_box = NormRect::new(i as f64 * 0.04, 0.1, i as f64 * 0.04 + 0.03, 0.5);
                    r.add("photo", one).unwrap();
                    // a clear of a face nobody has, at the same time
                    r.clear("photo", Some(&NormRect::new(5.0, 5.0, 6.0, 6.0))).unwrap();
                });
            }
        });
        assert_eq!(r.of("photo").len(), n);
    }

    #[test]
    fn damaged_or_foreign_files_read_as_nothing() {
        let list = vec![saved(0.1, 1)];
        let good = encode("a", &list);
        assert_eq!(decode(&good, "a").map(|v| v.len()), Some(1));
        assert!(decode(&good, "b").is_none());
        for cut in [0, 7, 9, 30, good.len() - 1] {
            assert!(decode(&good[..cut], "a").is_none(), "cut at {cut}");
        }
        let mut huge = good.clone();
        let at = good.len() - 6 * 16 - 16;
        huge[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(decode(&huge, "a").is_none());
        assert!(decode(b"LCGLARE1", "a").is_none());
    }
}
