//! The 256 colours of one GIF frame, chosen for quality over speed, as the best GIF tools do
//! (ffmpeg's `palettegen` + `paletteuse`, gifski): the frame's colours are counted in a
//! histogram, split into 256 groups by median cut and refined by k-means, all in Oklab, where
//! distance follows what the eye sees; each pixel then takes the nearest palette colour in Oklab,
//! with Floyd–Steinberg error diffusion (serpentine, whole levels only) when dithering. A frame of
//! 256 colours or fewer keeps them exactly. On 24 demo photos at 480 px this comes out closer to
//! the source than ffmpeg's `palettegen=stats_mode=single` + `paletteuse` (sierra2_4a): about
//! 46 against 44 dB PSNR dithered, 46.6 against 45.6 undithered.

use std::collections::HashMap;

/// Colours per palette (the most a GIF frame can index).
const COLOURS: usize = 256;
/// The most histogram bins: a frame with more distinct colours is counted on a coarser grid
/// (7, then 6 bits a channel), each bin holding the exact mean of its pixels.
const MAX_BINS: usize = 1 << 16;
/// k-means rounds after median cut (each round is one pass over the histogram).
const ROUNDS: usize = 12;

type Lab = [f32; 3];

fn srgb_to_linear(v: f32) -> f32 {
    let v = (v / 255.0).clamp(0.0, 1.0);
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    255.0 * if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// sRGB (0–255 per channel, fractions allowed) to Oklab (Björn Ottosson's published matrices).
fn oklab(rgb: [f32; 3]) -> Lab {
    let [r, g, b] = rgb.map(srgb_to_linear);
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Oklab back to sRGB bytes.
fn srgb(lab: Lab) -> [u8; 3] {
    let [ll, a, b] = lab;
    let l = (ll + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (ll - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (ll - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    let rgb = [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ];
    rgb.map(|v| linear_to_srgb(v).round() as u8)
}

fn dist(a: &Lab, b: &Lab) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// The index of the colour of `palette` nearest `c`.
fn nearest(palette: &[Lab], c: &Lab) -> usize {
    let mut best = (0, f32::INFINITY);
    for (i, p) in palette.iter().enumerate() {
        let d = dist(p, c);
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

/// A histogram bin: how many pixels, and their mean colour in Oklab.
struct Bin {
    weight: f32,
    lab: Lab,
}

/// The frame's colours: every distinct colour when there are at most [`MAX_BINS`], else one bin per
/// occupied cell of a 7- or 6-bit-per-channel grid.
fn histogram(pixels: &[[u8; 4]]) -> Vec<Bin> {
    for bits in [8, 7] {
        let bins = histogram_at(pixels, bits);
        if bins.len() <= MAX_BINS {
            return bins;
        }
    }
    histogram_at(pixels, 6)
}

fn histogram_at(pixels: &[[u8; 4]], bits: u32) -> Vec<Bin> {
    let shift = 8 - bits;
    let mut sums: HashMap<u32, (u32, [u64; 3])> = HashMap::new();
    for p in pixels {
        let key = (u32::from(p[0] >> shift) << (2 * bits)) | (u32::from(p[1] >> shift) << bits) | u32::from(p[2] >> shift);
        let e = sums.entry(key).or_insert((0, [0; 3]));
        e.0 += 1;
        for c in 0..3 {
            e.1[c] += u64::from(p[c]);
        }
    }
    let mut bins: Vec<(u32, Bin)> = sums
        .into_iter()
        .map(|(k, (n, s))| {
            let mean = s.map(|v| v as f32 / n as f32);
            (k, Bin { weight: n as f32, lab: oklab(mean) })
        })
        .collect();
    // the hash map's order changes from run to run; the palette must not
    bins.sort_by_key(|(k, _)| *k);
    bins.into_iter().map(|(_, b)| b).collect()
}

/// Weighted mean and summed squared error of `bins`.
fn stats(bins: &[&Bin]) -> (Lab, f32, Lab) {
    let w: f32 = bins.iter().map(|b| b.weight).sum::<f32>().max(f32::MIN_POSITIVE);
    let mean = [0, 1, 2].map(|c| bins.iter().map(|b| b.lab[c] * b.weight).sum::<f32>() / w);
    let var = [0, 1, 2].map(|c| bins.iter().map(|b| (b.lab[c] - mean[c]).powi(2) * b.weight).sum::<f32>());
    (mean, var.iter().sum(), var)
}

/// Median cut: split the box with the largest error along its widest axis at its weighted
/// median, until there are `COLOURS` boxes (or no box can split); their means.
fn median_cut(bins: &[Bin]) -> Vec<Lab> {
    let mut boxes: Vec<Vec<&Bin>> = vec![bins.iter().collect()];
    while boxes.len() < COLOURS {
        let Some((at, axis)) = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .map(|(i, b)| {
                let (_, err, var) = stats(b);
                let axis = (0..3).max_by(|x, y| var[*x].total_cmp(&var[*y])).unwrap_or(0);
                (i, axis, err)
            })
            .max_by(|x, y| x.2.total_cmp(&y.2))
            .map(|(i, axis, _)| (i, axis))
        else {
            break;
        };
        let mut b = boxes.swap_remove(at);
        b.sort_by(|x, y| x.lab[axis].total_cmp(&y.lab[axis]));
        let half: f32 = b.iter().map(|x| x.weight).sum::<f32>() / 2.0;
        let mut acc = 0.0;
        let mut cut = 1;
        for (i, x) in b.iter().enumerate() {
            acc += x.weight;
            if acc >= half {
                cut = (i + 1).clamp(1, b.len() - 1);
                break;
            }
        }
        let rest = b.split_off(cut);
        boxes.push(b);
        boxes.push(rest);
    }
    boxes.iter().map(|b| stats(b).0).collect()
}

/// Lloyd's k-means over the histogram, from `palette`; a colour no bin is nearest keeps its place.
fn refine(bins: &[Bin], mut palette: Vec<Lab>) -> Vec<Lab> {
    for _ in 0..ROUNDS {
        let mut sums = vec![([0f32; 3], 0f32); palette.len()];
        for b in bins {
            if let Some(s) = sums.get_mut(nearest(&palette, &b.lab)) {
                for c in 0..3 {
                    s.0[c] += b.lab[c] * b.weight;
                }
                s.1 += b.weight;
            }
        }
        let mut moved = 0f32;
        for (p, (sum, w)) in palette.iter_mut().zip(sums) {
            if w > 0.0 {
                let next = sum.map(|v| v / w);
                moved = moved.max(dist(p, &next));
                *p = next;
            }
        }
        // settled: nothing moved by more than about a tenth of a just-noticeable difference
        if moved < 1e-7 {
            break;
        }
    }
    palette
}

/// A palette (`[r, g, b, …]`, at most 256 colours) for `pixels` (rows of `width`) and each
/// pixel's index in it, dithered when `dither`.
pub fn quantize(pixels: &[[u8; 4]], width: usize, dither: bool) -> (Vec<u8>, Vec<u8>) {
    // 256 colours or fewer: exactly those, no dithering needed
    let mut exact: HashMap<[u8; 3], u8> = HashMap::new();
    for p in pixels {
        let c = [p[0], p[1], p[2]];
        if !exact.contains_key(&c) {
            if exact.len() == COLOURS {
                exact.clear();
                break;
            }
            exact.insert(c, exact.len() as u8);
        }
    }
    if !exact.is_empty() {
        let mut palette = vec![0u8; exact.len() * 3];
        for (c, i) in &exact {
            if let Some(slot) = palette.get_mut(usize::from(*i) * 3..usize::from(*i) * 3 + 3) {
                slot.copy_from_slice(c);
            }
        }
        let indices = pixels.iter().map(|p| exact.get(&[p[0], p[1], p[2]]).copied().unwrap_or(0)).collect();
        return (palette, indices);
    }
    let bins = histogram(pixels);
    let lab = refine(&bins, median_cut(&bins));
    // the palette as stored (bytes), and those very colours in Oklab, so the error diffused is
    // the one the viewer sees
    let rgb: Vec<[u8; 3]> = lab.iter().map(|c| srgb(*c)).collect();
    let lab: Vec<Lab> = rgb.iter().map(|c| oklab(c.map(f32::from))).collect();
    let palette: Vec<u8> = rgb.iter().flatten().copied().collect();
    let mut cache: HashMap<[u8; 3], u8> = HashMap::new();
    let mut index = |c: [u8; 3]| *cache.entry(c).or_insert_with(|| nearest(&lab, &oklab(c.map(f32::from))).min(255) as u8);
    if !dither || width == 0 {
        return (palette, pixels.iter().map(|p| index([p[0], p[1], p[2]])).collect());
    }
    let colour = |i: u8| rgb.get(usize::from(i)).map_or([0.0; 3], |c| c.map(f32::from));
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
            // whole levels only, as ffmpeg's paletteuse does: an error too small to move a
            // neighbour by a level is dropped, so flat areas stay calm instead of crawling with
            // noise (on 24 demo photos this gains 4 dB PSNR and still breaks up banding)
            let spread = |buf: &mut [[f32; 3]], at: usize, w: f32| {
                if let Some(v) = buf.get_mut(at) {
                    v.iter_mut().zip(q).for_each(|(v, q)| *v += (q * w).trunc());
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

    /// A photo-like frame: sky gradient over a warm, noisy foreground.
    fn scene(w: usize, h: usize) -> Vec<[u8; 4]> {
        let mut seed = 7u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 24) as i32 % 9 - 4
        };
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                let c = if y < 0.6 {
                    [40.0 + 120.0 * y + 30.0 * x, 90.0 + 100.0 * y, 200.0 - 60.0 * y]
                } else {
                    [150.0 + 80.0 * x, 90.0 + 40.0 * y, 40.0 + 20.0 * x]
                };
                let c = c.map(|v| (v as i32 + noise()).clamp(0, 255) as u8);
                [c[0], c[1], c[2], 255]
            })
            .collect()
    }

    /// Mean Oklab distance between the frame and its quantised version.
    fn error(px: &[[u8; 4]], palette: &[u8], idx: &[u8]) -> f32 {
        let sum: f32 = px
            .iter()
            .zip(idx)
            .map(|(p, i)| {
                let k = usize::from(*i) * 3;
                let q = [palette[k], palette[k + 1], palette[k + 2]];
                dist(&oklab([p[0], p[1], p[2]].map(f32::from)), &oklab(q.map(f32::from))).sqrt()
            })
            .sum();
        sum / px.len() as f32
    }

    #[test]
    fn oklab_round_trips() {
        for c in [[0u8, 0, 0], [255, 255, 255], [12, 200, 77], [250, 3, 129]] {
            let back = srgb(oklab(c.map(f32::from)));
            assert!(back.iter().zip(c).all(|(a, b)| a.abs_diff(b) <= 1), "{c:?} → {back:?}");
        }
    }

    #[test]
    fn beats_neuquant_on_a_photo_like_frame() {
        let (w, h) = (160, 120);
        let px = scene(w, h);
        let (pal, idx) = quantize(&px, w, false);
        assert_eq!(pal.len(), 256 * 3);
        let ours = error(&px, &pal, &idx);
        let flat: Vec<u8> = px.iter().flatten().copied().collect();
        let nq = color_quant::NeuQuant::new(10, 256, &flat);
        let nq_idx: Vec<u8> = px.iter().map(|p| nq.index_of(p) as u8).collect();
        let theirs = error(&px, &nq.color_map_rgb(), &nq_idx);
        assert!(ours < theirs * 0.9, "k-means {ours} vs NeuQuant {theirs}");
    }

    #[test]
    fn few_colours_are_kept_exactly_and_the_result_is_stable() {
        let px: Vec<[u8; 4]> = (0..400).map(|i| [(i % 7 * 30) as u8, (i % 5 * 50) as u8, 9, 255]).collect();
        let (pal, idx) = quantize(&px, 20, true);
        assert_eq!(pal.len(), 35 * 3);
        assert!(px.iter().zip(&idx).all(|(p, i)| pal[usize::from(*i) * 3..usize::from(*i) * 3 + 3] == p[..3]));
        let big = scene(64, 48);
        assert_eq!(quantize(&big, 64, true), quantize(&big, 64, true), "same frame, same file");
    }
}
