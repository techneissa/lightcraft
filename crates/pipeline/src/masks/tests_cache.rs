//! The mask alpha cache, brush parts stamped inside their bounding boxes and extended stroke by
//! stroke, and masks held for their bounds.

use super::*;
use crate::{Quality, RenderRequest, SourceInfo, StageCache, render, render_cached};
use lightcraft_develop::{DevelopSettings, MaskComponent};

fn frame(w: usize, h: usize) -> Frame {
    Frame::new(w, h, &DevelopSettings::default(), true)
}

/// A photo with edges and texture, and its log luminance.
fn scene(w: usize, h: usize) -> (Rgb32f, Plane) {
    let img = Rgb32f::from_fn(w, h, |x, y| {
        let edge = if (x / 24 + y / 20) % 2 == 0 { 0.6 } else { 0.15 };
        let n = ((x * 7 + y * 13) % 11) as f32 * 0.004;
        [edge + n, edge * 0.8 + n, edge * 0.6 + n]
    });
    let l = img.map(crate::local::log_lum);
    (img, l)
}

/// The strokes of a mask the way they were made before bounding boxes and caching: every stroke on a plane of the whole
/// frame, combined over all of it.
fn reference_brush(strokes: &[BrushStroke], frame: &Frame, w: usize, h: usize, img: &Rgb32f, log_l: &Plane) -> Plane {
    let mut out = Plane::new(w, h);
    for s in strokes {
        let bd = brush_dabs(s, frame, w, h);
        let (dens, erase, auto) = (bd.density, bd.erase, bd.auto);
        let mut stroke_alpha = Plane::new(w, h);
        for (k, d) in bd.dabs.iter().enumerate() {
            let (r, hard, flow) = bd.dab(k);
            let refc = auto.then(|| {
                let j = dab_pixel(*d, w, h);
                (log_l.data[j], chromaticity(img.data[j]))
            });
            let (x0, x1) = (((d.x - r).floor().max(0.0)) as usize, ((d.x + r).ceil().max(0.0) as usize).min(w));
            let (y0, y1) = (((d.y - r).floor().max(0.0)) as usize, ((d.y + r).ceil().max(0.0) as usize).min(h));
            for y in y0..y1 {
                for x in x0..x1 {
                    let dd = Point::new(x as f64 + 0.5, y as f64 + 0.5).dist(*d);
                    if dd > r {
                        continue;
                    }
                    let mut a = dab_alpha(dd, r, hard);
                    let i = y * w + x;
                    if let Some((rl, rch)) = refc {
                        a *= auto_similarity(log_l.data[i], chromaticity(img.data[i]), rl, rch);
                    }
                    let v = &mut stroke_alpha.data[i];
                    *v = accumulate(*v, a, flow, dens);
                }
            }
        }
        if auto {
            let (sigma, eps) = auto_refine(bd.r);
            let q = guided_cross(log_l, &stroke_alpha, sigma, eps);
            stroke_alpha = stroke_alpha.zip_map(&q, f32::max);
        }
        for (o, a) in out.data.iter_mut().zip(&stroke_alpha.data) {
            *o = if erase { *o * (1.0 - a) } else { o.max(*a) };
        }
    }
    out
}

/// Strokes of every kind: soft and hard, erasing, with pen pressure, running off the frame and
/// reaching the corners.
fn strokes(n: usize, auto: bool) -> Vec<BrushStroke> {
    (0..n)
        .map(|i| {
            let t = i as f64 / n.max(1) as f64;
            let (x, y) = (0.05 + 0.9 * ((t * 7.3) % 1.0), 0.05 + 0.9 * ((t * 3.1) % 1.0));
            BrushStroke {
                points: vec![Point::new(x - 0.3, y), Point::new(x, y + 0.1), Point::new(x + 0.35, y - 0.05)],
                size: 0.02 + 0.04 * (i % 3) as f64,
                feather: if i % 2 == 0 { 0.0 } else { 80.0 },
                flow: 40.0 + 15.0 * (i % 4) as f64,
                density: if i % 5 == 0 { 70.0 } else { 100.0 },
                erase: i % 6 == 5,
                auto_mask: auto,
                pressure: if i % 4 == 1 { vec![0.3, 1.0, 0.5] } else { Vec::new() },
            }
        })
        .collect()
}

fn brush_mask(id: u32, strokes: Vec<BrushStroke>) -> Mask {
    Mask {
        id,
        components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Brush { strokes } }],
        ..Default::default()
    }
}

