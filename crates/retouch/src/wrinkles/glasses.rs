//! Wrinkles v2, glasses: the people parse gives the whole of a pair of glasses (frame, lenses and often the skin beside
//! them) as one part, Others, and leaves it out of Facial Skin, so the crow's feet behind and beside a lens had no skin to
//! work on. Here the skin is taken back: inside Others, a pixel whose colour and light are the face's skin's (seen
//! through the lens, or beside the frame) is skin; the rest (the frame, a lens's rim, a reflection) is the frame, and the
//! frame is kept out with a thin margin ([`MARGIN`]) so no line search or fill reads or touches it.

use lightcraft_raster::morph::{max_filter, min_filter};
use lightcraft_raster::{Plane, Rgb32f};

use super::heal::smoothstep;

/// The thinnest frame, a fraction of the face (half its width): thinner dark lines inside the glasses are skin's folds.
const THICK: f64 = 0.004;
/// The frame's margin, a fraction of the face: a line ending at the frame is not filled from it.
const MARGIN: f64 = 0.006;
/// How far a pixel's chroma (log red over green, log blue over green) may lie from the skin's median, in spreads of the
/// skin's own (median absolute deviation × 1.4826), to count as skin; and how much darker or lighter (log light).
const CHROMA: (f32, f32) = (6.0, 10.0);
const DARKER: (f32, f32) = (2.0, 2.8);
const LIGHTER: (f32, f32) = (0.5, 0.8);
/// The least spread a chroma is given (log), so a face of one even colour does not make everything else "not skin".
const MIN_SPREAD: f32 = 0.05;

/// The skin seen through and beside the glasses (0…1): inside `others` (the Others part, 0…1), the pixels of the log
/// picture `lg` that look like the face's skin (measured where `skin`, Facial Skin, is), with the frame and a margin
/// round it left out. Zero everywhere when there is no Others part or too little skin to measure.
pub(super) fn seen_through(lg: &Rgb32f, skin: &Plane, others: &Plane, size: f64) -> Plane {
    let (w, h) = (lg.width, lg.height);
    let none = Plane::new(w, h);
    if skin.data.len() != lg.data.len() || others.data.len() != lg.data.len() || !others.data.iter().any(|v| *v > 0.3) {
        return none;
    }
    let chroma = |p: [f32; 3]| (p[0] - p[1], p[2] - p[1], 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]);
    let sample: Vec<(f32, f32, f32)> =
        lg.data.iter().zip(skin.data.iter().zip(&others.data)).filter(|(_, (s, o))| **s > 0.9 && **o < 0.1).map(|(p, _)| chroma(*p)).collect();
    if sample.len() < 256 {
        return none;
    }
    let stat = |pick: fn(&(f32, f32, f32)) -> f32| -> (f32, f32) {
        let mut v: Vec<f32> = sample.iter().map(pick).filter(|x| x.is_finite()).collect();
        let mid = v.len() / 2;
        if v.is_empty() {
            return (0.0, MIN_SPREAD);
        }
        let m = *v.select_nth_unstable_by(mid, f32::total_cmp).1;
        let mut d: Vec<f32> = v.iter().map(|x| (x - m).abs()).collect();
        let s = *d.select_nth_unstable_by(mid, f32::total_cmp).1 * 1.4826;
        (m, s.max(MIN_SPREAD))
    };
    let (rg, bg, l) = (stat(|c| c.0), stat(|c| c.1), stat(|c| c.2));
    // per pixel: whether its colour is the skin's, and whether its light is
    let parts: Vec<(f32, f32)> = lg
        .data
        .iter()
        .zip(&others.data)
        .map(|(p, o)| {
            if *o <= 0.3 {
                return (0.0, 0.0);
            }
            let (a, b, y) = chroma(*p);
            let off = ((a - rg.0) / rg.1).abs().max(((b - bg.0) / bg.1).abs());
            let colour = 1.0 - smoothstep(CHROMA.0, CHROMA.1, off);
            let light = 1.0 - smoothstep(DARKER.0, DARKER.1, l.0 - y).max(smoothstep(LIGHTER.0, LIGHTER.1, y - l.0));
            (colour, light)
        })
        .collect();
    let like = Plane { width: w, height: h, data: parts.iter().map(|(c, l)| c * l).collect() };
    // the frame: Others whose colour is not the skin's (a fold in deep shadow keeps the skin's colour and is skin, only
    // too dark to work on), or far lighter than it (a reflection)
    let frame = Plane {
        width: w,
        height: h,
        data: parts
            .iter()
            .zip(&others.data)
            .zip(lg.data.iter())
            .map(|(((c, _), o), p)| {
                let lighter = chroma(*p).2 - l.0 > LIGHTER.1;
                if *o > 0.3 && (*c < 0.5 || lighter) { 1.0 } else { 0.0 }
            })
            .collect(),
    };
    // only what is thick enough to be a frame: thinner dark lines are the skin's (a thin wire frame is kept out by the
    // line search, which takes nothing that dark for a wrinkle)
    let open = ((THICK * size).round() as usize).max(1);
    let frame = max_filter(&min_filter(&frame, open), open);
    let margin = ((MARGIN * size).ceil() as usize).max(1);
    let keep_out = max_filter(&frame, margin);
    like.zip_map(&keep_out, |s, k| s * (1.0 - k))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKIN: [f32; 3] = [0.45, 0.3, 0.24];

    /// Skin 200 px across with a pair of black frames (rows 60–64 and 96–100, columns 40–160) and the skin seen through
    /// the lens between them a little paler; Others covers rows 50–110, Facial Skin the rest.
    fn picture() -> (Rgb32f, Plane, Plane) {
        let lg = Rgb32f::from_fn(200, 200, |x, y| {
            let frame = (40..160).contains(&x) && ((60..65).contains(&y) || (96..101).contains(&y));
            let k = if frame {
                0.05
            } else if (65..96).contains(&y) {
                1.15
            } else {
                1.0
            } * (1.0 + 0.03 * (((x * 13 + y * 7) % 5) as f32 - 2.0) / 2.0);
            let p = if frame { [0.04, 0.04, 0.05] } else { SKIN.map(|c| c * k) };
            p.map(f32::ln)
        });
        let others = Plane::from_fn(200, 200, |_, y| if (50..111).contains(&y) { 1.0 } else { 0.0 });
        let skin = others.map(|o| 1.0 - o);
        (lg, skin, others)
    }

    #[test]
    fn the_skin_through_the_lens_is_skin_and_the_frame_and_its_margin_are_not() {
        let (lg, skin, others) = picture();
        let s = seen_through(&lg, &skin, &others, 200.0);
        assert!(s.get(100, 80) > 0.99, "through the lens");
        assert!(s.get(100, 55) > 0.99 && s.get(20, 80) > 0.99, "beside the frame");
        assert!(s.get(100, 62) == 0.0 && s.get(100, 98) == 0.0, "the frame");
        assert!(s.get(100, 66) == 0.0 && s.get(100, 58) == 0.0, "its margin");
        assert!(s.get(100, 20) == 0.0, "outside Others: Facial Skin's own");
    }

    #[test]
    fn no_glasses_no_skin_added() {
        let (lg, skin, _) = picture();
        assert!(seen_through(&lg, &skin, &Plane::new(200, 200), 200.0).data.iter().all(|v| *v == 0.0));
        assert!(seen_through(&lg, &skin, &Plane::new(3, 3), 200.0).data.iter().all(|v| *v == 0.0));
    }
}
