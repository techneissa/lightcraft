//! Finding the spots on a face's skin, once per photo (the provider calls it on the picture its masks come from).
//!
//! On the Facial Skin mask's box, reduced so the face is at most [`WORK`] pixels:
//!
//! 1. **Skin-only blurs** of an "absorbing" channel ([`ABSORB`]: mostly green, which blood and melanin both absorb)
//!    and of red, at the sizes of [`SCALES`] (each the picture weighted by the mask, divided by the blurred mask, so
//!    hair, eyes and lips never count as a spot's surroundings).
//! 2. **Response** per size: how much darker the pixel's blur is than the skin around it (the blur [`SURROUND`]
//!    sizes up), in natural log. A spot of radius r answers most at the size r/√2.
//! 3. **Spots** are the places darker than their surroundings by more than [`FIND`] at their best size and darker
//!    than every neighbour; no larger structure behind them (the last size, past 3 % of the face, answering more
//!    means a nostril or a shadow, not a spot); round (not a line or a wrinkle: the response's curvature is at most
//!    [`MAX_ELONGATION`] times longer one way); with skin all around; standing out from the skin's texture
//!    ([`STAND_OUT`]). Overlapping finds keep the strongest.
//! 4. **Not spots** ([`judge`]): a crease or a hair through it ([`LINE`]), the edge of something larger and darker
//!    beside it ([`SIDE`]: a nostril, a fold), the face's shading ([`BIG`]), or a strand Stray Hair's search found
//!    under it (a hair stump, a crease; `strands`).
//! 5. **Kinds** (`classify`): red (blood) is a blemish; a round sharp spot much darker than the spots around it is a
//!    mole; one of a field of like spots, in a field of [`classify::FRECKLED`] or more, is a freckle; where stubble
//!    grows, small dots and spots on textured skin are stubble and dropped; the rest are blemishes. On a freckled
//!    face the field is searched again, finer ([`IN_FIELD`]), so its fainter freckles go with the rest when the
//!    switch is off.
//! 6. **Donor**: a clean patch of skin next to each spot, whose texture the repair borrows.

use lightcraft_raster::blur::{gauss_taps, gaussian, gaussian_taps};
use lightcraft_raster::{Image, Plane, Rgb32f, par_rows};

use crate::input::{Spot, SpotKind, Spots};
use crate::{Part, Scene};

mod classify;
mod strands;
mod traits;

/// The face's size, in pixels, detection works at: a bigger face is reduced to it (spots under about 2 of these
/// pixels are not looked for), a smaller one is worked on as it is.
pub const WORK: f64 = 640.0;
/// A face under this size, in pixels of the picture spots are found on, is not looked at: its spots (0.3–3 % of
/// it) would be a pixel or two across.
pub const MIN_FACE: f64 = 160.0;

/// The sizes looked at (Gaussian σ, fractions of the face's size), smallest first. A spot of radius r answers most
/// at σ = r/√2, so all but the last cover spots of 0.3–3 % of the face across; the last only tells a larger
/// structure (it must answer less than the spot's own size).
pub const SCALES: [f64; 8] = [0.0011, 0.0016, 0.00234, 0.00342, 0.005, 0.0073, 0.0107, 0.0156];

/// The surroundings a size is compared with: the blur this many sizes up ([`SCALES`] continue by the same ratio).
const SURROUND: usize = 2;
/// The ratio between two neighbouring sizes.
const STEP: f64 = 1.46;

/// The absorbing channel's weights (R, G, B): spots darken green most (blood and melanin), blue next.
pub const ABSORB: [f32; 3] = [0.15, 0.65, 0.2];

/// A spot is darker than the skin around it by at least this (natural log: about 6 %).
pub const FIND: f32 = 0.06;
/// The two smallest sizes need this much more (pores and grain are small and faint): ×1.6 and ×1.3.
const SMALL_EXTRA: [f32; 2] = [0.6, 0.3];
/// How much a spot shows ([`Spot::strength`]) is its darkening times its radius (a fraction of the face), on a log
/// scale from [`FAINTEST`] (0) to [`PLAIN`] (1): a faint pore-sized dot to a dark spot 2 % of the face across.
pub const FAINTEST: f64 = 0.06 * 0.0016;
pub const PLAIN: f64 = 0.5 * 0.01;

