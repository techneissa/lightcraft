//! The Core Image backend: every Objective-C call of the crate, so all its `unsafe`.
//!
//! Rules (plan/colour/macos-base-plan.md §2.2): one autorelease pool per render, and every
//! Objective-C object a render touches is created and dropped inside it, so a rayon worker that
//! lives for the whole process leaks nothing; one `CIRAWFilter` per render, never shared; one shared
//! `CIContext` (Core Image documents contexts as thread-safe, and objc2 marks `CIContext`
//! `Send + Sync`); Objective-C exceptions are caught at each call site by `objc2::exception::catch`,
//! never unwound through Core Image or Metal. No Rust callback is handed to Core Image, so a Rust
//! panic cannot start inside Apple's frames.

use crate::copy::{BAND_ROWS, bands, rgba_to_rgb};
use crate::size::{extent_size, scaled};
use crate::{Error, Input, LinearImage, Request};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol, Sel};
use objc2::{ClassType, exception, msg_send, sel};
use objc2_core_foundation::{CFRetained, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceExtendedLinearITUR_2020};
use objc2_core_image::{
    CIContext, CIFilter, CIImage, CIRAWFilter, kCIContextOutputColorSpace, kCIContextUseSoftwareRenderer, kCIContextWorkingColorSpace,
    kCIFormatRGBAf, kCIInputAspectRatioKey, kCIInputImageKey, kCIInputScaleKey,
};
use objc2_foundation::{NSData, NSDictionary, NSNumber, NSObjectNSKeyValueCoding, NSOperatingSystemVersion, NSProcessInfo, NSString, NSURL};
use std::panic::AssertUnwindSafe;
use std::ptr::NonNull;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// Compile-time check (T16c): the shared context may live in a static and be used from any thread.
const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    send_sync::<Retained<CIContext>>();
};

/// The selectors the recipe cannot do without. If one is missing the recipe would silently differ
/// from what was measured, so the crate reports itself unavailable instead of rendering half of it.
fn required() -> [Sel; 15] {
    [
        sel!(setBoostAmount:),
        sel!(setShadowBias:),
        sel!(setExtendedDynamicRangeAmount:),
        sel!(setGamutMappingEnabled:),
        sel!(setLuminanceNoiseReductionAmount:),
        sel!(setColorNoiseReductionAmount:),
        sel!(setSharpnessAmount:),
        sel!(setContrastAmount:),
        sel!(setDetailAmount:),
        sel!(setMoireReductionAmount:),
        sel!(neutralChromaticity),
        sel!(neutralTemperature),
        sel!(neutralTint),
        sel!(decoderVersion),
        sel!(outputImage),
    ]
}

/// Set when a filter instance lacked a required selector: from then on [`available`] is false.
static MISSING: AtomicBool = AtomicBool::new(false);
static SUPPORTED_OS: OnceLock<bool> = OnceLock::new();

pub(crate) fn available() -> bool {
    !MISSING.load(Ordering::Relaxed)
        && *SUPPORTED_OS.get_or_init(|| {
            autoreleasepool(|_| {
                let at_least_12 = NSOperatingSystemVersion { majorVersion: 12, minorVersion: 0, patchVersion: 0 };
                NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(at_least_12)
                    && AnyClass::get(c"CIRAWFilter").is_some_and(|class| required().iter().all(|s| class.responds_to(*s)))
            })
        })
}

/// The OS version and build ("Version 27.0.1 (Build 27A…)").
pub(crate) fn os_build() -> String {
    autoreleasepool(|_| NSProcessInfo::processInfo().operatingSystemVersionString().to_string())
}

/// Turn an Objective-C exception into an error at the call site.
fn guarded<R>(what: &str, f: impl FnOnce() -> R) -> Result<R, Error> {
    exception::catch(AssertUnwindSafe(f)).map_err(|e| match e {
        Some(e) => Error::Os(format!("{what}: {e:?}")),
        None => Error::Os(format!("{what}: an Objective-C exception")),
    })
}

// ---- contexts ----

/// GPU first. If no GPU render has worked yet and one fails inside Core Image, the render is tried
/// once with the software renderer, and the session keeps it when that works.
static GPU: OnceLock<Option<Retained<CIContext>>> = OnceLock::new();
static CPU: OnceLock<Option<Retained<CIContext>>> = OnceLock::new();
static GPU_WORKED: AtomicBool = AtomicBool::new(false);
static USE_SOFTWARE: AtomicBool = AtomicBool::new(false);

