//! How far two folders of cached parses agree (issue #200): for every parse file in both, each part's mask over
//! everyone at the parse's size, cut at 0.5, as the share of pixels that agree and the intersection over union, and
//! the largest change of a soft value; and how far the face landmarks moved, in parse pixels.
//!
//! A cache key holds YuNet's face boxes to the last bit, which can differ between two runs on the same photo, so a
//! parse found under another key in the new folder is paired with the one of the same size whose first face is
//! nearest (both without people: the same size alone).
//!
//! ```text
//! cargo run --release -p lightcraft-parsing --example parse_agree -- REFERENCE_DIR NEW_DIR
//! ```

use std::path::Path;

use lightcraft_parsing::render::{self, Select};
use lightcraft_parsing::{Parsing, Part, cache};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [reference, new] = args.as_slice() else { return Err("usage: parse_agree REFERENCE_DIR NEW_DIR".into()) };
    let (reference, new) = (Path::new(reference), Path::new(new));
    let list = |dir: &Path| -> std::io::Result<Vec<String>> {
        let mut keys: Vec<String> = std::fs::read_dir(dir)?
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".lcparse")).map(str::to_string))
            .collect();
        keys.sort();
        Ok(keys)
    };
    let keys = list(reference)?;
    let mut unpaired: Vec<(String, Parsing)> = Vec::new();
    for k in list(new)? {
        if !keys.contains(&k)
            && let Some(p) = cache::load(new, &k)?
        {
            unpaired.push((k, p));
        }
    }
    let (mut worst_iou, mut worst_same, mut worst_soft, mut worst_mark, mut worst_big) = (1.0f64, 1.0f64, 0.0f32, 0.0f32, 1.0f64);
    for key in keys {
        let Some(a) = cache::load(reference, &key)? else { continue };
        let b = match cache::load(new, &key)? {
            Some(b) => b,
            None => match nearest(&a, &mut unpaired) {
                Some((k, b)) => {
                    println!("{key}: paired with {k}");
                    b
                }
                None => {
                    println!("{key}: not in both");
                    continue;
                }
            },
        };
        println!("{key}: {} × {}, people {} / {}", a.width, a.height, a.people.len(), b.people.len());
        for sel in Part::ALL.into_iter().map(Select::Everyone).chain([Select::Background]) {
            let Some((same, iou, soft, area)) = agree(&a, &b, sel) else { continue };
            println!(
                "    {:<20} area {:>7.3} %  same {:>7.3} %  IoU {:.4}  largest change {:.3}",
                format!("{sel:?}"),
                area * 100.0,
                same * 100.0,
                iou,
                soft
            );
            if area >= 0.005 {
                worst_big = worst_big.min(iou);
            }
            worst_iou = worst_iou.min(iou);
            worst_same = worst_same.min(same);
            worst_soft = worst_soft.max(soft);
        }
        for (k, (p, q)) in a.people.iter().zip(&b.people).enumerate() {
            let moved = p
                .landmarks
                .iter()
                .zip(&q.landmarks)
                .map(|(l, m)| ((l.x - m.x) * a.width as f32).hypot((l.y - m.y) * a.height as f32))
                .fold(0.0f32, f32::max);
            if !p.landmarks.is_empty() || !q.landmarks.is_empty() {
                println!("    person {k}: {} / {} landmarks, moved at most {moved:.3} px", p.landmarks.len(), q.landmarks.len());
                worst_mark = worst_mark.max(moved);
            }
        }
    }
    println!(
        "worst: same {:.3} %, IoU {worst_iou:.4} (parts over 0.5 % of the photo: {worst_big:.4}), largest soft change {worst_soft:.3}, landmarks {worst_mark:.3} px",
        worst_same * 100.0
    );
    Ok(())
}

/// The parse of `left` of `a`'s size whose first face is nearest `a`'s, taken out of `left`.
fn nearest(a: &Parsing, left: &mut Vec<(String, Parsing)>) -> Option<(String, Parsing)> {
    let face = |p: &Parsing| p.people.first().and_then(|q| q.face).map(|f| f.centre());
    let far = |p: &Parsing| match (face(a), face(p)) {
        (Some((x, y)), Some((u, v))) => (x - u).hypot(y - v),
        (None, None) => 0.0,
        _ => f32::INFINITY,
    };
    let at = left
        .iter()
        .enumerate()
        .filter(|(_, (_, p))| (p.width, p.height) == (a.width, a.height) && far(p) < 0.02)
        .min_by(|x, y| far(&x.1.1).total_cmp(&far(&y.1.1)))
        .map(|(i, _)| i)?;
    Some(left.swap_remove(at))
}

/// (share of pixels on the same side of 0.5, IoU of the parts above 0.5, largest change of a value, the reference's
/// share of the photo above 0.5); `None` when neither parse has the part.
fn agree(a: &Parsing, b: &Parsing, sel: Select) -> Option<(f64, f64, f32, f64)> {
    let (w, h) = (a.width.max(1), a.height.max(1));
    let (ma, mb) = (render::mask(a, sel, w, h), render::mask(b, sel, w, h));
    let (mut same, mut both, mut either, mut soft, mut on) = (0usize, 0usize, 0usize, 0.0f32, 0usize);
    for (x, y) in ma.iter().zip(&mb) {
        let (p, q) = (*x > 0.5, *y > 0.5);
        on += usize::from(p);
        same += usize::from(p == q);
        both += usize::from(p && q);
        either += usize::from(p || q);
        soft = soft.max((x - y).abs());
    }
    if either == 0 {
        return None;
    }
    let n = ma.len().max(1) as f64;
    Some((same as f64 / n, both as f64 / either as f64, soft, on as f64 / n))
}
