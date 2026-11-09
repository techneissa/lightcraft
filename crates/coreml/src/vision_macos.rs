//! The Apple Vision backend: every Objective-C and Core Video call of [`crate::vision`], so all its `unsafe`.
//!
//! The same rules as `macos.rs` (README.md): every call runs in an autorelease pool; Objective-C exceptions are not
//! caught, so Vision is never given a reason to throw: [`supported`] checks the macOS version and, with
//! `respondsToSelector:`, every class and selector used before any is sent; only indexes Vision listed are asked for;
//! an output pixel format is set only when Vision lists it. Pixel buffers are read and written only inside their
//! locked base address, row by row, after checking their format, size and row length.

use std::ptr::NonNull;
use std::sync::OnceLock;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{AnyThread, sel};
use objc2_core_foundation::{CFDictionary, CFRetained, CFString};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetDataSize, CVPixelBufferGetHeight,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferIsPlanar, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelFormatType_32BGRA, kCVPixelFormatType_OneComponent8,
    kCVPixelFormatType_OneComponent16Half, kCVPixelFormatType_OneComponent32Float, kCVReturnSuccess,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSIndexSet, NSOperatingSystemVersion, NSProcessInfo, NSUInteger};
use objc2_vision::{
    VNGenerateForegroundInstanceMaskRequest, VNGeneratePersonInstanceMaskRequest, VNGeneratePersonSegmentationRequest,
    VNGeneratePersonSegmentationRequestQualityLevel, VNImageOption, VNImageRequestHandler, VNInstanceMaskObservation, VNRequest,
};

use crate::layout::f16_to_f32;
use crate::vision::{Image, MAX_INSTANCES, Matte, Request};
use crate::{Error, Result};

fn at_least(major: isize) -> bool {
    let v = NSOperatingSystemVersion { majorVersion: major, minorVersion: 0, patchVersion: 0 };
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(v)
}

