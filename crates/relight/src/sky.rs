//! Rays between clouds (issue #330): the sun's beams through the gaps in a photo's clouds (crepuscular rays), and the dark
//! bands that converge on the point opposite the sun (anticrepuscular rays). Drawn in the photo's plane, with the clouds
//! as one flat layer: a real sky holds clouds at several heights.
//!
//! - **The sky** ([`Sky`]): the depth model's own sky scores when it gave them ([`Field::with_sky_scores`]); else far,
//!   bright, blue or grey pixels joined to the top of the photo. A photo with too little sky has none ([`Field::has_sky`]),
//!   and the rays do nothing there.
//! - **Cloud thickness** in the sky, from its colour, no model. With the sun ahead (Sky Rays) thick cloud is dark against
//!   the glow round the sun, thin cloud and haze nearly as bright, and blue is open sky. With the sun behind (Opposite
//!   Rays) clouds are grey or white against blue (the blue ratio against the clearest blue of this sky).
//! - **Sky Rays** are shaped by the user and cut by these clouds and near things: [`crate::sun_rays`].
//! - **Opposite Rays.** Each pixel looks along the line away from the point opposite the sun to the photo's edge and sums
//!   the clouds on the way. A few lines a little apart in angle are averaged, so a band is soft and widens away from the
//!   point. A band is what stands above or below the lines round it, so an evenly lit sky shows none: the lines above and
//!   below their neighbours are faint lighter and darker bands over the open sky. Each plane is given in units of its own
//!   brightest few, so a strength means the same on every photo.
//! - **The sun's place**, unless placed: the brightest small spot of the sky that stands out most from round it.
//!
//! The planes are on the shadows' reduced field ([`Field::reduced`]) and brought to the field's size by depth only, so a
//! band stops at a tree's outline against the sky but passes a cloud's edge smoothly. All of it is drawn from numbers here;
//! nothing is read.

use rayon::prelude::*;

use crate::field::Field;
use crate::light::Lobe;
use crate::occluder::Reduced;

/// The share of the reduced field that must be sky for a photo to have one, and the most it may be (all of it sky is
/// more likely a pale wall).
const SKY_MIN: f32 = 0.02;
const SKY_MAX: f32 = 0.95;
/// The least range of log depth between the photo's nearest and farthest for it to have a sky.
const DEPTH_MIN: f32 = 0.5;
/// A depth model's sky score is sky from the first, in full from the second (the model's own threshold is 0.3).
const SCORE: (f32, f32) = (0.15, 0.45);
/// A pixel is sky from this far (log depth) below the photo's farthest, in full from the second.
const FAR: (f32, f32) = (0.6, 0.25);
/// Steps along each pixel's line at most, and the most pixels one step spans (a cloud lump a few pixels wide still holds a
/// line); and the lines a little apart in angle round the sun it averages (radians; the penumbra, [`PENUMBRA`], does the
/// rest of a shaft's softness).
pub(crate) const STEPS: usize = 96;
pub(crate) const STEP_PX: f32 = 4.0;
const SPREAD: [f32; 3] = [-0.004, 0.0, 0.004];
/// How thick a cloud holds the light: optical depth per unit of cloud over the photo's long side.
pub(crate) const THICK: f32 = 40.0;
/// The air in front of the photo's farthest land, against the sky's, and where along the photo's depth (0 nearest, 1
/// farthest) land begins to have some.
const AIR_LAND: f32 = 0.85;
const AIR_FROM: f32 = 0.45;
/// How much the sun's place favours a spot that stands out from its surroundings over one merely bright, and a compact,
/// filled one over a thin rim.
const PEAK: f32 = 2.0;
const ROUND: f32 = 1.5;
/// How much cloud round a spot counts against it as the sun's place (stops of brightness per unit of cloud thickness), and
/// when the pick is sure: ahead of the runner-up by this score and with less cloud round it than this.
const CLOUDY: f32 = 3.0;
/// How much a bright neighbourhood counts for a spot: the sun sits in a broad glow (often clipped flat, so it barely stands
/// out from round it), a sunlit cloud's edge in a dimmer sky.
const WIDE: f32 = 1.0;
const SURE_MARGIN: f32 = 0.5;
const SURE_CLOUD: f32 = 0.3;
/// Two depths (log) closer than the first are one surface to the rays' upsampling; past it a sample fades over the second.
const SAME_DEPTH: (f32, f32) = (0.05, 0.08);
/// Softer copies of the cloud thickness (each twice the last), how wide a cloud's penumbra is per pixel of distance from
/// it (the sun's disc, half a degree, widened by the haze), and over what share of the long side the photo's edge fades
/// out beyond it.
pub(crate) const LEVELS: usize = 7;
pub(crate) const PENUMBRA: f32 = 0.3;
pub(crate) const EDGE_FADE: f32 = 0.1;
/// The angles (radians) round the centre over which a line's own light is softened.
const SOFT: [f32; 3] = [-0.006, 0.0, 0.006];
/// The angles (radians) round the centre a line is held against: a shaft is a line lit more or less than these.
const AROUND: [f32; 8] = [-0.3, -0.18, -0.1, -0.05, 0.05, 0.1, 0.18, 0.3];
/// How deep a dark band is over land, against over the sky: a gentle dimming of the haze in front of it.
const LAND_SHADE: f32 = 0.3;
/// Cells each way over which the headroom takes its least (then softened over half of it).
const ROOM_REACH: usize = 4;
/// The most headroom a pixel counts (units of 18 % grey): a black pixel under a shaft takes no more than this.
const ROOM_MAX: f32 = 8.0;

