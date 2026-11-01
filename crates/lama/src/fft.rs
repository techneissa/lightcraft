//! `rfft2` / `irfft2` (orthonormal), which Candle lacks, as matrix products on the model's own device.
//!
//! LaMa's Fourier units only ever transform a feature map of at most a few hundred pixels a side, so
//! a DFT as matrix products is exact (the matrices are built in f64) and cheap beside the
//! convolutions, and the data never leaves the GPU. A radix FFT would need a copy to the CPU and back
//! around every one of the model's 36 Fourier units.
//!
//! Feature maps are channels-last here: `(h, w, c)`, the shape the whole model runs in. A half
//! spectrum is `(2h, l, c)` with `l = w / 2 + 1`: the real plane (rows `0..h`), then the imaginary
//! plane (rows `h..2h`).

use candle_core::{DType, Device, Tensor};

use crate::{Error, Result};

/// Largest side a transform accepts (the matrices are `side × side`).
pub const MAX_SIDE: usize = 4096;

/// The DFT matrices for one `h × w` feature-map size.
pub struct Dft {
    h: usize,
    w: usize,
    l: usize,
    /// `(l, w)`: `cos / sqrt(h·w)`; the forward transform along `w`, real part.
    fwd_cos: Tensor,
    /// `(l, w)`: `-sin / sqrt(h·w)`; the forward transform along `w`, imaginary part.
    fwd_sin: Tensor,
    /// `(2h, 2h)`: `[[C, S], [-S, C]]`, the forward transform along `h`.
    fwd_h: Tensor,
    /// `(2h, 2h)`: `[[C, -S], [S, C]]`, the inverse transform along `h`.
    inv_h: Tensor,
    /// `(w, l)`: `c·cos / sqrt(h·w)`; the inverse transform along `w`, from the real part.
    inv_cos: Tensor,
    /// `(w, l)`: `-c·sin / sqrt(h·w)`; from the imaginary part.
    inv_sin: Tensor,
}

fn angle(a: usize, b: usize, n: usize) -> f64 {
    std::f64::consts::TAU * ((a * b) % n) as f64 / n as f64
}

impl Dft {
    pub fn new(h: usize, w: usize, device: &Device) -> Result<Self> {
        if h == 0 || w == 0 || h > MAX_SIDE || w > MAX_SIDE {
            return Err(Error::Input(format!("cannot transform a {w}×{h} feature map")));
        }
        let l = w / 2 + 1;
        let scale = 1.0 / ((h * w) as f64).sqrt();
        let (mut fwd_cos, mut fwd_sin) = (vec![0f32; l * w], vec![0f32; l * w]);
        let (mut inv_cos, mut inv_sin) = (vec![0f32; w * l], vec![0f32; w * l]);
        for k in 0..l {
            // the columns that are their own mirror image (DC, and Nyquist for an even width) count once
            let weight = if k == 0 || (w.is_multiple_of(2) && k == w / 2) { 1.0 } else { 2.0 };
            for x in 0..w {
                let a = angle(k, x, w);
                fwd_cos[k * w + x] = (a.cos() * scale) as f32;
                fwd_sin[k * w + x] = (-a.sin() * scale) as f32;
                inv_cos[x * l + k] = (weight * a.cos() * scale) as f32;
                inv_sin[x * l + k] = (-weight * a.sin() * scale) as f32;
            }
        }
        let mut fwd_h = vec![0f32; 4 * h * h];
        let mut inv_h = vec![0f32; 4 * h * h];
        for k in 0..h {
            for y in 0..h {
                let a = angle(k, y, h);
                let (c, s) = (a.cos() as f32, a.sin() as f32);
                // rows [0, h) = real out, [h, 2h) = imaginary out; columns likewise for the input
                fwd_h[k * 2 * h + y] = c;
                fwd_h[k * 2 * h + h + y] = s;
                fwd_h[(h + k) * 2 * h + y] = -s;
                fwd_h[(h + k) * 2 * h + h + y] = c;
                inv_h[k * 2 * h + y] = c;
                inv_h[k * 2 * h + h + y] = -s;
                inv_h[(h + k) * 2 * h + y] = s;
                inv_h[(h + k) * 2 * h + h + y] = c;
            }
        }
        let t = |v: Vec<f32>, r: usize, c: usize| Tensor::from_vec(v, (r, c), device);
        Ok(Self {
            h,
            w,
            l,
            fwd_cos: t(fwd_cos, l, w)?,
            fwd_sin: t(fwd_sin, l, w)?,
            fwd_h: t(fwd_h, 2 * h, 2 * h)?,
            inv_h: t(inv_h, 2 * h, 2 * h)?,
            inv_cos: t(inv_cos, w, l)?,
            inv_sin: t(inv_sin, w, l)?,
        })
    }