/// (class, instance selector) pairs every request uses.
fn common() -> [(&'static std::ffi::CStr, Sel); 2] {
    [(c"VNImageRequestHandler", sel!(initWithCVPixelBuffer:options:)), (c"VNImageRequestHandler", sel!(performRequests:error:))]
}

fn selectors(request: Request) -> Vec<(&'static std::ffi::CStr, Sel)> {
    match request {
        Request::PersonSegmentation => vec![
            (c"VNGeneratePersonSegmentationRequest", sel!(init)),
            (c"VNGeneratePersonSegmentationRequest", sel!(setQualityLevel:)),
            (c"VNGeneratePersonSegmentationRequest", sel!(supportedOutputPixelFormatsAndReturnError:)),
            (c"VNGeneratePersonSegmentationRequest", sel!(setOutputPixelFormat:)),
            (c"VNGeneratePersonSegmentationRequest", sel!(results)),
            (c"VNPixelBufferObservation", sel!(pixelBuffer)),
        ],
        Request::PersonInstances | Request::ForegroundInstances => {
            let class = match request {
                Request::PersonInstances => c"VNGeneratePersonInstanceMaskRequest",
                _ => c"VNGenerateForegroundInstanceMaskRequest",
            };
            vec![
                (class, sel!(init)),
                (class, sel!(results)),
                (c"VNInstanceMaskObservation", sel!(allInstances)),
                (c"VNInstanceMaskObservation", sel!(generateScaledMaskForImageForInstances:fromRequestHandler:error:)),
            ]
        }
    }
}

pub(crate) fn supported(request: Request) -> bool {
    static OK: [OnceLock<bool>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let (slot, major) = match request {
        Request::PersonSegmentation => (&OK[0], 12),
        Request::PersonInstances => (&OK[1], 14),
        Request::ForegroundInstances => (&OK[2], 14),
    };
    *slot.get_or_init(|| {
        autoreleasepool(|_| {
            at_least(major)
                && common().into_iter().chain(selectors(request)).all(|(class, s)| AnyClass::get(class).is_some_and(|c| c.responds_to(s)))
                && AnyClass::get(c"NSIndexSet").is_some_and(|c| c.responds_to(sel!(firstIndex)) && c.responds_to(sel!(indexGreaterThanIndex:)))
        })
    })
}

fn describe(err: &NSError) -> String {
    err.localizedDescription().to_string()
}

/// Unlocks the pixel buffer when dropped, whatever way the read or write ends.
struct Locked<'a> {
    buffer: &'a CVPixelBuffer,
    flags: CVPixelBufferLockFlags,
}

impl<'a> Locked<'a> {
    fn new(buffer: &'a CVPixelBuffer, flags: CVPixelBufferLockFlags) -> Result<Self> {
        // SAFETY: a live pixel buffer and a documented lock flag; the matching unlock is in `Drop`.
        let r = unsafe { CVPixelBufferLockBaseAddress(buffer, flags) };
        if r != kCVReturnSuccess {
            return Err(Error::Vision(format!("could not lock a pixel buffer ({r})")));
        }
        Ok(Self { buffer, flags })
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        // SAFETY: unlocks the lock taken in `new`, with the same flags.
        let _ = unsafe { CVPixelBufferUnlockBaseAddress(self.buffer, self.flags) };
    }
}

/// The base address, row length and size of a locked, non-planar buffer whose rows hold `w × bytes_per_pixel` bytes.
fn layout(buffer: &CVPixelBuffer, w: usize, h: usize, bytes_per_pixel: usize) -> Result<(NonNull<u8>, usize, usize)> {
    if CVPixelBufferIsPlanar(buffer) {
        return Err(Error::Vision("Vision gave a planar pixel buffer".into()));
    }
    let base = NonNull::new(CVPixelBufferGetBaseAddress(buffer).cast::<u8>()).ok_or_else(|| Error::Vision("a pixel buffer has no memory".into()))?;
    let row = CVPixelBufferGetBytesPerRow(buffer);
    let size = CVPixelBufferGetDataSize(buffer);
    let need = w.checked_mul(bytes_per_pixel).ok_or_else(|| Error::Vision("a pixel buffer is too wide".into()))?;
    // (the last row may be unpadded: the buffer must hold h - 1 full rows and one `need` row)
    let last = h.checked_sub(1).and_then(|r| r.checked_mul(row)).and_then(|o| o.checked_add(need));
    if row < need || last.is_none_or(|end| end > size) {
        return Err(Error::Vision(format!("a {w} × {h} pixel buffer has rows of {row} bytes in {size} bytes")));
    }
    Ok((base, row, size))
}

/// `img` as a 32-bit BGRA pixel buffer.
fn pixel_buffer(img: &Image) -> Result<CFRetained<CVPixelBuffer>> {
    let (w, h) = (img.width, img.height);
    let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
    // (Vision's instance requests read only IOSurface-backed buffers: an empty IOSurface properties dictionary asks
    // Core Video for one)
    let surface: CFRetained<CFDictionary<CFString, CFString>> = CFDictionary::empty();
    // SAFETY: a constant Core Video exports, valid for the process's life.
    let key: &CFString = unsafe { kCVPixelBufferIOSurfacePropertiesKey };
    let attributes: CFRetained<CFDictionary<CFString, CFDictionary>> = CFDictionary::from_slices(&[key], &[surface.as_opaque()]);
    // SAFETY: the default allocator, positive sizes checked by `Image::check`, a documented pixel format, an attributes
    // dictionary of documented keys and a valid out-pointer; the buffer comes back retained (Create rule) or the call
    // fails.
    let r = unsafe { CVPixelBufferCreate(None, w, h, kCVPixelFormatType_32BGRA, Some(attributes.as_opaque()), NonNull::from(&mut out)) };
    let out =
        NonNull::new(out).filter(|_| r == kCVReturnSuccess).ok_or_else(|| Error::Vision(format!("could not make a {w} × {h} pixel buffer ({r})")))?;
    // SAFETY: a +1 retained buffer from a Create function, owned from here on.
    let buffer = unsafe { CFRetained::from_raw(out) };
    let lock = Locked::new(&buffer, CVPixelBufferLockFlags(0))?;
    let (base, row, size) = layout(&buffer, w, h, 4)?;
    // SAFETY: the base address of the locked buffer, writable for `size` bytes while the lock is held (`layout` checked
    // that every row written below lies inside it).
    let bytes = unsafe { std::slice::from_raw_parts_mut(base.as_ptr(), size) };
    for y in 0..h {
        let Some(line) = bytes.get_mut(y * row..y * row + w * 4) else { return Err(Error::Vision("a pixel buffer is too small".into())) };
        for (x, px) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let [r, g, b] = img.rgb8(y * w + x);
            px.copy_from_slice(&[b, g, r, 255]);
        }
    }
    drop(lock);
    Ok(buffer)
}

/// A one-component (8-bit, half or float) buffer Vision made, as coverage.
fn read_matte(buffer: &CVPixelBuffer) -> Result<Matte> {
    let (w, h) = (CVPixelBufferGetWidth(buffer), CVPixelBufferGetHeight(buffer));
    if w == 0 || h == 0 || w.saturating_mul(h) > crate::vision::MAX_PIXELS {
        return Err(Error::Vision(format!("Vision gave a {w} × {h} mask")));
    }
    let format = CVPixelBufferGetPixelFormatType(buffer);
    let bpp = match format {
        f if f == kCVPixelFormatType_OneComponent8 => 1,
        f if f == kCVPixelFormatType_OneComponent16Half => 2,
        f if f == kCVPixelFormatType_OneComponent32Float => 4,
        other => return Err(Error::Vision(format!("Vision gave a mask in pixel format {other:#x}"))),
    };
    let _lock = Locked::new(buffer, CVPixelBufferLockFlags::ReadOnly)?;
    let (base, row, size) = layout(buffer, w, h, bpp)?;
    // SAFETY: the base address of the locked buffer, readable for `size` bytes while the lock is held.
    let bytes = unsafe { std::slice::from_raw_parts(base.as_ptr(), size) };
    let mut data = Vec::with_capacity(w * h);
    for y in 0..h {
        let line = bytes.get(y * row..y * row + w * bpp).ok_or_else(|| Error::Vision("a mask is too small".into()))?;
        data.extend(line.chunks_exact(bpp).map(|c| {
            let v = match *c {
                [a] => f32::from(a) / 255.0,
                [a, b] => f16_to_f32(u16::from_ne_bytes([a, b])),
                [a, b, c, d] => f32::from_ne_bytes([a, b, c, d]),
                _ => 0.0,
            };
            if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
        }));
    }
    Ok(Matte { width: w, height: h, data })
}

