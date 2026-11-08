//! Kernel ridge regression with its strength chosen per slider by leave-one-out error, in closed form.
//!
//! With `K = U S Uᵀ`, `(K + λI)⁻¹ = U (S + λ)⁻¹ Uᵀ`, so one eigen-decomposition gives the fit for every λ, and the
//! leave-one-out residual of example `i` is `αᵢ / [(K + λI)⁻¹]ᵢᵢ` without refitting. Targets are centred on
//! their mean, so a profile that finds nothing in the features falls back to the average edit, and a strong λ
//! is exactly that.

use serde::{Deserialize, Serialize};

use crate::linalg::{Square, symmetric_eigen};

/// How two photos' features are compared.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kernel {
    /// Weight of the encoder's features against the photo statistics (0: statistics only).
    pub encoder_weight: f64,
    /// `None`: linear. `Some(γ)`: Gaussian, `exp(−γ · distance²)`.
    pub rbf: Option<f64>,
}

/// Where the photo statistics end and the encoder's features start in a feature vector.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub photo: usize,
    pub encoder: usize,
}

impl Layout {
    pub fn dims(self) -> usize {
        self.photo + self.encoder
    }
}

impl Kernel {
    /// `k(a, b)` of two standardised feature vectors.
    pub fn eval(&self, layout: Layout, a: &[f32], b: &[f32]) -> f64 {
        let (ap, ae) = a.split_at(layout.photo.min(a.len()));
        let (bp, be) = b.split_at(layout.photo.min(b.len()));
        let we = if layout.encoder == 0 { 0.0 } else { self.encoder_weight };
        let pdim = layout.photo.max(1) as f64;
        let edim = layout.encoder.max(1) as f64;
        match self.rbf {
            None => {
                let dot = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(p, q)| *p as f64 * *q as f64).sum::<f64>();
                dot(ap, bp) / pdim + we * dot(ae, be) / edim
            }
            Some(gamma) => {
                let d2 = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(p, q)| (*p as f64 - *q as f64).powi(2)).sum::<f64>();
                let d = (d2(ap, bp) / pdim + we * d2(ae, be) / edim) / (1.0 + we);
                (-gamma * d).exp()
            }
        }
    }
}

/// A fitted group: dual weights per example and slider.
#[derive(Clone, Debug)]
pub struct Fit {
    /// Mean target per slider.
    pub mean: Vec<f64>,
    /// `n × t`, row-major.
    pub alpha: Vec<f64>,
    pub lambda: Vec<f64>,
    /// Leave-one-out mean squared error per slider.
    pub loo_mse: Vec<f64>,
}

/// The Gram matrix of `x` (n rows).
pub fn gram(kernel: &Kernel, layout: Layout, x: &[Vec<f32>]) -> Square {
    let n = x.len();
    let mut k = Square::zeros(n);
    for i in 0..n {
        for j in i..n {
            let (Some(a), Some(b)) = (x.get(i), x.get(j)) else { continue };
            let v = kernel.eval(layout, a, b);
            k.set(i, j, v);
            k.set(j, i, v);
        }
    }
    k
}

/// Kernel ridge fits of every column of `y` (`n` rows of `t`), each with the λ of the lowest leave-one-out error.
pub fn fit(k: &Square, y: &[Vec<f64>]) -> Fit {
    let n = k.n;
    let t = y.first().map_or(0, Vec::len);
    let mean: Vec<f64> = (0..t).map(|j| y.iter().map(|r| r.get(j).copied().unwrap_or(0.0)).sum::<f64>() / n.max(1) as f64).collect();
    let yc: Vec<Vec<f64>> = y.iter().map(|r| r.iter().zip(&mean).map(|(v, m)| v - m).collect()).collect();
    let e = symmetric_eigen(k);
    // Uᵀ y
    let mut uty = vec![0.0; n * t];
    for kk in 0..n {
        for (i, row) in yc.iter().enumerate() {
            let u = e.vectors.at(i, kk);
            for (j, v) in row.iter().enumerate() {
                if let Some(slot) = uty.get_mut(kk * t + j) {
                    *slot += u * v;
                }
            }
        }
    }
    let trace = e.values.iter().map(|v| v.max(0.0)).sum::<f64>() / n.max(1) as f64;
    let base = if trace > 1e-12 { trace } else { 1.0 };
    let mut best_alpha = vec![0.0; n * t];
    let mut best_lambda = vec![0.0; t];
    let mut best_mse = vec![f64::INFINITY; t];
    for step in -8..=8 {
        let lambda = base * 10f64.powf(step as f64 * 0.5);
        let inv: Vec<f64> = e.values.iter().map(|s| 1.0 / (s.max(0.0) + lambda)).collect();
        // diag of (K + λI)⁻¹
        let g: Vec<f64> = (0..n).map(|i| (0..n).map(|kk| e.vectors.at(i, kk).powi(2) * inv.get(kk).copied().unwrap_or(0.0)).sum()).collect();
        for j in 0..t {
            let alpha: Vec<f64> = (0..n)
                .map(|i| {
                    (0..n).map(|kk| e.vectors.at(i, kk) * inv.get(kk).copied().unwrap_or(0.0) * uty.get(kk * t + j).copied().unwrap_or(0.0)).sum()
                })
                .collect();
            let mse = alpha.iter().zip(&g).map(|(a, gi)| (a / gi.max(1e-300)).powi(2)).sum::<f64>() / n.max(1) as f64;
            if mse.is_finite() && mse < best_mse[j] {
                best_mse[j] = mse;
                best_lambda[j] = lambda;
                for (i, a) in alpha.iter().enumerate() {
                    if let Some(slot) = best_alpha.get_mut(i * t + j) {
                        *slot = *a;
                    }
                }
            }
        }
    }
    Fit { mean, alpha: best_alpha, lambda: best_lambda, loo_mse: best_mse }
}

/// The fitted targets for features `q` (standardised), given the examples `x` the fit was made on.
pub fn predict(kernel: &Kernel, layout: Layout, x: &[Vec<f32>], mean: &[f64], alpha: &[f64], q: &[f32]) -> Vec<f64> {
    let t = mean.len();
    let mut out = mean.to_vec();
    for (i, xi) in x.iter().enumerate() {
        let k = kernel.eval(layout, xi, q);
        for (j, o) in out.iter_mut().enumerate() {
            *o += k * alpha.get(i * t + j).copied().unwrap_or(0.0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_linear_rule_is_fitted_and_noise_falls_back_to_the_mean() {
        let layout = Layout { photo: 2, encoder: 0 };
        let kernel = Kernel { encoder_weight: 0.0, rbf: None };
        let x: Vec<Vec<f32>> = (0..30).map(|i| vec![(i as f32 - 15.0) / 8.0, ((i * 7 % 11) as f32 - 5.0) / 3.0]).collect();
        // column 0 follows the first feature; column 1 is noise around 5
        let y: Vec<Vec<f64>> = x.iter().enumerate().map(|(i, r)| vec![2.0 * r[0] as f64 + 1.0, 5.0 + if i % 2 == 0 { 1.0 } else { -1.0 }]).collect();
        let f = fit(&gram(&kernel, layout, &x), &y);
        let p = predict(&kernel, layout, &x, &f.mean, &f.alpha, &[1.0, 0.0]);
        assert!((p[0] - 3.0).abs() < 0.2, "{p:?}");
        assert!((p[1] - 5.0).abs() < 0.5, "{p:?}");
        assert!(f.loo_mse[0] < 0.05, "{:?}", f.loo_mse);
    }
}
