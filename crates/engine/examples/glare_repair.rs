//! Try Repair Glare (fork feature, `fork/glare-repair.md`) on a photo without building the app:
//!
//! ```text
//! cargo run --release -p lightcraft-engine --example glare_repair -- PHOTO OUT.jpg [--method flux|lama] [--lenses auto|whole|glints]
//!     [--face N]
//! ```
//!
//! Imports PHOTO into a session of its own (the people parts set and the models from the shared folders, as the app
//! has them), finds its faces, repairs the glasses of one face or of every face with glasses, and writes OUT: the
//! glasses crop of each repaired face before (left) and after (right), at Amount 100. Prints the time a face took.
use std::time::{Duration, Instant};

use lightcraft_engine::Session;
use lightcraft_raster::Rgba8;
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "PHOTO OUT.jpg [--method flux|lama] [--lenses auto|whole|glints] [--face N]";
    let (photo, out) = (args.first().ok_or(usage)?, args.get(1).ok_or(usage)?);
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let library = std::env::temp_dir().join(format!("lc-glare-example-{}", std::process::id()));
    let mut s = Session::new().with_fs().with_system_clock().with_default_face_models().with_default_lama_model().with_default_parsing_models();
    s.open_library(&library, false).map_err(|e| format!("library: {e:?}"))?;
    let r = s.execute("library.import", &json!({"paths": [photo]}))?;
    let id = r["imported"].get(0).and_then(Value::as_u64).ok_or(format!("import: {r}"))?;
    s.execute("library.select", &json!({"ids": [id], "active": id}))?;
    let t = Instant::now();
    let faces = s.execute("retouch.detect", &json!({}))?;
    eprintln!("faces found in {:.1} s: {}", t.elapsed().as_secs_f64(), faces["faces"].as_array().map_or(0, Vec::len));
    let mut params = json!({});
    if let Some(m) = flag("--method") {
        params["method"] = json!(m);
    }
    if let Some(l) = flag("--lenses") {
        params["lenses"] = json!(l);
    }
    if let Some(f) = flag("--face") {
        params["face"] = json!(f.parse::<usize>()?);
    }
    let t = Instant::now();
    let started = s.execute("retouch.glareRepair", &params)?;
    eprintln!("started: {started}");
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let st = s.execute("retouch.glareStatus", &json!({}))?;
        if st["running"].is_null() {
            eprintln!("done in {:.1} s: {st}", t.elapsed().as_secs_f64());
            break;
        }
    }
    let st = s.execute("retouch.glareStatus", &json!({}))?;
    let repaired: Vec<usize> = st["repaired"].as_array().into_iter().flatten().filter_map(|r| r["face"].as_u64()).map(|f| f as usize).collect();
    if repaired.is_empty() {
        return Err(format!("nothing repaired: {}", st["message"]).into());
    }
    // before and after, at the photo's size
    let pid = lightcraft_catalog::PhotoId(id);
    let render = |s: &mut Session, amount: f64| -> Result<Rgba8, Box<dyn std::error::Error>> {
        s.execute("retouch.set", &json!({"control": "retouch.glare.amount", "value": amount}))?;
        s.retouch_detect_now(pid, false).map_err(|e| e.to_string())?;
        let job = s.render_job(pid, 8192, 8192, false, false).ok_or("no render")?;
        Ok(job.run().rendered?.image)
    };
    let before = render(&mut s, 0.0)?;
    let after = render(&mut s, 100.0)?;
    let found = s.retouch_faces_of(pid).found.ok_or("no faces")?;
    let areas: Vec<_> = repaired.iter().filter_map(|i| found.input.faces.get(*i).and_then(|f| f.patch.clone())).map(|p| p.area).collect();
    let side = 768;
    // each patch's area, edges held past the photo's border
    let sheet = Rgba8::from_fn(side * 2, side * areas.len(), |x, y| {
        let Some(a) = areas.get(y / side) else { return [0, 0, 0, 255] };
        let src = if x / side == 0 { &before } else { &after };
        let nx = a.x0 + ((x % side) as f64 + 0.5) / side as f64 * a.width();
        let ny = a.y0 + ((y % side) as f64 + 0.5) / side as f64 * a.height();
        let sx = ((nx * src.width as f64) as isize).clamp(0, src.width as isize - 1) as usize;
        let sy = ((ny * src.height as f64) as isize).clamp(0, src.height as isize - 1) as usize;
        src.data.get(sy * src.width + sx).copied().unwrap_or([0, 0, 0, 255])
    });
    let image = lightcraft_codecs::encode::EncodeImage::rgba8(&sheet);
    let bytes = lightcraft_codecs::encode::encode_jpeg(&image, 92, lightcraft_codecs::encode::ChromaSubsampling::S444, &Default::default())?;
    std::fs::write(out, bytes)?;
    let _ = std::fs::remove_dir_all(&library);
    Ok(())
}