fn space() -> Option<CFRetained<CGColorSpace>> {
    // SAFETY: reading an immutable `CFString` constant exported by CoreGraphics.
    let name = unsafe { kCGColorSpaceExtendedLinearITUR_2020 };
    CGColorSpace::with_name(Some(name))
}

fn make_context(software: bool) -> Option<Retained<CIContext>> {
    let space = space()?;
    let space: &CFType = &space;
    let space: &AnyObject = space.as_ref();
    let yes = NSNumber::new_bool(true);
    // SAFETY: reading immutable `NSString` option keys exported by Core Image.
    let (working, output, soft) = unsafe { (kCIContextWorkingColorSpace, kCIContextOutputColorSpace, kCIContextUseSoftwareRenderer) };
    let mut keys = vec![working, output];
    let mut values: Vec<&AnyObject> = vec![space, space];
    if software {
        keys.push(soft);
        values.push(yes.as_ref());
    }
    let options = NSDictionary::from_slices(&keys, &values);
    // SAFETY: the options are the documented key types: colour spaces are `CGColorSpace`, the
    // software flag an `NSNumber` boolean. Sent with an optional result, as the binding's
    // non-null type would panic on null.
    let ctx: Option<Retained<CIContext>> =
        guarded("creating the Core Image context", || unsafe { msg_send![CIContext::class(), contextWithOptions: &*options] }).ok()?;
    ctx
}

fn context(software: bool) -> Option<&'static CIContext> {
    let cell = if software { &CPU } else { &GPU };
    cell.get_or_init(|| autoreleasepool(|_| make_context(software))).as_deref()
}

// ---- the filter ----

fn open(input: Input<'_>) -> Result<Retained<CIRAWFilter>, Error> {
    let filter = match input {
        Input::Path(path) => {
            let url = NSURL::from_file_path(path).ok_or(Error::Undecodable)?;
            // SAFETY: a plain class-method call with a valid `NSURL`; the result is retained.
            guarded("opening the raw", || unsafe { CIRAWFilter::filterWithImageURL(&url) })?
        }
        Input::Bytes { data, uti } => {
            let data = NSData::with_bytes(data);
            let hint = NSString::from_str(uti);
            // SAFETY: a plain class-method call with a valid `NSData` (a copy of `data`) and hint.
            guarded("opening the raw", || unsafe { CIRAWFilter::filterWithImageData_identifierHint(&data, Some(&hint)) })?
        }
    };
    filter.ok_or(Error::Undecodable)
}

/// The pinned recipe (§2.3). Required setters were checked; optional ones are skipped when missing.
fn apply_recipe(f: &CIRAWFilter) -> Result<(), Error> {
    if !required().iter().all(|s| f.respondsToSelector(*s)) {
        MISSING.store(true, Ordering::Relaxed);
        return Err(Error::Unsupported);
    }
    let has = |s: Sel| f.respondsToSelector(s);
    guarded("setting up the raw decoder", || {
        // SAFETY: setters of float/bool properties on a filter we own; each selector was checked
        // above (required) or is checked right before the call (optional).
        unsafe {
            f.setBoostAmount(0.0);
            f.setShadowBias(0.0);
            f.setExtendedDynamicRangeAmount(1.0);
            f.setGamutMappingEnabled(false);
            f.setLuminanceNoiseReductionAmount(0.0);
            f.setColorNoiseReductionAmount(0.0);
            f.setSharpnessAmount(0.0);
            f.setContrastAmount(0.0);
            f.setDetailAmount(0.0);
            f.setMoireReductionAmount(0.0);
            if has(sel!(setBoostShadowAmount:)) {
                f.setBoostShadowAmount(0.0);
            }
            if has(sel!(isLensCorrectionSupported)) && has(sel!(setLensCorrectionEnabled:)) && f.isLensCorrectionSupported() {
                f.setLensCorrectionEnabled(false);
            }
            if has(sel!(isLocalToneMapSupported)) && has(sel!(setLocalToneMapAmount:)) && f.isLocalToneMapSupported() {
                f.setLocalToneMapAmount(0.0);
            }
        }
        if has(sel!(setDespeckleAmount:)) {
            // SAFETY: `despeckleAmount` is `@property(readwrite) float` in CIRAWFilter.h (macOS 27
            // SDK) and the objc2 binding lacks it; the selector was just checked, the argument is
            // an `f32` (C `float`) and the setter returns nothing.
            let () = unsafe { msg_send![f, setDespeckleAmount: 0.0f32] };
        }
    })
}