    /// `rfft2` of a map `(h, w, c)`, orthonormal, as the half spectrum `(2h, l, c)`.
    pub fn rfft2(&self, x: &Tensor) -> Result<Tensor> {
        let (h, w, c) = x.dims3()?;
        if (h, w) != (self.h, self.w) {
            return Err(Error::Input(format!("transform built for {}×{}, given {w}×{h}", self.w, self.h)));
        }
        let x = x.contiguous()?;
        let xr = self.fwd_cos.broadcast_matmul(&x)?; // (h, l, c)
        let xi = self.fwd_sin.broadcast_matmul(&x)?;
        let planes = Tensor::cat(&[&xr, &xi], 0)?.contiguous()?.reshape((2 * h, self.l * c))?;
        Ok(self.fwd_h.matmul(&planes)?.reshape((2 * h, self.l, c))?)
    }

    /// `irfft2` of a half spectrum `(2h, l, c)`, orthonormal, back to a map `(h, w, c)`. The imaginary
    /// parts of the DC and Nyquist columns are ignored, as PyTorch does.
    pub fn irfft2(&self, spec: &Tensor) -> Result<Tensor> {
        let (h2, l, c) = spec.dims3()?;
        if h2 != 2 * self.h || l != self.l {
            return Err(Error::Input(format!("transform built for {}×{}, given a spectrum of {l}×{}", self.w, self.h, h2 / 2)));
        }
        let z = self.inv_h.matmul(&spec.contiguous()?.reshape((h2, l * c))?)?; // (2h, l·c)
        let zr = z.narrow(0, 0, self.h)?.reshape((self.h, l, c))?;
        let zi = z.narrow(0, self.h, self.h)?.reshape((self.h, l, c))?;
        Ok((self.inv_cos.broadcast_matmul(&zr)? + self.inv_sin.broadcast_matmul(&zi)?)?)
    }

    pub fn half_width(&self) -> usize {
        self.l
    }

    pub fn height(&self) -> usize {
        self.h
    }

