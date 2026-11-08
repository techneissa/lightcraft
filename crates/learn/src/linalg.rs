//! The little linear algebra training needs: the eigenvectors of a symmetric matrix (cyclic Jacobi rotations;
//! a profile has at most a few hundred examples, so `n` stays small).

/// A dense row-major `n × n` matrix.
#[derive(Clone, Debug)]
pub struct Square {
    pub n: usize,
    pub a: Vec<f64>,
}

impl Square {
    pub fn zeros(n: usize) -> Square {
        Square { n, a: vec![0.0; n * n] }
    }

    pub fn at(&self, r: usize, c: usize) -> f64 {
        self.a.get(r * self.n + c).copied().unwrap_or(0.0)
    }

    pub fn set(&mut self, r: usize, c: usize, v: f64) {
        if let Some(x) = self.a.get_mut(r * self.n + c) {
            *x = v;
        }
    }
}

/// Eigenvalues and eigenvectors (`vectors.at(row, k)` is component `row` of vector `k`) of a symmetric matrix.
pub struct Eigen {
    pub values: Vec<f64>,
    pub vectors: Square,
}

/// The eigen-decomposition of symmetric `m` by cyclic Jacobi rotations, until the off-diagonal part is
/// negligible or after 60 sweeps.
pub fn symmetric_eigen(m: &Square) -> Eigen {
    let n = m.n;
    let mut a = m.clone();
    let mut v = Square::zeros(n);
    for i in 0..n {
        v.set(i, i, 1.0);
    }
    let scale: f64 = a.a.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-300);
    for _sweep in 0..60 {
        let off: f64 = (0..n).flat_map(|r| (0..n).filter(move |c| *c != r).map(move |c| (r, c))).map(|(r, c)| a.at(r, c).powi(2)).sum::<f64>().sqrt();
        if off <= 1e-12 * scale {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = a.at(p, q);
                if apq.abs() <= 1e-300 {
                    continue;
                }
                let (app, aqq) = (a.at(p, p), a.at(q, q));
                let theta = (aqq - app) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (akp, akq) = (a.at(k, p), a.at(k, q));
                    a.set(k, p, c * akp - s * akq);
                    a.set(k, q, s * akp + c * akq);
                }
                for k in 0..n {
                    let (apk, aqk) = (a.at(p, k), a.at(q, k));
                    a.set(p, k, c * apk - s * aqk);
                    a.set(q, k, s * apk + c * aqk);
                }
                for k in 0..n {
                    let (vkp, vkq) = (v.at(k, p), v.at(k, q));
                    v.set(k, p, c * vkp - s * vkq);
                    v.set(k, q, s * vkp + c * vkq);
                }
            }
        }
    }
    Eigen { values: (0..n).map(|i| a.at(i, i)).collect(), vectors: v }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eigenvectors_rebuild_the_matrix() {
        let n = 7;
        let mut m = Square::zeros(n);
        for r in 0..n {
            for c in 0..n {
                let v = ((r * 3 + c * 5) % 11) as f64 + if r == c { 4.0 } else { 0.0 };
                m.set(r, c, v);
                m.set(c, r, v);
            }
        }
        // symmetrise (the loop above wrote the lower triangle last)
        for r in 0..n {
            for c in 0..r {
                let v = m.at(r, c);
                m.set(c, r, v);
            }
        }
        let e = symmetric_eigen(&m);
        for r in 0..n {
            for c in 0..n {
                let back: f64 = (0..n).map(|k| e.vectors.at(r, k) * e.values[k] * e.vectors.at(c, k)).sum();
                assert!((back - m.at(r, c)).abs() < 1e-8, "{r},{c}: {back} vs {}", m.at(r, c));
            }
        }
    }
}
