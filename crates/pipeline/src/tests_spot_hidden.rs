//! Fork feature (`fork/spot-rows.md`): a hidden spot has no effect on the CPU render.

use lightcraft_develop::{DevelopSettings, Spot, SpotMode};
use lightcraft_geom::Point;

use crate::{RenderRequest, SourceInfo, render};

fn clone_spot(hidden: bool) -> Spot {
    Spot {
        mode: SpotMode::Clone,
        points: vec![Point::new(0.5, 0.5)],
        size: 0.08,
        feather: 10.0,
        source_offset: Some(Point::new(0.2, 0.1)),
        hidden,
        ..Default::default()
    }
}

#[test]
fn a_hidden_spot_renders_as_if_it_were_not_there() {
    let src = lightcraft_scenes::demo_library()[3].render(480, 320);
    let png = |spots: Vec<Spot>| {
        let s = DevelopSettings { spots, ..Default::default() };
        render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(480, 320)).image.data
    };
    let plain = png(vec![]);
    assert_ne!(png(vec![clone_spot(false)]), plain, "a shown spot has an effect");
    assert_eq!(png(vec![clone_spot(true)]), plain, "a hidden spot has none");
}