/// How much a spot of darkening `contrast` (natural log) and radius `r` (a fraction of the face) shows, 0…1.
fn shows(contrast: f32, r: f64) -> f32 {
    let v = f64::from(contrast.max(0.0)) * r.max(0.0);
    if !(v.is_finite() && v > 0.0) {
        return 0.0;
    }
    ((v / FAINTEST).ln() / (PLAIN / FAINTEST).ln()).clamp(0.0, 1.0) as f32
}
/// The response's curvature may be at most this many times stronger one way than the other (a line or a wrinkle
/// is much longer than wide), per size: the smallest (a hair, a pore's edge) and the largest (a crease, the
/// corner of a nostril) must be rounder.
pub const MAX_ELONGATION: [f32; 7] = [2.0, 2.2, 2.5, 2.5, 2.2, 1.8, 1.8];
/// Red darkening as much as the rest, or more, is a shadow or a hair, not a spot: blood leaves red as it is and
/// melanin darkens it less than green. A spot's red share is at most this.
pub const NEUTRAL: f32 = 0.92;
/// A spot this much darker than the skin around it (natural log: about 30 %) is looked at whatever its colour.
pub const DARK_ENOUGH: f32 = 0.35;
/// … and may be this many times longer one way than [`MAX_ELONGATION`] allows.
const DARK_STRETCH: f32 = 1.5;
/// The mask must be at least this all around a spot (1.5 radii out): a spot at the skin's edge is a lash, a lip
/// line or hair.
const SKIN_ALL_AROUND: f32 = 0.9;

/// A spot reaches as far as the largest size that still answers this share of its best size.
const EXTENT: f32 = 0.75;

/// The texture around a spot is measured over this share of the face's size …
const BUSY: f64 = 0.02;
/// … and the spot must be this many times darker than the texture's mean response (older skin is textured, and
/// its marks stand out from it less than a pimple does from young skin).
pub const STAND_OUT: f32 = 2.5;

/// Skin around a place more textured than this is a beard, stubble or brows, not clean skin: nothing there is a
/// spot.
pub const TOO_BUSY: f32 = 0.06;

/// The blur the skin's brightness around a spot is measured with, a fraction of the face's size …
const BASE: f64 = 0.03;
/// … and the least share of the face's median skin brightness it may have.
pub const DARKEST_SKIN: f32 = 0.6;

/// Skin whose mean colour has less saturation than this is grey (a black-and-white photo).
const GREY: f64 = 0.04;
/// Red's share of the darkening taken on a grey photo, where colour can't tell a spot from a shadow (under
/// [`NEUTRAL`]: shape and contrast decide alone).
const GREY_RED_SHARE: f32 = 0.65;

/// A spot the darkness carries on past both ways (a crease, a wrinkle, a hair) by at least this share of its own
/// darkening ([`traits::Traits::line`]) is not a spot.
pub const LINE: f32 = 0.5;
/// A spot next to something larger and as dark (its darkest sixth around, [`traits::Traits::side`], at least this
/// share of its own darkening) is the edge of a nostril, a fold or a shadow, not a spot.
pub const SIDE: f32 = 0.6;
/// … or when that sixth is this much darker than the skin (natural log, about 40 %) whatever the spot is: a
/// nostril, the inside of a lip corner.
const SIDE_DARK: f32 = 0.5;
/// A spot at least this share of the face in radius …
const BIG: f64 = 0.006;
/// … whose darkness fades to less than this share of its centre's by its edge ([`traits::Traits::edge`]) is the
/// face's shading (the side of the nose, between the brows), not a mark, when it is also fainter than
/// [`BIG_FAINT`] and greyer than the face's spots (red's share of its darkening past their median by
/// [`SHADE_GREYER`] of the way to grey: less light darkens every colour alike): a large mark is dark to its edge
/// or dark, and a pimple is red.
const BIG_EDGE: f32 = 0.5;
const BIG_FAINT: f32 = 0.35;
const SHADE_GREYER: f32 = 0.35;

/// How hard a search looks: the least darkening ([`FIND`]) and how many times the skin's texture a spot must
/// stand out by ([`STAND_OUT`]).
#[derive(Clone, Copy)]
struct Look {
    find: f32,
    stand_out: f32,
}

