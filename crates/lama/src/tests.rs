//! Tests with a tiny synthetic generator (no real weights): the loader, the network's shapes, hostile input.

use std::collections::BTreeMap;
use std::io::Write;

use candle_core::{Device, Tensor};

use crate::{Error, Lama, read_generator};

/// A deterministic small value for weight number `i` of tensor `salt`.
fn value(salt: usize, i: usize) -> f32 {
    (((i * 2654435761usize + salt * 40503) % 1000) as f32 / 1000.0 - 0.5) * 0.2
}

/// `(name, shape)` of every tensor of a generator with base width `ngf` and `blocks` residual blocks.
pub fn layout(ngf: usize, blocks: usize) -> Vec<(String, Vec<usize>)> {
    let widths = [ngf, ngf * 2, ngf * 4, ngf * 8];
    let (g, l) = (widths[3] * 3 / 4, widths[3] / 4);
    let mut v: Vec<(String, Vec<usize>)> = Vec::new();
    let bn = |v: &mut Vec<(String, Vec<usize>)>, p: &str, c: usize| {
        for n in ["weight", "bias", "running_mean", "running_var"] {
            v.push((format!("{p}.{n}"), vec![c]));
        }
    };
    v.push(("1.ffc.convl2l.weight".into(), vec![ngf, 4, 7, 7]));
    bn(&mut v, "1.bn_l", ngf);
    for i in 0..3 {
        let p = i + 2;
        if i < 2 {
            v.push((format!("{p}.ffc.convl2l.weight"), vec![widths[i + 1], widths[i], 3, 3]));
            bn(&mut v, &format!("{p}.bn_l"), widths[i + 1]);
        } else {
            v.push((format!("{p}.ffc.convl2l.weight"), vec![l, widths[i], 3, 3]));
            v.push((format!("{p}.ffc.convl2g.weight"), vec![g, widths[i], 3, 3]));
            bn(&mut v, &format!("{p}.bn_l"), l);
            bn(&mut v, &format!("{p}.bn_g"), g);
        }
    }
    for b in 0..blocks {
        for c in ["conv1", "conv2"] {
            let p = format!("{}.{c}", 5 + b);
            v.push((format!("{p}.ffc.convl2l.weight"), vec![l, l, 3, 3]));
            v.push((format!("{p}.ffc.convl2g.weight"), vec![g, l, 3, 3]));
            v.push((format!("{p}.ffc.convg2l.weight"), vec![l, g, 3, 3]));
            v.push((format!("{p}.ffc.convg2g.conv1.0.weight"), vec![g / 2, g, 1, 1]));
            bn(&mut v, &format!("{p}.ffc.convg2g.conv1.1"), g / 2);
            v.push((format!("{p}.ffc.convg2g.conv2.weight"), vec![g, g / 2, 1, 1]));
            v.push((format!("{p}.ffc.convg2g.fu.conv_layer.weight"), vec![g, g, 1, 1]));
            bn(&mut v, &format!("{p}.ffc.convg2g.fu.bn"), g);
            bn(&mut v, &format!("{p}.bn_l"), l);
            bn(&mut v, &format!("{p}.bn_g"), g);
        }
    }
    let at = 5 + blocks + 1;
    for i in 0..3 {
        let (inp, out) = (widths[3 - i], widths[2 - i]);
        v.push((format!("{}.weight", at + 3 * i), vec![inp, out, 3, 3]));
        v.push((format!("{}.bias", at + 3 * i), vec![out]));
        bn(&mut v, &format!("{}", at + 3 * i + 1), out);
    }
    v.push((format!("{}.weight", at + 10), vec![3, ngf, 7, 7]));
    v.push((format!("{}.bias", at + 10), vec![3]));
    v
}