    /// A column of ones `(n, 1)` on this device: with a `(1, c)` row it adds a per-channel bias as a matrix product,
    /// which is much faster than a broadcast add on the GPU.
    pub fn ones(n: usize, device: &Device) -> Result<Tensor> {
        Ok(Tensor::ones((n, 1), DType::F32, device)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A brute-force 2-D DFT in f64: `(re, im)` of bin `(ky, kx)`, orthonormal.
    fn naive(x: &[f64], h: usize, w: usize, ky: usize, kx: usize) -> (f64, f64) {
        let (mut re, mut im) = (0.0, 0.0);
        for y in 0..h {
            for xx in 0..w {
                let a = std::f64::consts::TAU * (ky as f64 * y as f64 / h as f64 + kx as f64 * xx as f64 / w as f64);
                re += x[y * w + xx] * a.cos();
                im -= x[y * w + xx] * a.sin();
            }
        }
        let s = 1.0 / ((h * w) as f64).sqrt();
        (re * s, im * s)
    }

    fn sample(h: usize, w: usize) -> Vec<f64> {
        (0..h * w).map(|i| ((i * 37 % 101) as f64 / 50.0 - 1.0) + (i % 7) as f64 * 0.1).collect()
    }

    fn tensor(x: &[f64], h: usize, w: usize) -> Tensor {
        Tensor::from_vec(x.iter().map(|v| *v as f32).collect::<Vec<_>>(), (h, w, 1), &Device::Cpu).unwrap()
    }

    #[test]
    fn rfft2_matches_a_brute_force_dft_for_even_and_odd_sizes() {
        for (h, w) in [(4, 4), (6, 8), (5, 7), (8, 5), (1, 6), (7, 1)] {
            let dft = Dft::new(h, w, &Device::Cpu).unwrap();
            let x = sample(h, w);
            let spec = dft.rfft2(&tensor(&x, h, w)).unwrap();
            let flat = spec.flatten_all().unwrap().to_vec1::<f32>().unwrap();
            let l = dft.half_width();
            for ky in 0..h {
                for kx in 0..l {
                    let (r, i) = naive(&x, h, w, ky, kx);
                    let (gr, gi) = (flat[ky * l + kx] as f64, flat[(h + ky) * l + kx] as f64);
                    assert!((gr - r).abs() < 1e-4, "{h}x{w} re[{ky},{kx}]: {gr} vs {r}");
                    assert!((gi - i).abs() < 1e-4, "{h}x{w} im[{ky},{kx}]: {gi} vs {i}");
                }
            }
        }
    }

    #[test]
    fn a_two_by_two_transform_has_the_known_values() {
        // [[1, 2], [3, 4]]: DC = 10 / 2, horizontal = (1 - 2 + 3 - 4) / 2, vertical = (1 + 2 - 3 - 4) / 2
        let dft = Dft::new(2, 2, &Device::Cpu).unwrap();
        let t = Tensor::from_vec(vec![1f32, 2.0, 3.0, 4.0], (2, 2, 1), &Device::Cpu).unwrap();
        let flat = dft.rfft2(&t).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
        // real plane rows (ky = 0, 1), then the imaginary plane rows; two columns each
        for (got, want) in flat[..4].iter().zip([5.0, -1.0, -2.0, 0.0]) {
            assert!((got - want).abs() < 1e-5, "{flat:?}");
        }
        assert!(flat[4..].iter().all(|v| v.abs() < 1e-5), "{flat:?}");
    }

    #[test]
    fn irfft2_undoes_rfft2_for_even_and_odd_sizes() {
        for (h, w) in [(4, 4), (6, 8), (5, 7), (8, 5), (16, 16)] {
            let dft = Dft::new(h, w, &Device::Cpu).unwrap();
            let x = sample(h, w);
            let back = dft.irfft2(&dft.rfft2(&tensor(&x, h, w)).unwrap()).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
            for (a, b) in back.iter().zip(&x) {
                assert!((*a as f64 - b).abs() < 1e-4, "{h}x{w}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn irfft2_ignores_the_imaginary_part_of_the_dc_and_nyquist_columns() {
        // imaginary values in the first row of the DC and Nyquist columns are constant along the height after the
        // inverse along it, and the real transform along the width drops them: no signal
        let (h, w) = (4, 4);
        let dft = Dft::new(h, w, &Device::Cpu).unwrap();
        let l = dft.half_width();
        let mut spec = vec![0f32; 2 * h * l];
        spec[h * l] = 1.0; // imaginary, row 0, DC column
        spec[h * l + l - 1] = 2.0; // imaginary, row 0, Nyquist column
        let spec = Tensor::from_vec(spec, (2 * h, l, 1), &Device::Cpu).unwrap();
        let out = dft.irfft2(&spec).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert!(out.iter().all(|v| v.abs() < 1e-5), "{out:?}");
    }

    #[test]
    fn a_constant_image_has_only_a_dc_bin_and_channels_stay_apart() {
        let dft = Dft::new(8, 8, &Device::Cpu).unwrap();
        let mut data = vec![0.5f32; 8 * 8 * 3];
        for (i, v) in data.iter_mut().enumerate() {
            *v = [0.5, -1.0, 2.0][i % 3];
        }
        let t = Tensor::from_vec(data, (8, 8, 3), &Device::Cpu).unwrap();
        let v = dft.rfft2(&t).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
        for (c, want) in [4.0f32, -8.0, 16.0].iter().enumerate() {
            assert!((v[c] - want).abs() < 1e-4, "channel {c}: {} vs {want}", v[c]); // 64 · value / 8
        }
        // every value but the first row's first (real) bin is zero
        let others: f32 = v.iter().enumerate().filter(|(i, _)| *i >= 3).map(|(_, x)| x.abs()).fold(0.0, f32::max);
        assert!(others < 1e-4, "{others}");
    }

    #[test]
    fn hostile_sizes_are_errors() {
        assert!(Dft::new(0, 4, &Device::Cpu).is_err());
        assert!(Dft::new(4, 0, &Device::Cpu).is_err());
        assert!(Dft::new(MAX_SIDE + 1, 4, &Device::Cpu).is_err());
        let dft = Dft::new(4, 4, &Device::Cpu).unwrap();
        let wrong = Tensor::zeros((5, 4, 1), DType::F32, &Device::Cpu).unwrap();
        assert!(dft.rfft2(&wrong).is_err());
        assert!(dft.irfft2(&wrong).is_err());
    }
}
