//! The sharpening-mask preview (Alt-drag on Sharpening Masking, [`Overlay::SharpenMask`]): it shows the weight the
//! render sharpens with, on real renders.

use lightcraft_develop::DevelopSettings;
use lightcraft_raster::{Rgb32f, Rgba8};

use crate::{Overlay, RenderRequest, SourceInfo, render};

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

/// A soft 25 % step edge (no stripe-hard contrast) on a flat field.
fn soft_step() -> Rgb32f {
    Rgb32f::from_fn(240, 160, |x, _| [if x < 120 { 0.2 } else { 0.25 }; 3])
}

/// Process V3 sharpens a band of its own with a mask range of its own: the ⌥-drag preview must show the weight the
/// render uses, so a soft edge V3 sharpens at Masking 50 is not masked out in the preview.
#[test]
fn the_sharpen_mask_follows_the_process_v3_band_and_range() {
    let src = soft_step();
    let edge = |process: lightcraft_develop::Process| {
        let mut s = DevelopSettings { process: process.version(), ..Default::default() };
        s.detail.sharpen_masking = 50.0;
        let img = render_with(&src, &s, Overlay::SharpenMask);
        mean_in(&img, 116, 124)
    };
    let (v2, v3) = (edge(lightcraft_develop::Process::V2), edge(lightcraft_develop::Process::V3));
    assert!(v3 > v2 + 20.0, "V3's own mask range keeps a soft edge in the preview: V2 {v2}, V3 {v3}");
}
