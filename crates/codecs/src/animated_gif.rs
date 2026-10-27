//! Animated GIF writing: frames of 8-bit sRGB pixels, each with its own 256-colour palette
//! (NeuQuant, from `color_quant`) and optional Floyd–Steinberg dithering, streamed to any writer
//! one frame at a time. Photos keep smooth gradients only when dithered: 256 colours band skies.

use std::io::Write;

use crate::{Error, Result};

/// The largest frame side a GIF can describe.
pub const MAX_SIDE: usize = u16::MAX as usize;

/// NeuQuant's sample factor: 1 (every pixel, slowest) – 30; 10 is its documented compromise.
const SAMPLE_FACTOR: i32 = 10;

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

/// A 256-colour palette (`[r, g, b, …]`) for `pixels` (rows of `width`) and each pixel's index
/// in it, dithered (Floyd–Steinberg, serpentine) when `dither`.
pub fn quantize(pixels: &[[u8; 4]], width: usize, dither: bool) -> (Vec<u8>, Vec<u8>) {
    let opaque: Vec<u8> = pixels.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    let nq = color_quant::NeuQuant::new(SAMPLE_FACTOR, 256, &opaque);
    let palette = nq.color_map_rgb();
    let index = |c: [u8; 3]| nq.index_of(&[c[0], c[1], c[2], 255]).min(255) as u8;
    if !dither || width == 0 {
        return (palette, pixels.iter().map(|p| index([p[0], p[1], p[2]])).collect());
    }
    let colour = |i: u8| {
        let k = usize::from(i) * 3;
        palette.get(k..k + 3).map_or([0.0; 3], |c| [f32::from(c[0]), f32::from(c[1]), f32::from(c[2])])
    };
    let mut out = vec![0u8; pixels.len()];
    // the error carried into this row and the next, one entry per pixel plus a margin each side
    let mut here = vec![[0f32; 3]; width + 2];
    let mut next = vec![[0f32; 3]; width + 2];
    for (y, (row, dst)) in pixels.chunks(width).zip(out.chunks_mut(width)).enumerate() {
        let rtl = y % 2 == 1;
        for i in 0..row.len() {
            let x = if rtl { row.len() - 1 - i } else { i };
            let (Some(p), Some(e)) = (row.get(x), here.get(x + 1).copied()) else { continue };
            let want = [0, 1, 2].map(|c| (f32::from(p[c]) + e[c]).clamp(0.0, 255.0));
            let idx = index(want.map(|v| v.round() as u8));
            if let Some(d) = dst.get_mut(x) {
                *d = idx;
            }
            let got = colour(idx);
            let q = [0, 1, 2].map(|c| want[c] - got[c]);
            // ahead in this row, then below-behind, below, below-ahead (7, 3, 5, 1 sixteenths)
            let (ahead, behind) = if rtl { (x, x + 2) } else { (x + 2, x) };
            let spread = |buf: &mut [[f32; 3]], at: usize, w: f32| {
                if let Some(v) = buf.get_mut(at) {
                    v.iter_mut().zip(q).for_each(|(v, q)| *v += q * w);
                }
            };
            spread(&mut here, ahead, 7.0 / 16.0);
            spread(&mut next, behind, 3.0 / 16.0);
            spread(&mut next, x + 1, 5.0 / 16.0);
            spread(&mut next, ahead, 1.0 / 16.0);
        }
        std::mem::swap(&mut here, &mut next);
        next.iter_mut().for_each(|v| *v = [0.0; 3]);
    }
    (palette, out)
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
