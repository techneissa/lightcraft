//! MediaPipe's face landmark model v2 (Apache-2.0): 478 points (468 face + 5 per iris) from a 256 × 256 face crop.
//!
//! The weights are the `SceneWorks/mp-facemesh-v2` safetensors rehost (Apache-2.0) of the onnx2torch graph py-feat
//! published. That file stores the network as a short program of layers in its header (`program`): convolutions,
//! PReLU, adds, 2 × 2 max pools, zero padding, a sigmoid and reshapes. This module checks the program (known ops
//! only, every input defined before use, every weight present with a fitting shape, bounded size) and runs it.

use std::collections::HashMap;

use candle_core::{Device, Tensor};
use candle_nn::Module;
use serde::Deserialize;

use crate::nn::Conv;
use crate::weights::Weights;
use crate::{Error, Result};

/// Side of the square input.
pub const SIDE: usize = 256;
/// Points it gives.
pub const POINTS: usize = 478;
const MAX_NODES: usize = 4096;
const LANDMARKS_OUT: &str = "identity__741";
const PRESENCE_OUT: &str = "identity_1";

#[derive(Deserialize)]
struct Program {
    inputs: Vec<String>,
    outputs: Vec<String>,
    nodes: Vec<RawNode>,
}

#[derive(Deserialize)]
struct RawNode {
    op: String,
    out: String,
    inputs: Vec<String>,
    #[serde(default)]
    weight: Option<String>,
    #[serde(default)]
    bias: Option<String>,
    #[serde(default)]
    slope: Option<String>,
    #[serde(default)]
    stride: Option<[usize; 2]>,
    #[serde(default)]
    padding: Option<[usize; 2]>,
    #[serde(default)]
    dilation: Option<[usize; 2]>,
    #[serde(default)]
    groups: Option<usize>,
    #[serde(default)]
    kernel: Option<[usize; 2]>,
    #[serde(default)]
    pads: Option<Vec<[usize; 2]>>,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    shape: Option<Vec<i64>>,
}

enum Op {
    Conv(Conv),
    Prelu(Tensor),
    Add,
    MaxPool(usize),
    /// Zero padding after, per dimension (N, C, H, W).
    Pad([(usize, usize); 4]),
    Sigmoid,
    Reshape(Vec<i64>),
}

struct Node {
    op: Op,
    inputs: Vec<usize>,
    /// Values whose last use this node is (dropped after it runs).
    frees: Vec<usize>,
}

/// The face landmark network.
pub struct FaceMesh {
    nodes: Vec<Node>,
    input: usize,
    landmarks: usize,
    presence: usize,
    slots: usize,
}

/// What the mesh found in one crop.
#[derive(Clone, Debug)]
pub struct MeshOut {
    /// Crop pixels (0 to 256) and depth in the same unit.
    pub points: Vec<[f32; 3]>,
    /// How sure it is that the crop holds a face (0 to 1).
    pub presence: f32,
}

fn bad(what: impl std::fmt::Display) -> Error {
    Error::Model(format!("face mesh: {what}"))
}

impl FaceMesh {
    pub fn load(w: &Weights, dev: &Device) -> Result<Self> {
        let text = w.metadata("program").ok_or_else(|| bad("the file has no program"))?;
        let prog: Program = serde_json::from_str(text).map_err(|e| bad(format!("unreadable program: {e}")))?;
        if prog.nodes.is_empty() || prog.nodes.len() > MAX_NODES || prog.inputs.len() != 1 {
            return Err(bad("the program has an unexpected size"));
        }
        let mut ids: HashMap<String, usize> = HashMap::new();
        for name in &prog.inputs {
            let n = ids.len();
            ids.insert(name.clone(), n);
        }
        let mut nodes = Vec::with_capacity(prog.nodes.len());
        for raw in &prog.nodes {
            let inputs = raw
                .inputs
                .iter()
                .map(|i| ids.get(i).copied().ok_or_else(|| bad(format!("`{i}` is used before it is made"))))
                .collect::<Result<Vec<_>>>()?;
            let want = if raw.op == "add" { 2 } else { 1 };
            if inputs.len() != want {
                return Err(bad(format!("{} takes {want} input(s)", raw.op)));
            }
            let op = Self::op(raw, w, dev)?;
            nodes.push(Node { op, inputs, frees: Vec::new() });
            let n = ids.len();
            if ids.insert(raw.out.clone(), n).is_some() {
                return Err(bad(format!("`{}` is made twice", raw.out)));
            }
        }
        let find = |name: &str| ids.get(name).copied().ok_or_else(|| bad(format!("no output `{name}`")));
        let (landmarks, presence) = (find(LANDMARKS_OUT)?, find(PRESENCE_OUT)?);
        if !prog.outputs.iter().any(|o| o == LANDMARKS_OUT) {
            return Err(bad("the landmarks are not an output"));
        }
        // free each value after its last use (the outputs are kept)
        let mut last: HashMap<usize, usize> = HashMap::new();
        for (k, n) in nodes.iter().enumerate() {
            for &i in &n.inputs {
                last.insert(i, k);
            }
        }
        for (v, k) in last {
            if v != landmarks
                && v != presence
                && let Some(n) = nodes.get_mut(k)
            {
                n.frees.push(v);
            }
        }
        Ok(Self { nodes, input: 0, landmarks, presence, slots: ids.len() })
    }

