//! The sun past an edge (issue #332): a drag to an edge snaps to the depth step and goes just behind it, the glow is
//! brightest there and falls off away from it, and the edge's own shadow cuts the beam.

use crate::{Effects, Field, Light, LightKind};

const W: usize = 320;
const H: usize = 200;

/// A near wall (log depth −0.6) on the left half and the open far side (0.4) on the right, with a small near patch in the
/// bottom left corner either way (so both scenes have the same nearest depth: the air begins at a share of it).
fn scene(wall: bool) -> Field {
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            if (wall && u < 0.5) || (u < 0.16 && v > 0.84) { -0.6 } else { 0.4 }
        })
        .collect();
    Field::from_parts(W, H, lz.clone(), lz.iter().map(|l| if *l > 0.0 { -1.0 } else { -3.0 }).collect()).unwrap()
}

fn light_at(x: f64, y: f64, depth: f64) -> Light {
    Light { x, y, depth, amount: 1.0, softness: 0.2, warmth: 0.0, wrap: 0.4, kind: LightKind::Point }
}

fn glowing(glow: f64, burst: f64) -> Effects {
    Effects { edge_glow: glow, sunburst: burst, ..Effects::default() }
}

#[test]
fn a_light_dragged_near_an_edge_goes_onto_it_and_just_behind_it() {
    let f = scene(true);
    let (near, far) = (f.light_depth_of(-0.6).0, f.light_depth_of(0.4).0);
    // from the wall's side, the far side's, and a good way off (the nearest edge is the step at the middle)
    for from in [0.44, 0.57, 0.38] {
        let s = f.snap_to_edge(from, 0.5).unwrap_or_else(|| panic!("no edge found from {from}"));
        assert!((s.x - 0.5).abs() < 0.02, "from {from}: x {}", s.x);
        assert!((s.y - 0.5).abs() < 0.02, "from {from}: y {}", s.y);
        // (behind it: at the far side's depth, not the wall's)
        assert!(s.depth > near + 0.5 * (far - near) && s.depth >= far, "from {from}: depth {} (wall {near}, far side {far})", s.depth);
    }
    // a faint step on the wall (an arm's edge on a shirt) is no outline: the light goes past it to the wall's edge
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let u = (i % W) as f32 / W as f32;
            if u < 0.45 {
                -1.0
            } else if u < 0.5 {
                -0.88
            } else {
                0.4
            }
        })
        .collect();
    let faint = Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap();
    let s = faint.snap_to_edge(0.46, 0.5).unwrap();
    assert!((s.x - 0.5).abs() < 0.02, "snapped to the faint step: x {}", s.x);
    // no edge near: the light stays where it was put; a flat scene has none at all
    assert_eq!(f.snap_to_edge(0.95, 0.5), None);
    assert_eq!(scene(false).snap_to_edge(0.5, 0.5), None);
    assert_eq!(f.snap_to_edge(f64::NAN, 0.5), None);
}