pub(crate) fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    if t.is_finite() { t * t * (3.0 - 2.0 * t) } else { 0.0 }
}

pub(crate) fn percentile(v: &mut [f32], q: f32) -> f32 {
    v.sort_by(f32::total_cmp);
    let i = ((v.len().saturating_sub(1)) as f32 * q.clamp(0.0, 1.0)).round() as usize;
    v.get(i).copied().unwrap_or(0.0)
}

/// The guide's linear colour over the cells of the reduced field the shadows use for a field of `w × h` (each cell's mean;
/// the cells as [`Reduced`] makes them).
/// And each cell's brightest channel of its brightest pixel in the screen's primaries (what a cell's mean hides: the sun's
/// rim, a highlight on a far ridge).
pub(crate) fn tints_of(rgb: &[[f32; 3]], w: usize, h: usize) -> (Vec<[f32; 3]>, Vec<f32>) {
    let screen = lightcraft_color::REC2020.to_space(&lightcraft_color::SRGB);
    let f = w.max(h).div_ceil(crate::shadow::LOW_SIDE.max(1)).max(1);
    let (tw, th) = (w.div_ceil(f), h.div_ceil(f));
    (0..tw * th)
        .into_par_iter()
        .map(|i| {
            let (cx, cy) = (i % tw, i / tw);
            let (mut sum, mut n, mut top) = ([0f32; 3], 0f32, 0f32);
            for y in cy * f..((cy + 1) * f).min(h) {
                for x in cx * f..((cx + 1) * f).min(w) {
                    if let Some(c) = rgb.get(y * w + x).filter(|c| c.iter().all(|v| v.is_finite())) {
                        (0..3).for_each(|k| sum[k] += c[k].max(0.0));
                        top = top.max(screen_top(&screen, *c));
                        n += 1.0;
                    }
                }
            }
            (if n > 0.0 { sum.map(|s| s / n) } else { [0.0; 3] }, top)
        })
        .unzip()
}

/// The brightest channel of the linear Rec.2020 colour `c` in sRGB's primaries: what meets the screen's white first (a
/// saturated orange's red is well above its Rec.2020 red).
pub(crate) fn screen_top(screen: &lightcraft_color::Mat3, c: [f32; 3]) -> f32 {
    let s = screen.apply_f32(c);
    let v = s[0].max(s[1]).max(s[2]);
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

/// A depth model's sky `scores` (`sw × sh`) on the cells of the reduced field for a field of `w × h` (bilinear at each cell's
/// middle).
pub(crate) fn scores_of(scores: &[f32], (sw, sh): (usize, usize), (w, h): (usize, usize)) -> Vec<f32> {
    let f = w.max(h).div_ceil(crate::shadow::LOW_SIDE.max(1)).max(1);
    let (tw, th) = (w.div_ceil(f), h.div_ceil(f));
    (0..tw * th)
        .map(|i| {
            let (u, v) = ((((i % tw) * f) as f32 + f as f32 / 2.0) / w.max(1) as f32, (((i / tw) * f) as f32 + f as f32 / 2.0) / h.max(1) as f32);
            crate::filter::bilinear(scores, sw, sh, u * sw as f32, v * sh as f32)
        })
        .collect()
}

/// A photo's sky over the reduced field: how surely each pixel is sky, and its clouds' thickness. See the module docs.
#[derive(Clone, Debug)]
pub struct Sky {
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) sky: Vec<f32>,
    /// Cloud thickness (0..1) for the sun's beams, the sun ahead (thick cloud is dark against the sky's glow; blue is open),
    /// and for the rays opposite, the sun behind (cloud is grey or white, open sky blue).
    pub(crate) backlit: Vec<f32>,
    pub(crate) frontlit: Vec<f32>,
    /// The air in front of each pixel up to its depth (0..1): all of it over the sky, much over far land and the haze at the
    /// horizon, none over near things.
    pub(crate) air: Vec<f32>,
    /// The sky softened over a few percent of the long side (where the opposite bands show).
    pub(crate) soft: Vec<f32>,
    /// How surely a pixel the sky leaves out is a dark cloud bank all the same (far, dark against the sky, grey): no beam
    /// shows over it.
    pub(crate) bank: Vec<f32>,
    /// Whether the photo has a sky at all.
    pub(crate) found: bool,
    /// The brightest place of the sky, and the middle of its lowest edge (the horizon), in normalized photo coordinates.
    pub(crate) bright: (f32, f32),
    /// Whether the sun's place found from the sky is a clear pick (no runner-up nearly as good, little cloud round it).
    pub(crate) bright_sure: bool,
    pub(crate) horizon: (f32, f32),
    /// The colour of the sky's brightest part (luminance 1): the light the beams scatter is the sun's as the sky shows it.
    pub(crate) glow: [f32; 3],
}

