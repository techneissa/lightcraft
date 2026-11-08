//! Try the eyes feature on a picture without building the app:
//! `cargo run -p lightcraft-retouch --example eyes -- in.jpg out.jpg --amount 50 [--face x0,y0,x1,y1]`
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod common;

use lightcraft_retouch::Feature;

fn main() -> std::process::ExitCode {
    common::main(<lightcraft_retouch::eyes::Eyes as Feature>::KEY)
}
