//! Apple Vision's own masks (issue #115): person segmentation, one mask per person, and one per foreground subject.
//! Vision runs them on the Neural Engine in a fraction of a second, with no model file to download.
//!
//! Every call takes an [`Image`] (8-bit or float RGB) and gives coverage mattes (0 = background, 1 = the person or
//! subject) at the photo's aspect. Instance masks are read one at a time: the subject masks fold into one matte as
//! they are read, and the per-person masks kept add up to at most [`MAX_KEPT_PIXELS`]. A Mac without the request (older than macOS 14), a damaged image or a Vision failure
//! is an [`Error`]; the caller falls back to its own masks. The Objective-C calls are in `vision_macos.rs`.

use crate::{Error, Result, imp_vision};

/// Longest side of an image handed to Vision.
pub const MAX_SIDE: usize = 16_384;
/// Most pixels of an image handed to Vision (64 M).
pub const MAX_PIXELS: usize = 64 << 20;
/// Most masks read back from one request (Vision finds at most four people; subjects are capped here).
pub const MAX_INSTANCES: usize = 16;
/// Most mask pixels one call keeps (64 M, 256 MB as `f32`): the per-person masks of a large photo add up.
pub const MAX_KEPT_PIXELS: usize = MAX_PIXELS;

/// The pixels of an [`Image`], row-major RGB without padding.
#[derive(Clone, Copy, Debug)]
pub enum Pixels<'a> {
    /// 8-bit sRGB.
    Rgb8(&'a [u8]),
    /// Float sRGB-encoded values, 0 to 1 (clamped).
    RgbF32(&'a [f32]),
}

/// A picture for Vision: `width × height` RGB pixels, upright.
#[derive(Clone, Copy, Debug)]
pub struct Image<'a> {
    pub width: usize,
    pub height: usize,
    pub pixels: Pixels<'a>,
}

impl Image<'_> {
    /// Sizes that add up and stay within [`MAX_SIDE`] and [`MAX_PIXELS`], before anything is allocated.
    pub fn check(&self) -> Result<()> {
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || w.saturating_mul(h) > MAX_PIXELS {
            return Err(Error::Input(format!("a {w} × {h} picture is outside what Vision takes (1 to {MAX_SIDE} a side)")));
        }
        let len = match self.pixels {
            Pixels::Rgb8(p) => p.len(),
            Pixels::RgbF32(p) => p.len(),
        };
        if w * h * 3 != len {
            return Err(Error::Input(format!("a {w} × {h} RGB picture has {} values, not {len}", w * h * 3)));
        }
        Ok(())
    }

    /// The pixel at `i` (row-major) as 8-bit RGB.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn rgb8(&self, i: usize) -> [u8; 3] {
        let at = 3 * i;
        match self.pixels {
            Pixels::Rgb8(p) => [0, 1, 2].map(|c| p.get(at + c).copied().unwrap_or(0)),
            Pixels::RgbF32(p) => [0, 1, 2].map(|c| {
                let v = p.get(at + c).copied().unwrap_or(0.0);
                // (NaN reads as 0)
                if v.is_nan() { 0 } else { (v.clamp(0.0, 1.0) * 255.0).round() as u8 }
            }),
        }
    }
}

/// A coverage matte, row-major: 0 is background, 1 the person or subject.
#[derive(Clone, Debug, PartialEq)]
pub struct Matte {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Matte {
    /// Whether the sizes add up and every value is within 0 to 1.
    pub fn is_valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.width.checked_mul(self.height) == Some(self.data.len())
            && self.data.iter().all(|v| (0.0..=1.0).contains(v))
    }