#[test]
fn bounded_strokes_equal_strokes_on_the_whole_frame() {
    let (w, h) = (160, 100);
    let (img, l) = scene(w, h);
    let f = frame(w, h);
    let s = strokes(12, false);
    let got = shape_alpha(&MaskShape::Brush { strokes: s.clone() }, &f, w, h, &img, &l, 0.0, None);
    assert_eq!(got.data, reference_brush(&s, &f, w, h, &img, &l).data);
    // a stroke wholly off the frame, and an empty one, change nothing
    let odd = vec![
        BrushStroke { points: vec![Point::new(-3.0, -3.0), Point::new(-2.0, -3.0)], ..Default::default() },
        BrushStroke::default(),
        BrushStroke { points: vec![Point::new(f64::NAN, 0.5)], ..Default::default() },
    ];
    let got = shape_alpha(&MaskShape::Brush { strokes: odd }, &f, w, h, &img, &l, 0.0, None);
    assert!(got.data.iter().all(|v| *v == 0.0));
}

/// An Auto Mask stroke refines its alpha with a guided filter on the stroke's box and a margin; away from the paint
/// the filter changes nothing, so it matches the filter on the whole frame (float rounding aside).
#[test]
fn bounded_auto_mask_strokes_match_strokes_on_the_whole_frame() {
    let (w, h) = (160, 100);
    let (img, l) = scene(w, h);
    let f = frame(w, h);
    let s = strokes(5, true);
    let got = shape_alpha(&MaskShape::Brush { strokes: s.clone() }, &f, w, h, &img, &l, 0.0, None);
    let want = reference_brush(&s, &f, w, h, &img, &l);
    let worst = got.data.iter().zip(&want.data).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "worst difference {worst}");
}

#[test]
fn a_new_stroke_rasterizes_only_that_stroke() {
    let (w, h) = (160, 100);
    let (img, l) = scene(w, h);
    let f = frame(w, h);
    let keys = AlphaKeys { geo: 1, image: 2 };
    let mut cache = AlphaCache::default();
    let all = strokes(31, false);
    let eval = |cache: &mut AlphaCache, n: usize| {
        let before = strokes_rasterized();
        let m = [brush_mask(1, all[..n].to_vec())];
        let e = evaluate_cached(&m, &f, w, h, &img, &l, 0.0, None, keys, cache);
        (strokes_rasterized() - before, e.into_iter().next().map(|e| e.alpha))
    };
    let (n, first) = eval(&mut cache, 30);
    assert_eq!(n, 30, "a new part is rasterized whole");
    let (n, extended) = eval(&mut cache, 31);
    assert_eq!(n, 1, "one more stroke stamps one stroke");
    let (n, undone) = eval(&mut cache, 30);
    assert_eq!(n, 0, "back to the last state: no stroke");
    assert_eq!(undone, first);
    let fresh = evaluate(&[brush_mask(1, all.clone())], &f, w, h, &img, &l, 0.0, None).remove(0).alpha;
    assert_eq!(extended, Some(fresh), "stamping over the held plane equals rasterizing everything");
    // the first of the strokes changed: nothing it has in common with the held part
    let mut edited = all.clone();
    edited[0].size *= 1.5;
    let before = strokes_rasterized();
    let _ = evaluate_cached(&[brush_mask(1, edited)], &f, w, h, &img, &l, 0.0, None, keys, &mut cache);
    assert_eq!(strokes_rasterized() - before, 31);
}

#[test]
fn the_brush_planes_held_are_bounded() {
    let (w, h) = (64, 40);
    let (img, l) = scene(w, h);
    let f = frame(w, h);
    let keys = AlphaKeys { geo: 1, image: 2 };
    let mut cache = AlphaCache::default();
    for k in 0..20 {
        let m = [brush_mask(1, strokes(3 + k, false))];
        let _ = evaluate_cached(&m, &f, w, h, &img, &l, 0.0, None, keys, &mut cache);
    }
    assert!(cache.bytes() <= 5 * w * h * 4, "{} bytes held", cache.bytes());
}