    fn op(raw: &RawNode, w: &Weights, dev: &Device) -> Result<Op> {
        let tensor = |name: &Option<String>| -> Result<Tensor> {
            let name = name.as_deref().ok_or_else(|| bad(format!("{} has no weight name", raw.op)))?;
            Ok(w.read(name)?.to_device(dev)?)
        };
        Ok(match raw.op.as_str() {
            "conv2d" => {
                let k = tensor(&raw.weight)?;
                let groups = raw.groups.unwrap_or(1).max(1);
                let [o, per, kh, kw] = k.dims() else { return Err(bad("a kernel is not 4-D")) };
                let (o, per, kh, kw) = (*o, *per, *kh, *kw);
                let stride = raw.stride.unwrap_or([1, 1]);
                let padding = raw.padding.unwrap_or([0, 0]);
                if kh != kw
                    || stride[0] != stride[1]
                    || padding[0] != padding[1]
                    || padding[0] >= kh
                    || raw.dilation.is_some_and(|d| d != [1, 1])
                    || o % groups != 0
                    || per == 0
                    || kh > 15
                    || stride[0] == 0
                    || stride[0] > 4
                {
                    return Err(bad("a convolution has settings this runner does not take"));
                }
                let b = match &raw.bias {
                    Some(_) => {
                        let b = tensor(&raw.bias)?;
                        if b.dims() != [o] {
                            return Err(bad("a bias does not fit its kernel"));
                        }
                        Some(b)
                    }
                    None => None,
                };
                Op::Conv(Conv::from_parts(k, b, stride[0], padding[0], groups))
            }
            "prelu" => {
                let s = tensor(&raw.slope)?;
                let c = s.elem_count();
                Op::Prelu(s.reshape((1, c, 1, 1))?)
            }
            "add" => Op::Add,
            "sigmoid" => Op::Sigmoid,
            "maxpool2d" => match (raw.kernel, raw.stride, raw.padding) {
                (Some([k, k2]), Some([s, s2]), p) if k == k2 && s == s2 && s == k && (1..=4).contains(&k) && p.is_none_or(|p| p == [0, 0]) => {
                    Op::MaxPool(k)
                }
                _ => return Err(bad("a max pool has settings this runner does not take")),
            },
            "pad" => {
                let pads = raw.pads.as_deref().ok_or_else(|| bad("a pad has no sizes"))?;
                let [a, b, c, d] = pads else { return Err(bad("a pad is not 4-D")) };
                if raw.value.is_some_and(|v| v != 0.0) || [a, b, c, d].iter().any(|p| p[0] > 256 || p[1] > 256) {
                    return Err(bad("a pad has settings this runner does not take"));
                }
                Op::Pad([(a[0], a[1]), (b[0], b[1]), (c[0], c[1]), (d[0], d[1])])
            }
            "reshape" => {
                let shape = raw.shape.clone().ok_or_else(|| bad("a reshape has no shape"))?;
                if shape.is_empty() || shape.len() > 6 || shape.iter().filter(|&&d| d == -1).count() > 1 || shape.iter().any(|&d| d == 0 || d < -1) {
                    return Err(bad("a reshape has a shape this runner does not take"));
                }
                Op::Reshape(shape)
            }
            other => return Err(bad(format!("unknown layer `{other}`"))),
        })
    }

    #[cfg(test)]
    fn ops(&self) -> usize {
        self.nodes.len()
    }

