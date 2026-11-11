#!/usr/bin/env python3
"""Convert BiRefNet (Subject masks) and BiRefNet Matting (the hair edge) to Core ML models that run on the GPU.

Run once on a Mac, by hand (issue #264, fork/coreml.md). The app never runs Python: it finds the `.mlpackage` this
writes in the shared models folder, compiles it once with Core ML and runs it through `crates/coreml`. Convert from
the `model.safetensors` already in that folder (read only, checked against the SHA-256 `crates/birefnet/src/fetch.rs`
pins); never download the weights a second time. The candle port in `crates/birefnet` stays the reference.

The network is BiRefNet's own PyTorch code (`birefnet.py` and `BiRefNet_config.py` of the Hugging Face repo
`ZhengPeng7/BiRefNet`, MIT), read from `--code`; it is not part of LightCraft. It also needs `timm`, `torchvision`,
`transformers`, `einops` and `kornia` in the venv. Core ML has no deformable convolution, so torchvision's
`deform_conv2d` is replaced by the same sum written with `grid_sample` (bilinear, zeros outside), one kernel tap at a
time.

    uv venv -p 3.12 venv && VIRTUAL_ENV=venv uv pip install -r tools/requirements-coreml-convert.txt timm torchvision transformers einops kornia
    M=~/Library/Application\\ Support/ArtCraft/models
    venv/bin/python -I tools/convert-coreml-birefnet.py --model birefnet --code hf/ --models "$M" --out out/ --install
    venv/bin/python -I tools/convert-coreml-birefnet.py --model birefnet-matting --code hf/ --models "$M" --out out/ --install

Each package takes `pixels` [1, 3, 1024, 1024] (the photo stretched to the square with Pillow's bilinear filter and
ImageNet-normalised, as `crates/birefnet/src/image.rs` makes it) and gives `logits` [1, 1, 1024, 1024], the last
decoder output before its sigmoid, the numbers candle's `Matte` holds. One fixed shape (1024², the side
`Variant::input` gives both models): Core ML plans a model on the GPU again on every new input shape, which costs
more than the run. Float16 weights and compute; the app loads it on the GPU (`CPU_AND_GPU`): the Neural Engine took
11 s a run and over 4 minutes to load (#115).

Writes `out/<folder>.mlpackage` and `out/<folder>-manifest.json` (each file's size and SHA-256, the check against
float32 PyTorch, the tool versions). `--install` also copies the `.mlpackage` into `<models>/<folder>/coreml/`.
"""

import argparse
import hashlib
import json
import pathlib
import platform
import shutil
import sys
import time
import types

import numpy as np
import torch
import torch.nn.functional as F

SIDE = 1024
MODELS = {
    "birefnet": {
        "sha256": "9ab37426bf4de0567af6b5d21b16151357149139362e6e8992021b8ce356a154",
        "label": "BiRefNet",
    },
    "birefnet-matting": {
        "sha256": "a9875de5b1e6c8eb5fdaa8c727a82927ce442cdc87ba3abee6a77e6fa46c25bb",
        "label": "BiRefNet Matting",
    },
}
LICENSE = "MIT (ZhengPeng7/BiRefNet)"
MEAN = np.array([0.485, 0.456, 0.406], np.float32).reshape(1, 3, 1, 1)
STD = np.array([0.229, 0.224, 0.225], np.float32).reshape(1, 3, 1, 1)


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def files_of(package):
    return [
        {"path": str(p.relative_to(package.parent)), "bytes": p.stat().st_size, "sha256": digest(p)}
        for p in sorted(package.rglob("*"))
        if p.is_file()
    ]