/// The first search, over the whole skin.
const FIRST: Look = Look { find: FIND, stand_out: STAND_OUT };
/// The second, inside a freckle field ([`classify::in_field`]): a field's fainter freckles, against skin whose
/// texture the field itself is.
const IN_FIELD: Look = Look { find: FIELD_FIND, stand_out: FIELD_STAND_OUT };
pub const FIELD_FIND: f32 = 0.03;
pub const FIELD_STAND_OUT: f32 = 1.2;

/// Most spots kept per face (the strongest; a freckled face has hundreds).
pub const MAX_SPOTS: usize = 1000;
/// Most pixels of the mask's box looked at (the box is reduced to the face's [`WORK`] size first).
const MAX_BOX: usize = 1 << 26;
/// Most candidate places looked at further (the strongest).
const MAX_CANDIDATES: usize = 20_000;

/// The skin of one face, reduced: what detection works on.
struct Work {
    /// The box's corner in the image's pixels and the reduction (image pixels per work pixel).
    x0: usize,
    y0: usize,
    factor: usize,
    /// The face's size in work pixels.
    size: f64,
    w: usize,
    h: usize,
    /// Per pixel: red, green and blue × mask, and the mask.
    weighted: Image<[f32; 4]>,
    /// The face's skin area in image pixels.
    skin_px: f64,
    /// The skin has no colour (a black-and-white photo): red can't tell a spot from a shadow.
    grey: bool,
}

/// One place that answered, with what it is measured by.
#[derive(Clone, Copy, Debug)]
struct Found {
    x: usize,
    y: usize,
    scale: usize,
    contrast: f32,
    /// Red's darkening as a share of the absorbing channel's.
    red_share: f32,
    elongation: f32,
    /// Radius in work pixels.
    radius: f64,
    /// How textured the skin around it is ([`busy`]).
    busy: f32,
}

/// The spots on face `face` of `scene` (on `img`, the picture the face's masks come from). Empty without a Facial
/// Skin mask, or when the face has no skin in the picture.
pub fn find(img: &Rgb32f, scene: &Scene<'_>, face: usize) -> Spots {
    let Some(work) = work(img, scene, face) else { return Spots::default() };
    let size_px = scene.faces.get(face).map_or(1.0, |f| f.size).max(1.0);
    let skin = work.skin_px / (size_px * size_px);
    let (resp, red) = responses(&work);
    let busy = busy(&work, &resp);
    let base = base(&work);
    let mut found = candidates(&work, &resp, &red, &busy, FIRST);
    found.retain(|f| is_skin_bright(&base, f));
    found.sort_by(|a, b| b.contrast.total_cmp(&a.contrast));
    let mut kept = suppress(&[], found, MAX_SPOTS);
    let hairs = strands::Map::of(scene, face);
    // shading darkens red nearly as much as the rest; spots on this face darken it this much at most
    let mut shares: Vec<f32> = kept.iter().map(|f| f.red_share).collect();
    let mid = shares.len() / 2;
    let typical = if shares.is_empty() { 1.0 } else { *shares.select_nth_unstable_by(mid, f32::total_cmp).1 };
    let shade = typical + SHADE_GREYER * (1.0 - typical).max(0.0);
    let beard = beard(scene, face, &work);
    let judge = |f: &Found, field: bool| judge(&work, scene, (hairs.as_ref(), beard.as_ref()), f, field, shade);
    let mut seen: Vec<(Found, traits::Traits)> = kept.iter().filter_map(|f| judge(f, false)).collect();
    let mut kinds = classify::kinds(&seen, work.size, beard.as_ref());
    // a freckle field is looked at again, finer: its fainter freckles go with the rest when they are not kept
    let freckles: Vec<Found> = seen.iter().zip(&kinds).filter(|(_, k)| **k == Some(SpotKind::Freckle)).map(|((f, _), _)| *f).collect();
    if freckles.len() >= classify::FRECKLED {
        let mut more = candidates(&work, &resp, &red, &busy, IN_FIELD);
        more.retain(|f| is_skin_bright(&base, f) && classify::in_field(f, &freckles, work.size));
        more.sort_by(|a, b| b.contrast.total_cmp(&a.contrast));
        let more = suppress(&kept, more, MAX_SPOTS.saturating_sub(kept.len()));
        for f in &more {
            if let Some((f, t)) = judge(f, true) {
                kinds.push(Some(if classify::is_red(&t) { SpotKind::Blemish } else { SpotKind::Freckle }));
                seen.push((f, t));
            }
        }
        kept.extend(more);
    }
    let list = seen
        .iter()
        .zip(kinds)
        .filter_map(|((f, _), kind)| {
            let kind = kind?;
            let at = to_norm(scene, &work, f.x as f64 + 0.5, f.y as f64 + 0.5)?;
            let donor = donor(&work, &resp, &kept, f).and_then(|(dx, dy)| to_norm(scene, &work, dx, dy));
            let strength = shows(f.contrast, f.radius / work.size);
            Some(Spot { at, radius: f.radius * work.factor as f64 / size_px, donor, kind, strength })
        })
        .collect();
    Spots { list, skin }
}

