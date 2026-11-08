//! Fork feature (`fork/spot-rows.md`): a hidden spot (its row's eye off) has no effect on the CPU or on the GPU.

use lightcraft_develop::{DevelopSettings, Spot, SpotMode};
use lightcraft_geom::Point;
use lightcraft_pipeline::{RenderRequest, SourceInfo, render};

fn spot(at: (f64, f64), offset: (f64, f64), hidden: bool) -> Spot {
    Spot {
        mode: SpotMode::Clone,
        points: vec![Point::new(at.0, at.1)],
        size: 0.06,
        feather: 20.0,
        source_offset: Some(Point::new(offset.0, offset.1)),
        hidden,
        ..Default::default()
    }
}

#[test]
fn a_hidden_spot_renders_as_if_it_were_not_there_on_both() {
    if !lightcraft_gpu::available() {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    let src = std::sync::Arc::new(lightcraft_scenes::demo_library()[3].render(900, 600));
    let info = SourceInfo { raw: true, ..Default::default() };
    let req = RenderRequest::fit(640, 640);
    let with = |hidden: Option<bool>| {
        let mut s = DevelopSettings::default();
        s.spots.push(spot((0.3, 0.4), (0.25, 0.1), false));
        if let Some(h) = hidden {
            s.spots.push(spot((0.7, 0.6), (-0.25, -0.1), h));
        }
        s
    };
    let cpu = |s: &DevelopSettings| render(&src, &info, s, &req).image;
    let gpu = |s: &DevelopSettings| lightcraft_gpu::render(&src, &info, s, &req, None).expect("gpu render").image;
    let (alone, hidden, shown) = (with(None), with(Some(true)), with(Some(false)));
    assert_eq!(cpu(&hidden).data, cpu(&alone).data, "CPU: a hidden spot changes nothing");
    assert_eq!(gpu(&hidden).data, gpu(&alone).data, "GPU: a hidden spot changes nothing");
    assert_ne!(cpu(&shown).data, cpu(&alone).data, "the second spot has an effect while it is shown");
    // and the two renderers agree on the picture with the hidden spot, as they do without it
    let (c, g) = (cpu(&hidden), gpu(&hidden));
    let worst = c.data.iter().zip(&g.data).flat_map(|(p, q)| (0..3).map(move |k| p[k].abs_diff(q[k]))).max().unwrap_or(0);
    assert!(worst <= 3, "CPU and GPU differ by {worst} levels");
}