/// An effect slider and the Amount change what the mask does, not where it is: its alpha is not evaluated again.
#[test]
fn effects_and_amount_do_not_change_a_masks_alpha() {
    let (w, h) = (96, 64);
    let (img, l) = scene(w, h);
    let f = frame(w, h);
    let keys = AlphaKeys { geo: 1, image: 2 };
    let mut cache = AlphaCache::default();
    let mut m = brush_mask(1, strokes(4, false));
    let first = evaluate_cached(std::slice::from_ref(&m), &f, w, h, &img, &l, 0.0, None, keys, &mut cache).remove(0);
    let before = mask_evaluations();
    for k in 1..6 {
        m.adjust.exposure = 0.1 * k as f64;
        m.adjust.contrast = 5.0 * k as f64;
        m.adjust.amount = 100.0 - 10.0 * k as f64;
        m.name = format!("renamed {k}");
        let e = evaluate_cached(std::slice::from_ref(&m), &f, w, h, &img, &l, 0.0, None, keys, &mut cache).remove(0);
        assert!(Arc::ptr_eq(&e.alpha, &first.alpha));
        assert!((e.amount - m.adjust.amount as f32 / 100.0).abs() < 1e-6);
    }
    assert_eq!(mask_evaluations(), before, "a slider drag evaluated a mask");
    // what the mask is made of does count
    m.refine = 30.0;
    let _ = evaluate_cached(std::slice::from_ref(&m), &f, w, h, &img, &l, 0.0, None, keys, &mut cache);
    assert_eq!(mask_evaluations(), before + 1);
    // and `Evaluated::plane` is the alpha at the mask's amount, as `evaluate_one` makes it
    let e = evaluate(std::slice::from_ref(&m), &f, w, h, &img, &l, 0.0, None).remove(0);
    assert_eq!(e.plane().data, evaluate_one(&m, &f, w, h, &img, &l, 0.0, None).data);
}

#[test]
fn an_alpha_is_held_for_its_bounds() {
    let mut p = Plane::new(20, 10);
    assert!(MaskAlpha::from_plane(p.clone()).is_zero());
    assert_eq!(MaskAlpha::from_plane(p.clone()).bounds()[2], 0);
    p.data[3 * 20 + 5] = 0.5;
    p.data[6 * 20 + 12] = 1.0;
    p.data[4 * 20 + 9] = f32::NAN;
    let a = MaskAlpha::from_plane(p.clone());
    assert_eq!(a.bounds(), [5, 3, 13, 7]);
    assert_eq!(a.bytes(), 8 * 4 * 4);
    for y in 0..10 {
        for x in 0..20 {
            let (got, want) = (a.at(x, y), p.data[y * 20 + x]);
            assert!(got == want || (got.is_nan() && want.is_nan()), "({x},{y})");
        }
    }
    assert_eq!(a.at(99, 99), 0.0);
    assert_eq!(a.row(0), None);
    assert_eq!(a.row(3).map(|(x0, r)| (x0, r.len())), Some((5, 8)));
    let back = a.to_plane();
    assert_eq!((back.width, back.height), (20, 10));
    assert!(back.data.iter().zip(&p.data).all(|(a, b)| a == b || (a.is_nan() && b.is_nan())));
    // a full plane is kept as it is
    let full = Plane::from_fn(8, 4, |x, y| 0.1 + (x + y) as f32 * 0.01);
    assert_eq!(MaskAlpha::from_plane(full.clone()).to_plane(), full);
}

fn spot_settings(masks: usize) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    for i in 0..masks {
        let shape = match i % 3 {
            0 => MaskShape::Radial { center: Point::new(0.5, 0.4), rx: 0.15 + 0.01 * i as f64, ry: 0.1, angle: 20.0, feather: 60.0, invert: false },
            1 => MaskShape::Brush { strokes: strokes(3, false) },
            _ => MaskShape::Linear { start: Point::new(0.5, 0.2), end: Point::new(0.5, 0.7) },
        };
        let mut m =
            Mask { id: i as u32 + 1, components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }], ..Default::default() };
        m.adjust.exposure = 0.5;
        m.adjust.saturation = 20.0;
        m.adjust.contrast = 10.0 * i as f64;
        if i == masks - 1 {
            m.adjust.color_hue = 30.0;
            m.adjust.color_sat = 50.0;
        }
        s.masks.push(m);
    }
    s
}

