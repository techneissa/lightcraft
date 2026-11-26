//! SCRATCH for issue #438, never merged: what the output would be if mask alphas (and brush parts) were stored smaller.
//!
//! The CPU pipeline rounds a value to the storage format and reads it straight back as f32 where the app would store it.
//! The GPU renderer does the same with real packed words (`wgsl/maskpack.wgsl`). `LC_MASK_STORE` = f32 (default), u16,
//! u8 or f16; [`set`] changes it inside one process.

use lightcraft_raster::Plane;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    F32,
    U16,
    U8,
    F16,
    /// u16, except the masks with Decontaminate on (kept f32)
    U16ButDecontam,
}

static FORMAT: AtomicU8 = AtomicU8::new(u8::MAX);

pub fn set(f: Format) {
    FORMAT.store(f as u8, Ordering::SeqCst);
}

pub fn get() -> Format {
    match FORMAT.load(Ordering::SeqCst) {
        0 => Format::F32,
        1 => Format::U16,
        2 => Format::U8,
        3 => Format::F16,
        4 => Format::U16ButDecontam,
        _ => {
            let f = match std::env::var("LC_MASK_STORE").as_deref() {
                Ok("u16") => Format::U16,
                Ok("u8") => Format::U8,
                Ok("f16") => Format::F16,
                _ => Format::F32,
            };
            set(f);
            f
        }
    }
}

fn round(v: f32, f: Format) -> f32 {
    match f {
        Format::F32 => v,
        Format::U16 | Format::U16ButDecontam => (v.clamp(0.0, 1.0) * 65535.0 + 0.5).floor() / 65535.0,
        Format::U8 => (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() / 255.0,
        Format::F16 => half::f16::from_f32(v).to_f32(),
    }
}

/// A mask's final alpha: left f32 for a decontaminated mask under [`Format::U16ButDecontam`].
pub(crate) fn store_mask(p: &mut Plane, decontaminated: bool) {
    if !(decontaminated && get() == Format::U16ButDecontam) {
        store(p);
    }
}

pub(crate) fn store(p: &mut Plane) {
    let f = get();
    if f != Format::F32 {
        p.data.iter_mut().for_each(|v| *v = round(*v, f));
    }
}
