use super::*;

/// A synthetic face mesh in a `w × h` picture: the face's 468 points round an oval `face` pixels wide centred on
/// (`cx`, `cy`), both eyes `ear` open; the subject's left eye (the far one when turned) `far` as wide as the right.
fn mesh(w: f32, h: f32, (cx, cy): (f32, f32), face: f32, ear: f32, far: f32) -> Vec<[f32; 3]> {
    let mut px = vec![[0f32; 3]; 478];
    for (k, p) in px.iter_mut().enumerate().take(MESH_POINTS) {
        let a = k as f32 / MESH_POINTS as f32 * std::f32::consts::TAU;
        *p = [cx + 0.5 * face * a.cos(), cy + 0.65 * face * a.sin(), 0.0];
    }
    let ew = 0.2 * face;
    for (side, ((corners, lids), ring)) in EYES.iter().zip(&OPENINGS).enumerate() {
        let wide = if side == 0 { ew } else { ew * far };
        // the subject's right eye is on the left of the picture; each eye's outer corner is away from the nose
        let (ex, ey) = (cx + if side == 0 { -0.2 } else { 0.2 } * face, cy - 0.1 * face);
        let out = if side == 0 { -1.0 } else { 1.0 };
        for (i, &k) in ring.iter().enumerate() {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            px[k] = [ex + 0.5 * wide * a.cos(), ey + 0.5 * ear * ew * a.sin(), 0.0];
        }
        px[corners[0]] = [ex + out * 0.5 * wide, ey, 0.0];
        px[corners[1]] = [ex - out * 0.5 * wide, ey, 0.0];
        for (j, [up, down]) in lids.iter().enumerate() {
            let x = ex + out * (0.25 - 0.25 * j as f32) * wide;
            px[*up] = [x, ey - 0.5 * ear * ew, 0.0];
            px[*down] = [x, ey + 0.5 * ear * ew, 0.0];
        }
    }
    px.iter().map(|p| [p[0] / w, p[1] / h, p[2] / w]).collect()
}

fn face_rect(w: f32, h: f32, (cx, cy): (f32, f32), face: f32) -> [f32; 4] {
    [(cx - 0.5 * face) / w, (cy - 0.65 * face) / h, (cx + 0.5 * face) / w, (cy + 0.65 * face) / h]
}

fn flat(w: usize, h: usize) -> Vec<f32> {
    vec![0.5; w * h]
}

fn read_one(lm: &[[f32; 3]], w: usize, h: usize, face: f32) -> FaceEyes {
    let px = flat(w, h);
    let img = Luma { data: &px, width: w, height: h };
    let rect = face_rect(w as f32, h as f32, (w as f32 / 2.0, h as f32 / 2.0), face);
    read(&img, &[FaceIn { rect, landmarks: lm }])[0]
}

#[test]
fn open_and_closed_eyes_read_the_same_at_any_face_size() {
    for (w, h, face) in [(2048, 1365, 600.0), (2048, 1365, 120.0), (800, 600, 60.0)] {
        let c = (w as f32 / 2.0, h as f32 / 2.0);
        let open = mesh(w as f32, h as f32, c, face, 0.3, 1.0);
        let shut = mesh(w as f32, h as f32, c, face, 0.05, 1.0);
        let half = mesh(w as f32, h as f32, c, face, 0.2, 1.0);
        let (o, s, m) = (read_one(&open, w, h, face), read_one(&shut, w, h, face), read_one(&half, w, h, face));
        assert_eq!((o.closed, s.closed, m.closed), (Some(false), Some(true), Some(false)), "face {face}px");
        let ear = o.eyes[0].ear.unwrap();
        assert!((ear - 0.3).abs() < 0.01, "{ear}");
        assert!(o.main);
    }
}

