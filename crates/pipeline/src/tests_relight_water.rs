//! The underwater look in the render (fork, issue #333): off changes no byte, far things lose red first and go blue-green,
//! caustics and shafts show, and a draft equals an export.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_raster::{Rgb32f, Rgba8};
use lightcraft_relight::Field;

use crate::{RenderRequest, SourceInfo, relight, render};

const W: usize = 480;
const H: usize = 320;

fn near(u: f32, v: f32) -> bool {
    (0.15..0.4).contains(&u) && (0.25..0.7).contains(&v)
}

/// A neutral grey photo with a little texture: a nearer square (log depth −0.8) on a far wall (0) that is a floor below the
/// horizon (so the caustics' sun, from above, has a surface that faces up).
fn photo() -> (Rgb32f, SourceInfo) {
    let src = Rgb32f::from_fn(W, H, |x, y| {
        let t = 0.18 + 0.03 * ((x as f32 * 0.3).sin() * (y as f32 * 0.2).cos());
        [t; 3]
    });
    let (fw, fh) = (240, 160);
    let at = |i: usize| ((i % fw) as f32 / fw as f32 + 0.002, (i / fw) as f32 / fh as f32 + 0.003);
    let lz = (0..fw * fh)
        .map(|i| {
            let (u, v) = at(i);
            if near(u, v) {
                -0.8
            } else if v > 0.55 {
                // a floor: nearer towards the bottom
                0.9 - 1.4 * (v - 0.55)
            } else {
                0.9
            }
        })
        .collect();
    // (the far water above the floor is the bright open water the sun's rays go out through)
    let lum = (0..fw * fh).map(|i| if at(i).1 < 0.55 && !near(at(i).0, at(i).1) { -0.5 } else { -3.0 }).collect();
    let field = Field::from_parts(fw, fh, lz, lum).unwrap();
    (src, SourceInfo { relight: Some(relight::Input { field: Arc::new(field), key: 333 }), ..SourceInfo::default() })
}

fn set(s: &mut DevelopSettings, pairs: &[(&str, f64)]) {
    for (id, v) in pairs {
        assert!(lightcraft_develop::relight::set(s, id, *v), "{id}");
    }
}

fn mean_rgb(img: &Rgba8, (u0, v0, u1, v1): (f32, f32, f32, f32)) -> [f32; 3] {
    let (w, h) = (img.width as f32, img.height as f32);
    let (x0, x1, y0, y1) = ((u0 * w) as usize, (u1 * w) as usize, (v0 * h) as usize, (v1 * h) as usize);
    let mut sum = [0f32; 3];
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get(x, y);
            (0..3).for_each(|c| sum[c] += f32::from(p[c]));
        }
    }
    let n = ((x1 - x0) * (y1 - y0)).max(1) as f32;
    sum.map(|v| v / n)
}

#[test]
fn water_off_changes_no_byte_and_leaves_an_old_edit_as_it_was() {
    let (src, info) = photo();
    let req = RenderRequest::fit(W, H);
    let plain = render(&src, &SourceInfo::default(), &DevelopSettings::default(), &req).image;
    // Clarity and Tint alone turn nothing on; nor does the look without a field
    let mut s = DevelopSettings::default();
    set(&mut s, &[("relight.waterClarity", 10.0), ("relight.waterTint", 80.0)]);
    assert!(render(&src, &info, &s, &req).image.data == plain.data);
    set(&mut s, &[("relight.water", 80.0), ("relight.waterShafts", 60.0), ("relight.waterCaustics", 60.0)]);
    assert!(render(&src, &SourceInfo::default(), &s, &req).image.data == plain.data);
    // an edit with a light and no water renders as it did, and its saved form holds no water
    let mut lit = DevelopSettings::default();
    set(&mut lit, &[("relight.amount", 70.0), ("relight.x", 0.3), ("relight.y", 0.5)]);
    let with_water_off = {
        let mut m = lit.clone();
        set(&mut m, &[("relight.water", 50.0), ("relight.waterCaustics", 50.0)]);
        set(&mut m, &[("relight.water", 0.0), ("relight.waterCaustics", 0.0)]);
        m
    };
    assert!(render(&src, &info, &with_water_off, &req).image.data == render(&src, &info, &lit, &req).image.data);
    assert!(serde_json::to_value(&lit.relight).unwrap().get("water").is_none());
    // the section's eye turns the water off with the rest
    s.set_section_enabled("relight", false);
    assert!(render(&src, &info, &s, &req).image.data == plain.data);
}

