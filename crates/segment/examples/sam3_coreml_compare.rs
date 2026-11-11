//! Research only (issue #268, branch `research/268-sam3-coreml`): times SAM 3 on candle the way the app
//! runs it and writes its tensors for a comparison against Core ML conversions.
//!
//! `cargo run -p lightcraft-segment --release --example sam3_coreml_compare -- <model dir> <spec.json> <input dir> <out dir>`
//!
//! `spec.json`: `[{"photo": "name", "clicks": [[x, y], ...], "prompts": ["dog", ...]}, ...]`; each photo is
//! `<input dir>/<name>.rgb` (8-bit RGB) with `<name>.json` (`{"w", "h"}`). The first photo is run 3 more
//! times (the repeat check). Writes raw little-endian f32 files and `<out dir>/candle.json`.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use lightcraft_segment::{Click, INPUT, Sam3, mask};

fn write_f32(path: &Path, v: &[f32]) -> Result<(), Box<dyn std::error::Error>> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    for x in v {
        f.write_all(&x.to_le_bytes())?;
    }
    Ok(())
}

fn ms(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 10_000.0).round() / 10.0
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [dir, spec, input, out] = a.as_slice() else { return Err("usage: sam3_coreml_compare <model dir> <spec.json> <input dir> <out dir>".into()) };
    let spec: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(spec)?)?;
    let photos = spec.as_array().ok_or("spec: array")?;
    let t = Instant::now();
    let mut model = Sam3::load(Path::new(dir))?;
    let load_ms = ms(t);
    eprintln!("load {load_ms} ms on {:?}", model.device());
    let mut report = serde_json::json!({"load_ms": load_ms, "device": format!("{:?}", model.device()), "photos": []});
    for (pi, p) in photos.iter().enumerate() {
        let name = p["photo"].as_str().ok_or("photo")?;
        let dims: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(Path::new(input).join(format!("{name}.json")))?)?;
        let (w, h) = (dims["w"].as_u64().ok_or("w")? as usize, dims["h"].as_u64().ok_or("h")? as usize);
        let rgb = std::fs::read(Path::new(input).join(format!("{name}.rgb")))?;
        let od = Path::new(out).join(name).join("candle");
        std::fs::create_dir_all(&od)?;
        write_f32(&od.join("pixels.f32"), &mask::preprocess(&rgb, w, h, INPUT)?)?;
        let rounds = if pi == 0 { 4 } else { 1 };
        let mut rec = serde_json::json!({"photo": name, "encode_ms": [], "prepare_clicks_ms": [], "clicks": [], "prompts": []});
        for round in 0..rounds {
            let t = Instant::now();
            let mut enc = model.encode(&rgb, w, h)?;
            lightcraft_segment::sync(&model)?;
            rec["encode_ms"].as_array_mut().ok_or("a")?.push(ms(t).into());
            let t = Instant::now();
            model.prepare_clicks(&mut enc)?;
            lightcraft_segment::sync(&model)?;
            rec["prepare_clicks_ms"].as_array_mut().ok_or("a")?.push(ms(t).into());
            let backbone: Vec<f32> = enc.backbone().ok_or("candle encoded no backbone")?.flatten_all()?.to_vec1()?;
            write_f32(&od.join(format!("backbone-r{round}.f32")), &backbone)?;
            for (ci, c) in p["clicks"].as_array().ok_or("clicks")?.iter().enumerate() {
                let click = Click { x: c[0].as_f64().ok_or("x")? as f32, y: c[1].as_f64().ok_or("y")? as f32, positive: true };
                let t = Instant::now();
                let pred = model.segment_clicks(&mut enc, &[click])?;
                let el = ms(t);
                write_f32(&od.join(format!("click{ci}-r{round}.f32")), &pred.logits)?;
                if round == 0 {
                    rec["clicks"].as_array_mut().ok_or("a")?.push(serde_json::json!({"ms": [el], "score": pred.score}));
                } else if let Some(r) = rec["clicks"].get_mut(ci) {
                    r["ms"].as_array_mut().ok_or("a")?.push(el.into());
                }
            }
            for (ti, text) in p["prompts"].as_array().ok_or("prompts")?.iter().enumerate() {
                let text = text.as_str().ok_or("prompt")?;
                let t = Instant::now();
                let pred = model.segment_text_with(&mut enc, text, 0.5, false)?;
                let el = ms(t);
                if let Some(pr) = &pred {
                    write_f32(&od.join(format!("text{ti}-r{round}.f32")), &pr.logits)?;
                }
                if round == 0 {
                    // the raw detector output, for the merge on the Core ML side (not timed)
                    let o = model.detect(&mut enc, text)?;
                    let s = |x: f32| 1.0 / (1.0 + (-x).exp());
                    let keep: Vec<usize> = (0..o.logits.len()).filter(|i| o.logits.get(*i).is_some_and(|l| s(*l) * s(o.presence) > 0.3)).collect();
                    let mut kept = Vec::new();
                    for i in &keep {
                        kept.extend(o.masks.get(*i)?.flatten_all()?.to_vec1::<f32>()?);
                    }
                    write_f32(&od.join(format!("text{ti}-kept.f32")), &kept)?;
                    write_f32(&od.join(format!("text{ti}-semantic.f32")), &o.semantic)?;
                    rec["prompts"].as_array_mut().ok_or("a")?.push(serde_json::json!({
                        "text": text, "ms": [el], "found": pred.is_some(), "score": pred.as_ref().map(|p| p.score),
                        "logits": o.logits, "presence": o.presence, "boxes": o.boxes, "kept": keep,
                    }));
                } else if let Some(r) = rec["prompts"].get_mut(ti) {
                    r["ms"].as_array_mut().ok_or("a")?.push(el.into());
                }
            }
        }
        eprintln!("{name}: {}", rec);
        report["photos"].as_array_mut().ok_or("a")?.push(rec);
    }
    std::fs::write(Path::new(out).join("candle.json"), serde_json::to_string_pretty(&report)?)?;
    Ok(())
}