#[test]
fn tiny_and_profile_faces_are_unknown_not_open() {
    let (w, h) = (2048.0, 1365.0);
    let c = (1024.0, 680.0);
    // a face 20 px wide: eyes 4 px
    let tiny = read_one(&mesh(w, h, c, 20.0, 0.3, 1.0), 2048, 1365, 20.0);
    assert_eq!((tiny.closed, tiny.eyes[0].ear, tiny.eyes[1].ear), (None, None, None));
    // in profile: the far eye a fifth as wide
    let profile = read_one(&mesh(w, h, c, 400.0, 0.05, 0.2), 2048, 1365, 400.0);
    assert_eq!(profile.closed, None, "{profile:?}");
    // turned: the far eye is not read, the near one is
    let turned = read_one(&mesh(w, h, c, 400.0, 0.05, 0.5), 2048, 1365, 400.0);
    assert_eq!((turned.eyes[1].ear, turned.closed), (None, Some(true)), "{turned:?}");
    assert!(turned.eyes[0].ear.is_some());
}

#[test]
fn hostile_landmarks_never_panic_and_read_as_unknown() {
    let (w, h) = (640usize, 480usize);
    let mut nan = mesh(w as f32, h as f32, (320.0, 240.0), 200.0, 0.3, 1.0);
    for p in nan.iter_mut().step_by(3) {
        *p = [f32::NAN, f32::INFINITY, f32::NAN];
    }
    let few: Vec<[f32; 3]> = vec![[0.5, 0.5, 0.0]; 100];
    let same: Vec<[f32; 3]> = vec![[0.5, 0.5, 0.0]; 478];
    for lm in [&nan[..], &few[..], &[][..], &same[..]] {
        let r = read_one(lm, w, h, 200.0);
        assert_eq!((r.closed, r.focus), (None, None), "{r:?}");
    }
    // a face box of NaNs, a picture whose size does not match its pixels, no faces
    let px = flat(w, h);
    let img = Luma { data: &px, width: w, height: h };
    let r = read(&img, &[FaceIn { rect: [f32::NAN; 4], landmarks: &nan }]);
    assert!(!r[0].main);
    let bad = Luma { data: &px, width: w + 1, height: h };
    assert_eq!(read(&bad, &[FaceIn { rect: [0.0, 0.0, 1.0, 1.0], landmarks: &nan }]), vec![FaceEyes::default()]);
    assert!(read(&img, &[]).is_empty());
    assert_eq!(summary(&[]), Summary::default());
    let empty = Luma { data: &[], width: 0, height: 0 };
    assert_eq!(read(&empty, &[FaceIn { rect: [0.0, 0.0, 1.0, 1.0], landmarks: &[] }]).len(), 1);
}

#[test]
fn only_main_faces_decide_the_photo() {
    let (w, h) = (2048usize, 1365usize);
    let (wf, hf) = (w as f32, h as f32);
    let big = mesh(wf, hf, (700.0, 600.0), 400.0, 0.3, 1.0);
    let small = mesh(wf, hf, (1600.0, 600.0), 150.0, 0.05, 1.0);
    let px = flat(w, h);
    let img = Luma { data: &px, width: w, height: h };
    let faces = [
        FaceIn { rect: face_rect(wf, hf, (700.0, 600.0), 400.0), landmarks: &big },
        FaceIn { rect: face_rect(wf, hf, (1600.0, 600.0), 150.0), landmarks: &small },
    ];
    let r = read(&img, &faces);
    assert_eq!((r[0].main, r[1].main, r[1].closed), (true, false, Some(true)));
    let s = summary(&r);
    assert_eq!((s.faces, s.main, s.closed), (2, 1, Some(false)), "a bystander blinking is not the photo's");
    // two main faces, one blinking: the photo's eyes are closed
    let other = mesh(wf, hf, (1500.0, 600.0), 380.0, 0.05, 1.0);
    let faces = [
        FaceIn { rect: face_rect(wf, hf, (700.0, 600.0), 400.0), landmarks: &big },
        FaceIn { rect: face_rect(wf, hf, (1500.0, 600.0), 380.0), landmarks: &other },
    ];
    let s = summary(&read(&img, &faces));
    assert_eq!((s.main, s.closed), (2, Some(true)));
}