def deform_via_grid(input, offset, weight, bias=None, stride=(1, 1), padding=(0, 0), dilation=(1, 1), mask=None):
    """torchvision's `deform_conv2d` (dilation 1, one offset group), written with `grid_sample`:
    out[o, y, x] = sum over taps k and channels c of w[o, c, k] * mask[k, y, x] * input[c] read bilinearly at
    (y * s - pad + ki + dy, x * s - pad + kj + dx), zeros outside."""
    _, _, h, w = input.shape
    _, _, kh, kw = weight.shape
    sh, sw = (stride, stride) if isinstance(stride, int) else stride
    ph, pw = (padding, padding) if isinstance(padding, int) else padding
    ho = (h + 2 * ph - kh) // sh + 1
    wo = (w + 2 * pw - kw) // sw + 1
    ys = (torch.arange(ho, dtype=input.dtype) * sh - ph).view(1, ho, 1)
    xs = (torch.arange(wo, dtype=input.dtype) * sw - pw).view(1, 1, wo)
    acc = None
    for ki in range(kh):
        for kj in range(kw):
            k = ki * kw + kj
            # torchvision's layout: (dy, dx) of each tap, interleaved
            py = ys + ki + offset[:, 2 * k]
            px = xs + kj + offset[:, 2 * k + 1]
            # align_corners=False: pixel centre i is (2i + 1) / n - 1
            grid = torch.stack([(2 * px + 1) / w - 1, (2 * py + 1) / h - 1], dim=-1)
            s = F.grid_sample(input, grid, mode="bilinear", padding_mode="zeros", align_corners=False)
            if mask is not None:
                s = s * mask[:, k : k + 1]
            t = F.conv2d(s, weight[:, :, ki : ki + 1, kj : kj + 1])
            acc = t if acc is None else acc + t
    if bias is not None:
        acc = acc + bias.view(1, -1, 1, 1)
    return acc


def load_net(code, weights):
    """BiRefNet's own network from `code`, with `weights` (strict: every tensor named as the checkpoint names it)."""
    sys.path.insert(0, str(code))
    src = (code / "birefnet.py").read_text()
    src = src.replace("from .BiRefNet_config", "from BiRefNet_config")
    # (the window reverse divides a traced shape; the batch is always 1 here)
    src = src.replace("B = int(windows.shape[0] / (H * W / window_size / window_size))", "B = 1")
    mod = types.ModuleType("birefnet_upstream")
    mod.__file__ = str(code / "birefnet.py")
    sys.modules["birefnet_upstream"] = mod
    exec(compile(src, str(code / "birefnet.py"), "exec"), mod.__dict__)
    mod.deform_conv2d = deform_via_grid
    from safetensors.torch import load_file

    net = mod.BiRefNet(bb_pretrained=False)
    state = {k: v.float() for k, v in load_file(str(weights)).items()}
    missing, unexpected = net.load_state_dict(state, strict=False)
    # (buffers the checkpoint does not keep, such as Swin's relative position index, are rebuilt by the code)
    missing = [k for k in missing if "relative_position_index" not in k]
    if missing or unexpected:
        sys.exit(f"{weights} does not fit BiRefNet's code: missing {missing[:5]}, unexpected {unexpected[:5]}")
    return net.float().eval()


class Logits(torch.nn.Module):
    """The last decoder output, before its sigmoid."""

    def __init__(self, net):
        super().__init__()
        self.net = net

    def forward(self, pixels):
        return self.net(pixels)[-1]