/// A picture Core Image will produce, with what LightCraft needs to know about it.
struct Planned {
    image: Retained<CIImage>,
    /// Bitmap origin (Core Image coordinates, y up) and size.
    origin: (f64, f64),
    size: (u32, u32),
    native: (u32, u32),
    white_xy: [f64; 2],
    white_kelvin: f32,
    white_tint: f32,
    decoder_version: String,
}

fn plan(input: Input<'_>, request: &Request) -> Result<Planned, Error> {
    let f = open(input)?;
    apply_recipe(&f)?;
    // SAFETY: getters on a filter we own; each selector is required and was checked. The binding
    // types `decoderVersion` as never null, but Core Image returns null for data it cannot read
    // (the binding would then panic), so it is sent with an optional result.
    let (white, kelvin, tint, version) = guarded("reading the white balance", || unsafe {
        let version: Option<Retained<NSString>> = msg_send![&*f, decoderVersion];
        (f.neutralChromaticity(), f.neutralTemperature(), f.neutralTint(), version)
    })?;
    let version = version.ok_or(Error::Undecodable)?;
    // SAFETY: `outputImage` is required and was checked; the image is retained.
    let image = guarded("decoding the raw", || unsafe { f.outputImage() })?.ok_or(Error::Undecodable)?;
    // SAFETY: a getter on an image we own.
    let extent = unsafe { image.extent() };
    let native = extent_size(extent.size.width, extent.size.height).ok_or(Error::Undecodable)?;
    if !(white.x.is_finite() && white.y.is_finite() && white.x > 0.0 && white.y > 0.0) {
        return Err(Error::Undecodable);
    }
    let size = scaled(native, request.long_edge)?;
    let image = if size == native { image } else { lanczos(&image, native, size)? };
    // SAFETY: a getter on an image we own.
    let out = unsafe { image.extent() };
    Ok(Planned {
        image,
        origin: (out.origin.x.round(), out.origin.y.round()),
        size,
        native,
        white_xy: [white.x, white.y],
        white_kelvin: kelvin,
        white_tint: tint,
        decoder_version: version.to_string(),
    })
}

/// Lanczos-resample `image` from `from` to exactly `to` pixels (`CILanczosScaleTransform`).
fn lanczos(image: &CIImage, from: (u32, u32), to: (u32, u32)) -> Result<Retained<CIImage>, Error> {
    let sx = f64::from(to.0) / f64::from(from.0);
    let sy = f64::from(to.1) / f64::from(from.1);
    let scale = NSNumber::new_f64(sx);
    let aspect = NSNumber::new_f64(sy / sx);
    // SAFETY: a class-method call with no arguments, sent with an optional result (the binding's
    // non-null type would panic on null); the new filter is retained and ours.
    let lz: Option<Retained<CIFilter>> = guarded("scaling the raw", || unsafe { msg_send![CIFilter::class(), lanczosScaleTransformFilter] })?;
    let lz = lz.ok_or_else(|| Error::Os("no Lanczos scale filter".into()))?;
    // SAFETY: reading immutable `NSString` key constants exported by Core Image.
    let (k_image, k_scale, k_aspect) = unsafe { (kCIInputImageKey, kCIInputScaleKey, kCIInputAspectRatioKey) };
    guarded("scaling the raw", || {
        // SAFETY: key-value setters with the documented types of CILanczosScaleTransform (a
        // `CIImage` input, `NSNumber` scale and aspect ratio) on a filter we own.
        unsafe {
            lz.setValue_forKey(Some(image.as_ref()), k_image);
            lz.setValue_forKey(Some(scale.as_ref()), k_scale);
            lz.setValue_forKey(Some(aspect.as_ref()), k_aspect);
            lz.outputImage()
        }
    })?
    .ok_or(Error::Undecodable)
}