/// Spot `f` with its look, or `None` when it is not a spot: a crease or a hair through it ([`LINE`]), the edge of
/// something larger and darker ([`SIDE`], [`SIDE_DARK`]), the face's shading ([`BIG`], greyer than `shade`), or on a strand Stray Hair's
/// search found (a hair stump in stubble, a crease). In a freckle field (`field`) the freckles around a spot are
/// as dark as it and line up by chance: only a nostril or a fold as dark as [`SIDE_DARK`] counts there.
fn judge(
    work: &Work,
    scene: &Scene<'_>,
    (hairs, beard): (Option<&strands::Map<'_>>, Option<&classify::Beard>),
    f: &Found,
    field: bool,
    shade: f32,
) -> Option<(Found, traits::Traits)> {
    let t = traits::measure(work, f)?;
    let at = to_norm(scene, work, f.x as f64 + 0.5, f.y as f64 + 0.5)?;
    let factor = work.factor as f64;
    let stubbly = beard.is_some_and(|b| b.holds(f.x as f64 + 0.5, f.y as f64 + 0.5));
    let on_hair = hairs.is_some_and(|h| h.covers(at, f.radius * factor, work.size * factor, stubbly));
    let shading = f.radius >= BIG * work.size && t.edge < BIG_EDGE && f.contrast < BIG_FAINT && f.red_share >= shade;
    let beside = (t.side >= SIDE && !field) || t.side * t.dark >= SIDE_DARK;
    let line = t.line >= LINE && !field;
    (!line && !beside && !shading && !on_hair).then_some((*f, t))
}

/// The face's beard area ([`classify::Beard`]) from its first five landmarks, in work pixels.
fn beard(scene: &Scene<'_>, face: usize, w: &Work) -> Option<classify::Beard> {
    let f = scene.faces.get(face)?;
    let view = scene.view();
    let k = w.factor as f64;
    let px = |i: usize| -> Option<(f64, f64)> {
        let (x, y) = view.to_px(f.face.landmark(i)?.0, f.face.landmark(i)?.1)?;
        Some(((x - w.x0 as f64) / k, (y - w.y0 as f64) / k))
    };
    classify::Beard::new([px(0)?, px(1)?], px(2)?, [px(3)?, px(4)?])
}

/// Work pixel (`x`, `y`) → normalised photo coordinates.
fn to_norm(scene: &Scene<'_>, w: &Work, x: f64, y: f64) -> Option<(f64, f64)> {
    let f = w.factor as f64;
    let p = scene.view().to_norm(w.x0 as f64 + x * f, w.y0 as f64 + y * f);
    (p.0.is_finite() && p.1.is_finite()).then_some(p)
}

fn work(img: &Rgb32f, scene: &Scene<'_>, face: usize) -> Option<Work> {
    if img.data.len() != img.width.saturating_mul(img.height) {
        return None;
    }
    let size = scene.faces.get(face)?.size;
    // on a smaller face a spot is a pixel or two: noise, not something to retouch
    if size < MIN_FACE {
        return None;
    }
    let mask = scene.faces.get(face)?.face.mask(Part::FacialSkin)?;
    let [x0, y0, x1, y1] = scene.mask_box(face, Part::FacialSkin)?;
    let (bw, bh) = (x1 - x0, y1 - y0);
    if bw.saturating_mul(bh) > MAX_BOX || !size.is_finite() {
        return None;
    }
    let factor = ((size / WORK).floor() as usize).clamp(1, bw.max(bh).max(1));
    let (w, h) = (bw.div_ceil(factor), bh.div_ceil(factor));
    let view = scene.view();
    let mut weighted = Image::<[f32; 4]>::new(w, h);
    let mut skin_rows = vec![0.0f64; h];
    par_rows(&mut weighted.data, w.max(1), |sy, row| {
        let ys = y0 + sy * factor..(y0 + (sy + 1) * factor).min(y1);
        for (sx, o) in row.iter_mut().enumerate() {
            let xs = x0 + sx * factor..(x0 + (sx + 1) * factor).min(x1);
            let mut acc = [0.0f32; 4];
            for y in ys.clone() {
                for x in xs.clone() {
                    let (nx, ny) = view.to_norm(x as f64 + 0.5, y as f64 + 0.5);
                    let m = mask.sample(nx, ny).clamp(0.0, 1.0);
                    let p = img.data.get(y * img.width + x).copied().unwrap_or_default();
                    acc = [acc[0] + p[0].max(0.0) * m, acc[1] + p[1].max(0.0) * m, acc[2] + p[2].max(0.0) * m, acc[3] + m];
                }
            }
            let n = (ys.len() * xs.len()).max(1) as f32;
            *o = acc.map(|v| if v.is_finite() { v / n } else { 0.0 });
        }
    });
    for (s, row) in skin_rows.iter_mut().zip(weighted.data.chunks(w.max(1))) {
        *s = row.iter().map(|p| f64::from(p[3])).sum::<f64>() * (factor * factor) as f64;
    }
    let skin_px: f64 = skin_rows.iter().sum();
    // the skin's mean colour, from up to 128 × 128 samples
    let step = (bw.max(bh) / 128).max(1);
    let mut mean = [0.0f64; 3];
    for y in (y0..y1).step_by(step) {
        for x in (x0..x1).step_by(step) {
            let (nx, ny) = view.to_norm(x as f64 + 0.5, y as f64 + 0.5);
            let m = f64::from(mask.sample(nx, ny));
            let p = img.data.get(y * img.width + x).copied().unwrap_or_default();
            mean = std::array::from_fn(|c| mean[c] + f64::from(p[c].max(0.0)) * m);
        }
    }
    let (hi, lo) = (mean[0].max(mean[1]).max(mean[2]), mean[0].min(mean[1]).min(mean[2]));
    let grey = hi <= 0.0 || (hi - lo) / hi < GREY;
    (skin_px > 0.0).then_some(Work { x0, y0, factor, size: size / factor as f64, w, h, weighted, skin_px, grey })
}

/// Blurs under this σ (pixels) use exact Gaussian taps (the three-box blur snaps small sizes to whole pixels).
const EXACT_BELOW: f32 = 2.0;

fn blur(img: &Image<[f32; 4]>, sigma: f32) -> Image<[f32; 4]> {
    if sigma < EXACT_BELOW && sigma.is_finite() && sigma > 0.0 { gaussian_taps(img, &gauss_taps(sigma)) } else { gaussian(img, sigma) }
}

/// The blur at size `i` ([`SCALES`], continued by [`STEP`] past its end), in work pixels.
fn sigma(work: &Work, i: usize) -> f32 {
    let last = SCALES.len() - 1;
    let f = SCALES.get(i).copied().unwrap_or_else(|| SCALES[last] * STEP.powi((i - last) as i32));
    (f * work.size) as f32
}

/// Per size of [`SCALES`]: the response (how much darker than the surroundings, natural log; 0 off the skin) and
/// red's darkening at the same size.
fn responses(work: &Work) -> (Vec<Plane>, Vec<Plane>) {
    let (w, h) = (work.w, work.h);
    let n = SCALES.len();
    let mut levels: Vec<Image<[f32; 4]>> = (0..SURROUND).map(|i| blur(&work.weighted, sigma(work, i))).collect();
    let (mut resp, mut red) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for i in 0..n {
        let outer = blur(&work.weighted, sigma(work, i + SURROUND));
        let inner = levels.remove(0);
        let mut r = Plane::new(w, h);
        let mut d = Plane::new(w, h);
        let row_of = |img: &Image<[f32; 4]>, y: usize| img.data.get(y * w..(y + 1) * w).unwrap_or(&[]).to_vec();
        par_rows(&mut r.data, w.max(1), |y, row| {
            let (a, b) = (row_of(&inner, y), row_of(&outer, y));
            for ((o, p), q) in row.iter_mut().zip(&a).zip(&b) {
                *o = if p[3] >= 0.5 && q[3] > 1e-3 { ln_ratio(absorb(q) / q[3], absorb(p) / p[3]) } else { 0.0 };
            }
        });
        par_rows(&mut d.data, w.max(1), |y, row| {
            let (a, b) = (row_of(&inner, y), row_of(&outer, y));
            for ((o, p), q) in row.iter_mut().zip(&a).zip(&b) {
                *o = if p[3] >= 0.5 && q[3] > 1e-3 { ln_ratio(q[0] / q[3], p[0] / p[3]) } else { 0.0 };
            }
        });
        resp.push(r);
        red.push(d);
        levels.push(outer);
    }
    (resp, red)
}

/// The absorbing channel ([`ABSORB`]) of a weighted pixel (weighted like it).
fn absorb(p: &[f32; 4]) -> f32 {
    p[0] * ABSORB[0] + p[1] * ABSORB[1] + p[2] * ABSORB[2]
}

/// ln(a / b), 0 when either is not a positive number.
fn ln_ratio(a: f32, b: f32) -> f32 {
    const DARK: f32 = 1e-5;
    if a > DARK && b > DARK && a.is_finite() && b.is_finite() { (a / b).ln() } else { 0.0 }
}

fn at(p: &Plane, x: isize, y: isize) -> f32 {
    if x < 0 || y < 0 || x as usize >= p.width || y as usize >= p.height {
        return 0.0;
    }
    p.data.get(y as usize * p.width + x as usize).copied().unwrap_or(0.0)
}

/// The threshold of size `i`.
fn threshold(i: usize) -> f32 {
    FIND * (1.0 + SMALL_EXTRA.get(i).copied().unwrap_or(0.0))
}

/// Every place darker than its neighbours at its best size, by more than that size's threshold, with no larger
/// structure behind it, round and with skin all around.
fn candidates(work: &Work, resp: &[Plane], red: &[Plane], busy: &Plane, look: Look) -> Vec<Found> {
    let (w, h) = (work.w, work.h);
    let detect = SCALES.len() - 1;
    // the best size per pixel
    let mut best = Plane::new(w, h);
    let mut scale = vec![0u8; w * h];
    for (i, r) in resp.iter().enumerate().take(detect) {
        for (j, v) in r.data.iter().enumerate() {
            if let (Some(b), Some(s)) = (best.data.get_mut(j), scale.get_mut(j))
                && *v > *b
            {
                *b = *v;
                *s = i as u8;
            }
        }
    }
    let mut out = Vec::new();
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let j = y * w + x;
            let (v, i) = (best.data.get(j).copied().unwrap_or(0.0), scale.get(j).copied().unwrap_or(0) as usize);
            if v < threshold(i) * look.find / FIND {
                continue;
            }
            let (xi, yi) = (x as isize, y as isize);
            // a local maximum (ties go to the first in reading order)
            let n = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];
            let peak = n.iter().enumerate().all(|(k, (dx, dy))| {
                let o = at(&best, xi + dx, yi + dy);
                if k < 4 { v > o } else { v >= o }
            });
            if !peak {
                continue;
            }
            // no larger structure behind it
            if resp.get(detect).is_some_and(|r| at(r, xi, yi) >= v) {
                continue;
            }
            let Some(f) = measure(work, resp, red, busy, (x, y, i, v), look.stand_out) else { continue };
            out.push(f);
            if out.len() >= MAX_CANDIDATES {
                return out;
            }
        }
    }
    out
}

