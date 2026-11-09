//! Where a feature works, as a map ([`crate::Feature::area`]): the grey / white view the Portrait tab shows while a
//! slider is dragged with ⌥ held. Most features use the default, the union of their part masks; the areas here are
//! for the ones whose reach is something else: Blemishes' spots, Wrinkles' areas, Stray Hair's strands, Glasses
//! Glare's lenses and Face Reshape's outline. Each reads the faces' input only, so it is cheap and never moves with a
//! slider.

use lightcraft_raster::Plane;

use crate::feature::union_area;
use crate::wrinkles::areas::{self, Area};
use crate::{Part, Scene};

/// A pixel centre (`x` + ½, `y` + ½) of the scene's image.
fn centre(x: usize, y: usize) -> (f64, f64) {
    (x as f64 + 0.5, y as f64 + 0.5)
}

/// Blemishes: a soft disc on each spot the face has (at least a few pixels across, so a small spot shows).
pub fn spots(scene: &Scene<'_>, faces: &[usize]) -> Option<Plane> {
    union_area(scene, faces, |f| {
        let ctx = scene.faces.get(f)?;
        let spots = ctx.face.spots.as_ref()?;
        let mut out = Plane::filled(scene.width, scene.height, 0.0);
        for s in &spots.list {
            let Some((cx, cy)) = scene.view().to_px(s.at.0, s.at.1).filter(|(x, y)| x.is_finite() && y.is_finite()) else { continue };
            let r = (s.radius * ctx.size).max(2.0) * 1.3;
            let (x0, x1) = (((cx - r - 1.0).floor().max(0.0)) as usize, ((cx + r + 1.0).ceil().max(0.0) as usize).min(scene.width));
            let (y0, y1) = (((cy - r - 1.0).floor().max(0.0)) as usize, ((cy + r + 1.0).ceil().max(0.0) as usize).min(scene.height));
            for y in y0..y1 {
                for x in x0..x1 {
                    let (px, py) = centre(x, y);
                    let v = ((r - ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()) / (0.3 * r)).clamp(0.0, 1.0) as f32;
                    if let Some(o) = out.data.get_mut(y * scene.width + x) {
                        *o = o.max(v);
                    }
                }
            }
        }
        Some(out)
    })
}

/// Wrinkles: the face's four areas (forehead, crow's feet, cheeks and smile lines, neck), each cut by the skin mask
/// it works in. A face without the mesh has none.
pub fn wrinkles(scene: &Scene<'_>, faces: &[usize]) -> Option<Plane> {
    union_area(scene, faces, |f| {
        let ctx = scene.faces.get(f)?;
        let mut out = Plane::filled(scene.width, scene.height, 0.0);
        for area in Area::ALL {
            let skin = Part::FacialSkin;
            let part = if area == Area::Neck { Part::BodySkin } else { skin };
            if ctx.face.mask(part).is_none() {
                continue;
            }
            for region in areas::regions(ctx, area) {
                let [bx0, by0, bx1, by1] = region.bounds();
                let (x0, x1) = (bx0.floor().max(0.0) as usize, (bx1.ceil().max(0.0) as usize).min(scene.width));
                let (y0, y1) = (by0.floor().max(0.0) as usize, (by1.ceil().max(0.0) as usize).min(scene.height));
                for y in y0..y1 {
                    for x in x0..x1 {
                        let w = region.weight(centre(x, y)) * scene.mask_at(f, part, x, y);
                        if let Some(o) = out.data.get_mut(y * scene.width + x) {
                            *o = o.max(w);
                        }
                    }
                }
            }
        }
        Some(out)
    })
}

/// Stray Hair: the cells of the face's strand search that hold a strand, by their weight.
pub fn strands(scene: &Scene<'_>, faces: &[usize]) -> Option<Plane> {
    union_area(scene, faces, |f| {
        let s = scene.faces.get(f)?.face.strands.as_ref()?;
        if !s.area.is_valid() || s.width == 0 || s.height == 0 {
            return None;
        }
        let [x0, y0, x1, y1] = scene.area_box(&s.area)?;
        let mut out = Plane::filled(scene.width, scene.height, 0.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let (nx, ny) = scene.view().to_norm(x as f64 + 0.5, y as f64 + 0.5);
                let (u, v) = ((nx - s.area.x0) / s.area.width(), (ny - s.area.y0) / s.area.height());
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let Some(cell) = s.cell((u * s.width as f64) as usize, (v * s.height as f64) as usize) else { continue };
                if let Some(o) = out.data.get_mut(y * scene.width + x) {
                    *o = f32::from(cell[0]) / 255.0;
                }
            }
        }
        Some(out)
    })
}

/// Glasses Glare: the lenses, as the repair made for the face (its feathered lens mask) says; before a repair, the
/// glasses the people parse found (Others).
pub fn lenses(scene: &Scene<'_>, faces: &[usize]) -> Option<Plane> {
    union_area(scene, faces, |f| {
        let ctx = scene.faces.get(f)?;
        let Some(patch) = ctx.face.patch.as_deref() else { return scene.mask_plane(f, Part::Others) };
        let mut out = Plane::filled(scene.width, scene.height, 0.0);
        let [x0, y0, x1, y1] = scene.area_box(&patch.area)?;
        for y in y0..y1 {
            for x in x0..x1 {
                let (nx, ny) = scene.view().to_norm(x as f64 + 0.5, y as f64 + 0.5);
                if let (Some((a, _)), Some(o)) = (patch.sample(nx, ny), out.data.get_mut(y * scene.width + x)) {
                    *o = a;
                }
            }
        }
        Some(out)
    })
}

/// The face mesh's outline, in [`crate::Face::landmarks`] order after YuNet's five (MediaPipe's face oval).
const OVAL: [usize; 36] = [
    10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377, 152, 148, 176, 149, 150, 136, 172, 58, 132, 93, 234,
    127, 162, 21, 54, 103, 67, 109,
];

/// Face Reshape: the face inside its outline (the mesh's oval; the ellipse of the face's box without a mesh).
pub fn outline(scene: &Scene<'_>, faces: &[usize]) -> Option<Plane> {
    union_area(scene, faces, |f| {
        let ctx = scene.faces.get(f)?;
        let first = crate::reshape::MESH;
        let poly: Vec<(f64, f64)> = if ctx.landmarks.len() >= first + crate::reshape::MESH_POINTS {
            OVAL.iter().filter_map(|i| ctx.landmarks.get(first + i).copied().flatten()).collect()
        } else {
            Vec::new()
        };
        let [bx0, by0, bx1, by1] = ctx.bounds;
        let (c, rx, ry) = (((bx0 + bx1) / 2.0, (by0 + by1) / 2.0), (bx1 - bx0) / 2.0, (by1 - by0) / 2.0);
        let edge = (ctx.size * 0.02).max(1.0);
        Some(Plane::from_fn(scene.width, scene.height, |x, y| {
            let p = centre(x, y);
            let d = if poly.len() >= 3 {
                areas::polygon_inside(&poly, p)
            } else {
                let k = (((p.0 - c.0) / rx.max(1e-9)).powi(2) + ((p.1 - c.1) / ry.max(1e-9)).powi(2)).sqrt();
                (1.0 - k) * rx.min(ry)
            };
            ((d / edge) * 0.5 + 0.5).clamp(0.0, 1.0) as f32
        }))
    })
}