#[test]
fn the_glow_is_brightest_at_the_edge_and_falls_off_away_from_it() {
    let f = scene(true);
    let s = f.snap_to_edge(0.45, 0.5).unwrap();
    let l = light_at(s.x, s.y, s.depth);
    let m = f.edge_glow(&l, &glowing(1.0, 0.0)).unwrap().m;
    let at = |u: f32, v: f32| m[(v * H as f32) as usize * W + (u * W as f32) as usize];
    let edge = at(s.x as f32 + 0.004, s.y as f32);
    assert!(edge > 0.5, "edge {edge}");
    // along the open side it falls off with every step away, and is small far off
    let row: Vec<f32> = [0.52, 0.56, 0.62, 0.72, 0.9].iter().map(|u| at(*u, 0.5)).collect();
    assert!(row.windows(2).all(|p| p[1] < p[0]), "open side {row:?}");
    assert!(row[4] < 0.1 * edge, "reaches too far {row:?}");
    // and up the edge, away from the light
    assert!(at(0.52, 0.9) < 0.5 * at(0.52, 0.5) && at(0.52, 0.1) < 0.5 * at(0.52, 0.5));
    // the wall that hides the light takes only what wraps over the edge, a little way in
    let (over, deep) = (at(0.49, 0.5), at(0.3, 0.5));
    assert!(over > 0.02 * edge && over < at(0.51, 0.5), "over the edge {over}, open side {}", at(0.51, 0.5));
    assert!(deep < 0.1 * over, "the wall's face {deep}, over the edge {over}");
    // nothing without Edge Glow and Sunburst, or for a light with no place
    assert!(f.edge_glow(&l, &glowing(0.0, 0.0)).is_none());
    // a sunburst adds a few faint rays on the open side, much fainter on the wall's side (it is backlit)
    let with = f.edge_glow(&l, &glowing(1.0, 1.0)).unwrap().m;
    let rays_only: Vec<f32> = with.iter().zip(&m).map(|(a, b)| a - b).collect();
    let (open_side, wall_side): (f32, f32) = (
        rays_only.iter().enumerate().filter(|(i, _)| i % W > W / 2).map(|(_, v)| *v).sum(),
        rays_only.iter().enumerate().filter(|(i, _)| i % W < W / 2 - 8).map(|(_, v)| *v).sum(),
    );
    assert!(open_side > 0.0 && wall_side < 0.3 * open_side, "rays: open {open_side}, wall {wall_side}");
}

/// The beam of `l` with haze 0.6 on `f`, read at normalized `(u, v)` (the reduced field is the field here).
fn beam(f: &Field, l: &Light) -> impl Fn(f32, f32) -> f32 + use<> {
    let rig = f.rig(l, 0.0, 0.0);
    let shafts = f.shafts(&rig, l, false).unwrap();
    let b = f.beam(&shafts, &rig, &Effects { haze: 0.6, scatter: 0.0, ..Effects::default() });
    let (w, h) = b.size();
    move |u: f32, v: f32| b.at((u * w as f32) as usize, (v * h as f32) as usize)
}

#[test]
fn the_edges_own_shadow_cuts_the_beam() {
    // the light sits just behind the wall's edge: the air in front of the wall is in the wall's shadow, the air on the open
    // side is lit, and with no wall (the same light in a flat scene) both sides are alike
    let walled = scene(true);
    let s = walled.snap_to_edge(0.45, 0.5).unwrap();
    let l = light_at(s.x, s.y, s.depth);
    let (cut, open) = (beam(&walled, &l), beam(&scene(false), &l));
    let (shadow_side, lit_side) = (cut(0.4, 0.5), cut(0.6, 0.5));
    assert!(lit_side > 0.0 && shadow_side < 0.3 * lit_side, "wall's side {shadow_side}, open side {lit_side}");
    let (flat_left, flat_right) = (open(0.4, 0.5), open(0.6, 0.5));
    assert!(flat_left > 0.7 * flat_right, "no wall: {flat_left} vs {flat_right}");
    assert!(shadow_side < 0.3 * flat_left, "the wall cuts the beam: {shadow_side} vs {flat_left} with no wall");
}

#[test]
fn a_sun_shines_from_the_point_it_was_put_at() {
    let f = scene(false);
    // behind the scene (the azimuth is about ±180), and higher for a point higher on the photo
    let (az_mid, el_mid) = f.sun_towards(0.5, 0.5);
    let (az_up, el_up) = f.sun_towards(0.5, 0.1);
    assert!(az_mid.abs() > 170.0 && el_mid.abs() < 1.0, "{az_mid} {el_mid}");
    assert!(el_up > 15.0 && az_up.abs() > 170.0, "{az_up} {el_up}");
    // the point's own side: left of the middle is the sun's left
    assert!(f.sun_towards(0.2, 0.3).0 < -180.0 + 60.0 && f.sun_towards(0.2, 0.3).0 > -180.0);
}