/// The place's shape and colour; `None` when it is a line or lies at the skin's edge.
fn measure(work: &Work, resp: &[Plane], red: &[Plane], busy: &Plane, place: (usize, usize, usize, f32), stand_out: f32) -> Option<Found> {
    let (x, y, i, v) = place;
    let r = resp.get(i)?;
    let s = sigma(work, i);
    let radius = f64::from(s) * std::f64::consts::SQRT_2;
    let (xi, yi) = (x as isize, y as isize);
    let step = (s.round() as isize).max(1);
    let at_r = |dx: isize, dy: isize| at(r, xi + dx * step, yi + dy * step);
    let st = (step * step) as f32;
    // the curvature of the response (negated: a peak curves down)
    let dxx = -(at_r(1, 0) + at_r(-1, 0) - 2.0 * v) / st;
    let dyy = -(at_r(0, 1) + at_r(0, -1) - 2.0 * v) / st;
    let dxy = -(at_r(1, 1) - at_r(1, -1) - at_r(-1, 1) + at_r(-1, -1)) / (4.0 * st);
    let (tr, det) = (dxx + dyy, dxx * dyy - dxy * dxy);
    if det <= 0.0 || tr <= 0.0 {
        return None;
    }
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let (big, small) = (tr / 2.0 + disc, tr / 2.0 - disc);
    let elongation = if small > 0.0 { big / small } else { f32::INFINITY };
    // a dark spot may be less round: a mole beside stubble or a fold reads a little stretched, and a line through it
    // is told by what is around it ([`LINE`])
    let stretch = if v >= DARK_ENOUGH { DARK_STRETCH } else { 1.0 };
    if elongation > MAX_ELONGATION.get(i).copied().unwrap_or(0.0) * stretch {
        return None;
    }
    if !skin_all_around(work, x as f64 + 0.5, y as f64 + 0.5, radius * 1.5 + 1.0) {
        return None;
    }
    let around: f32 = (0..8)
        .map(|k| {
            let a = f64::from(k) * std::f64::consts::TAU / 8.0;
            let d = radius * 3.0 + 1.0;
            at(busy, (x as f64 + d * a.cos()) as isize, (y as f64 + d * a.sin()) as isize)
        })
        .sum::<f32>()
        / 8.0;
    if v < stand_out * around || around > TOO_BUSY {
        return None;
    }
    // (without colour every spot reads as brown: shape and contrast alone tell them apart)
    let red_share = if work.grey { GREY_RED_SHARE } else { red.get(i).map_or(0.0, |d| at(d, xi, yi)) / v.max(1e-6) };
    // (a mole can be as grey as a shadow: a dark spot is looked at further, and a shadow or a hair is told by its
    // shape and what is around it)
    if red_share > NEUTRAL && v < DARK_ENOUGH {
        return None;
    }
    // a mark with a faint wide rim answers almost as much at the next sizes up: it reaches as far as they do
    let reach = (i + 1..SCALES.len() - 1).take_while(|k| resp.get(*k).is_some_and(|p| at(p, xi, yi) >= EXTENT * v)).last().unwrap_or(i);
    let radius = f64::from(sigma(work, reach)) * std::f64::consts::SQRT_2;
    Some(Found { x, y, scale: i, contrast: v, red_share, elongation, radius, busy: around })
}