def check_input():
    """A smooth made-up photo: a bright disc on a darker gradient, normalised as the app normalises."""
    y, x = np.mgrid[0:SIDE, 0:SIDE].astype(np.float32) / SIDE
    disc = ((x - 0.5) ** 2 + (y - 0.45) ** 2 < 0.09).astype(np.float32)
    rgb = np.stack([0.2 + 0.3 * x + 0.5 * disc, 0.25 + 0.2 * y + 0.4 * disc, 0.3 + 0.1 * x * y + 0.3 * disc])[None]
    return ((rgb - MEAN) / STD).astype(np.float32)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--model", required=True, choices=sorted(MODELS))
    ap.add_argument("--code", required=True, type=pathlib.Path, help="folder with BiRefNet's birefnet.py and BiRefNet_config.py")
    ap.add_argument("--models", required=True, type=pathlib.Path, help="the shared models folder (read only)")
    ap.add_argument("--out", required=True, type=pathlib.Path)
    ap.add_argument("--install", action="store_true", help="also copy the .mlpackage into <models>/<folder>/coreml/")
    a = ap.parse_args()

    import coremltools as ct

    spec = MODELS[a.model]
    weights = a.models / a.model / "model.safetensors"
    if digest(weights) != spec["sha256"]:
        sys.exit(f"{weights} is not the pinned {spec['label']} model.safetensors")
    model = Logits(load_net(a.code.resolve(), weights)).eval()
    example = torch.zeros(1, 3, SIDE, SIDE)
    t = time.time()
    with torch.no_grad():
        exported = torch.export.export(model, (example,)).run_decompositions({})
    # (coremltools has no `alias` op; each is its input)
    graph = exported.graph_module.graph
    for node in list(graph.nodes):
        if node.op == "call_function" and "alias" in str(node.target):
            node.replace_all_uses_with(node.args[0])
            graph.erase_node(node)
    exported.graph_module.recompile()
    print(f"exported in {time.time() - t:.0f} s")
    t = time.time()
    ml = ct.convert(
        exported,
        convert_to="mlprogram",
        inputs=[ct.TensorType(name="pixels", shape=(1, 3, SIDE, SIDE), dtype=np.float32)],
        outputs=[ct.TensorType(name="logits", dtype=np.float32)],
        compute_precision=ct.precision.FLOAT16,
        minimum_deployment_target=ct.target.macOS14,
        compute_units=ct.ComputeUnit.CPU_AND_GPU,
    )
    print(f"converted in {time.time() - t:.0f} s")
    ml.short_description = f"{spec['label']} for LightCraft (fork/coreml.md)"
    ml.license = LICENSE
    a.out.mkdir(parents=True, exist_ok=True)
    package = a.out / f"{a.model}.mlpackage"
    shutil.rmtree(package, ignore_errors=True)
    ml.save(str(package))

    # Check against the float32 PyTorch network (the Rust test then checks the app's path against candle).
    x = check_input()
    with torch.no_grad():
        ref = model(torch.from_numpy(x)).numpy()
    t = time.time()
    got = np.asarray(ml.predict({"pixels": x})["logits"], np.float32)
    took = time.time() - t
    sig = lambda v: 1 / (1 + np.exp(-v))
    diff = np.abs(sig(ref) - sig(got))
    check = {"matteMaxDiff": round(float(diff.max()), 4), "matteMeanDiff": round(float(diff.mean()), 5), "ms": round(took * 1e3)}
    print(f"Core ML vs PyTorch fp32: matte max |diff| {check['matteMaxDiff']}, mean {check['matteMeanDiff']}, {check['ms']} ms")

    manifest = {
        "model": a.model,
        "package": package.name,
        "input": {"name": "pixels", "shape": [1, 3, SIDE, SIDE]},
        "output": {"name": "logits", "shape": [1, 1, SIDE, SIDE]},
        "precision": "float16",
        "source": {"file": "model.safetensors", "sha256": spec["sha256"]},
        "license": LICENSE,
        "files": files_of(package),
        "check": check,
        "tools": {
            "python": platform.python_version(),
            "torch": torch.__version__,
            "coremltools": ct.__version__,
            "numpy": np.__version__,
            "macos": platform.mac_ver()[0],
            "machine": platform.machine(),
        },
    }
    (a.out / f"{a.model}-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    if a.install:
        dest = a.models / a.model / "coreml" / package.name
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.rmtree(dest, ignore_errors=True)
        shutil.copytree(package, dest)
        print(f"installed {dest}")


if __name__ == "__main__":
    main()
