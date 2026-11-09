#!/usr/bin/env python3
"""Convert the people-parsing face mesh (MediaPipe face landmarks v2) to a Core ML model.

Run once on a Mac, by hand (fork/coreml.md, fork/people-parsing.md). The app never runs Python: it finds the
`.mlpackage` this writes in the shared models folder, compiles it once with Core ML and runs it through
`crates/coreml`. Convert from the `face_landmarks_detector.safetensors` already in that folder (read only); never
download the weights a second time. The candle runner in `crates/parsing/src/facemesh.rs` stays the reference; this
network is the same one: the file stores it as a short program of layers in its header (`program`), and both run that
program, layer by layer.

    uv venv -p 3.12 venv && VIRTUAL_ENV=venv uv pip install -r tools/requirements-coreml-convert.txt
    M=~/Library/Application\\ Support/ArtCraft/models
    venv/bin/python tools/convert-coreml-facemesh.py --models "$M" --out out/ --install

The model (`facemesh-v2.mlpackage`) takes `pixels` [B, 3, 256, 256] (face crops, values 0 to 1, B = 1 to 8) and gives
`mesh` [B, 1435] in float32: per crop the 478 points (x, y, z in crop pixels) and last the face score before its
sigmoid, as `facemesh.rs` reads them (one output, so the app reads one tensor per call).

Writes `out/facemesh-v2.mlpackage`, `out/facemesh-v2.mlmodelc` (compiled, for a quick local check) and
`out/facemesh-v2-manifest.json` (each file's size and SHA-256, the check, the tool versions). `--install` also copies
the `.mlpackage` into `<models>/facemesh-v2/coreml/`.
"""

import argparse
import hashlib
import json
import pathlib
import platform
import shutil
import sys
import time

import numpy as np
import torch
import torch.nn.functional as F
from safetensors import safe_open

FOLDER = "facemesh-v2"
FILE = "face_landmarks_detector.safetensors"
SHA256 = "94e0dda4bb58733fc7454997868232bdcc2b11531a18d2bbfec5479c75f574f4"
LICENSE = "Apache-2.0 (Google MediaPipe face landmarks v2; SceneWorks/mp-facemesh-v2 rehost)"
SIDE = 256
POINTS = 478
BATCHES = list(range(1, 9))
LANDMARKS_OUT = "identity__741"
PRESENCE_OUT = "identity_1"


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


class FaceMesh(torch.nn.Module):
    """The header's program, run as `facemesh.rs` runs it."""

    def __init__(self, path):
        super().__init__()
        with safe_open(str(path), framework="pt") as f:
            prog = json.loads(f.metadata()["program"])
            tensors = {k: f.get_tensor(k).float() for k in f.keys()}
        if len(prog["inputs"]) != 1:
            sys.exit("the program does not take one input")
        self.input = prog["inputs"][0]
        self.nodes = prog["nodes"]
        self.params = torch.nn.ParameterDict()
        for k, n in enumerate(self.nodes):
            for field in ("weight", "bias", "slope"):
                if n.get(field):
                    t = tensors[n[field]]
                    if field == "slope":
                        t = t.reshape(1, -1, 1, 1)
                    self.params[f"{k}_{field}"] = torch.nn.Parameter(t, requires_grad=False)

    def forward(self, pixels):
        vals = {self.input: pixels}
        for k, n in enumerate(self.nodes):
            x = [vals[i] for i in n["inputs"]]
            op = n["op"]
            if op == "conv2d":
                bias = self.params[f"{k}_bias"] if n.get("bias") else None
                y = F.conv2d(x[0], self.params[f"{k}_weight"], bias, n.get("stride", [1, 1])[0], n.get("padding", [0, 0])[0], 1, n.get("groups", 1) or 1)
            elif op == "prelu":
                # as candle: relu(x) - relu(-x) * slope
                y = torch.relu(x[0]) - torch.relu(-x[0]) * self.params[f"{k}_slope"]
            elif op == "add":
                y = x[0] + x[1]
            elif op == "sigmoid":
                y = torch.sigmoid(x[0])
            elif op == "maxpool2d":
                y = F.max_pool2d(x[0], n["kernel"][0], n["stride"][0])
            elif op == "pad":
                flat = []
                for lo, hi in reversed(n["pads"]):
                    flat += [lo, hi]
                y = F.pad(x[0], flat)
            elif op == "reshape":
                y = x[0].reshape(n["shape"])
            else:
                sys.exit(f"unknown layer {op}")
            vals[n["out"]] = y
        b = pixels.shape[0]
        return torch.cat([vals[LANDMARKS_OUT].reshape(b, POINTS * 3), vals[PRESENCE_OUT].reshape(b, 1)], dim=1)