/// How textured the skin is around each place: the mean size of the small responses lighter than their
/// surroundings (the second size), doubled, over [`BUSY`] of the face. A beard, stubble or rough skin is busy
/// both ways and a spot must stand out from it; a field of freckles is dark spots on even skin, so the freckles
/// don't count as texture against each other.
fn busy(work: &Work, resp: &[Plane]) -> Plane {
    let Some(r) = resp.get(1) else { return Plane::new(work.w, work.h) };
    let abs = Plane { width: r.width, height: r.height, data: r.data.iter().map(|v| (-v).max(0.0) * 2.0).collect() };
    gaussian(&abs, (BUSY * work.size) as f32)
}

/// The skin's brightness around each place ([`BASE`] blur), as a ratio to the face's median skin brightness.
fn base(work: &Work) -> Plane {
    let blurred = blur(&work.weighted, (BASE * work.size) as f32);
    let mut ys: Vec<f32> = blurred.data.iter().filter(|p| p[3] >= 0.9).map(|p| absorb(p) / p[3]).filter(|v| v.is_finite()).collect();
    let median = if ys.is_empty() {
        0.0
    } else {
        let mid = ys.len() / 2;
        *ys.select_nth_unstable_by(mid, f32::total_cmp).1
    };
    Plane {
        width: work.w,
        height: work.h,
        data: blurred.data.iter().map(|p| if p[3] > 1e-3 && median > 1e-6 { absorb(p) / p[3] / median } else { 0.0 }).collect(),
    }
}

