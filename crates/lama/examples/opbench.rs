use candle_core::{DType, Device, Tensor};
use std::time::Instant;
fn time(name: &str, dev: &Device, n: usize, mut f: impl FnMut() -> Tensor) {
    let _ = f(); dev.synchronize().unwrap();
    let t = Instant::now();
    let mut last = None;
    for _ in 0..n { last = Some(f()); }
    drop(last); dev.synchronize().unwrap();
    println!("{name:40} {:8.2} ms", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
}
fn im2col3(x: &Tensor) -> Tensor {
    // (1, c, h, w) -> (c*9, (h-2)*(w-2)), rows ordered (c, ky, kx) as the weight's flattened layout
    let (_, c, h, w) = x.dims4().unwrap();
    let (oh, ow) = (h - 2, w - 2);
    let mut parts = Vec::new();
    for ky in 0..3 { for kx in 0..3 { parts.push(x.narrow(2, ky, oh).unwrap().narrow(3, kx, ow).unwrap().reshape((c, 1, oh * ow)).unwrap()); } }
    Tensor::cat(&parts, 1).unwrap().reshape((c * 9, oh * ow)).unwrap()
}
fn main() {
    let dev = if std::env::args().nth(1).as_deref() == Some("cpu") { Device::Cpu } else { lightcraft_lama::best_device() };
    let s: usize = std::env::args().nth(2).map(|v| v.parse().unwrap()).unwrap_or(64);
    let n = s * s;
    println!("{:?} feature map {s}x{s}", dev.location());
    let r = |shape: &[usize]| Tensor::randn(0f32, 1f32, shape, &dev).unwrap();
    let x128 = r(&[128, n]); let x384 = r(&[384, n]);
    let w1 = r(&[192, 384]);
    time("matmul 192x384 @ 384xN (1x1 conv)", &dev, 10, || w1.matmul(&x384).unwrap());
    let b = r(&[384, 1]);
    time("broadcast_add bias (384,N)+(384,1)", &dev, 10, || x384.broadcast_add(&b).unwrap());
    time("plain add (384,N)+(384,N)", &dev, 10, || (&x384 + &x384).unwrap());
    time("relu (384,N)", &dev, 10, || x384.relu().unwrap());
    time("affine mul_add", &dev, 10, || x384.affine(2.0, 1.0).unwrap());
    // gather im2col 3x3 pad 1 (clamped indices instead of reflect for timing)
    let idx: Vec<u32> = (0..9 * n).map(|i| (i % n) as u32).collect();
    let idx = Tensor::from_vec(idx, 9 * n, &dev).unwrap();
    time("gather im2col 128ch (index_select dim1)", &dev, 5, || x128.index_select(&idx, 1).unwrap());
    time("gather im2col 384ch", &dev, 5, || x384.index_select(&idx, 1).unwrap());
    let w3 = r(&[128, 128 * 9]);
    let col = x128.index_select(&idx, 1).unwrap().reshape((128 * 9, n)).unwrap();
    time("matmul 128x1152 @ 1152xN", &dev, 5, || w3.matmul(&col).unwrap());
    let w3b = r(&[512, 128 * 9]);
    time("matmul 512x1152 @ 1152xN", &dev, 5, || w3b.matmul(&col).unwrap());
    let col3 = x384.index_select(&idx, 1).unwrap().reshape((384 * 9, n)).unwrap();
    let w3c = r(&[128, 384 * 9]);
    time("matmul 128x3456 @ 3456xN", &dev, 5, || w3c.matmul(&col3).unwrap());
    let ones = Tensor::ones((1, n), DType::F32, &dev).unwrap();
    let bcol = r(&[384, 1]);
    time("bias via rank-1 matmul + add", &dev, 10, || x384.add(&bcol.matmul(&ones).unwrap()).unwrap());
    // shifted-slice conv 3x3: input (C, Hp*Wp)
    let (hp, wp) = (s + 2, s + 2);
    let xp128 = r(&[128, hp * wp]); let xp384 = r(&[384, hp * wp]);
    let len = s * wp - 2;
    let taps = |xp: &Tensor| -> Tensor {
        let mut v = Vec::new();
        for ky in 0..3 { for kx in 0..3 { v.push(xp.narrow(1, ky * wp + kx, len).unwrap().contiguous().unwrap()); } }
        Tensor::cat(&v, 0).unwrap()
    };
    time("taps copy+cat 128ch", &dev, 5, || taps(&xp128));
    time("taps copy+cat 384ch", &dev, 5, || taps(&xp384));
    let w128 = r(&[512, 1152]); let c128 = taps(&xp128);
    time("matmul 512x1152 @ cols", &dev, 5, || w128.matmul(&c128).unwrap());
    let w384 = r(&[128, 3456]); let c384 = taps(&xp384);
    time("matmul 128x3456 @ cols", &dev, 5, || w384.matmul(&c384).unwrap());
    time("narrow ONE tap contiguous 384ch", &dev, 10, || xp384.narrow(1, 1, len).unwrap().contiguous().unwrap());
    let _ = DType::F32;
}