    /// Run on `B` crops, `B × 3 × 256 × 256` with values 0 to 1.
    pub fn forward(&self, x: &Tensor) -> Result<Vec<MeshOut>> {
        let b = x.dim(0)?;
        let mut vals: Vec<Option<Tensor>> = vec![None; self.slots];
        if let Some(slot) = vals.get_mut(self.input) {
            *slot = Some(x.clone());
        }
        for (k, node) in self.nodes.iter().enumerate() {
            let arg = |i: usize| -> Result<&Tensor> {
                node.inputs.get(i).and_then(|&v| vals.get(v)).and_then(Option::as_ref).ok_or_else(|| bad("a value is missing"))
            };
            let y = match &node.op {
                Op::Conv(c) => c.forward(arg(0)?)?,
                Op::Prelu(s) => {
                    let t = arg(0)?;
                    (t.relu()? + t.neg()?.relu()?.neg()?.broadcast_mul(s)?)?
                }
                Op::Add => arg(0)?.broadcast_add(arg(1)?)?,
                Op::Sigmoid => candle_nn::ops::sigmoid(arg(0)?)?,
                Op::MaxPool(k) => arg(0)?.max_pool2d_with_stride(*k, *k)?,
                Op::Pad(p) => {
                    let mut t = arg(0)?.clone();
                    for (dim, &(lo, hi)) in p.iter().enumerate() {
                        if lo > 0 || hi > 0 {
                            t = t.pad_with_zeros(dim, lo, hi)?;
                        }
                    }
                    t
                }
                Op::Reshape(shape) => {
                    let t = arg(0)?;
                    let known =
                        shape.iter().filter(|&&d| d > 0).try_fold(1i64, |a, &d| a.checked_mul(d)).ok_or_else(|| bad("a reshape is too large"))?;
                    let n = i64::try_from(t.elem_count()).map_err(|_| bad("too large"))?;
                    if known <= 0 || n % known != 0 {
                        return Err(bad("a reshape does not fit"));
                    }
                    let dims = shape
                        .iter()
                        .map(|&d| usize::try_from(if d == -1 { n / known } else { d }).map_err(|_| bad("bad reshape")))
                        .collect::<Result<Vec<_>>>()?;
                    t.reshape(dims)?
                }
            };
            for &f in &node.frees {
                if let Some(slot) = vals.get_mut(f) {
                    *slot = None;
                }
            }
            if let Some(slot) = vals.get_mut(self.input + 1 + k) {
                *slot = Some(y);
            }
        }
        let get = |v: usize| vals.get(v).and_then(Option::as_ref).ok_or_else(|| bad("an output is missing"));
        let pts = get(self.landmarks)?.reshape((b, POINTS * 3))?.to_dtype(candle_core::DType::F32)?.to_vec2::<f32>()?;
        let pres = get(self.presence)?.reshape((b, ()))?.to_dtype(candle_core::DType::F32)?.to_vec2::<f32>()?;
        Ok(pts
            .into_iter()
            .zip(pres)
            .map(|(p, s)| MeshOut { points: p.as_chunks::<3>().0.to_vec(), presence: s.first().map_or(0.0, |&l| 1.0 / (1.0 + (-l).exp())) })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(dir: &std::path::Path, name: &str, nodes: &str, weights: &str, data: &[u8]) -> Result<FaceMesh> {
        let prog = format!(r#"{{"inputs":["x"],"outputs":["identity__741"],"nodes":[{nodes}]}}"#);
        let header = format!(r#"{{"__metadata__":{{"program":{}}}{weights}}}"#, serde_json::to_string(&prog).unwrap());
        let p = crate::weights::tests::write(dir, name, &header, data);
        FaceMesh::load(&Weights::open(&p)?, &Device::Cpu)
    }

    #[test]
    fn padding_wider_than_the_kernel_and_huge_reshapes_are_refused() {
        let dir = crate::tests::scratch("facemesh-bounds");
        let w = r#","k":{"dtype":"F32","shape":[1,1,3,3],"data_offsets":[0,36]}"#;
        let conv = |pad: usize| {
            format!(
                r#"{{"op":"conv2d","out":"identity__741","inputs":["x"],"weight":"k","stride":[1,1],"padding":[{pad},{pad}]}},{{"op":"sigmoid","out":"identity_1","inputs":["identity__741"]}}"#
            )
        };
        assert_eq!(program(&dir, "ok", &conv(1), w, &[0u8; 36]).unwrap().ops(), 2);
        assert!(program(&dir, "pad", &conv(3), w, &[0u8; 36]).is_err());
        let huge = r#"{"op":"reshape","out":"identity__741","inputs":["x"],"shape":[4611686018427387904,4611686018427387904]},{"op":"sigmoid","out":"identity_1","inputs":["identity__741"]}"#;
        let mesh = program(&dir, "huge", huge, "", &[]).unwrap();
        let x = candle_core::Tensor::zeros((1, 3, 4, 4), candle_core::DType::F32, &Device::Cpu).unwrap();
        assert!(mesh.forward(&x).is_err());
    }
}