/// A spot's surroundings are lit skin: at least [`DARKEST_SKIN`] of the face's median. Darker is a beard, stubble,
/// the hairline or deep shadow, where there is nothing to retouch.
fn is_skin_bright(base: &Plane, f: &Found) -> bool {
    at(base, f.x as isize, f.y as isize) >= DARKEST_SKIN
}

/// The mask (reduced, not blurred) at least [`SKIN_ALL_AROUND`] at the centre and at 12 points `r` out.
fn skin_all_around(work: &Work, cx: f64, cy: f64, r: f64) -> bool {
    let m = |x: f64, y: f64| {
        if x < 0.0 || y < 0.0 {
            return 0.0;
        }
        let (xi, yi) = (x as usize, y as usize);
        if xi >= work.w || yi >= work.h {
            return 0.0;
        }
        work.weighted.data.get(yi * work.w + xi).map_or(0.0, |p| p[3])
    };
    m(cx, cy) >= SKIN_ALL_AROUND
        && (0..12).all(|k| {
            let a = k as f64 * std::f64::consts::TAU / 12.0;
            m(cx + r * a.cos(), cy + r * a.sin()) >= SKIN_ALL_AROUND
        })
}

/// The strongest first: a find overlapping a stronger one, or one of `prior` (an earlier search's), is the same
/// spot. At most `most`.
fn suppress(prior: &[Found], found: Vec<Found>, most: usize) -> Vec<Found> {
    let mut kept: Vec<Found> = Vec::new();
    let overlaps = |k: &Found, f: &Found| {
        let d = ((k.x as f64 - f.x as f64).powi(2) + (k.y as f64 - f.y as f64).powi(2)).sqrt();
        d < (k.radius + f.radius) * 0.75
    };
    for f in found {
        if kept.len() >= most {
            break;
        }
        if !prior.iter().chain(&kept).any(|k| overlaps(k, &f)) {
            kept.push(f);
        }
    }
    kept
}