impl Sky {
    /// Memory held (bytes).
    pub(crate) fn bytes(&self) -> usize {
        (self.sky.len() + self.backlit.len() + self.frontlit.len() + self.air.len() + self.soft.len() + self.bank.len()) * 4
    }

    fn of(field: &Field, r: &Reduced) -> Sky {
        let (w, h) = (r.w, r.h);
        let n = w * h;
        let none = || Sky {
            w,
            h,
            sky: vec![0.0; n],
            backlit: vec![0.0; n],
            frontlit: vec![0.0; n],
            air: vec![0.0; n],
            soft: vec![0.0; n],
            bank: vec![0.0; n],
            found: false,
            bright: (0.5, 0.0),
            bright_sure: false,
            horizon: (0.5, 0.5),
            glow: [1.0; 3],
        };
        // (a photo whose depth barely varies, a wall or a flat page, has no far end to be sky)
        let range = field.far - field.near;
        let model = field.sky_scores.len() == n;
        if field.tint.len() != n || n == 0 || (!model && (range.is_nan() || range < DEPTH_MIN)) {
            return none();
        }
        let far = field.far;
        // (how likely each pixel is sky, before it is joined to the top)
        let likely: Vec<f32> = (0..n)
            .into_par_iter()
            .map(|i| {
                let c = field.tint.get(i).copied().unwrap_or([0.0; 3]);
                let lz = r.lz.get(i).copied().unwrap_or(0.0);
                let y = lightcraft_color::luminance_2020(c).max(1e-6);
                let l = (y / 0.18).log2();
                let (hi, lo) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
                let sat = if hi > 1e-6 { (hi - lo) / hi } else { 0.0 };
                let blue = if c[2] > 1e-6 { (c[2] - c[0]) / c[2] } else { 0.0 };
                // (far; bright enough; blue, or grey to white: not the green of leaves or the brown of earth)
                let depth = smooth(far - FAR.0, far - FAR.1, lz);
                let bright = smooth(-4.0, -1.5, l);
                let colour = smooth(0.0, 0.25, blue).max(1.0 - smooth(0.2, 0.45, sat));
                depth * bright * colour
            })
            .collect();
        // (joined to the top of the photo through likely sky: a bright far wall in the middle of the photo is not sky)
        let mut joined = vec![false; n];
        let mut stack: Vec<usize> = (0..w).filter(|&x| likely.get(x).is_some_and(|v| *v > 0.35)).collect();
        while let Some(i) = stack.pop() {
            if joined.get(i).copied().unwrap_or(true) {
                continue;
            }
            if let Some(j) = joined.get_mut(i) {
                *j = true;
            }
            let (x, y) = (i % w, i / w);
            let next = [(x > 0).then(|| i - 1), (x + 1 < w).then_some(i + 1), (y > 0).then(|| i - w), (y + 1 < h).then_some(i + w)];
            for q in next.into_iter().flatten() {
                if likely.get(q).is_some_and(|v| *v > 0.35) && !joined.get(q).copied().unwrap_or(true) {
                    stack.push(q);
                }
            }
        }
        // (the depth model's own sky when it gave one: it knows a dark cloud from a far mountain, which colour can't tell)
        let sky: Vec<f32> = if model {
            field.sky_scores.iter().map(|v| smooth(SCORE.0, SCORE.1, *v)).collect()
        } else {
            likely.iter().zip(&joined).map(|(v, j)| if *j { smooth(0.35, 0.7, *v) } else { 0.0 }).collect()
        };
        let sure: Vec<usize> = (0..n).filter(|&i| sky.get(i).is_some_and(|v| *v > 0.5)).collect();
        // (all of it sky by colour alone is more likely a pale wall; the model knows)
        if (sure.len() as f32) < SKY_MIN * n as f32 || (!model && (sure.len() as f32) > SKY_MAX * n as f32) {
            return none();
        }
        let colour = |i: usize| field.tint.get(i).copied().unwrap_or([0.0; 3]);
        let blue_of = |c: [f32; 3]| if c[2] > 1e-6 { ((c[2] - c[0]) / c[2]).clamp(-1.0, 1.0) } else { 0.0 };
        let lum_of = |c: [f32; 3]| (lightcraft_color::luminance_2020(c).max(1e-6) / 0.18).log2();
        let mut blues: Vec<f32> = sure.iter().map(|&i| blue_of(colour(i))).collect();
        let mut lums: Vec<f32> = sure.iter().map(|&i| lum_of(colour(i))).collect();
        let clear = percentile(&mut blues, 0.9);
        let (l_lo, l_hi) = (percentile(&mut lums, 0.1), percentile(&mut lums, 0.95));
        // (the sun ahead: thick cloud is dark against the glow round the sun, thin cloud and haze nearly as bright; blue is open sky)
        let backlit: Vec<f32> = (0..n)
            .map(|i| {
                let c = colour(i);
                let dark = ((l_hi - lum_of(c)) / (l_hi - l_lo).max(0.5)).clamp(0.0, 1.0);
                // (blue is open sky, unless it is among the darkest of the sky: a storm cloud's base is often a dark blue-grey)
                let open = smooth(0.15, 0.45, blue_of(c)) * (1.0 - smooth(0.6, 0.9, dark));
                smooth(0.25, 0.85, dark) * (1.0 - open)
            })
            .collect();
        // (the sun behind: clouds are grey or white against a blue sky; with little blue anywhere, none can be told)
        let by_blue = smooth(0.06, 0.2, clear);
        let frontlit: Vec<f32> = (0..n)
            .map(|i| {
                let blue = ((clear - blue_of(colour(i))) / clear.max(1e-3)).clamp(0.0, 1.0);
                by_blue * smooth(0.15, 0.75, blue)
            })
            .collect();
        // (soft over about 1 % of the long side, within the sky)
        let soft = (w.max(h) / 100).max(1);
        let (backlit, frontlit) = (sky_mean(&backlit, &sky, w, h, soft), sky_mean(&frontlit, &sky, w, h, soft));
        // (the sun's place: the brightest small spot of the sky that stands out most from its surroundings, the sun's disc or the
        // brightest gap, rather than a large sunlit cloud)
        let lums = lums_at(field, w, h);
        let small = sky_mean(&lums, &sky, w, h, (w.max(h) / 80).max(1));
        let wide = sky_mean(&lums, &sky, w, h, (w.max(h) / 12).max(2));
        // (how filled the bright spot round a pixel is: a sun's disc or a gap is round and filled, a silver lining a thin rim)
        let rad = (w.max(h) / 60).max(2);
        let fill = |i: usize| {
            let (x, y, peak) = (i % w, i / w, small.get(i).copied().unwrap_or(0.0));
            let (mut on, mut all) = (0f32, 0f32);
            for yy in y.saturating_sub(rad)..(y + rad + 1).min(h) {
                for xx in x.saturating_sub(rad)..(x + rad + 1).min(w) {
                    if (xx.abs_diff(x).pow(2) + yy.abs_diff(y).pow(2)) <= rad * rad {
                        all += 1.0;
                        if lums.get(yy * w + xx).is_some_and(|l| *l >= peak - 0.5) {
                            on += 1.0;
                        }
                    }
                }
            }
            if all > 0.0 { on / all } else { 0.0 }
        };
        // (how cloudy round a pixel is, by the thickness map with the sun ahead: a sunlit cloud's edge sits in dark cloud, the
        // sun in open, bright sky)
        let cloudy = sky_mean(&backlit, &sky, w, h, (w.max(h) / 12).max(2));
        let score = |i: usize| {
            let (a, b) = (small.get(i).copied().unwrap_or(f32::MIN), wide.get(i).copied().unwrap_or(0.0));
            a + PEAK * (a - b) + WIDE * b - CLOUDY * cloudy.get(i).copied().unwrap_or(1.0)
        };
        // (the best few by brightness against round them, then the roundest of those)
        let mut ranked: Vec<usize> = sure.clone();
        ranked.sort_by(|a, b| score(*b).total_cmp(&score(*a)));
        // (one per neighbourhood: the next candidates are other spots, not the same spot's pixels)
        let mut picks: Vec<usize> = Vec::new();
        for i in ranked {
            if picks.len() >= 12 {
                break;
            }
            if picks.iter().all(|&j| (i % w).abs_diff(j % w).max((i / w).abs_diff(j / w)) > 3 * rad) {
                picks.push(i);
            }
        }
        let mut scored: Vec<(usize, f32)> = picks.into_iter().map(|i| (i, score(i) + ROUND * fill(i))).collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        let top = scored.first().map(|p| p.0);
        // (unsure: a runner-up nearly as good, or cloud round the pick)
        let margin = match (scored.first(), scored.get(1)) {
            (Some(a), Some(b)) => a.1 - b.1,
            _ => 0.0,
        };
        let bright_sure = top.is_some_and(|i| margin >= SURE_MARGIN && cloudy.get(i).copied().unwrap_or(1.0) < SURE_CLOUD);
        let at = |i: usize| (((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32);
        // (an unsure pick is kept a little inside the frame, where its handle is plain to see and to take)
        let bright = top.map_or((0.5, 0.1), at);
        let bright = if bright_sure { bright } else { (bright.0.clamp(0.08, 0.92), bright.1.clamp(0.06, 0.94)) };
        // (the colour of the sky's brightest tenth, without its clipped white)
        let cut = l_hi - 1.0;
        let (sum, n_glow) = sure.iter().map(|&i| colour(i)).filter(|c| lum_of(*c) >= cut).fold(([0f32; 3], 0f32), |(a, n), c| {
            let y = lightcraft_color::luminance_2020(c).max(1e-6);
            ([a[0] + c[0] / y, a[1] + c[1] / y, a[2] + c[2] / y], n + 1.0)
        });
        let glow = if n_glow > 0.0 {
            let g = sum.map(|v| v / n_glow);
            let y = lightcraft_color::luminance_2020(g).max(1e-6);
            // (half way to white: a beam is light-coloured, a paler version of the sky's own glow)
            g.map(|v| (0.5 + 0.5 * v / y).clamp(0.0, 1.25))
        } else {
            [1.0; 3]
        };
        // (the air in front of each pixel; and a far, dark, grey pixel the sky leaves out is a cloud bank the depth model took
        // for land)
        let span = (far - field.near).max(0.1);
        // (the air takes the sky softened over a few percent of the long side: where the depth model's sky stops along a
        // straight line in the clouds, the shafts fade out rather than end on that line)
        let ones = vec![1.0; n];
        let soft = sky_mean(&sky, &ones, w, h, (w.max(h) / 25).max(1));
        let (air, bank): (Vec<f32>, Vec<f32>) = (0..n)
            .map(|i| {
                let k = soft.get(i).copied().unwrap_or(0.0);
                let lz = r.lz.get(i).copied().unwrap_or(0.0);
                let t = ((lz - field.near) / span).clamp(0.0, 1.0);
                let air = k + (1.0 - k) * AIR_LAND * smooth(AIR_FROM, 1.0, t);
                let c = colour(i);
                let (hi, lo) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
                let sat = if hi > 1e-6 { (hi - lo) / hi } else { 0.0 };
                let dark = 1.0 - smooth(l_lo - 0.5, l_lo + 0.5, lum_of(c));
                let bank = (1.0 - k) * smooth(far - 0.25, far - 0.08, lz) * dark * (1.0 - smooth(0.15, 0.35, sat));
                (air, bank)
            })
            .unzip();
        // (the horizon: the lowest sky row each column has, the middle of the columns that have one)
        let mut lows: Vec<f32> =
            (0..w).filter_map(|x| (0..h).rev().find(|y| sky.get(y * w + x).is_some_and(|v| *v > 0.5)).map(|y| y as f32)).collect();
        let low = percentile(&mut lows, 0.5);
        let horizon = (0.5, ((low + 1.0) / h as f32).min(1.0));
        Sky { w, h, sky, backlit, frontlit, air, soft, bank, found: true, bright, bright_sure, horizon, glow }
    }
}

/// The field's luminance (log2 over 18 % grey) on the reduced cells.
fn lums_at(field: &Field, w: usize, h: usize) -> Vec<f32> {
    (0..w * h).map(|i| (lightcraft_color::luminance_2020(field.tint.get(i).copied().unwrap_or([0.0; 3])).max(1e-6) / 0.18).log2()).collect()
}

/// `v` (`w × h`) averaged over each pixel's neighbours within `radius` each way (a box, in two passes of running sums; the
/// photo's edge reads as the nearest pixel on it).
pub(crate) fn box_blur(v: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let pass = |src: &[f32], len: usize, count: usize, at: &dyn Fn(usize, usize) -> usize| -> Vec<f32> {
        let mut out = vec![0f32; src.len()];
        let r = radius as isize;
        for line in 0..count {
            let read = |k: isize| src.get(at(line, k.clamp(0, len as isize - 1) as usize)).copied().unwrap_or(0.0);
            let mut sum: f32 = (-r..=r).map(read).sum();
            for k in 0..len {
                if let Some(o) = out.get_mut(at(line, k)) {
                    *o = sum / (2 * radius + 1) as f32;
                }
                sum += read(k as isize + r + 1) - read(k as isize - r);
            }
        }
        out
    };
    if w == 0 || h == 0 || v.len() != w * h {
        return v.to_vec();
    }
    let rows = pass(v, w, h, &|y, x| y * w + x);
    pass(&rows, h, w, &|x, y| y * w + x)
}

/// `v` averaged over each pixel's neighbours within `radius`, weighted by `weight` (a pixel with none keeps its own).
pub(crate) fn sky_mean(v: &[f32], weight: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let mut out = vec![0f32; v.len()];
    out.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (mut sum, mut n) = (0f32, 0f32);
            for yy in y.saturating_sub(radius)..(y + radius + 1).min(h) {
                for xx in x.saturating_sub(radius)..(x + radius + 1).min(w) {
                    let q = yy * w + xx;
                    let wt = weight.get(q).copied().unwrap_or(0.0);
                    sum += wt * v.get(q).copied().unwrap_or(0.0);
                    n += wt;
                }
            }
            *o = if n > 1e-4 { sum / n } else { v.get(y * w + x).copied().unwrap_or(0.0) };
        }
    });
    out
}