/// Render `planned` into a new 3-float buffer, band by band through one RGBA scratch.
fn draw(planned: &Planned, ctx: &CIContext, band_rows: u32) -> Result<Vec<f32>, Error> {
    let (w, h) = planned.size;
    let too_large = || Error::TooLarge { width: w, height: h };
    let px = usize::try_from(u64::from(w) * u64::from(h)).map_err(|_| too_large())?;
    let mut rgb = vec![0f32; px.checked_mul(3).ok_or_else(too_large)?];
    let band_rows = band_rows.clamp(1, h.max(1));
    let row = usize::try_from(w).map_err(|_| too_large())?;
    let mut scratch = vec![0f32; row.checked_mul(4).and_then(|r| r.checked_mul(band_rows as usize)).ok_or_else(too_large)?];
    let space = space().ok_or_else(|| Error::Os("no extended linear Rec.2020 colour space".into()))?;
    // SAFETY: reading an immutable constant exported by Core Image.
    let format = unsafe { kCIFormatRGBAf };
    let row_bytes = isize::try_from(row * 16).map_err(|_| too_large())?;
    for (top, rows) in bands(h, band_rows) {
        let len = row * 4 * rows as usize;
        let Some(band) = scratch.get_mut(..len) else { return Err(too_large()) };
        // Core Image's y axis points up: the band `top..top + rows` (rows counted from the top)
        // starts `h - top - rows` above the bottom edge. The bitmap's first row is its top.
        let y = planned.origin.1 + f64::from(h - top - rows);
        let bounds = CGRect::new(CGPoint::new(planned.origin.0, y), CGSize::new(f64::from(w), f64::from(rows)));
        let ptr = NonNull::from(&mut *band).cast();
        guarded("rendering the raw", || {
            // SAFETY: `band` is `w × rows × 4` floats = `rows × row_bytes` bytes, exactly the
            // RGBAf bitmap of `bounds` (`w` wide, `rows` tall) with `row_bytes = w × 16`, so Core
            // Image writes inside it; it stays borrowed mutably (and alive) for the whole call.
            unsafe { ctx.render_toBitmap_rowBytes_bounds_format_colorSpace(&planned.image, ptr, row_bytes, bounds, format, Some(&space)) }
        })?;
        let start = row * 3 * top as usize;
        let Some(dst) = rgb.get_mut(start..start + row * 3 * rows as usize) else { return Err(too_large()) };
        rgba_to_rgb(band, dst);
    }
    Ok(rgb)
}

/// Where a render failed: reading the file, or drawing with a context (the only case worth a
/// software retry).
enum Failed {
    File(Error),
    Draw(Error),
}

fn render_in_pool(input: Input<'_>, request: &Request, software: bool, band_rows: u32) -> Result<LinearImage, Failed> {
    autoreleasepool(|_| {
        let planned = plan(input, request).map_err(Failed::File)?;
        let ctx = context(software).ok_or_else(|| Failed::Draw(Error::Os("no Core Image context".into())))?;
        let rgb = draw(&planned, ctx, band_rows).map_err(Failed::Draw)?;
        Ok(LinearImage {
            width: planned.size.0,
            height: planned.size.1,
            rgb,
            white_xy: planned.white_xy,
            white_kelvin: planned.white_kelvin,
            white_tint: planned.white_tint,
            native_size: planned.native,
            decoder_version: planned.decoder_version,
        })
    })
}

pub(crate) fn render(input: Input<'_>, request: &Request, cancel: &AtomicBool) -> Result<LinearImage, Error> {
    render_bands(input, request, cancel, BAND_ROWS)
}

pub(crate) fn render_bands(input: Input<'_>, request: &Request, cancel: &AtomicBool, band_rows: u32) -> Result<LinearImage, Error> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    if !available() {
        return Err(Error::Unsupported);
    }
    let software = USE_SOFTWARE.load(Ordering::Relaxed);
    let out = match render_in_pool(input, request, software, band_rows) {
        Ok(img) => {
            if !software {
                GPU_WORKED.store(true, Ordering::Relaxed);
            }
            Ok(img)
        }
        Err(Failed::Draw(_)) if !software && !GPU_WORKED.load(Ordering::Relaxed) => match render_in_pool(input, request, true, band_rows) {
            Ok(img) => {
                USE_SOFTWARE.store(true, Ordering::Relaxed);
                Ok(img)
            }
            Err(Failed::File(e) | Failed::Draw(e)) => Err(e),
        },
        Err(Failed::File(e) | Failed::Draw(e)) => Err(e),
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    out
}

#[cfg(test)]
#[path = "mac_tests.rs"]
mod tests;
