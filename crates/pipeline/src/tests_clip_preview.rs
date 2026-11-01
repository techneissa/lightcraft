//! The Alt-drag previews ([`Overlay::ClipHighlights`], [`Overlay::ClipShadows`], [`Overlay::SharpenMask`],
//! [`Overlay::Greyscale`]): what each shows, on synthetic pixels and on real renders.

use lightcraft_develop::DevelopSettings;
use lightcraft_raster::{Rgb32f, Rgba8};

use crate::visualize::{clip_view, greyscale};
use crate::{Overlay, RenderRequest, SourceInfo, render};

fn px(c: [u8; 3]) -> [u8; 4] {
    [c[0], c[1], c[2], 255]
}

#[test]
fn highlight_clipping_is_a_black_image_with_the_clipped_channels() {
    let src = [[255, 255, 255], [255, 10, 10], [10, 255, 255], [255, 255, 0], [128, 128, 128], [0, 0, 0]];
    let mut img = Rgba8 { width: src.len(), height: 1, data: src.map(px).to_vec() };
    clip_view(&mut img, true);
    let want = [[255, 255, 255], [255, 0, 0], [0, 255, 255], [255, 255, 0], [0, 0, 0], [0, 0, 0]];
    assert_eq!(img.data, want.map(px).to_vec());
}

#[test]
fn shadow_clipping_is_a_white_image_with_the_crushed_channels() {
    let src = [[0, 0, 0], [0, 100, 100], [100, 0, 100], [100, 100, 0], [128, 128, 128], [255, 255, 255]];
    let mut img = Rgba8 { width: src.len(), height: 1, data: src.map(px).to_vec() };
    clip_view(&mut img, false);
    // black when all three are at 0; a crushed channel drops out of the white (red out = cyan)
    let want = [[0, 0, 0], [0, 255, 255], [255, 0, 255], [255, 255, 0], [255, 255, 255], [255, 255, 255]];
    assert_eq!(img.data, want.map(px).to_vec());
}

#[test]
fn greyscale_has_equal_channels_at_the_luma() {
    let mut img = Rgba8 { width: 3, height: 1, data: vec![px([255, 0, 0]), px([0, 255, 0]), px([40, 40, 40])] };
    greyscale(&mut img);
    assert_eq!(img.data, vec![px([54; 3]), px([182; 3]), px([40; 3])]);
}

/// Left half flat, right half hard vertical stripes.
fn flat_and_stripes() -> Rgb32f {
    Rgb32f::from_fn(240, 160, |x, _| {
        let v = if x < 120 {
            0.2
        } else if (x / 4) % 2 == 0 {
            0.0
        } else {
            0.6
        };
        [v; 3]
    })
}

fn render_with(src: &Rgb32f, s: &DevelopSettings, overlay: Overlay) -> Rgba8 {
    let req = RenderRequest { overlay, ..RenderRequest::fit(240, 160) };
    render(src, &SourceInfo::default(), s, &req).image
}

fn mean_in(img: &Rgba8, x0: usize, x1: usize) -> f64 {
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in 0..img.height {
        for x in x0..x1 {
            sum += f64::from(img.data[y * img.width + x][0]);
            n += 1.0;
        }
    }
    sum / n
}

#[test]
fn the_clipping_previews_render_only_from_clipped_pixels() {
    let src = flat_and_stripes();
    let mut s = DevelopSettings::default();
    s.light.exposure = 3.0;
    let hi = render_with(&src, &s, Overlay::ClipHighlights);
    assert!(hi.data.iter().all(|p| p.iter().take(3).all(|&v| v == 0 || v == 255)));
    assert!(hi.data.iter().any(|p| p[..3] == [255, 255, 255]), "+3 EV clips the bright stripes to white");
    assert!(hi.data.iter().any(|p| p[..3] == [0, 0, 0]));
    s.light.exposure = -3.0;
    let lo = render_with(&src, &s, Overlay::ClipShadows);
    assert!(lo.data.iter().all(|p| p.iter().take(3).all(|&v| v == 0 || v == 255)));
    assert!(lo.data.iter().any(|p| p[..3] == [0, 0, 0]), "-3 EV crushes the dark stripes to black");
    // no clipping, no warning: a mid grey is a plain black / white image
    let calm = render_with(&Rgb32f::from_fn(240, 160, |_, _| [0.18; 3]), &DevelopSettings::default(), Overlay::ClipHighlights);
    assert!(calm.data.iter().all(|p| p[..3] == [0, 0, 0]));
}

#[test]
fn the_greyscale_preview_matches_the_photo_s_luma() {
    let src = flat_and_stripes();
    let s = DevelopSettings::default();
    let plain = render_with(&src, &s, Overlay::None);
    let grey = render_with(&src, &s, Overlay::Greyscale);
    for (a, b) in plain.data.iter().zip(&grey.data) {
        assert!(b[0] == b[1] && b[1] == b[2]);
        let luma = 0.2126 * f32::from(a[0]) + 0.7152 * f32::from(a[1]) + 0.0722 * f32::from(a[2]);
        assert!((f32::from(b[0]) - luma).abs() <= 1.0);
    }
}

#[test]
fn the_sharpen_mask_shows_edges_only_once_masking_is_raised() {
    let src = flat_and_stripes();
    let mut s = DevelopSettings::default();
    // Amount is still 0: the preview needs no sharpening on to show the mask
    s.detail.sharpen_masking = 0.0;
    let all = render_with(&src, &s, Overlay::SharpenMask);
    assert!(all.data.iter().all(|p| p[..3] == [255, 255, 255]), "masking 0 sharpens everywhere");
    s.detail.sharpen_masking = 80.0;
    let edges = render_with(&src, &s, Overlay::SharpenMask);
    let (flat, striped) = (mean_in(&edges, 10, 100), mean_in(&edges, 130, 230));
    assert!(flat < 5.0, "the flat half is masked out: {flat}");
    assert!(striped > 100.0, "the stripes' edges stay: {striped}");
    // the mask is black and white (grey only at its soft edge): never coloured
    assert!(edges.data.iter().all(|p| p[0] == p[1] && p[1] == p[2]));
}

#[test]
fn the_new_overlays_have_their_own_keys_and_survive_the_worker_round_trip() {
    let all = [Overlay::ClipHighlights, Overlay::ClipShadows, Overlay::SharpenMask, Overlay::Greyscale];
    let mut keys: Vec<u64> = all.iter().map(|o| o.key()).collect();
    keys.push(Overlay::None.key());
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), 5);
    for o in all {
        let (k, v) = o.to_parts();
        assert_eq!(Overlay::from_parts(k, v), o);
    }
}