/// Dragging a mask's own sliders or its Amount, or the global Exposure, with masks that don't read the image or the
/// exposure evaluates no mask, and the picture is the one a render without caches makes.
#[test]
fn an_effect_drag_over_masks_evaluates_none_and_equals_a_full_render() {
    let (w, h) = (320, 200);
    let src = Arc::new(scene(w, h).0);
    let info = SourceInfo { raw: true, ..Default::default() };
    let cache = StageCache::default();
    let mut s = spot_settings(5);
    for req in [RenderRequest::fit(300, 200), RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(150, 100) }] {
        render_cached(&src, &info, &s, &req, &cache);
        // (the held alphas and brush planes count with the cache's memory)
        assert!(cache.bytes() > 0);
        let mut evaluated = 0;
        for k in 1..8 {
            match k % 3 {
                0 => s.masks[0].adjust.exposure = 0.1 * k as f64,
                1 => s.masks[1].adjust.amount = 100.0 - 7.0 * k as f64,
                _ => s.light.exposure = 0.05 * k as f64,
            }
            let before = mask_evaluations();
            let a = render_cached(&src, &info, &s, &req, &cache).image;
            evaluated += mask_evaluations() - before;
            let b = render(&src, &info, &s, &req).image;
            assert_eq!(a, b, "tick {k}");
        }
        assert_eq!(evaluated, 0, "a slider drag evaluated {evaluated} masks");
    }
}

/// The last mask with a colour tint wins where masks overlap, and a mask with no Amount tints nothing, so it can't take
/// the tint from the mask before it.
#[test]
fn a_mask_with_no_amount_does_not_take_the_tint() {
    let (w, h) = (120, 80);
    let src = Arc::new(scene(w, h).0);
    let info = SourceInfo { raw: true, ..Default::default() };
    let req = RenderRequest::fit(120, 80);
    let tinted = |hue: f64, amount: f64| {
        let mut m = Mask {
            id: 1,
            components: vec![MaskComponent {
                name: None,
                op: MaskOp::Add,
                invert: false,
                shape: MaskShape::Radial { center: Point::new(0.5, 0.5), rx: 0.4, ry: 0.4, angle: 0.0, feather: 50.0, invert: false },
            }],
            ..Default::default()
        };
        m.adjust.color_hue = hue;
        m.adjust.color_sat = 60.0;
        m.adjust.amount = amount;
        m
    };
    let one = DevelopSettings { masks: vec![tinted(10.0, 100.0)], ..Default::default() };
    let mut two = one.clone();
    let mut second = tinted(200.0, 0.0);
    second.id = 2;
    two.masks.push(second);
    assert_eq!(render(&src, &info, &one, &req).image, render(&src, &info, &two, &req).image);
    // with an Amount it does win
    two.masks[1].adjust.amount = 100.0;
    assert_ne!(render(&src, &info, &one, &req).image, render(&src, &info, &two, &req).image);
}