/// A clean patch of skin next to spot `f` (work pixels): of 16 places 2.6 and 3.4 radii away, the one with skin all
/// around, clear of every spot, answering least at the spot's size and closest to the skin around the spot in
/// brightness. `None` when none has skin all around.
fn donor(work: &Work, resp: &[Plane], spots: &[Found], f: &Found) -> Option<(f64, f64)> {
    let (cx, cy) = (f.x as f64 + 0.5, f.y as f64 + 0.5);
    let r = f.radius;
    let resp = resp.get(f.scale)?;
    let bright = |x: f64, y: f64| {
        let (xi, yi) = (x.max(0.0) as usize, y.max(0.0) as usize);
        work.weighted.data.get(yi.min(work.h.saturating_sub(1)) * work.w + xi.min(work.w.saturating_sub(1))).map_or(0.0, |p| absorb(p) / p[3].max(1e-3))
    };
    let ring: f32 =
        (0..8).map(|k| f64::from(k) * std::f64::consts::TAU / 8.0).map(|a| bright(cx + 1.8 * r * a.cos(), cy + 1.8 * r * a.sin())).sum::<f32>() / 8.0;
    let mut best: Option<(f32, (f64, f64))> = None;
    for dist in [2.6, 3.4] {
        for k in 0..8 {
            let a = (f64::from(k) + if dist > 3.0 { 0.5 } else { 0.0 }) * std::f64::consts::TAU / 8.0;
            let (dx, dy) = (cx + dist * r * a.cos(), cy + dist * r * a.sin());
            if !skin_all_around(work, dx, dy, r * 1.6 + 1.0) {
                continue;
            }
            let clear = spots.iter().all(|o| {
                let d = ((o.x as f64 + 0.5 - dx).powi(2) + (o.y as f64 + 0.5 - dy).powi(2)).sqrt();
                std::ptr::eq(o, f) || d > (o.radius + r) * 1.3
            });
            if !clear {
                continue;
            }
            let pts = (0..9).map(|j| {
                let b = f64::from(j) * std::f64::consts::TAU / 8.0;
                let rr = if j == 8 { 0.0 } else { r * 0.7 };
                (dx + rr * b.cos(), dy + rr * b.sin())
            });
            let (mut busy, mut light) = (0.0f32, 0.0f32);
            for (px, py) in pts {
                busy += at(resp, px as isize, py as isize).abs();
                light += bright(px, py);
            }
            let score = busy / 9.0 + ln_ratio(light / 9.0, ring).abs();
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, (dx, dy)));
            }
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sizes_cover_spots_of_three_tenths_to_three_percent_of_the_face() {
        let across = |s: f64| 2.0 * std::f64::consts::SQRT_2 * s;
        assert!((across(SCALES[0]) - 0.003).abs() < 0.0003);
        assert!((across(SCALES[SCALES.len() - 2]) - 0.03).abs() < 0.002);
        for w in SCALES.windows(2) {
            assert!((w[1] / w[0] - STEP).abs() < 0.02, "{w:?}");
        }
    }
}