#[test]
fn the_halo_falls_off_the_same_every_way_whatever_depth_lies_behind_it() {
    // a near wall on the left and the open side on the right whose depth runs from the wall's own at the top to far at the
    // bottom (a ground or a stair, the old depth mask cut a column out of the halo where the depth crossed its thresholds):
    // points the same distance from the light on the open side, to the right, up and down, glow alike
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            if u < 0.5 { -1.0 } else { -1.0 + 2.5 * v }
        })
        .collect();
    let f = Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap();
    let s = f.snap_to_edge(0.47, 0.5).unwrap();
    let m = f.edge_glow(&light_at(s.x, s.y, s.depth), &glowing(1.0, 0.0)).unwrap().m;
    // (the same distance, 56 pixels of the field's long side, at three angles, all on the open side)
    let at = |deg: f32| {
        let (dx, dy) = (56.0 * deg.to_radians().cos(), 56.0 * deg.to_radians().sin());
        let (x, y) = ((s.x as f32 * W as f32 + dx) as usize, (s.y as f32 * H as f32 + dy) as usize);
        m[y * W + x]
    };
    let (right, up, down) = (at(0.0), at(-60.0), at(60.0));
    let (hi, lo) = (right.max(up).max(down), right.min(up).min(down));
    assert!(lo > 0.0 && hi < 1.3 * lo, "right {right}, up {up}, down {down}");
}

#[test]
fn open_sky_far_from_the_sun_gets_much_less_added_light_than_sky_near_it() {
    // a sun low behind a far wall that is all sky (the nearest thing is a small patch in a corner), in haze: the air in front of
    // the sky scatters the sun's light mostly onward, so the sky glows by the sun and stays itself away from it. The beam was
    // as bright over the whole far wall as haze at that depth gets, a veil
    let sun = Light { kind: LightKind::Sun(crate::Sun { azimuth: 180.0, elevation: 12.0 }), softness: 0.0, ..light_at(0.5, 0.5, 0.5) };
    let f = scene(false);
    let rig = f.rig(&sun, 0.0, 0.0);
    let shafts = f.shafts(&rig, &sun, false).unwrap();
    let b = f.beam(&shafts, &rig, &Effects { haze: 0.6, ..Effects::default() });
    let (bw, bh) = b.size();
    let at = |u: f32, v: f32| b.at((u * bw as f32) as usize, (v * bh as f32) as usize);
    // (the sun is at about (0.5, 0.22) on the photo)
    let (near, far) = (at(0.5, 0.27), at(0.12, 0.8));
    assert!(near > 0.0 && far < 0.25 * near, "sky by the sun {near}, far from it {far}");
}

/// The glow of a light snapped onto the wall's edge of `scene(true)` (or of `scene(true)` with a small near block on the open side
/// when `block`), glow only, as the plane's rows.
fn snapped_glow(block: bool) -> Vec<f32> {
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            let blocked = block && (0.53..0.57).contains(&u) && (0.3..0.7).contains(&v);
            if u < 0.5 || blocked { -1.0 } else { 0.4 }
        })
        .collect();
    let f = Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap();
    let s = f.snap_to_edge(0.47, 0.5).unwrap();
    f.edge_glow(&light_at(s.x, s.y, s.depth), &glowing(1.0, 0.0)).unwrap().m
}

#[test]
fn the_rim_is_brightest_next_to_the_light_and_fades_within_a_short_way_along_the_outline() {
    let m = snapped_glow(false);
    // the rim: the pixel on the wall's side of the outline less the wall a few pixels in (the halo alone)
    let rim = |y: usize| m[y * W + 159] - m[y * W + 150];
    let (peak, along) = (rim(H / 2), rim(H / 2 - 22));
    assert!(peak > 0.5, "rim by the light {peak}");
    assert!(along < 0.15 * peak, "rim 22 pixels along the outline {along} of {peak} by the light");
}

