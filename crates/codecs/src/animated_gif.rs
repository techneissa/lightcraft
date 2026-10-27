//! Animated GIF writing: frames of 8-bit sRGB pixels, each with its own 256-colour palette
//! ([`quantize`]: median cut and k-means in Oklab) and optional Floyd–Steinberg dithering,
//! streamed to any writer one frame at a time. Photos keep smooth gradients only when dithered:
//! 256 colours band skies.

use std::io::Write;

use crate::{Error, Result};

mod quantize;
pub use quantize::quantize;

/// The largest frame side a GIF can describe.
pub const MAX_SIDE: usize = u16::MAX as usize;

fn err(e: impl std::fmt::Display) -> Error {
    Error::Encode(format!("GIF: {e}"))
}

/// An animated GIF being written: [`GifWriter::add`] each frame, then [`GifWriter::finish`].
pub struct GifWriter<W: Write> {
    enc: gif::Encoder<W>,
    width: usize,
    height: usize,
    frames: usize,
}

impl<W: Write> GifWriter<W> {
    /// A `width × height` animation (1–65535 px a side) that plays forever when `loop_forever`,
    /// else once.
    pub fn new(w: W, width: usize, height: usize, loop_forever: bool) -> Result<Self> {
        let (Ok(gw), Ok(gh)) = (u16::try_from(width), u16::try_from(height)) else {
            return Err(Error::TooLarge(width as u64, height as u64));
        };
        if gw == 0 || gh == 0 {
            return Err(err(format!("empty frame size {width}×{height}")));
        }
        let mut enc = gif::Encoder::new(w, gw, gh, &[]).map_err(err)?;
        // without the loop extension viewers play the frames once
        if loop_forever {
            enc.set_repeat(gif::Repeat::Infinite).map_err(err)?;
        }
        Ok(GifWriter { enc, width, height, frames: 0 })
    }

    /// Append a frame of `width × height` pixels (alpha is ignored), shown for `delay`
    /// hundredths of a second.
    pub fn add(&mut self, pixels: &[[u8; 4]], delay: u16, dither: bool) -> Result<()> {
        if Some(pixels.len()) != self.width.checked_mul(self.height) {
            return Err(err(format!("a frame of {} pixels for a {}×{} animation", pixels.len(), self.width, self.height)));
        }
        let (palette, indices) = quantize(pixels, self.width, dither);
        let frame = gif::Frame {
            delay,
            width: self.width as u16,
            height: self.height as u16,
            palette: Some(palette),
            buffer: std::borrow::Cow::Owned(indices),
            ..Default::default()
        };
        self.enc.write_frame(&frame).map_err(err)?;
        self.frames += 1;
        Ok(())
    }

    /// Frames written so far.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// End the file and hand the writer back.
    pub fn finish(self) -> Result<W> {
        self.enc.into_inner().map_err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> Vec<[u8; 4]> {
        (0..w * h).map(|i| [(i % w * 255 / w) as u8, (i / w * 255 / h) as u8, 90, 255]).collect()
    }

    #[test]
    fn writes_a_looping_animation_a_decoder_reads_back() {
        let (w, h) = (40, 24);
        let mut g = GifWriter::new(Vec::new(), w, h, true).unwrap();
        g.add(&gradient(w, h), 50, true).unwrap();
        g.add(&vec![[200, 30, 30, 255]; w * h], 150, false).unwrap();
        assert_eq!(g.frames(), 2);
        let bytes = g.finish().unwrap();
        assert!(bytes.starts_with(b"GIF89a"));
        assert!(bytes.windows(11).any(|x| x == b"NETSCAPE2.0"), "loops forever");
        let mut d = gif::DecodeOptions::new();
        d.set_color_output(gif::ColorOutput::RGBA);
        let mut r = d.read_info(bytes.as_slice()).unwrap();
        let first = r.read_next_frame().unwrap().unwrap();
        assert_eq!((first.width, first.height, first.delay), (40, 24, 50));
        let second = r.read_next_frame().unwrap().unwrap();
        assert_eq!(second.delay, 150);
        // a flat colour survives quantising
        let px = &second.buffer[..4];
        assert!(px[0].abs_diff(200) <= 4 && px[1].abs_diff(30) <= 4 && px[2].abs_diff(30) <= 4, "{px:?}");
        assert!(r.read_next_frame().unwrap().is_none());
    }

    #[test]
    fn plays_once_without_the_loop_extension() {
        let mut g = GifWriter::new(Vec::new(), 8, 8, false).unwrap();
        g.add(&gradient(8, 8), 10, true).unwrap();
        assert!(!g.finish().unwrap().windows(11).any(|x| x == b"NETSCAPE2.0"));
    }

    #[test]
    fn dithering_keeps_the_average_colour_of_a_gradient() {
        let (w, h) = (64, 64);
        let px = gradient(w, h);
        let (pal, idx) = quantize(&px, w, true);
        assert_eq!(pal.len(), 256 * 3);
        let mean = |f: &dyn Fn(usize) -> f64| (0..px.len()).map(f).sum::<f64>() / px.len() as f64;
        let src = mean(&|i| f64::from(px[i][0]));
        let out = mean(&|i| f64::from(pal[usize::from(idx[i]) * 3]));
        assert!((src - out).abs() < 1.5, "{src} vs {out}");
    }

    #[test]
    fn bad_sizes_and_frames_are_errors() {
        assert!(GifWriter::new(Vec::new(), 0, 10, true).is_err());
        assert!(GifWriter::new(Vec::new(), MAX_SIDE + 1, 10, true).is_err());
        let mut g = GifWriter::new(Vec::new(), 4, 4, true).unwrap();
        assert!(g.add(&[[0; 4]; 15], 10, true).is_err());
        assert_eq!(g.frames(), 0);
        // a 1-pixel frame quantises
        let mut one = GifWriter::new(Vec::new(), 1, 1, false).unwrap();
        one.add(&[[9, 9, 9, 255]], 10, true).unwrap();
    }
}