pub fn synthetic(ngf: usize, blocks: usize) -> BTreeMap<String, Tensor> {
    layout(ngf, blocks)
        .into_iter()
        .enumerate()
        .map(|(salt, (name, shape))| {
            let n: usize = shape.iter().product();
            let bn_stat = |s: &str| name.ends_with(s);
            let data: Vec<f32> = (0..n)
                .map(|i| {
                    if bn_stat(".running_var") || (bn_stat(".weight") && shape.len() == 1) {
                        1.0
                    } else if bn_stat(".running_mean") || (bn_stat(".bias") && name.contains("bn")) {
                        0.0
                    } else {
                        value(salt, i)
                    }
                })
                .collect();
            (name, Tensor::from_vec(data, shape, &Device::Cpu).unwrap())
        })
        .collect()
}

fn frame(w: usize, h: usize) -> (Vec<f32>, Vec<f32>) {
    let rgb: Vec<f32> = (0..w * h * 3).map(|i| ((i * 7) % 255) as f32 / 255.0).collect();
    let mut hole = vec![0f32; w * h];
    for y in h / 4..h / 2 {
        for x in w / 4..w / 2 {
            hole[y * w + x] = 1.0;
        }
    }
    (rgb, hole)
}

#[test]
fn a_tiny_generator_fills_a_hole_and_keeps_the_rest() {
    let model = Lama::from_tensors(synthetic(8, 2), &Device::Cpu).unwrap();
    for (w, h) in [(64, 48), (50, 37), (8, 8), (1, 1)] {
        let (rgb, hole) = frame(w, h);
        let out = model.inpaint(&rgb, &hole, w, h, &|| false).unwrap();
        assert_eq!(out.len(), rgb.len());
        assert!(out.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)), "{w}x{h}");
        for i in 0..w * h {
            if hole[i] == 0.0 {
                assert_eq!(out[i * 3..i * 3 + 3], rgb[i * 3..i * 3 + 3], "{w}x{h} pixel {i} changed outside the hole");
            }
        }
    }
}

#[test]
fn the_fill_is_deterministic_and_stops_when_asked() {
    let model = Lama::from_tensors(synthetic(8, 2), &Device::Cpu).unwrap();
    let (rgb, hole) = frame(32, 32);
    let a = model.inpaint(&rgb, &hole, 32, 32, &|| false).unwrap();
    let b = model.inpaint(&rgb, &hole, 32, 32, &|| false).unwrap();
    assert_eq!(a, b);
    assert!(matches!(model.inpaint(&rgb, &hole, 32, 32, &|| true), Err(Error::Cancelled)));
}

#[test]
fn hostile_pictures_are_errors_not_panics() {
    let model = Lama::from_tensors(synthetic(8, 1), &Device::Cpu).unwrap();
    let input = |r: crate::Result<Vec<f32>>| matches!(r, Err(Error::Input(_)));
    assert!(input(model.inpaint(&[], &[], 0, 0, &|| false)));
    assert!(input(model.inpaint(&[0.0; 3], &[0.0, 0.0], 1, 1, &|| false)));
    assert!(input(model.inpaint(&[0.0; 12], &[0.0; 2], 2, 2, &|| false)));
    assert!(input(model.inpaint(&[f32::NAN; 3], &[1.0], 1, 1, &|| false)));
    assert!(input(model.inpaint(&[], &[], crate::MAX_SIDE + 1, 1, &|| false)));
    assert!(input(model.inpaint(&[], &[], usize::MAX, usize::MAX, &|| false)));
}