/// Which rays: the sun's own, or the bands converging on the point opposite the sun.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rays {
    Sun,
    Opposite,
}

/// A sky's rays at the field's size, each per unit of their strength: the light they add (units of 18 % grey: a bright
/// ray's full share of the headroom below white) and how much they darken (0..1; Sky Rays darken nothing).
#[derive(Clone, Debug)]
pub struct SkyRays {
    pub light: Lobe,
    pub shade: Lobe,
    /// The beams' colour from the sky's own glow (scene-linear Rec.2020, luminance about 1).
    pub glow: [f32; 3],
}

impl SkyRays {
    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        self.light.bytes() + self.shade.bytes()
    }
}

impl Field {
    /// `v` (one value per pixel of the reduced field `r`, 0..1) at the field's size: bilinear, but a sample of another depth
    /// counts for little, so the rays stop at a near thing's outline against the sky and pass a cloud's edge in the sky
    /// smoothly (an edge-aware filter by brightness would cut them along every cloud's outline).
    pub(crate) fn depth_upsampled(&self, r: &Reduced, v: &[f32]) -> Vec<f32> {
        let (w, h) = (self.width, self.height);
        let mut out = vec![0f32; w * h];
        if v.len() != r.w * r.h || w == 0 || r.w == 0 || r.h == 0 {
            return out;
        }
        let f = r.f as f32;
        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let fy = ((y as f32 - (r.f / 2) as f32) / f).clamp(0.0, (r.h - 1) as f32);
            let (y0, ty) = (fy as usize, fy.fract());
            let y1 = (y0 + 1).min(r.h - 1);
            for (x, o) in row.iter_mut().enumerate() {
                let fx = ((x as f32 - (r.f / 2) as f32) / f).clamp(0.0, (r.w - 1) as f32);
                let (x0, tx) = (fx as usize, fx.fract());
                let x1 = (x0 + 1).min(r.w - 1);
                let lz = self.ln_z.get(y * w + x).copied().unwrap_or(0.0);
                let (mut sum, mut weights, mut nearest) = (0f32, 0f32, (f32::MAX, 0f32));
                for (qy, wy) in [(y0, 1.0 - ty), (y1, ty)] {
                    for (qx, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                        let q = qy * r.w + qx;
                        let (Some(val), Some(l)) = (v.get(q), r.lz.get(q)) else { continue };
                        let d = ((lz - l).abs() - SAME_DEPTH.0).max(0.0) / SAME_DEPTH.1;
                        let wt = wx * wy * (-d * d).exp();
                        sum += val * wt;
                        weights += wt;
                        if d < nearest.0 {
                            nearest = (d, *val);
                        }
                    }
                }
                let s = if weights > 1e-6 { sum / weights } else { nearest.1 };
                *o = if s.is_finite() { s.max(0.0) } else { 0.0 };
            }
        });
        out
    }

    pub(crate) fn sky(&self) -> &Sky {
        self.sky.get_or_init(|| Sky::of(self, self.reduced(false)))
    }

    /// Whether the photo has a sky (issue #330): enough far, bright, blue or grey pixels joined to its top.
    pub fn has_sky(&self) -> bool {
        self.sky().found
    }

    /// [`Field::has_sky`] when a render has already looked (`None` before: looking takes a moment, not for the UI's thread).
    pub fn sky_known(&self) -> Option<bool> {
        self.sky.get().map(|s| s.found)
    }

    /// Where the rays centre when they aren't placed (normalized photo coordinates): the sky's brightest place for the sun's
    /// beams, a little below the middle of its horizon for the point opposite the sun.
    pub fn rays_centre(&self, rays: Rays) -> (f32, f32) {
        let s = self.sky();
        match rays {
            Rays::Sun => s.bright,
            Rays::Opposite => (s.horizon.0, (s.horizon.1 + 0.05).min(1.5)),
        }
    }

    /// Whether the sun's place found from the sky is a clear pick, when a render has already looked (`None` before or without
    /// a sky). The panel asks the user to drag the sun when it isn't.
    pub fn rays_centre_sure(&self) -> Option<bool> {
        self.sky.get().filter(|s| s.found).map(|s| s.bright_sure)
    }

    /// [`Field::rays_centre`] when a render has already looked at the sky (`None` before, or without a sky: not for the UI's
    /// thread to work out).
    pub fn rays_centre_known(&self, rays: Rays) -> Option<(f32, f32)> {
        self.sky.get().filter(|s| s.found).map(|_| self.rays_centre(rays))
    }

    /// Each reduced cell's headroom below white for `headroom` (see [`Field::opposite_rays`]): by its brightest channel, a deep
    /// blue sky's blue being nearer white than its luminance; the least of it within a few cells, softened over half that, so a
    /// ray brought up to full size never spills onto a brighter neighbour and stays smooth over a patchy sky.
    /// With `tops` (each cell's brightest channel of its brightest pixel, [`Field::tint_top`]) that pixel counts, not the
    /// cell's mean.
    pub(crate) fn least_room(&self, w: usize, h: usize, headroom: &(dyn Fn(f32) -> f32 + Sync), tops: Option<&[f32]>) -> Vec<f32> {
        let screen = lightcraft_color::REC2020.to_space(&lightcraft_color::SRGB);
        let room: Vec<f32> = (0..w * h)
            .map(|i| {
                let c = self.tint.get(i).copied().unwrap_or([0.0; 3]);
                let top = tops.and_then(|t| t.get(i)).copied().unwrap_or(0.0);
                let r = headroom(screen_top(&screen, c).max(top).clamp(0.0, 1.0));
                if r.is_finite() { r.clamp(0.0, ROOM_MAX) } else { 0.0 }
            })
            .collect();
        let least: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                (y.saturating_sub(ROOM_REACH)..(y + ROOM_REACH + 1).min(h))
                    .flat_map(|yy| (x.saturating_sub(ROOM_REACH)..(x + ROOM_REACH + 1).min(w)).map(move |xx| yy * w + xx))
                    .filter_map(|q| room.get(q).copied())
                    .fold(ROOM_MAX, f32::min)
            })
            .collect();
        sky_mean(&least, &vec![1.0; w * h], w, h, ROOM_REACH / 2)
    }

    /// The soft rays converging on the point `centre` opposite the sun (normalized photo coordinates; may lie outside the
    /// photo), `None` for a photo without a sky. See the module docs. `headroom` gives, for a pixel the guide shows at linear
    /// `y` (0..1, its brightest channel), how much light (units of 18 % grey, in the render's scene-linear light) it can take
    /// before it shows as white: a lighter band adds a share of that, so it never clips.
    pub fn opposite_rays(&self, centre: (f32, f32), headroom: &(dyn Fn(f32) -> f32 + Sync)) -> Option<SkyRays> {
        let s = self.sky();
        if !s.found || !(centre.0.is_finite() && centre.1.is_finite()) {
            return None;
        }
        let r = self.reduced(false);
        let (w, h) = (s.w, s.h);
        if (w, h) != (r.w, r.h) || w < 2 || h < 2 {
            return None;
        }
        let long = w.max(h) as f32;
        let c = (centre.0.clamp(-4.0, 5.0) * w as f32, centre.1.clamp(-4.0, 5.0) * h as f32);
        let cell = |x: f32, y: f32| (x >= 0.0 && y >= 0.0 && x < w as f32 && y < h as f32).then(|| (y as usize) * w + x as usize);
        // (what holds the light: the clouds of the sky, and a dark cloud bank the depth model took for land)
        let thick: Vec<f32> = (0..w * h)
            .map(|i| {
                let (k, b) = (s.sky.get(i).copied().unwrap_or(0.0), s.bank.get(i).copied().unwrap_or(0.0));
                (s.frontlit.get(i).copied().unwrap_or(0.0) * k).max(b)
            })
            .collect();
        let levels = soft_levels(&thick, w, h);
        let thick_at = |x: f32, y: f32, radius: f32| level_at(&levels, w, h, long, (x, y), radius);
        // (the bands are faint and broad: a wide penumbra)
        let penumbra = 3.0 * PENUMBRA;
        // (one line from `p` away from the point to the photo's edge: how much light comes through)
        let march = |p: (f32, f32)| -> Option<f32> {
            let (dx, dy) = (c.0 - p.0, c.1 - p.1);
            let d = (dx * dx + dy * dy).sqrt();
            if d < 1.0 || !d.is_finite() {
                return Some(1.0);
            }
            let (u, reach) = ((-dx / d, -dy / d), long * 1.5);
            let n = STEPS.min(((reach / STEP_PX).ceil() as usize).max(1));
            let ds = reach / n as f32;
            let mut tau = 0f32;
            for k in 0..n {
                let t = (k as f32 + 0.5) * ds;
                tau += thick_at(p.0 + u.0 * t, p.1 + u.1 * t, penumbra * t) * ds / long;
            }
            Some((-THICK * tau).exp())
        };
        // (rotate `p` round the centre by `a` radians)
        let turn = |p: (f32, f32), a: f32| {
            let (sn, cs) = a.sin_cos();
            let (x, y) = (p.0 - c.0, p.1 - c.1);
            (c.0 + x * cs - y * sn, c.1 + x * sn + y * cs)
        };
        let spread = 3.0;
        // (each pixel's light through the clouds: a few lines a little apart in angle, so a band's edge is soft and widens)
        let through: Vec<f32> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let p = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                let (sum, n) = SPREAD.iter().filter_map(|a| march(turn(p, spread * a))).fold((0f32, 0f32), |(s, n), t| (s + t, n + 1.0));
                if n > 0.0 { sum / n } else { 1.0 }
            })
            .collect();
        // (a band is a line lit more, or less, than the lines round it at the same distance from the centre; a line round it
        // that starts off the photo is marched there, not read from the photo's)
        let at = |q: (f32, f32)| cell(q.0, q.1).and_then(|i| through.get(i).copied()).or_else(|| march(q));
        // (how open the air in front of each pixel is, not cloud nor a cloud bank: low-passed over a couple of percent of the
        // long side, so a patchy haze weighs the bands smoothly)
        let ones = vec![1.0f32; w * h];
        let open_raw: Vec<f32> = (0..w * h)
            .map(|i| {
                let (k, cl, b) =
                    (s.sky.get(i).copied().unwrap_or(0.0), s.frontlit.get(i).copied().unwrap_or(0.0), s.bank.get(i).copied().unwrap_or(0.0));
                (1.0 - cl * k) * (1.0 - b)
            })
            .collect();
        let open = sky_mean(&open_raw, &ones, w, h, (w.max(h) / 50).max(1));
        let raw: Vec<f32> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let p = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                let own = through.get(i).copied().unwrap_or(1.0);
                let (sum, n) = AROUND.iter().filter_map(|a| at(turn(p, *a))).fold((0f32, 0f32), |(s, n), t| (s + t, n + 1.0));
                if n > 0.0 { own - sum / n } else { 0.0 }
            })
            .collect();
        let (bright, dark): (Vec<f32>, Vec<f32>) = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let p = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                // (the band softened across a few lines either side, so its edge is no hard step; a line off the photo reads the
                // nearest one on it, so there are no steps along the edges)
                let (ss, sn) = SOFT
                    .iter()
                    .filter_map(|a| {
                        let q = turn(p, spread * a);
                        let (qx, qy) = (q.0.clamp(0.0, w as f32 - 1.0), q.1.clamp(0.0, h as f32 - 1.0));
                        raw.get((qy as usize) * w + qx as usize).copied()
                    })
                    .fold((0f32, 0f32), |(s, n), t| (s + t, n + 1.0));
                let band = if sn > 0.0 { ss / sn } else { raw.get(i).copied().unwrap_or(0.0) };
                let v = band * s.soft.get(i).copied().unwrap_or(0.0) * open.get(i).copied().unwrap_or(0.0);
                let fin = |x: f32| if x.is_finite() { x.max(0.0) } else { 0.0 };
                (fin(v), fin(-v))
            })
            .unzip();
        // (both in units of the stronger's brightest few, so a strength means the same on every photo)
        let top = {
            let mut on: Vec<f32> = bright.iter().chain(&dark).copied().filter(|x| *x > 1e-5).collect();
            if on.len() < 8 {
                return Some(SkyRays {
                    light: Lobe { m: vec![0.0; self.width * self.height] },
                    shade: Lobe { m: vec![0.0; self.width * self.height] },
                    glow: s.glow,
                });
            }
            percentile(&mut on, 0.99).max(1e-4)
        };
        // (the bands are faint: at most one unit of 18 % grey however much headroom a dark sky has)
        let least = self.least_room(w, h, headroom, None);
        let light: Vec<f32> = bright.iter().zip(&least).map(|(v, r)| (v / top).clamp(0.0, 1.0) * r.min(1.0)).collect();
        // (a dark band over land is a cloud's shadow in the haze in front of it: a gentle dimming, shallower than over the sky)
        let shade: Vec<f32> = dark.iter().zip(&s.sky).map(|(v, k)| (v / top).clamp(0.0, 1.0) * (LAND_SHADE + (1.0 - LAND_SHADE) * k)).collect();
        let up = |v: &[f32]| Lobe { m: self.depth_upsampled(r, v) };
        Some(SkyRays { light: up(&light), shade: up(&shade), glow: s.glow })
    }
}