/// A request handler for `img`.
fn handler(img: &Image) -> Result<(Retained<VNImageRequestHandler>, CFRetained<CVPixelBuffer>)> {
    let buffer = pixel_buffer(img)?;
    let options: Retained<NSDictionary<VNImageOption, AnyObject>> = NSDictionary::new();
    // SAFETY: `initWithCVPixelBuffer:options:` (checked in `supported`) on a fresh allocation, with a live BGRA buffer
    // and an empty options dictionary; never nil.
    let handler = unsafe { VNImageRequestHandler::initWithCVPixelBuffer_options(VNImageRequestHandler::alloc(), &buffer, &options) };
    Ok((handler, buffer))
}

fn perform(handler: &VNImageRequestHandler, request: &VNRequest) -> Result<()> {
    let requests = NSArray::from_slice(&[request]);
    handler.performRequests_error(&requests).map_err(|e| Error::Vision(describe(&e)))
}

pub(crate) fn person_segmentation(img: &Image) -> Result<Matte> {
    autoreleasepool(|_| {
        let (handler, _buffer) = handler(img)?;
        // SAFETY: `init` of an NSObject subclass (checked in `supported`); never nil.
        let request = unsafe { VNGeneratePersonSegmentationRequest::init(VNGeneratePersonSegmentationRequest::alloc()) };
        // SAFETY: setters and a getter checked in `supported`; the quality is a documented value, and the float output
        // format is set only when Vision lists it as supported.
        unsafe {
            request.setQualityLevel(VNGeneratePersonSegmentationRequestQualityLevel::Accurate);
            let float = kCVPixelFormatType_OneComponent32Float;
            if let Ok(formats) = request.supportedOutputPixelFormatsAndReturnError()
                && formats.iter().any(|n| n.unsignedIntValue() == float)
            {
                request.setOutputPixelFormat(float);
            }
        }
        perform(&handler, &request)?;
        // SAFETY: getters checked in `supported`; `results` may be nil and is received as optional, `pixelBuffer` is
        // never nil on an observation Vision made.
        let buffer = unsafe {
            let results = request.results().ok_or_else(|| Error::Vision("Vision gave no person segmentation".into()))?;
            let first = results.firstObject().ok_or_else(|| Error::Vision("Vision gave no person segmentation".into()))?;
            first.pixelBuffer()
        };
        read_matte(&buffer)
    })
}

/// The indexes Vision lists in `set`, at most [`MAX_INSTANCES`].
fn indexes(set: &NSIndexSet) -> Vec<NSUInteger> {
    // (NSNotFound is NSIntegerMax)
    let none = isize::MAX as NSUInteger;
    let mut out = Vec::new();
    let mut i = set.firstIndex();
    while i != none && out.len() < MAX_INSTANCES {
        out.push(i);
        i = set.indexGreaterThanIndex(i);
    }
    out
}

pub(crate) fn instances(img: &Image, kind: Request) -> Result<Vec<Matte>> {
    autoreleasepool(|_| {
        let (handler, _buffer) = handler(img)?;
        // SAFETY: `init` of NSObject subclasses (checked in `supported`); never nil. `results` is received as optional.
        let observations: Option<Retained<NSArray<VNInstanceMaskObservation>>> = unsafe {
            match kind {
                Request::PersonInstances => {
                    let r = VNGeneratePersonInstanceMaskRequest::init(VNGeneratePersonInstanceMaskRequest::alloc());
                    perform(&handler, &r)?;
                    r.results()
                }
                Request::ForegroundInstances => {
                    let r = VNGenerateForegroundInstanceMaskRequest::init(VNGenerateForegroundInstanceMaskRequest::alloc());
                    perform(&handler, &r)?;
                    r.results()
                }
                Request::PersonSegmentation => return Err(Error::Vision("not an instance request".into())),
            }
        };
        // (nobody found: no observation, or one with no instances)
        let Some(first) = observations.and_then(|o| o.firstObject()) else { return Ok(Vec::new()) };
        // SAFETY: a getter checked in `supported`; never nil.
        let all = unsafe { first.allInstances() };
        let mut out = Vec::new();
        for i in indexes(&all) {
            // (index 0 is the background in Vision's label map, never a person or subject)
            if i == 0 {
                continue;
            }
            let one = NSIndexSet::indexSetWithIndex(i);
            // SAFETY: checked in `supported`; `i` is one of the observation's own instances and the handler is the one
            // that made it, as documented. Failures come back as an NSError.
            let mask = unsafe { first.generateScaledMaskForImageForInstances_fromRequestHandler_error(&one, &handler) }
                .map_err(|e| Error::Vision(describe(&e)))?;
            out.push(read_matte(&mask)?);
        }
        Ok(out)
    })
}
