//! Render one raw and print its size and time, to measure peak memory with `/usr/bin/time -l`:
//! `render <long edge, 0 = full> <file> [--bytes] [--draft]`. Writes nothing.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use lightcraft_macraw::{Input, Request, render, uti_for_extension};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(edge), Some(file)) = (args.first().and_then(|a| a.parse::<u32>().ok()), args.get(1)) else {
        eprintln!("usage: render <long edge, 0 = full> <file> [--bytes] [--draft]");
        return ExitCode::FAILURE;
    };
    let path = Path::new(file);
    let bytes = if args.iter().any(|a| a == "--bytes") {
        match std::fs::read(path) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("{file}: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    let uti = path.extension().and_then(|e| uti_for_extension(&e.to_string_lossy())).unwrap_or("");
    let input = match &bytes {
        Some(data) => Input::Bytes { data, uti },
        None => Input::Path(path),
    };
    let request = Request { long_edge: (edge > 0).then_some(edge), draft: args.iter().any(|a| a == "--draft"), expected_size: None };
    let t = std::time::Instant::now();
    match render(input, &request, &AtomicBool::new(false)) {
        Ok(img) => {
            println!("{}×{} in {:.3} s ({})", img.width, img.height, t.elapsed().as_secs_f64(), if bytes.is_some() { "bytes" } else { "path" });
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{file}: {e}");
            ExitCode::FAILURE
        }
    }
}