#[test]
fn far_things_lose_red_first_and_turn_blue_green() {
    let (src, info) = photo();
    let req = RenderRequest::fit(W, H);
    let (near_box, far_wall) = ((0.2, 0.35, 0.35, 0.6), (0.6, 0.05, 0.9, 0.25));
    let mut s = DevelopSettings::default();
    set(&mut s, &[("relight.water", 80.0)]);
    let wet = render(&src, &info, &s, &req).image;
    let dry = render(&src, &SourceInfo::default(), &DevelopSettings::default(), &req).image;
    let ratio = |c: [f32; 3]| (c[0] / c[2], c[1] / c[2]);
    // the dry photo is neutral everywhere
    let d = mean_rgb(&dry, far_wall);
    assert!((d[0] - d[2]).abs() < 2.0, "{d:?}");
    let (n, f) = (mean_rgb(&wet, near_box), mean_rgb(&wet, far_wall));
    // the nearest thing keeps its colours, the far wall has lost red against blue, and green against blue less than red
    assert!((ratio(n).0 - 1.0).abs() < 0.08, "the nearest thing is not touched: {n:?}");
    assert!(ratio(f).0 < 0.5 * ratio(n).0, "red did not fall off faster than blue: near {n:?}, far {f:?}");
    assert!(ratio(f).0 < ratio(f).1 && ratio(f).1 < 1.0, "red goes first, then green: {f:?}");
    // far things lose contrast: the texture's spread on the far wall is smaller than without water
    let spread = |img: &Rgba8| {
        let (w, h) = (img.width as f32, img.height as f32);
        let v: Vec<f32> = (((0.05 * h) as usize)..((0.25 * h) as usize))
            .flat_map(|y| (((0.6 * w) as usize)..((0.9 * w) as usize)).map(move |x| (x, y)))
            .map(|(x, y)| f32::from(img.get(x, y)[2]))
            .collect();
        let m = v.iter().sum::<f32>() / v.len() as f32;
        (v.iter().map(|a| (a - m) * (a - m)).sum::<f32>() / v.len() as f32).sqrt() / m
    };
    assert!(spread(&wet) < spread(&dry), "no loss of contrast: {} against {}", spread(&wet), spread(&dry));
    // a clearer water carries red farther
    let mut clear = s.clone();
    set(&mut clear, &[("relight.waterClarity", 100.0)]);
    let c = mean_rgb(&render(&src, &info, &clear, &req).image, far_wall);
    assert!(ratio(c).0 > ratio(f).0, "clear water: {c:?}, murky {f:?}");
}

#[test]
fn caustics_and_shafts_change_the_picture_and_a_small_render_and_a_draft_equal_the_full_one() {
    let (src, info) = photo();
    let req = RenderRequest::fit(W, H);
    let plain = render(&src, &info, &DevelopSettings::default(), &req).image;
    for pairs in [[("relight.waterCaustics", 70.0)], [("relight.waterShafts", 70.0)]] {
        let mut s = DevelopSettings::default();
        set(&mut s, &pairs);
        assert!(render(&src, &info, &s, &req).image.data != plain.data, "{pairs:?} changed nothing");
    }
    // a smaller render, and a draft (a drag's, `Quality::Draft`) at the full size, agree with the export on average
    let mut s = DevelopSettings::default();
    set(&mut s, &[("relight.water", 60.0), ("relight.waterShafts", 50.0), ("relight.waterCaustics", 50.0)]);
    let small = render(&src, &info, &s, &RenderRequest::fit(W / 4, H / 4)).image;
    let big = render(&src, &info, &s, &RenderRequest::fit(W, H)).image;
    let (a, b) = (mean_rgb(&small, (0.0, 0.0, 1.0, 1.0)), mean_rgb(&big, (0.0, 0.0, 1.0, 1.0)));
    assert!((0..3).all(|c| (a[c] - b[c]).abs() < 6.0), "small {a:?}, export {b:?}");
    let draft = render(&src, &info, &s, &RenderRequest { quality: crate::Quality::Draft, ..RenderRequest::fit(W, H) }).image;
    let d = mean_rgb(&draft, (0.0, 0.0, 1.0, 1.0));
    assert!((0..3).all(|c| (d[c] - b[c]).abs() < 6.0), "draft {d:?}, export {b:?}");
}

#[test]
fn the_shafts_still_show_with_eight_lights_and_caustics() {
    // the photo's eight lights and the water's sun of the caustics are nine in the scene; the shafts' beam is a tenth
    // entry and was cut by a limit of nine
    let (src, info) = photo();
    let req = RenderRequest::fit(W, H);
    let mut s = DevelopSettings::default();
    s.relight.lights =
        (0..8).map(|i| lightcraft_develop::relight::LightSettings { amount: 5.0, x: 0.1 + 0.1 * i as f64, ..Default::default() }).collect();
    set(&mut s, &[("relight.waterCaustics", 50.0)]);
    let without = render(&src, &info, &s, &req).image;
    set(&mut s, &[("relight.waterShafts", 80.0)]);
    assert!(render(&src, &info, &s, &req).image.data != without.data, "the shafts did nothing with eight lights");
}

#[test]
fn the_planes_make_the_per_colour_gain_the_finish_stage_applies() {
    // the water's exposure, temperature and tint, put through the finish stage's own function on a grey, give the transmittance's
    // ratios and luminance
    let lum = |c: [f32; 3]| 0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2];
    let w = lightcraft_relight::Water { amount: 1.0, clarity: 0.4, tint: 0.3 };
    for x in [0.1, 0.5, 1.0] {
        let t = w.transmittance(x);
        let (stops, temp, tint, _) = w.planes_at(x, 0.1);
        let g = stops.exp2();
        let out = crate::finish::local_temp_tint([g; 3], temp, tint);
        for (o, tc) in out.iter().zip(t) {
            assert!((o / lum(out) - tc / lum(t)).abs() < 0.05, "{out:?} vs {t:?}");
        }
        assert!((lum(out) - lum(t)).abs() < 1e-3, "{out:?} vs {t:?}");
    }
}