/// A deterministic texture value 0..1.
fn noise(x: usize, y: usize) -> f32 {
    let mut v = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
    v ^= v >> 15;
    v = v.wrapping_mul(0x2C1B_3C6D);
    v ^= v >> 12;
    (v & 0xffff) as f32 / 65535.0
}

/// Box blur of the pixels inside `(x0, y0, x1, y1)`.
fn blur(px: &mut [f32], w: usize, h: usize, (x0, y0, x1, y1): (usize, usize, usize, usize), r: usize) {
    let src = px.to_vec();
    for y in y0..y1.min(h) {
        for x in x0..x1.min(w) {
            let mut acc = 0.0;
            let mut n = 0.0;
            for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                    acc += src[yy * w + xx];
                    n += 1.0;
                }
            }
            px[y * w + x] = acc / n;
        }
    }
}

/// A 600 × 600 picture with one face 300 px wide: skin with fine texture, eyes with stronger detail, the background
/// textured as strongly as `background`.
fn portrait(background: f32) -> (Vec<f32>, Vec<[f32; 3]>, [f32; 4], [(usize, usize, usize, usize); 2]) {
    let (w, h) = (600usize, 600usize);
    let (cx, cy, face) = (300.0, 300.0, 300.0);
    let lm = mesh(w as f32, h as f32, (cx, cy), face, 0.3, 1.0);
    let ew = 0.2 * face;
    let eyes = [-0.2f32, 0.2].map(|dx| {
        let (ex, ey) = (cx + dx * face, cy - 0.1 * face);
        ((ex - 0.75 * ew) as usize, (ey - 0.45 * ew) as usize, (ex + 0.75 * ew) as usize, (ey + 0.45 * ew) as usize)
    });
    let mut px = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = ((x as f32 - cx) / (0.5 * face), (y as f32 - cy) / (0.65 * face));
            let in_face = dx * dx + dy * dy <= 1.0;
            let in_eye = eyes.iter().any(|&(x0, y0, x1, y1)| (x0..x1).contains(&x) && (y0..y1).contains(&y));
            let amp = if in_eye {
                0.45
            } else if in_face {
                0.12
            } else {
                background
            };
            px[y * w + x] = 0.5 + amp * (noise(x, y) - 0.5);
        }
    }
    (px, lm, face_rect(w as f32, h as f32, (cx, cy), face), eyes)
}

fn focus_of(px: &[f32], lm: &[[f32; 3]], rect: [f32; 4]) -> Option<f32> {
    let img = Luma { data: px, width: 600, height: 600 };
    read(&img, &[FaceIn { rect, landmarks: lm }])[0].focus
}

#[test]
fn eyes_in_focus_score_high() {
    let (px, lm, rect, _) = portrait(0.0);
    let f = focus_of(&px, &lm, rect).unwrap();
    assert!(f > 90.0, "{f}");
    // a soft background takes nothing away
    let (px, lm, rect, _) = portrait(0.05);
    assert!(focus_of(&px, &lm, rect).unwrap() > 90.0);
}

#[test]
fn blurred_eyes_on_a_sharp_face_score_low() {
    let (mut px, lm, rect, eyes) = portrait(0.0);
    let sharp = focus_of(&px, &lm, rect).unwrap();
    for e in eyes {
        blur(&mut px, 600, 600, e, 4);
    }
    let soft = focus_of(&px, &lm, rect).unwrap();
    assert!(soft < Eyes::FOCUS_MIN && soft < sharp - 50.0, "sharp {sharp}, blurred eyes {soft}");
}

#[test]
fn a_shot_focused_on_the_background_scores_low() {
    // the background has more detail than the face, and the whole face is soft
    let (mut px, lm, rect, _) = portrait(0.9);
    blur(&mut px, 600, 600, (150, 105, 450, 495), 4);
    let f = focus_of(&px, &lm, rect).unwrap();
    assert!(f < Eyes::FOCUS_MIN, "{f}");
}