/// The planes a loupe tints (`mask_planes`) are the alphas of the masks asked for with their Amount, ask no second evaluation after
/// the photo's render, and come only when the asker does not hold them (the signature says what they are made from).
#[test]
fn mask_planes_are_the_asked_masks_alphas_and_reuse_the_render() {
    let src = Arc::new(Rgb32f::from_fn(64, 48, |x, y| [0.2 + x as f32 * 0.005, 0.3, 0.2 + y as f32 * 0.004]));
    let info = SourceInfo { raw: true, ..Default::default() };
    let radial = |id, amount| Mask {
        id,
        components: vec![MaskComponent {
            name: None,
            op: MaskOp::Add,
            invert: false,
            shape: MaskShape::Radial { center: lightcraft_geom::Point::new(0.5, 0.35), rx: 0.2, ry: 0.2, angle: 0.0, feather: 50.0, invert: false },
        }],
        adjust: LocalAdjustments { exposure: 0.5, amount, ..Default::default() },
        ..Mask::default()
    };
    let hidden = Mask { visible: false, ..radial(5, 100.0) };
    let mut s = DevelopSettings { masks: vec![radial(1, 100.0), hidden, radial(2, 50.0)], ..DevelopSettings::default() };
    let req = RenderRequest::fit(64, 64);
    let cache = StageCache::default();
    render_cached(&src, &info, &s, &req, &cache);
    let all = PlanesAsk { want: MaskWant::All, known: None };
    let before = super::mask_evaluations();
    let out = crate::mask_planes(&src, &info, &s, &req, Some(&cache), &all);
    assert_eq!(super::mask_evaluations() - before, 0, "the render's alphas are reused");
    let planes = out.planes.expect("planes");
    assert_eq!(planes.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![1, 2], "visible masks only");
    let (full, half) = (&planes[0].1, &planes[1].1);
    assert_eq!((full.width, full.height), (64, 48));
    let (a, b) = (full.data.iter().copied().fold(0.0f32, f32::max), half.data.iter().copied().fold(0.0f32, f32::max));
    assert!((a - 1.0).abs() < 1e-3 && (b - 0.5).abs() < 1e-3, "the Amount scales the plane: {a} {b}");
    // one mask: only its plane
    let one = crate::mask_planes(&src, &info, &s, &req, Some(&cache), &PlanesAsk { want: MaskWant::One(2), known: None });
    let one_planes = one.planes.expect("planes");
    assert_eq!(one_planes.len(), 1);
    assert_eq!(one_planes[0].1.data, half.data);
    assert_ne!(one.signature, out.signature, "another mask, other planes");
    // the asker holds them: none come, whatever else changed (an effect, the global Exposure) that they are not made from
    let held = PlanesAsk { want: MaskWant::One(2), known: Some(one.signature) };
    s.masks[2].adjust.exposure = -1.0;
    s.masks[0].adjust.contrast = 30.0;
    s.light.exposure = 0.7;
    let again = crate::mask_planes(&src, &info, &s, &req, Some(&cache), &held);
    assert_eq!(again.signature, one.signature);
    assert!(again.planes.is_none(), "the same planes are not made again");
    // what they are made from does count: the shape, the Amount, the size
    s.masks[2].adjust.amount = 80.0;
    assert_ne!(crate::mask_planes(&src, &info, &s, &req, Some(&cache), &held).signature, one.signature, "the Amount");
    s.masks[2].adjust.amount = 50.0;
    if let MaskShape::Radial { rx, .. } = &mut s.masks[2].components[0].shape {
        *rx = 0.3;
    }
    let moved = crate::mask_planes(&src, &info, &s, &req, Some(&cache), &held);
    assert_ne!(moved.signature, one.signature, "the shape");
    assert!(moved.planes.is_some());
    let small = crate::mask_planes(&src, &info, &s, &RenderRequest::fit(32, 32), None, &held);
    assert_ne!(small.signature, moved.signature, "the size");
    // without a cache it is the same
    let fresh = crate::mask_planes(&src, &info, &s, &req, None, &PlanesAsk { want: MaskWant::One(2), known: None });
    assert_eq!(fresh.signature, moved.signature);
}

