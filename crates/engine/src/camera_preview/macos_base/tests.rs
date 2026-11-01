//! T6: the tone of a macOS Colour photo fitted to its camera JPEG.
use super::super::*;
use super::{BaseReference, fit_base_tone};
use lightcraft_geom::Orientation;

/// A linear base `w` × `h` with a spread of brightness and colour, and the camera JPEG of it (linear values): the base
/// through a filmic curve, the colours kept (the decoder's colours are the camera's).
fn scene(w: usize, h: usize) -> (Rgb32f, Rgb32f) {
    let mut base = Rgb32f::new(w, h);
    let mut jpeg = base.clone();
    for (i, (src, dst)) in base.data.iter_mut().zip(&mut jpeg.data).enumerate() {
        let y = 0.004 + ((i * 7919) % 6144) as f32 / 6144.0 * 0.6;
        let rgb = [0.6 + (i % 11) as f32 * 0.07, 1.0, 0.6 + (i % 17) as f32 * 0.05];
        *src = rgb.map(|v| v * y / luminance_2020(rgb));
        let display = 1.6 * y / (y + 0.35);
        *dst = src.map(|v| v * display / y);
    }
    (base, jpeg)
}

fn reference(image: Rgb32f) -> BaseReference {
    BaseReference { image, dro: false }
}

#[test]
fn the_tone_is_fitted_with_the_decoders_colours() {
    let (base, jpeg) = scene(96, 64);
    let look = fit_base_tone(&reference(jpeg.clone()), Orientation::Normal, &base).expect("an accepted fit");
    assert_eq!(look.matrix, Mat3::IDENTITY, "the decoder's colours are kept");
    assert!(look.hue_sat.is_none());
    let error = base.data.iter().zip(&jpeg.data).map(|(x, y)| (look.tone.apply(luminance_2020(*x)) - luminance_2020(*y)).abs()).sum::<f32>()
        / base.data.len() as f32;
    assert!(error < 0.01, "mean luminance error {error}");
}

/// The camera JPEG is stored in sensor orientation; the decoder's base is upright: the JPEG is turned before pairing.
#[test]
fn a_portrait_jpeg_is_turned_to_the_upright_base() {
    let (base, jpeg) = scene(96, 64);
    let upright = base.clone().into_oriented(Orientation::Rotate90);
    assert!(fit_base_tone(&reference(jpeg.clone()), Orientation::Rotate90, &upright).is_some());
    assert!(fit_base_tone(&reference(jpeg), Orientation::Normal, &upright).is_none(), "another frame: refused");
}

/// No usable pairs (a black JPEG) or a tiny base: the fit is refused.
#[test]
fn an_unusable_jpeg_is_refused() {
    let (base, mut jpeg) = scene(96, 64);
    jpeg.map_in_place(|_| [0.0; 3]);
    assert!(fit_base_tone(&reference(jpeg), Orientation::Normal, &base).is_none());
    let (small, small_jpeg) = scene(8, 6);
    assert!(fit_base_tone(&reference(small_jpeg), Orientation::Normal, &small).is_none());
}