#[test]
fn a_checkpoint_with_a_wrong_shape_or_a_missing_key_is_an_error() {
    let mut wrong = synthetic(8, 1);
    wrong.insert("1.ffc.convl2l.weight".into(), Tensor::zeros((8, 3, 7, 7), candle_core::DType::F32, &Device::Cpu).unwrap());
    assert!(matches!(Lama::from_tensors(wrong, &Device::Cpu), Err(Error::Model(_))));
    for name in
        ["2.ffc.convl2l.weight", "5.conv1.ffc.convg2g.fu.bn.running_var", "5.conv2.bn_g.bias", "7.weight", "13.weight", "17.weight", "17.bias"]
    {
        let mut missing = synthetic(8, 1);
        missing.remove(name);
        assert!(matches!(Lama::from_tensors(missing, &Device::Cpu), Err(Error::Model(_))), "{name}");
    }
    // a layer with another width than its neighbours
    let mut odd = synthetic(8, 1);
    odd.insert("10.weight".into(), Tensor::zeros((64, 7, 3, 3), candle_core::DType::F32, &Device::Cpu).unwrap());
    assert!(matches!(Lama::from_tensors(odd, &Device::Cpu), Err(Error::Model(_))));
    assert!(matches!(Lama::from_tensors(BTreeMap::new(), &Device::Cpu), Err(Error::Model(_))));
    let mut no_blocks = synthetic(8, 1);
    no_blocks.retain(|k, _| !k.starts_with("5."));
    assert!(matches!(Lama::from_tensors(no_blocks, &Device::Cpu), Err(Error::Model(_))));
}

// ---- a synthetic TorchScript-style archive ----