/// `thick` (`w × h`) softened a little (a cloud's edge in the photo is no opaque knife edge), then [`LEVELS`] softer copies of
/// it, each twice as soft: the penumbra a line meets the farther it is from what it passes.
pub(crate) fn soft_levels(thick: &[f32], w: usize, h: usize) -> Vec<Vec<f32>> {
    let ones = vec![1.0f32; w * h];
    let mut levels: Vec<Vec<f32>> = vec![sky_mean(thick, &ones, w, h, (w.max(h) / 200).max(1))];
    for k in 1..LEVELS {
        let prev = levels.last().cloned().unwrap_or_default();
        levels.push(box_blur(&prev, w, h, 1 << (k - 1)));
    }
    levels
}

/// [`soft_levels`] at `(x, y)` (reduced cells) blurred over `radius` cells: between the two copies nearest that softness,
/// bilinear within each. Off the photo, the photo's edge carries on, fading out over a tenth of the long side `long`.
pub(crate) fn level_at(levels: &[Vec<f32>], w: usize, h: usize, long: f32, (x, y): (f32, f32), radius: f32) -> f32 {
    let (cx, cy) = (x.clamp(0.0, w as f32 - 1.0), y.clamp(0.0, h as f32 - 1.0));
    let out = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
    let lv = radius.max(1.0).log2().clamp(0.0, (LEVELS - 1) as f32);
    let (a, f) = (lv as usize, lv.fract());
    let read = |k: usize| levels.get(k.min(LEVELS - 1)).map_or(0.0, |l| crate::filter::bilinear(l, w, h, cx + 0.5, cy + 0.5));
    (read(a) + (read(a + 1) - read(a)) * f) * (-out / (EDGE_FADE * long)).exp()
}
