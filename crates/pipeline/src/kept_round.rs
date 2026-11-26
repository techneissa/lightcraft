//! SCRATCH PROTOTYPE for issue #417, never merged: what the output would be if kept data were stored smaller.
//!
//! A value is rounded to the storage format and read straight back as f32 where the app would store it, so the render
//! is what a store of that format gives. Set per process:
//! `LC_KEEP_IMG` (the original, the resampled and linear images, the linear planes), `LC_KEEP_LOG` (the log-luminance
//! planes) and `LC_KEEP_MASK` (mask alphas and brush parts). Values: `f32` (default), `f16`, `f16rtz` (rounded toward
//! zero, as a GPU may), and for masks also `u16`, `u8`.

use lightcraft_raster::{Plane, Rgb32f};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Format {
    F32,
    F16,
    F16Rtz,
    U16,
    U8,
}

fn read(var: &str) -> Format {
    let f = match std::env::var(var).as_deref() {
        Ok("f16") => Format::F16,
        Ok("f16rtz") => Format::F16Rtz,
        Ok("u16") => Format::U16,
        Ok("u8") => Format::U8,
        _ => Format::F32,
    };
    eprintln!("kept_round: {var} = {f:?}");
    f
}

fn formats() -> (Format, Format, Format) {
    static F: OnceLock<(Format, Format, Format)> = OnceLock::new();
    *F.get_or_init(|| (read("LC_KEEP_IMG"), read("LC_KEEP_LOG"), read("LC_KEEP_MASK")))
}

#[inline]
fn round(v: f32, f: Format) -> f32 {
    match f {
        Format::F32 => v,
        Format::F16 => half::f16::from_f32(v).to_f32(),
        Format::F16Rtz => {
            let h = half::f16::from_f32(v);
            let back = h.to_f32();
            if back.abs() > v.abs() { half::f16::from_bits(h.to_bits().wrapping_sub(1)).to_f32() } else { back }
        }
        Format::U16 => (v.clamp(0.0, 1.0) * 65535.0).round() / 65535.0,
        Format::U8 => (v.clamp(0.0, 1.0) * 255.0).round() / 255.0,
    }
}

/// `LC_DUMP=dir`: write each kept buffer (before rounding) as raw little-endian f32.
fn dump(kind: &str, data: &[f32]) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let Some(dir) = std::env::var_os("LC_DUMP") else { return };
    let n = N.fetch_add(1, Ordering::SeqCst);
    let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    if let Err(e) = std::fs::write(std::path::Path::new(&dir).join(format!("{n:02}-{kind}.f32")), bytes) {
        eprintln!("kept_round: dump failed: {e}");
    }
}

fn slice(data: &mut [f32], f: Format) {
    if f != Format::F32 {
        data.iter_mut().for_each(|v| *v = round(*v, f));
    }
}

/// Whether images are stored smaller (so the original needs a rounded copy).
pub(crate) fn images_rounded() -> bool {
    formats().0 != Format::F32 || std::env::var_os("LC_DUMP").is_some()
}

pub(crate) fn img(i: &mut Rgb32f) {
    dump("image", i.data.as_flattened());
    slice(i.data.as_flattened_mut(), formats().0);
}

pub(crate) fn linear_plane(p: &mut Plane) {
    dump("plane", &p.data);
    slice(&mut p.data, formats().0);
}

pub(crate) fn log_plane(mut p: Plane) -> Plane {
    dump("logplane", &p.data);
    slice(&mut p.data, formats().1);
    p
}

pub(crate) fn mask(p: &mut Plane) {
    dump("mask", &p.data);
    slice(&mut p.data, formats().2);
}
