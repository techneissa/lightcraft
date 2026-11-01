use super::*;
use candle_nn::var_builder::SimpleBackend;

/// Deterministic small pseudo-random weights for any name and shape, so a forward pass of a tiny
/// network runs without a checkpoint.
struct Random;
impl SimpleBackend for Random {
    fn get(&self, s: candle_core::Shape, name: &str, _: candle_nn::Init, _: DType, dev: &Device) -> R<Tensor> {
        let mut state = name.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
        let data: Vec<f32> = (0..s.elem_count())
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((state >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.4
            })
            .collect();
        Tensor::from_vec(data, s, dev)
    }
    fn get_unchecked(&self, name: &str, _: DType, _: &Device) -> R<Tensor> {
        candle_core::bail!("{name}: unchecked access is not used")
    }
    fn contains_tensor(&self, _: &str) -> bool {
        true
    }
}

fn tiny() -> DirsConfig {
    DirsConfig {
        dim: 4,
        enc: [1, 1, 1, 1],
        dec: [1, 1, 1, 1],
        prior_blocks: 1,
        swin: SwinConfig { embed: 16, depths: [1, 2, 1, 1], heads: [2, 2, 2, 2], window: 4 },
    }
}

fn tiny_net() -> DirsNet {
    let vb = VarBuilder::from_backend(Box::new(Random), DType::F32, Device::Cpu);
    DirsNet::from_builder(vb, tiny(), Device::Cpu).unwrap()
}

fn picture(w: usize, h: usize) -> Vec<u8> {
    (0..w * h * 3).map(|i| ((i * 37 + i / 7) % 251) as u8).collect()
}

#[test]
fn tiny_network_runs_at_sizes_that_need_padding_and_is_deterministic() {
    let net = tiny_net();
    for (w, h) in [(64, 96), (40, 24), (33, 35)] {
        let rgb = picture(w, h);
        let a = net.infer(&rgb, w, h).unwrap();
        assert_eq!(a.len(), w * h);
        assert!(a.iter().all(|p| p.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))));
        assert_eq!(a, net.infer(&rgb, w, h).unwrap());
    }
    assert!(tiny_net().infer(&picture(40, 24), 40, 24).unwrap() == net.infer(&picture(40, 24), 40, 24).unwrap());
}

#[test]
fn the_picture_changes_the_result() {
    let net = tiny_net();
    let a = net.infer(&picture(64, 64), 64, 64).unwrap();
    let b = net.infer(&vec![200u8; 64 * 64 * 3], 64, 64).unwrap();
    assert_ne!(a, b);
}

#[test]
fn input_sizes_that_do_not_fit_are_errors() {
    let net = tiny_net();
    assert!(net.infer(&[], 0, 0).is_err());
    assert!(net.infer(&[0; 12], 2, 3).is_err());
    assert!(net.infer(&picture(8, 8), 2000, 1).is_err());
}

#[test]
fn missing_tensors_and_wrong_shapes_are_errors_not_panics() {
    let err = |tensors| match DirsNet::from_tensors(tensors, tiny(), Device::Cpu) {
        Ok(_) => String::new(),
        Err(e) => e.to_string(),
    };
    assert!(err(HashMap::new()).contains("cannot find tensor"));
    // A real name with the wrong shape: the first weight the forward pass reads.
    let mut wrong = HashMap::new();
    wrong.insert("swin.patch_embed.proj.weight".to_string(), Tensor::zeros((16, 3, 2, 2), DType::F32, &Device::Cpu).unwrap());
    assert!(err(wrong).contains("shape mismatch"));
}

#[test]
fn damaged_checkpoint_files_are_errors_not_panics() {
    let dir = std::env::temp_dir().join(format!("lc-dirs-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut out = HashMap::new();
    let read = |name: &str, bytes: &[u8], out: &mut HashMap<String, Tensor>| {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        read_pickle(&path, "state_dict", &Device::Cpu, out, |n| Some(n.to_string()))
    };
    assert!(read("empty.ckpt", b"", &mut out).is_err());
    assert!(read("text.ckpt", &vec![b'x'; 4096], &mut out).is_err());
    // A zip header that stops short, and a zip with a data.pkl that is not a pickle.
    assert!(read("short.ckpt", b"PK\x03\x04\x14\x00\x00\x00\x00\x00", &mut out).is_err());
    let mut noise = b"PK\x05\x06".to_vec();
    noise.extend(vec![0xffu8; 200]);
    assert!(read("noise.ckpt", &noise, &mut out).is_err());
    assert!(read_pickle(&dir.join("missing.ckpt"), "state_dict", &Device::Cpu, &mut out, |n| Some(n.to_string())).is_err());
    assert!(DirsNet::load(&dir, true).is_err());
    assert!(out.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
