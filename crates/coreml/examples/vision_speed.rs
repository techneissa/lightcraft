//! Apple Vision's masks on photos, timed (fork feature, issue #115): person segmentation (Accurate), the per-person
//! masks and the subject masks, each run after one warm-up, and their mattes written as grey PGM files.
//!
//! `cargo run --release -p lightcraft-coreml --example vision_speed -- OUT_DIR W H PHOTO.rgb [W H PHOTO.rgb ...]`
//!
//! A `.rgb` file is the photo's raw 8-bit RGB pixels, row by row (`W × H × 3` bytes).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Instant;

use lightcraft_coreml::vision::{self, Image, Matte, Pixels};

fn pgm(path: &std::path::Path, m: &Matte) {
    let mut out = format!("P5\n{} {}\n255\n", m.width, m.height).into_bytes();
    out.extend(m.data.iter().map(|v| (v * 255.0).round() as u8));
    std::fs::write(path, out).expect("write");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.len() >= 4 && (args.len() - 1).is_multiple_of(3), "usage: vision_speed OUT_DIR W H PHOTO.rgb ...");
    assert!(vision::available(), "Apple Vision masks need macOS 14 or later");
    let out = std::path::PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).expect("out dir");
    for photo in args[1..].chunks(3) {
        let (w, h): (usize, usize) = (photo[0].parse().expect("W"), photo[1].parse().expect("H"));
        let px = std::fs::read(&photo[2]).expect("photo");
        let stem = std::path::Path::new(&photo[2]).file_stem().unwrap().to_string_lossy().to_string();
        let img = Image { width: w, height: h, pixels: Pixels::Rgb8(&px) };
        let time = |name: &str, f: &dyn Fn() -> Vec<Matte>| {
            let _ = f();
            let t = Instant::now();
            let ms = f();
            println!(
                "{stem:28} {name:20} {:7.0} ms  {} mask(s){}",
                t.elapsed().as_secs_f64() * 1000.0,
                ms.len(),
                ms.first().map(|m| format!(", {} × {}", m.width, m.height)).unwrap_or_default()
            );
            for (i, m) in ms.iter().enumerate() {
                pgm(&out.join(format!("{stem}.{name}.{i}.pgm")), m);
            }
        };
        time("person-segmentation", &|| vec![vision::person_segmentation(&img).unwrap()]);
        time("person-instances", &|| vision::person_instances(&img).unwrap().masks);
        time("foreground", &|| vision::foreground_instances(&img).unwrap().masks);
    }
}