#[test]
fn a_near_flat_frame_has_unknown_focus() {
    // pixel noise a thousandth of the range: no detail to compare the eyes against
    let (w, h) = (600usize, 600usize);
    let lm = mesh(w as f32, h as f32, (300.0, 300.0), 300.0, 0.3, 1.0);
    let rect = face_rect(w as f32, h as f32, (300.0, 300.0), 300.0);
    let px: Vec<f32> = (0..w * h).map(|i| 0.5 + 0.001 * (noise(i % w, i / w) - 0.5)).collect();
    let r = read(&Luma { data: &px, width: w, height: h }, &[FaceIn { rect, landmarks: &lm }])[0];
    assert_eq!((r.closed, r.focus), (Some(false), None), "{r:?}");
}

fn face(closed: Option<bool>, focus: Option<f32>) -> FaceEyes {
    FaceEyes { closed, focus, main: true, ..FaceEyes::default() }
}

#[test]
fn an_unreadable_main_face_leaves_the_photo_unknown_unless_another_is_closed() {
    let open = face(Some(false), Some(80.0));
    let unknown = face(None, None);
    let closed = face(Some(true), None);
    let s = summary(&[open, unknown]);
    assert_eq!((s.closed, s.focus), (None, None), "one face could not be read: the photo is unknown, not open");
    let s = summary(&[open, unknown, closed]);
    assert_eq!(s.closed, Some(true), "a closed main face decides");
    let s = summary(&[open, open]);
    assert_eq!((s.closed, s.focus), (Some(false), Some(80.0)));
    // open eyes whose focus could not be measured
    let s = summary(&[open, face(Some(false), None)]);
    assert_eq!((s.closed, s.focus), (Some(false), None));
    // a soft face decides even when another could not be measured
    let s = summary(&[face(Some(false), Some(10.0)), face(Some(false), None)]);
    assert_eq!(s.focus, Some(10.0));
    // a bystander (not main) is ignored
    let bystander = FaceEyes { main: false, ..face(None, None) };
    assert_eq!(summary(&[open, bystander]).closed, Some(false));
}

#[test]
fn one_soft_main_face_makes_the_eyes_out_of_focus() {
    // one sharp face and three soft ones
    let faces = [face(Some(false), Some(95.0)), face(Some(false), Some(12.0)), face(Some(false), Some(20.0)), face(Some(false), Some(18.0))];
    let s = summary(&faces);
    assert!(s.focus.is_some_and(|f| f < FOCUS_MIN), "{s:?}");
    // all sharp is not flagged
    let s = summary(&[face(Some(false), Some(95.0)), face(Some(false), Some(70.0))]);
    assert_eq!(s.focus, Some(70.0));
    // a closed face has no focus and does not hide the others'
    let s = summary(&[face(Some(true), None), face(Some(false), Some(60.0))]);
    assert_eq!(s.focus, Some(60.0));
}

#[test]
fn an_eye_that_cannot_be_read_does_not_make_the_other_unknown() {
    let (w, h) = (2048usize, 1365usize);
    // eyes read 0.09: closed on the two-eye line (0.11), open on the one-eye line (0.08)
    for (bad, good) in [(1usize, 0usize), (0, 1)] {
        let mut lm = mesh(w as f32, h as f32, (1024.0, 680.0), 500.0, 0.09, 1.0);
        let (_, lids) = &EYES[bad];
        lm[lids[1][0]] = [f32::NAN, f32::NAN, f32::NAN];
        let r = read_one(&lm, w, h, 500.0);
        assert_eq!((r.eyes[bad].ear, r.closed), (None, Some(false)), "bad eye {bad}: {r:?}");
        assert!(r.eyes[good].ear.is_some(), "{r:?}");
    }
    // a corner missing altogether
    let mut lm = mesh(w as f32, h as f32, (1024.0, 680.0), 500.0, 0.05, 1.0);
    lm[EYES[1].0[0]] = [f32::NAN; 3];
    let r = read_one(&lm, w, h, 500.0);
    assert_eq!((r.eyes[1].ear, r.closed), (None, Some(true)), "{r:?}");
}

/// The threshold the catalog flags with.
struct Eyes;
impl Eyes {
    const FOCUS_MIN: f32 = 35.0;
}
