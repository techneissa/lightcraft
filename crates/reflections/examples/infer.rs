//! Developer equivalence and timing check:
//! cargo run --release -p lightcraft-reflections --example infer -- MODEL_DIR INPUT.rgb W H OUTPUT.f32 [cpu] [runs]
//! The model id is the folder name (`dsrnet-s`, `dirs-mugi`, …); INPUT.rgb is raw 8-bit RGB.
use std::io::Write;
use std::time::Instant;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |n| args.get(n).ok_or("MODEL_DIR INPUT.rgb W H OUTPUT.f32 [cpu] [runs]");
    let (w, h) = (get(2)?.parse::<usize>()?, get(3)?.parse::<usize>()?);
    let input = std::fs::read(get(1)?)?;
    let dir = std::path::Path::new(get(0)?);
    let model = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let cpu = args.iter().skip(5).any(|x| x == "cpu");
    let runs = args.iter().skip(5).find_map(|x| x.parse::<usize>().ok()).unwrap_or(1);
    let start = Instant::now();
    let net = lightcraft_reflections::Runner::load(dir, model, cpu)?;
    eprintln!("load {:.1}s", start.elapsed().as_secs_f32());
    let mut result = Vec::new();
    for run in 1..=runs {
        let start = Instant::now();
        result = net.infer(&input, w, h)?;
        eprintln!("run {run}: {:.1}s", start.elapsed().as_secs_f32());
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(get(4)?)?);
    for pixel in result {
        for channel in pixel {
            f.write_all(&channel.to_le_bytes())?;
        }
    }
    f.flush()?;
    Ok(())
}