#[test]
fn an_inner_edge_facing_away_from_the_light_gets_no_rim() {
    // a small near block on the open side: its edge towards the light has a rim, its far edge (facing away from the light) has none
    let m = snapped_glow(true);
    let at = |x: usize| m[(H / 2) * W + x];
    let (towards, away, middle) = (at(170), at(182), at(176));
    assert!(towards > 1.5 * middle, "the edge facing the light {towards}, the middle {middle}");
    assert!(away <= middle, "the edge facing away from the light has a rim: {away}, the middle {middle}");
}

#[test]
fn the_core_is_not_drawn_on_what_is_nearer_than_the_edge_it_hides_behind() {
    // the light sits behind the wall's edge: a few pixels in on the wall (nearer than the edge) there is no core, the same
    // distance out on the open side there is
    let m = snapped_glow(false);
    let (wall, open) = (m[(H / 2) * W + 155], m[(H / 2) * W + 165]);
    assert!(open > 0.3 && wall < 0.2 * open, "on the wall {wall}, on the open side {open}");
}

#[test]
fn the_glow_goes_on_past_a_roofs_slopes_and_only_the_roof_itself_hides_it() {
    // a roof: a triangle of near depth under open far sky, the light just behind its apex. The sky beside the apex and below its
    // height (outside the triangle) is open air: it gets the glow as much as sky the same distance above the apex. A line or a
    // half-plane through the apex cut the glow off flat there
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            if v > 0.45 && (u - 0.5).abs() < (v - 0.45) * 0.9 { -0.6 } else { 0.4 }
        })
        .collect();
    let f = Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap();
    let s = f.snap_to_edge(0.5, 0.4).unwrap();
    let m = f.edge_glow(&light_at(s.x, s.y, s.depth), &glowing(1.0, 0.8)).unwrap().m;
    let (ax, ay) = (s.x as f32 * W as f32, s.y as f32 * H as f32);
    let at = |dx: f32, dy: f32| m[((ay + dy) as usize) * W + (ax + dx) as usize];
    // (41 pixels from the apex: straight up; and out to the side and a little below it, beyond the roof's slope)
    let (above, beside_l, beside_r) = (at(0.0, -41.0), at(-40.0, 8.0), at(40.0, 8.0));
    assert!(above > 0.0 && beside_l > 0.5 * above && beside_r > 0.5 * above, "above {above}, beside left {beside_l}, right {beside_r}");
    // and the roof's own pixels, just under the apex, take no core
    let inside = at(0.0, 12.0);
    assert!(inside < 0.5 * above.max(at(0.0, -12.0)), "on the roof {inside}");
}

#[test]
fn the_glow_fades_smoothly_to_nothing_and_stays_small_next_to_the_frame() {
    // (issue #346) a light in open air at the app's defaults (Softness 50, Edge Glow 70, Sunburst 60), on a field the size of the
    // app's: along a line out from it the glow only falls, and is all but gone well before it stops, so no circle shows where it
    // stops; and a quarter of the frame away it is a small share of what it is by the light (the old cut-off was a hard disc)
    let (w, h) = (1600, 1066);
    let f = Field::from_parts(w, h, vec![0.4; w * h], vec![-2.0; w * h]).unwrap();
    let l = Light { softness: 0.5, ..light_at(0.3, 0.5, 0.5) };
    let m = f.edge_glow(&l, &glowing(0.7, 0.6)).unwrap().m;
    let (cx, cy) = (480usize, 533usize);
    let row: Vec<f32> = (cx + 8..w).map(|x| m[cy * w + x]).collect();
    assert!(row.windows(2).all(|p| p[1] <= p[0]), "the glow rises again along the line");
    let at = |share: f32| m[cy * w + cx + (share * w as f32) as usize];
    let by = at(0.02);
    assert!(at(0.25) < 0.01 * by, "a quarter of the frame away {} of {by} by the light", at(0.25));
    // (the last bit before it stops is no step: under a thousandth of the glow by the light, and 0 past it)
    let last = row.iter().rposition(|v| *v > 0.0).unwrap_or(0);
    let step = row.get(last.saturating_sub(4)).copied().unwrap_or(0.0);
    assert!(step < 1e-3 * by, "the glow ends with a step of {step} against {by} by the light");
}
