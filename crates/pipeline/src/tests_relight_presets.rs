//! Light presets through the render (fork, issue #301): a preset's key lands on the subject, not on a far background behind it,
//! because its lights are placed at the subject's own depth.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::relight_presets::{Subject, built_in};
use lightcraft_raster::{Rgb32f, Rgba8};
use lightcraft_relight::Field;

use crate::{RenderRequest, SourceInfo, relight, render};

const W: usize = 480;
const H: usize = 320;
/// The subject: a small box in the middle of the photo, standing in front of a wall that is much farther.
const SUBJECT: [f64; 4] = [0.42, 0.25, 0.58, 0.6];

fn in_subject(u: f32, v: f32) -> bool {
    (0.42..0.58).contains(&u) && (0.25..0.6).contains(&v)
}

fn scene() -> Rgb32f {
    Rgb32f::from_fn(W, H, |x, y| {
        let (u, v) = (x as f32 / W as f32, y as f32 / H as f32);
        let t = 0.1 + 0.02 * ((u * 40.0).sin() * (v * 30.0).cos());
        [t; 3]
    })
}

/// The subject at log depth 0 and the wall behind it at 3 (twenty times as far).
fn field() -> Field {
    let (fw, fh) = (240, 160);
    let lz =
        (0..fw * fh).map(|i| if in_subject((i % fw) as f32 / fw as f32 + 0.002, (i / fw) as f32 / fh as f32 + 0.003) { 0.0 } else { 3.0 }).collect();
    Field::from_parts(fw, fh, lz, vec![-2.0; fw * fh]).unwrap()
}

fn mean(img: &Rgba8, (u0, v0, u1, v1): (f32, f32, f32, f32)) -> f32 {
    let (w, h) = (img.width as f32, img.height as f32);
    let (x0, x1, y0, y1) = ((u0 * w) as usize, (u1 * w) as usize, (v0 * h) as usize, (v1 * h) as usize);
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get(x, y);
            sum += 0.25 * f32::from(p[0]) + 0.6 * f32::from(p[1]) + 0.15 * f32::from(p[2]);
        }
    }
    sum / ((x1 - x0) * (y1 - y0)).max(1) as f32
}

/// How much brighter (a ratio) the preset's lights make the `region` of the scene, for each built-in preset that has a key
/// and for a subject with and without its depth.
fn brightening(subject: Subject, region: (f32, f32, f32, f32), preset: usize) -> f32 {
    let src = scene();
    let info = SourceInfo { relight: Some(relight::Input { field: Arc::new(field()), key: 3 }), ..SourceInfo::default() };
    let req = RenderRequest::fit(W, H);
    let plain = render(&src, &info, &DevelopSettings::default(), &req).image;
    let lit = render(&src, &info, &built_in()[preset].onto(&DevelopSettings::default(), &subject), &req).image;
    mean(&lit, region) / mean(&plain, region)
}

#[test]
fn a_presets_key_lights_the_subject_and_not_the_wall_far_behind_it() {
    let f = field();
    let face = Subject::from_face(SUBJECT).unwrap();
    let lz = f.depth_in(SUBJECT).unwrap();
    let (d, span) = f.light_depth_of(lz);
    let own = face.with_depth(100.0 * d, span);
    let (on_subject, beside) = ((0.44, 0.3, 0.56, 0.55), (0.62, 0.3, 0.74, 0.55));
    // the Rembrandt, Butterfly and Split keys
    for preset in [0, 1, 2] {
        let (subject, wall) = (brightening(own, on_subject, preset), brightening(own, beside, preset));
        assert!(subject > wall + 0.15, "{}: the subject brightens {subject}, the wall beside it {wall}", built_in()[preset].name);
        // placed by the scene's depth alone (the subject's depth not known) the key is far behind the subject, on the wall
        let blind = brightening(face, on_subject, preset);
        assert!(subject > blind + 0.1, "{}: with its depth the subject brightens {subject}, without {blind}", built_in()[preset].name);
    }
}
