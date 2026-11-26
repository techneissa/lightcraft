//! SCRATCH for issue #438, never merged: make one AI selection and print it as mask JSON, to be reused by every render.
//! `u16_ai <photo> <subject|sky> <model id> <out.json>` (needs `--features sam`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use lightcraft_engine::Session;
use serde_json::json;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut s = Session::new().with_fs().with_system_clock().with_default_sam3_model();
    let r = s.execute("library.import", &json!({"paths": [std::fs::canonicalize(&a[0]).unwrap()]})).unwrap();
    let id = r["imported"][0].as_u64().expect("imported");
    s.execute("library.select", &json!({"ids": [id], "active": id})).unwrap();
    println!("{}", s.execute("segment.selectionModel", &json!({"model": a[2]})).unwrap());
    println!("{}", s.execute("mask.add", &json!({"kind": a[1]})).unwrap());
    let d = s.execute("develop.get", &json!({})).unwrap();
    let masks = d.get("settings").unwrap_or(&d)["masks"].clone();
    let text = serde_json::to_string(&masks).unwrap();
    println!("{} bytes; {}", text.len(), &text[..text.len().min(200)]);
    std::fs::write(&a[3], text).unwrap();
}