    /// The coverage at normalized point `(x, y)` of the photo, bilinear between pixel centres.
    pub fn at(&self, x: f32, y: f32) -> f32 {
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 || self.data.len() < w * h {
            return 0.0;
        }
        let fx = (x * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = (y * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let v = |x: usize, y: usize| self.data.get(y * w + x).copied().unwrap_or(0.0);
        (v(x0, y0) * (1.0 - tx) + v(x1, y0) * tx) * (1.0 - ty) + (v(x0, y1) * (1.0 - tx) + v(x1, y1) * tx) * ty
    }

    /// This matte at the aspect of a `w × h` photo: as it is when it already has that aspect (within 1 %), else
    /// resampled to the photo's aspect keeping its own long side.
    pub fn at_aspect(self, w: usize, h: usize) -> Matte {
        if w == 0 || h == 0 || self.width == 0 || self.height == 0 {
            return self;
        }
        let long = self.width.max(self.height);
        let (tw, th) = if w >= h { (long, (long * h).div_ceil(w).max(1)) } else { ((long * w).div_ceil(h).max(1), long) };
        // (the same aspect within 1 %: a rounded side is not resampled)
        let (mine, theirs) = (self.width.saturating_mul(h), self.height.saturating_mul(w));
        if mine.abs_diff(theirs).saturating_mul(100) <= theirs {
            return self;
        }
        self.resized(tw, th)
    }

    /// This matte stretched to `w × h`, bilinear.
    pub fn resized(&self, w: usize, h: usize) -> Matte {
        let data = (0..w * h).map(|i| self.at(((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32)).collect();
        Matte { width: w, height: h, data }
    }
}

/// The mattes Vision found: one per person (largest first), each at the photo's aspect.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Instances {
    pub masks: Vec<Matte>,
}

/// `m` folded into `acc`: the largest coverage at each pixel of `acc` (the first matte sets the size).
pub(crate) fn fold_max(acc: &mut Option<Matte>, m: Matte) {
    let Some(a) = acc else {
        *acc = Some(m);
        return;
    };
    if (m.width, m.height) == (a.width, a.height) {
        for (o, v) in a.data.iter_mut().zip(&m.data) {
            *o = o.max(*v);
        }
        return;
    }
    let (w, h) = (a.width.max(1), a.height.max(1));
    for (i, o) in a.data.iter_mut().enumerate() {
        *o = o.max(m.at(((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32));
    }
}

/// Counts `m` into the pixels kept so far; an error past [`MAX_KEPT_PIXELS`].
pub(crate) fn keep(kept: &mut usize, m: &Matte) -> Result<()> {
    *kept = kept.saturating_add(m.data.len());
    if *kept > MAX_KEPT_PIXELS {
        return Err(Error::Input(format!("the masks of a {} × {} picture add up to more than {MAX_KEPT_PIXELS} pixels", m.width, m.height)));
    }
    Ok(())
}

/// The Vision requests this crate makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// `VNGeneratePersonSegmentationRequest`, quality Accurate: everyone together, with fine hair (macOS 12).
    PersonSegmentation,
    /// `VNGeneratePersonInstanceMaskRequest`: one mask per person, up to four (macOS 14).
    PersonInstances,
    /// `VNGenerateForegroundInstanceMaskRequest`: one mask per foreground subject, any kind (macOS 14).
    ForegroundInstances,
}

/// Whether this Mac runs `request` (false off the Mac).
pub fn supported(request: Request) -> bool {
    imp_vision::supported(request)
}

/// Whether all three requests run here.
pub fn available() -> bool {
    [Request::PersonSegmentation, Request::PersonInstances, Request::ForegroundInstances].into_iter().all(supported)
}

fn ready(img: &Image, request: Request) -> Result<()> {
    img.check()?;
    if !supported(request) {
        return Err(Error::Vision(format!("{} needs macOS 14 or later on a Mac", label(request))));
    }
    Ok(())
}

fn label(request: Request) -> &'static str {
    match request {
        Request::PersonSegmentation => "Person segmentation",
        Request::PersonInstances => "Person masks",
        Request::ForegroundInstances => "Subject masks",
    }
}

/// Everyone in `img` as one matte (Accurate quality), at the photo's aspect. A photo with nobody gives an empty matte.
pub fn person_segmentation(img: &Image) -> Result<Matte> {
    ready(img, Request::PersonSegmentation)?;
    let m = imp_vision::person_segmentation(img)?;
    checked(m.at_aspect(img.width, img.height))
}

/// One matte per person in `img` (up to four), each at the photo's size, largest first.
pub fn person_instances(img: &Image) -> Result<Instances> {
    ready(img, Request::PersonInstances)?;
    let (mut masks, mut kept) = (Vec::new(), 0usize);
    imp_vision::instances(img, Request::PersonInstances, &mut |m| {
        let m = checked(m.at_aspect(img.width, img.height))?;
        keep(&mut kept, &m)?;
        masks.push(m);
        Ok(())
    })?;
    largest_first(&mut masks);
    Ok(Instances { masks })
}

/// Every foreground subject in `img` (people, animals, things) as one matte, the largest coverage at each pixel, at
/// the photo's size; `None` when Vision found none. Each mask is folded in as it is read.
pub fn foreground_union(img: &Image) -> Result<Option<Matte>> {
    ready(img, Request::ForegroundInstances)?;
    let mut out = None;
    imp_vision::instances(img, Request::ForegroundInstances, &mut |m| {
        fold_max(&mut out, checked(m.at_aspect(img.width, img.height))?);
        Ok(())
    })?;
    Ok(out)
}

fn checked(m: Matte) -> Result<Matte> {
    if m.is_valid() { Ok(m) } else { Err(Error::Output(format!("Vision gave a damaged {} × {} matte", m.width, m.height))) }
}

/// Largest first, so callers that keep one keep the main person.
pub(crate) fn largest_first(masks: &mut [Matte]) {
    // (a valid matte's mean coverage is finite and not negative, so its bits sort as the number does)
    masks.sort_by_cached_key(|m| std::cmp::Reverse((m.data.iter().map(|v| f64::from(*v)).sum::<f64>() / m.data.len().max(1) as f64).to_bits()));
}
