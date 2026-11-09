#!/usr/bin/env python3
"""Convert Learn My Edits' image encoder (DINOv2-small) to a Core ML model for the Neural Engine.

Run once on a Mac, by hand (issue #105, fork/coreml.md). The app never runs Python: it finds the `.mlpackage` this
writes in the shared models folder, compiles it once with Core ML and runs it through `crates/coreml`. Convert from
the `model.safetensors` already in that folder (read only); never download the weights a second time. The candle port in
`crates/learn/src/encoder.rs` stays the reference; this network is the same one, written again in PyTorch from the
same published tensor names, with the same 224 x 224 input and the same 37 x 37 -> 16 x 16 position table resample,
so both give the same 768 numbers.

    uv venv -p 3.12 venv && VIRTUAL_ENV=venv uv pip install -r tools/requirements-coreml-convert.txt
    M=~/Library/Application\ Support/ArtCraft/models/dinov2-small
    venv/bin/python tools/convert-coreml-dinov2.py --weights "$M/model.safetensors" --out out/ --install "$M"

Writes `out/dinov2-small.mlpackage`, `out/dinov2-small.mlmodelc` (compiled, for a quick local check) and
`out/coreml-manifest.json` (each file's size and SHA-256, the tool versions). `--install DIR` also copies the
`.mlpackage` into `DIR/coreml/` (the model folder, e.g. ~/Library/Application Support/ArtCraft/models/dinov2-small).
"""

import argparse
import hashlib
import json
import math
import pathlib
import platform
import shutil
import sys

import numpy as np
import torch
from safetensors.torch import load_file

NAME = "dinov2-small"
WEIGHTS_SHA256 = "ae1e99fcefd534ed978cdeb8326f08030c96e28b7a81ffcbc98a857c84d14be1"
INPUT, PATCH, HIDDEN, HEADS, LAYERS, PRETRAIN_GRID, EPS = 224, 14, 384, 6, 12, 37, 1e-6
GRID = INPUT // PATCH


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def resample_weights(n_in, n_out):
    """The taps of `crates/learn/src/features.rs` `resample_weights`: antialiased Catmull-Rom (a = -0.5)."""
    scale = n_in / n_out
    support = 2.0 * max(scale, 1.0)
    inv = 1.0 / max(scale, 1.0)

    def cubic(x):
        x, a = abs(x), -0.5
        if x < 1.0:
            return ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
        if x < 2.0:
            return (((x - 5.0) * x + 8.0) * x - 4.0) * a
        return 0.0

    out = []
    for o in range(n_out):
        centre = (o + 0.5) * scale
        lo = max(int(math.floor(centre - support)), 0)
        hi = min(int(math.ceil(centre + support)), n_in)
        taps = [(i, cubic((i + 0.5 - centre) * inv)) for i in range(lo, hi)]
        taps = [(i, w) for i, w in taps if w != 0.0]
        total = sum(w for _, w in taps)
        if abs(total) < 1e-12:
            taps = [(min(int(centre), n_in - 1), 1.0)]
        else:
            taps = [(i, w / total) for i, w in taps]
        out.append([(i, np.float32(w)) for i, w in taps])
    return out


def positions(table):
    """The class position and the 37 x 37 patch table resampled to 16 x 16, as `encoder.rs` `resample_positions`."""
    table = table.reshape(-1).numpy().astype(np.float32)
    cls, grid = table[:HIDDEN], table[HIDDEN:].reshape(PRETRAIN_GRID, PRETRAIN_GRID, HIDDEN)
    taps = resample_weights(PRETRAIN_GRID, GRID)
    out = np.zeros((GRID, GRID, HIDDEN), np.float32)
    for y, ry in enumerate(taps):
        for x, rx in enumerate(taps):
            v = np.zeros(HIDDEN, np.float32)
            for r, wr in ry:
                for c, wc in rx:
                    v += np.float32(wr * wc) * grid[r, c]
            out[y, x] = v
    full = np.concatenate([cls[None, :], out.reshape(GRID * GRID, HIDDEN)], 0)
    return torch.from_numpy(full[None])