/// The keys hash the numbers themselves: the same definition hashes alike, and a change to any one number of any part (one
/// coordinate of a stroke somewhere in the middle, a pressure sample, a gradient's end, a stored selection) is a new key.
#[test]
fn a_change_to_any_one_number_of_a_mask_is_a_new_key() {
    use lightcraft_geom::Point;
    let keys = AlphaKeys { geo: 7, image: 9 };
    let strokes: Vec<BrushStroke> = (0..300)
        .map(|i| BrushStroke {
            points: (0..40).map(|k| Point::new(0.1 + 0.002 * k as f64 + 0.0001 * i as f64, 0.5)).collect(),
            pressure: (0..40).map(|k| 0.5 + 0.01 * k as f32).collect(),
            erase: i % 6 == 5,
            ..BrushStroke::default()
        })
        .collect();
    let mask_of =
        |shape: MaskShape| Mask { id: 1, components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }], ..Mask::default() };
    let base = mask_of(MaskShape::Brush { strokes: strokes.clone() });
    let key = alpha_key(&base, keys, 0.0);
    assert_eq!(alpha_key(&mask_of(MaskShape::Brush { strokes: strokes.clone() }), keys, 0.0), key, "the same definition, the same key");
    let chain = brush_chain(&strokes);
    assert_eq!(brush_chain(&strokes), chain);
    // one coordinate (one ulp) of one point of stroke 123
    let mut edited = strokes.clone();
    edited[123].points[17].x = f64::from_bits(edited[123].points[17].x.to_bits() + 1);
    assert_ne!(alpha_key(&mask_of(MaskShape::Brush { strokes: edited.clone() }), keys, 0.0), key, "one coordinate of one point");
    let changed = brush_chain(&edited);
    assert_eq!(changed[..123], chain[..123], "the strokes before it keep their place in the chain");
    assert!(changed[123..].iter().zip(&chain[123..]).all(|(a, b)| a != b), "it and every stroke after it are new");
    // a pressure sample, a flag, the order
    let mut edited = strokes.clone();
    edited[299].pressure[39] += 0.01;
    assert_ne!(brush_chain(&edited)[299], chain[299], "a pressure sample");
    let mut edited = strokes.clone();
    edited[5].erase = !edited[5].erase;
    assert_ne!(brush_chain(&edited)[5], chain[5], "the erase flag");
    let mut edited = strokes.clone();
    edited.swap(10, 11);
    assert_ne!(brush_chain(&edited)[11], chain[11], "the order");
    // what the key says besides the shape
    let mut m = base.clone();
    m.refine = 10.0;
    assert_ne!(alpha_key(&m, keys, 0.0), key, "Refine Edges");
    assert_ne!(alpha_key(&base, AlphaKeys { geo: 8, ..keys }, 0.0), key, "the frame");
    // the shapes: one number of each
    let seg = lightcraft_develop::SegMask::from_logits(8, &[3.0; 64]);
    let seg2 = lightcraft_develop::SegMask::from_logits(8, &[-3.0; 64]);
    let pairs: Vec<(&str, MaskShape, MaskShape)> = vec![
        (
            "linear",
            MaskShape::Linear { start: Point::new(0.1, 0.2), end: Point::new(0.5, 0.6) },
            MaskShape::Linear { start: Point::new(0.1, 0.2), end: Point::new(0.5, 0.6001) },
        ),
        (
            "radial",
            MaskShape::Radial { center: Point::new(0.5, 0.5), rx: 0.2, ry: 0.2, angle: 0.0, feather: 50.0, invert: false },
            MaskShape::Radial { center: Point::new(0.5, 0.5), rx: 0.2, ry: 0.2, angle: 0.0, feather: 50.0, invert: true },
        ),
        (
            "color range",
            MaskShape::ColorRange { samples: vec![[0.5, 0.0, 0.1]], refine: 50.0 },
            MaskShape::ColorRange { samples: vec![[0.5, 0.0, 0.11]], refine: 50.0 },
        ),
        (
            "luminance range",
            MaskShape::LuminanceRange { lo: 0.1, hi: 0.9, lo_feather: 0.1, hi_feather: 0.1 },
            MaskShape::LuminanceRange { lo: 0.1, hi: 0.9, lo_feather: 0.1, hi_feather: 0.2 },
        ),
        ("depth range", MaskShape::DepthRange { lo: 0.1, hi: 0.9, feather: 0.1 }, MaskShape::DepthRange { lo: 0.1, hi: 0.8, feather: 0.1 }),
        (
            "subject",
            MaskShape::Subject { seg: Some(seg.clone()), detail: Vec::new(), edge: 0.0 },
            MaskShape::Subject { seg: Some(seg2.clone()), detail: Vec::new(), edge: 0.0 },
        ),
        (
            "sky",
            MaskShape::Sky { seg: Some(seg.clone()), detail: Vec::new(), edge: 0.0 },
            MaskShape::Sky { seg: Some(seg.clone()), detail: vec![seg2.clone()], edge: 0.0 },
        ),
        (
            "background",
            MaskShape::Background { seg: None, detail: Vec::new(), edge: 0.0 },
            MaskShape::Background { seg: None, detail: Vec::new(), edge: 5.0 },
        ),
        (
            "object",
            MaskShape::Object { hint: vec![Point::new(0.5, 0.5)], exclude: Vec::new(), seg: None, detail: Vec::new(), edge: 0.0 },
            MaskShape::Object { hint: vec![Point::new(0.5, 0.5)], exclude: vec![Point::new(0.1, 0.1)], seg: None, detail: Vec::new(), edge: 0.0 },
        ),
        (
            "prompt",
            MaskShape::Prompt { text: "the car".into(), seg: None, detail: Vec::new(), edge: 0.0 },
            MaskShape::Prompt { text: "the cat".into(), seg: None, detail: Vec::new(), edge: 0.0 },
        ),
        ("people", MaskShape::People { person: 1, parts: vec!["face".into()] }, MaskShape::People { person: 1, parts: vec!["hair".into()] }),
        ("landscape", MaskShape::Landscape { class: "water".into() }, MaskShape::Landscape { class: "sky".into() }),
    ];
    let mut all = std::collections::HashSet::new();
    for (name, a, b) in pairs {
        let (ka, kb) = (alpha_key(&mask_of(a.clone()), keys, 0.0), alpha_key(&mask_of(b), keys, 0.0));
        assert_ne!(ka, kb, "{name}: one number changed");
        assert_eq!(ka, alpha_key(&mask_of(a), keys, 0.0), "{name}: the same, the same");
        assert!(all.insert(ka), "{name}: another kind of shape with the same numbers has the same key");
    }
}