def files_of(package):
    return [
        {"path": str(p.relative_to(package)), "bytes": p.stat().st_size, "sha256": digest(p)}
        for p in sorted(package.rglob("*"))
        if p.is_file()
    ]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--models", required=True, type=pathlib.Path, help="the shared models folder (read only)")
    ap.add_argument("--out", required=True, type=pathlib.Path)
    ap.add_argument("--install", action="store_true", help=f"also copy the .mlpackage into <models>/{FOLDER}/coreml/")
    a = ap.parse_args()

    import coremltools as ct

    weights = a.models / FOLDER / FILE
    if digest(weights) != SHA256:
        sys.exit(f"{weights} is not the pinned face mesh")
    model = FaceMesh(weights).eval()
    example = torch.zeros(1, 3, SIDE, SIDE)
    shapes = ct.EnumeratedShapes(shapes=[[b, 3, SIDE, SIDE] for b in BATCHES], default=[1, 3, SIDE, SIDE])
    with torch.no_grad():
        traced = torch.jit.trace(model, example)
    ml = ct.convert(
        traced,
        convert_to="mlprogram",
        inputs=[ct.TensorType(name="pixels", shape=shapes, dtype=np.float32)],
        outputs=[ct.TensorType(name="mesh", dtype=np.float32)],
        # float32 on the GPU: as fast as float16 for this small network (about 10 ms for 8 crops on an M4 Pro) and equal
        # to PyTorch's float32 to 0.001 px, where float16 moved points by up to 0.5 px and the Neural Engine by 3.5 px
        compute_precision=ct.precision.FLOAT32,
        minimum_deployment_target=ct.target.macOS13,
        compute_units=ct.ComputeUnit.CPU_AND_GPU,
    )
    ml.short_description = "MediaPipe face mesh v2 for LightCraft people parsing (fork/people-parsing.md)"
    ml.license = LICENSE
    a.out.mkdir(parents=True, exist_ok=True)
    package = a.out / f"{FOLDER}.mlpackage"
    compiled = a.out / f"{FOLDER}.mlmodelc"
    for old in (package, compiled):
        shutil.rmtree(old, ignore_errors=True)
    ml.save(str(package))
    shutil.move(str(ct.utils.compile_model(str(package))), str(compiled))

    # Check against the float32 PyTorch reference on each compute unit (what the Rust test then checks against
    # candle). Random pixels hold no face; the points still follow the same network.
    x = torch.rand(4, 3, SIDE, SIDE, generator=torch.Generator().manual_seed(7))
    with torch.no_grad():
        ref = model(x).numpy()
    ml.predict({"pixels": x.numpy()})
    t = time.time()
    got = ml.predict({"pixels": x.numpy()})["mesh"]
    took = time.time() - t
    pts = float(np.abs(got[:, :-1] - ref[:, :-1]).max())
    pres = float(np.abs(got[:, -1] - ref[:, -1]).max())
    print(f"Core ML vs PyTorch fp32 (4 crops): points max |diff| {pts:.4f} px, presence logit {pres:.5f}, {took * 1e3:.1f} ms")
    report = [{"shape": list(x.shape), "pointsMaxDiff": round(pts, 5), "presenceMaxDiff": round(pres, 6)}]

    manifest = {
        "model": FOLDER,
        "package": package.name,
        "input": {"name": "pixels", "shapes": [list(s.shape) for s in shapes.shapes]},
        "output": {"name": "mesh"},
        "precision": "float32",
        "source": {"file": FILE, "sha256": SHA256},
        "license": LICENSE,
        "files": files_of(package),
        "check": report,
        "tools": {
            "python": platform.python_version(),
            "torch": torch.__version__,
            "coremltools": ct.__version__,
            "numpy": np.__version__,
            "macos": platform.mac_ver()[0],
            "machine": platform.machine(),
        },
    }
    (a.out / f"{FOLDER}-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest["files"], indent=2))
    if a.install:
        dest = a.models / FOLDER / "coreml" / package.name
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.rmtree(dest, ignore_errors=True)
        shutil.copytree(package, dest)
        print(f"installed {dest}")


if __name__ == "__main__":
    main()