class Block(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.norm1 = torch.nn.LayerNorm(HIDDEN, eps=EPS)
        self.q, self.k, self.v = (torch.nn.Linear(HIDDEN, HIDDEN) for _ in range(3))
        self.proj = torch.nn.Linear(HIDDEN, HIDDEN)
        self.scale1 = torch.nn.Parameter(torch.zeros(HIDDEN))
        self.norm2 = torch.nn.LayerNorm(HIDDEN, eps=EPS)
        self.fc1 = torch.nn.Linear(HIDDEN, 4 * HIDDEN)
        self.fc2 = torch.nn.Linear(4 * HIDDEN, HIDDEN)
        self.scale2 = torch.nn.Parameter(torch.zeros(HIDDEN))

    def forward(self, x):
        n = x.shape[1]
        h = self.norm1(x)
        split = lambda t: t.reshape(1, n, HEADS, HIDDEN // HEADS).transpose(1, 2)
        q, k, v = split(self.q(h)), split(self.k(h)), split(self.v(h))
        att = torch.softmax((q @ k.transpose(-1, -2)) * (HIDDEN // HEADS) ** -0.5, dim=-1)
        o = (att @ v).transpose(1, 2).reshape(1, n, HIDDEN)
        x = x + self.proj(o) * self.scale1
        h = self.fc2(torch.nn.functional.gelu(self.fc1(self.norm2(x))))
        return x + h * self.scale2


class Encoder(torch.nn.Module):
    """`pixels` [1, 3, 224, 224], ImageNet-normalised -> `features` [1, 768]: class token, then mean patch token."""

    def __init__(self, w):
        super().__init__()
        self.patch = torch.nn.Conv2d(3, HIDDEN, PATCH, stride=PATCH)
        self.blocks = torch.nn.ModuleList(Block() for _ in range(LAYERS))
        self.norm = torch.nn.LayerNorm(HIDDEN, eps=EPS)
        e = "embeddings."
        self.patch.weight.data = w[e + "patch_embeddings.projection.weight"]
        self.patch.bias.data = w[e + "patch_embeddings.projection.bias"]
        self.register_buffer("cls", w[e + "cls_token"].reshape(1, 1, HIDDEN))
        self.register_buffer("pos", positions(w[e + "position_embeddings"]))
        for i, b in enumerate(self.blocks):
            p = f"encoder.layer.{i}."
            a = p + "attention."
            pairs = [
                (b.norm1, p + "norm1"),
                (b.q, a + "attention.query"),
                (b.k, a + "attention.key"),
                (b.v, a + "attention.value"),
                (b.proj, a + "output.dense"),
                (b.norm2, p + "norm2"),
                (b.fc1, p + "mlp.fc1"),
                (b.fc2, p + "mlp.fc2"),
            ]
            for m, name in pairs:
                m.weight.data, m.bias.data = w[name + ".weight"], w[name + ".bias"]
            b.scale1.data, b.scale2.data = w[p + "layer_scale1.lambda1"], w[p + "layer_scale2.lambda1"]
        self.norm.weight.data, self.norm.bias.data = w["layernorm.weight"], w["layernorm.bias"]

    def forward(self, pixels):
        x = self.patch(pixels).flatten(2).transpose(1, 2)
        x = torch.cat([self.cls, x], 1) + self.pos
        for b in self.blocks:
            x = b(x)
        x = self.norm(x)
        return torch.cat([x[:, 0], x[:, 1:].mean(1)], -1)


def files_of(package):
    return [
        {"path": str(p.relative_to(package)), "bytes": p.stat().st_size, "sha256": digest(p)}
        for p in sorted(package.rglob("*"))
        if p.is_file()
    ]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--weights", required=True, type=pathlib.Path, help="DINOv2-small's model.safetensors (88 MB)")
    ap.add_argument("--out", required=True, type=pathlib.Path)
    ap.add_argument("--install", type=pathlib.Path, help="also copy the .mlpackage into DIR/coreml/")
    a = ap.parse_args()

    import coremltools as ct

    if digest(a.weights) != WEIGHTS_SHA256:
        sys.exit(f"{a.weights} is not the pinned DINOv2-small model.safetensors")
    model = Encoder(load_file(str(a.weights))).eval()
    example = torch.zeros(1, 3, INPUT, INPUT)
    with torch.no_grad():
        traced = torch.jit.trace(model, example)
    ml = ct.convert(
        traced,
        convert_to="mlprogram",
        inputs=[ct.TensorType(name="pixels", shape=example.shape, dtype=np.float32)],
        outputs=[ct.TensorType(name="features", dtype=np.float32)],
        compute_precision=ct.precision.FLOAT16,
        minimum_deployment_target=ct.target.macOS13,
    )
    ml.short_description = "DINOv2-small image encoder for LightCraft Learn My Edits (224 x 224 in, 768 numbers out)"
    ml.license = "Apache-2.0 (facebook/dinov2-small)"
    a.out.mkdir(parents=True, exist_ok=True)
    package = a.out / f"{NAME}.mlpackage"
    compiled = a.out / f"{NAME}.mlmodelc"
    for old in (package, compiled):
        shutil.rmtree(old, ignore_errors=True)
    ml.save(str(package))
    shutil.move(str(ct.utils.compile_model(str(package))), str(compiled))

    # Check against the float32 PyTorch reference on a random input (what the Rust test then checks against candle).
    x = torch.randn(1, 3, INPUT, INPUT, generator=torch.Generator().manual_seed(7))
    with torch.no_grad():
        ref = model(x).numpy().reshape(-1)
    got = ml.predict({"pixels": x.numpy()})["features"].reshape(-1)
    cos = float(np.dot(ref, got) / (np.linalg.norm(ref) * np.linalg.norm(got)))
    print(f"Core ML vs PyTorch fp32: cosine {cos:.6f}, max |diff| {np.abs(ref - got).max():.4f}")

    manifest = {
        "model": NAME,
        "package": package.name,
        "input": {"name": "pixels", "shape": [1, 3, INPUT, INPUT]},
        "output": {"name": "features", "shape": [1, 2 * HIDDEN]},
        "precision": "float16",
        "source": {"file": "model.safetensors", "sha256": WEIGHTS_SHA256},
        "files": files_of(package),
        "checkCosine": round(cos, 6),
        "tools": {
            "python": platform.python_version(),
            "torch": torch.__version__,
            "coremltools": ct.__version__,
            "numpy": np.__version__,
            "macos": platform.mac_ver()[0],
            "machine": platform.machine(),
        },
    }
    (a.out / "coreml-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest["files"], indent=2))
    if a.install:
        dest = a.install / "coreml" / package.name
        shutil.rmtree(dest, ignore_errors=True)
        shutil.copytree(package, dest)
        print(f"installed {dest}")


if __name__ == "__main__":
    main()
