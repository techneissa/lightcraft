//! Where a feature works, as a map ([`crate::Feature::area`]): the grey / white view the Portrait tab shows while a
//! slider is dragged with ⌥ held. Most features use the default, the union of their part masks; the areas here are
//! for the ones whose reach is something else: Blemishes' spots, Wrinkles' areas, Stray Hair's strands, Glasses
//! Glare's lenses and Face Reshape's outline. Each reads the faces' input only, so it is cheap and never moves with a
//! slider.

use lightcraft_raster::Plane;

use crate::feature::union_area;
use crate::wrinkles::FACE_OVAL;
use crate::wrinkles::areas::{self, Area};
use crate::{Part, Scene};

/// A pixel centre (`x` + ½, `y` + ½) of the scene's image.
fn centre(x: usize, y: usize) -> (f64, f64) {
    (x as f64 + 0.5, y as f64 + 0.5)
}

/// Blemishes: a disc on each spot the repair works on, as the repair places it ([`crate::blemish::place`]: the radius
/// held to `MAX_RADIUS`, spots too small to see or outside the skin mask's box left out), of the radius it repairs
/// fully (`CORE` spot radii), edged one pixel soft.
pub fn spots(scene: &Scene<'_>, faces: &[usize]) -> Option<Plane> {
    union_area(scene, faces, |f| {
        let ctx = scene.faces.get(f)?;
        let spots = ctx.face.spots.as_ref()?;
        let b = scene.mask_box(f, Part::FacialSkin)?;
        let mut out = Plane::filled(scene.width, scene.height, 0.0);
        for p in spots.list.iter().filter_map(|s| crate::blemish::place(scene, ctx.size, b, s)) {
            let r = p.r * crate::blemish::CORE;
            let (x0, x1) = (((p.c.0 - r - 1.0).floor().max(0.0)) as usize, ((p.c.0 + r + 1.0).ceil().max(0.0) as usize).min(scene.width));
            let (y0, y1) = (((p.c.1 - r - 1.0).floor().max(0.0)) as usize, ((p.c.1 + r + 1.0).ceil().max(0.0) as usize).min(scene.height));
            for y in y0..y1 {
                for x in x0..x1 {
                    let (px, py) = centre(x, y);
                    let v = (r + 0.5 - ((px - p.c.0).powi(2) + (py - p.c.1).powi(2)).sqrt()).clamp(0.0, 1.0) as f32;
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

/// Face Reshape: the face inside its outline (the mesh's oval; the ellipse of the face's box without a mesh). The
/// polygon is filled once per face, by scanlines over its box.
pub fn outline(scene: &Scene<'_>, faces: &[usize]) -> Option<Plane> {
    union_area(scene, faces, |f| {
        let ctx = scene.faces.get(f)?;
        let first = crate::reshape::MESH;
        let mesh: Vec<(f64, f64)> = if ctx.landmarks.len() >= first + crate::reshape::MESH_POINTS {
            FACE_OVAL.iter().filter_map(|i| ctx.landmarks.get(first + i).copied().flatten()).collect()
        } else {
            Vec::new()
        };
        let poly = if mesh.len() == FACE_OVAL.len() {
            mesh
        } else {
            let [bx0, by0, bx1, by1] = ctx.bounds;
            let (c, rx, ry) = (((bx0 + bx1) / 2.0, (by0 + by1) / 2.0), (bx1 - bx0) / 2.0, (by1 - by0) / 2.0);
            (0..ELLIPSE_POINTS)
                .map(|i| {
                    let t = i as f64 * std::f64::consts::TAU / ELLIPSE_POINTS as f64;
                    (c.0 + rx * t.cos(), c.1 + ry * t.sin())
                })
                .collect()
        };
        Some(fill_polygon(&poly, scene.width, scene.height))
    })
}

/// Points of the polygon that stands for the face box's ellipse.
const ELLIPSE_POINTS: usize = 64;
/// Sample rows per pixel row of [`fill_polygon`]'s coverage.
const SUB_ROWS: usize = 4;

/// The coverage (0…1) of the polygon `poly` over a `w × h` image: scanlines at [`SUB_ROWS`] per pixel row over the
/// polygon's box only, each span giving its exact horizontal coverage. A polygon that is not a polygon (under three
/// points, or a point that is not a number) covers nothing.
fn fill_polygon(poly: &[(f64, f64)], w: usize, h: usize) -> Plane {
    let mut out = Plane::filled(w, h, 0.0);
    if poly.len() < 3 || poly.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
        return out;
    }
    let (y_lo, y_hi) = poly.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
    let (row0, row1) = ((y_lo.floor().max(0.0)) as usize, (y_hi.ceil().max(0.0) as usize).min(h));
    let mut xs: Vec<f64> = Vec::with_capacity(poly.len());
    for y in row0..row1 {
        let Some(row) = out.data.get_mut(y * w..(y + 1) * w) else { continue };
        for sub in 0..SUB_ROWS {
            let yy = y as f64 + (sub as f64 + 0.5) / SUB_ROWS as f64;
            xs.clear();
            for (i, a) in poly.iter().enumerate() {
                let b = poly.get((i + 1) % poly.len()).unwrap_or(a);
                if (a.1 > yy) != (b.1 > yy) {
                    xs.push(a.0 + (yy - a.1) / (b.1 - a.1) * (b.0 - a.0));
                }
            }
            xs.sort_by(f64::total_cmp);
            let (spans, _) = xs.as_chunks::<2>();
            for &[xa, xb] in spans {
                let (x0, x1) = (xa.clamp(0.0, w as f64), xb.clamp(0.0, w as f64));
                for x in (x0.floor() as usize)..(x1.ceil() as usize).min(w) {
                    let cover = (x1.min(x as f64 + 1.0) - x0.max(x as f64)).max(0.0) / SUB_ROWS as f64;
                    if let Some(v) = row.get_mut(x) {
                        *v += cover as f32;
                    }
                }
            }
        }
    }
    out.data.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
    out
}