enum Node {
    Module(&'static str, Vec<(String, Node)>),
    Tensor { key: usize, shape: Vec<usize>, long: bool },
}

fn unicode(out: &mut Vec<u8>, s: &str) {
    out.push(b'X');
    out.extend((s.len() as u32).to_le_bytes());
    out.extend(s.as_bytes());
}

fn int(out: &mut Vec<u8>, v: usize) {
    out.push(b'J');
    out.extend((v as u32).to_le_bytes());
}

fn tuple_of_ints(out: &mut Vec<u8>, v: &[usize]) {
    out.push(b'(');
    v.iter().for_each(|x| int(out, *x));
    out.push(b't');
}

fn write(out: &mut Vec<u8>, node: &Node) {
    match node {
        Node::Module(class, fields) => {
            out.extend(format!("c__torch__\n{class}\n").as_bytes());
            out.push(b')');
            out.push(b'R');
            out.push(b'}');
            out.push(b'(');
            for (k, v) in fields {
                unicode(out, k);
                write(out, v);
            }
            out.push(b'u');
            out.push(b'b');
        }
        Node::Tensor { key, shape, long } => {
            out.extend(b"ctorch._utils\n_rebuild_tensor_v2\n");
            out.push(b'(');
            out.push(b'(');
            unicode(out, "storage");
            out.extend(format!("ctorch\n{}\n", if *long { "LongStorage" } else { "FloatStorage" }).as_bytes());
            unicode(out, &key.to_string());
            unicode(out, "cpu");
            int(out, shape.iter().product());
            out.push(b't');
            out.push(b'Q');
            int(out, 0);
            tuple_of_ints(out, shape);
            let mut stride = vec![1usize; shape.len()];
            for i in (0..shape.len().saturating_sub(1)).rev() {
                stride[i] = stride[i + 1] * shape[i + 1];
            }
            tuple_of_ints(out, &stride);
            out.push(0x89);
            out.push(b'N');
            out.push(b't');
            out.push(b'R');
        }
    }
}

/// Writes `big-lama.pt`-shaped archive: `model.generator.model.<names>` plus a discriminator and a step counter.
fn archive(path: &std::path::Path, tensors: &BTreeMap<String, Tensor>, damage: bool) {
    // build the module tree: "1.ffc.convl2l.weight" -> 1 -> ffc -> convl2l -> weight
    fn insert(fields: &mut Vec<(String, Node)>, parts: &[&str], leaf: Node) {
        let Some((first, rest)) = parts.split_first() else { return };
        if rest.is_empty() {
            fields.push((first.to_string(), leaf));
            return;
        }
        let at = match fields.iter().position(|(k, _)| k == first) {
            Some(i) => i,
            None => {
                fields.push((first.to_string(), Node::Module("Module", Vec::new())));
                fields.len() - 1
            }
        };
        if let Node::Module(_, inner) = &mut fields[at].1 {
            insert(inner, rest, leaf);
        }
    }
    let mut seq: Vec<(String, Node)> = Vec::new();
    let mut blobs: Vec<Vec<u8>> = Vec::new();
    for (name, t) in tensors {
        let data: Vec<f32> = t.flatten_all().unwrap().to_vec1().unwrap();
        blobs.push(data.iter().flat_map(|v| v.to_le_bytes()).collect());
        let parts: Vec<&str> = name.split('.').collect();
        insert(&mut seq, &parts, Node::Tensor { key: blobs.len() - 1, shape: t.dims().to_vec(), long: false });
    }
    // a counter and a discriminator that must be skipped
    blobs.push(vec![0u8; 8]);
    seq.push(("num_batches_tracked".into(), Node::Tensor { key: blobs.len() - 1, shape: vec![], long: true }));
    blobs.push(vec![0u8; 16]);
    let disc = Node::Module("Module", vec![("weight".into(), Node::Tensor { key: blobs.len() - 1, shape: vec![4], long: false })]);
    let generator = Node::Module("Module", vec![("model".into(), Node::Module("Sequential", seq))]);
    let model = Node::Module("Trainer", vec![("generator".into(), generator), ("discriminator".into(), disc)]);
    let root = Node::Module("JITWrapper", vec![("model".into(), model)]);
    let mut pkl = vec![0x80, 2];
    write(&mut pkl, &root);
    pkl.push(b'.');
    if damage {
        pkl.truncate(pkl.len() / 2);
    }
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("big-lama/data.pkl", opts).unwrap();
    zip.write_all(&pkl).unwrap();
    for (i, b) in blobs.iter().enumerate() {
        zip.start_file(format!("big-lama/data/{i}"), opts).unwrap();
        zip.write_all(b).unwrap();
    }
    zip.finish().unwrap();
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lightcraft-lama-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn a_torchscript_archive_loads_and_matches_the_tensors_it_was_made_from() {
    let tensors = synthetic(8, 2);
    let path = scratch("tiny.pt");
    archive(&path, &tensors, false);
    let read = read_generator(&path, &Device::Cpu).unwrap();
    assert_eq!(read.len(), tensors.len(), "the discriminator and the step counter must be left out");
    for (name, t) in &tensors {
        let got: Vec<f32> = read[name].flatten_all().unwrap().to_vec1().unwrap();
        let want: Vec<f32> = t.flatten_all().unwrap().to_vec1().unwrap();
        assert_eq!(got, want, "{name}");
    }
    // and it runs: the same fill as from the in-memory tensors
    let (rgb, hole) = frame(32, 32);
    let a = Lama::load(&path, &Device::Cpu).unwrap().inpaint(&rgb, &hole, 32, 32, &|| false).unwrap();
    let b = Lama::from_tensors(tensors, &Device::Cpu).unwrap().inpaint(&rgb, &hole, 32, 32, &|| false).unwrap();
    assert_eq!(a, b);
    std::fs::remove_file(path).ok();
}

#[test]
fn damaged_and_foreign_files_are_errors() {
    let tensors = synthetic(8, 1);
    let path = scratch("damaged.pt");
    archive(&path, &tensors, true);
    assert!(matches!(read_generator(&path, &Device::Cpu), Err(Error::Model(_))));
    std::fs::write(&path, b"not a zip file at all").unwrap();
    assert!(matches!(read_generator(&path, &Device::Cpu), Err(Error::Model(_))));
    std::fs::write(&path, []).unwrap();
    assert!(matches!(read_generator(&path, &Device::Cpu), Err(Error::Model(_))));
    assert!(matches!(read_generator(&scratch("missing.pt"), &Device::Cpu), Err(Error::Model(_))));
    // a zip without a generator in it
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("x/data.pkl", zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(b"\x80\x02N.").unwrap();
    zip.finish().unwrap();
    assert!(matches!(read_generator(&path, &Device::Cpu), Err(Error::Model(_))));
    std::fs::remove_file(path).ok();
}
