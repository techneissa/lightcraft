#!/usr/bin/env python3
"""Convert LaMa (big-lama, the Remove and heal fill) to one Core ML model that runs on the Neural Engine.

Run once on a Mac, by hand (issue #268, fork/coreml.md). The app never runs Python: it finds the `.mlpackage` this
writes in the shared models folder, compiles it once with Core ML and runs it through `crates/coreml`. Convert from
the `big-lama.pt` already in that folder (read only, checked against the SHA-256 `crates/engine/src/lama_fill/download.rs`
pins); never download the weights a second time. The candle port in `crates/lama` stays the reference.

The network below is big-lama's generator written again in PyTorch from LightCraft's candle port (`crates/lama/src`:
`net.rs`, `layers.rs`, `cl.rs`), not LaMa's own code; only the weights are big-lama's (Apache-2.0). It matches the
TorchScript original to 3e-6. Its Fourier units use `torch.fft.rfftn` / `irfftn`, which coremltools lowers to plain
matrix products; that version runs whole on the Neural Engine.

    uv venv -p 3.12 venv && VIRTUAL_ENV=venv uv pip install -r tools/requirements-coreml-convert.txt
    M=~/Library/Application\\ Support/ArtCraft/models
    venv/bin/python -I tools/convert-coreml-lama.py --models "$M" --out out/ --install

One package, `lama.mlpackage`, with one function per square input side (`side128` to `side768`: `--sides`),
sharing one weights file (`MLModelConfiguration.functionName`, macOS 15 or later). Each takes `input` [1, 4, s, s]
(the gamma-encoded picture with the hole blanked, then the hole: 1 where to fill), as candle's `Lama::generate`
takes it, and gives `fill` [1, 3, s, s] in 0..1. Core ML plans a model again on every new input shape, so the app
pads a crop up to the next side (`crates/lama/src/coreml.rs`). Float16 weights and compute; the app loads it on the
Neural Engine (`CPU_AND_NE`): 95 ms a 512 px fill against 0.3 s on the GPU and 0.7-0.9 s on candle (#268).

Writes `out/lama.mlpackage` and `out/lama-manifest.json` (each file's size and SHA-256, the check against float32
PyTorch per side, the tool versions). `--install` also copies the `.mlpackage` into `<models>/lama/coreml/`.
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
import torch.nn as nn
import torch.nn.functional as F

SOURCE = "big-lama.pt"
SOURCE_SHA256 = "7ba7aa7ac37a4d41fdbbeba3a2af7ead18058552997e3a3cd1a3b2210c9e6b4c"
LICENSE = "Apache-2.0 (big-lama weights, advimman/lama)"
SIDES = (128, 256, 384, 512, 768)
BN_EPS = 1e-5


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


def load_generator(path):
    """The generator's weights by `<index>.<module>.<param>`, from the TorchScript archive (only its state is read)."""
    m = torch.jit.load(str(path), map_location="cpu")
    out = {}
    for k, v in m.state_dict().items():
        at = k.find("generator.model.")
        if at >= 0 and v.dtype in (torch.float32, torch.float64):
            out[k[at + len("generator.model."):]] = v.float()
    return out


def fold_bn(w, prefix):
    g, b, mean, var = (w[f"{prefix}.{n}"] for n in ("weight", "bias", "running_mean", "running_var"))
    scale = g / torch.sqrt(var + BN_EPS)
    return scale, b - mean * scale


def param(t):
    return nn.Parameter(t.contiguous(), requires_grad=False)


class ConvBN(nn.Module):
    """Convolution (no bias) with reflect padding, BatchNorm folded in, ReLU."""

    def __init__(self, weight, scale, shift, stride=1, pad=0):
        super().__init__()
        self.weight = param(weight * scale.view(-1, 1, 1, 1))
        self.bias = param(shift.clone())
        self.stride, self.pad = stride, pad

    def forward(self, x):
        if self.pad:
            x = F.pad(x, (self.pad,) * 4, mode="reflect")
        return F.relu(F.conv2d(x, self.weight, self.bias, stride=self.stride))


class FourierUnit(nn.Module):
    def __init__(self, w, prefix, c):
        super().__init__()
        conv = w[f"{prefix}.conv_layer.weight"].view(2 * c, 2 * c)
        scale, shift = fold_bn(w, f"{prefix}.bn")
        conv = conv * scale[:, None]
        # the file interleaves parts (2i real, 2i+1 imaginary); reorder to [real..., imag...]
        perm = torch.cat([torch.arange(0, 2 * c, 2), torch.arange(1, 2 * c, 2)])
        self.weight = param(conv[perm][:, perm].reshape(2 * c, 2 * c, 1, 1))
        self.bias = param(shift[perm])
        self.c = c

    def forward(self, x):
        h, w = x.shape[-2:]
        spec = torch.fft.rfftn(x, dim=(-2, -1), norm="ortho")
        z = F.relu(F.conv2d(torch.cat([spec.real, spec.imag], 1), self.weight, self.bias))
        zr, zi = z[:, : self.c], z[:, self.c:]
        return torch.fft.irfftn(torch.complex(zr, zi), s=(h, w), dim=(-2, -1), norm="ortho")


class BlockLayer(nn.Module):
    """One FFC layer of a residual block (3x3, reflect padded), BN and ReLU on both branches."""

    def __init__(self, w, prefix, g):
        super().__init__()
        sl, bl = fold_bn(w, f"{prefix}.bn_l")
        sg, bg = fold_bn(w, f"{prefix}.bn_g")
        f = f"{prefix}.ffc"
        sp = f"{f}.convg2g"
        self.l2l = param(w[f"{f}.convl2l.weight"] * sl.view(-1, 1, 1, 1))
        self.g2l = param(w[f"{f}.convg2l.weight"] * sl.view(-1, 1, 1, 1))
        self.l2g = param(w[f"{f}.convl2g.weight"] * sg.view(-1, 1, 1, 1))
        self.bias_l = param(bl.clone())
        self.bias_g = param(bg.clone())
        s1, b1 = fold_bn(w, f"{sp}.conv1.1")
        self.conv1 = param(w[f"{sp}.conv1.0.weight"] * s1.view(-1, 1, 1, 1))
        self.bias1 = param(b1.clone())
        self.fu = FourierUnit(w, f"{sp}.fu", g // 2)
        self.conv2 = param(w[f"{sp}.conv2.weight"] * sg.view(-1, 1, 1, 1))

    def forward(self, xl, xg):
        pl = F.pad(xl, (1, 1, 1, 1), mode="reflect")
        pg = F.pad(xg, (1, 1, 1, 1), mode="reflect")
        out_l = F.relu(F.conv2d(pl, self.l2l) + F.conv2d(pg, self.g2l, self.bias_l))
        t = F.relu(F.conv2d(xg, self.conv1, self.bias1))
        spectral = F.conv2d(t + self.fu(t), self.conv2)
        out_g = F.relu(F.conv2d(pl, self.l2g, self.bias_g) + spectral)
        return out_l, out_g


class Up(nn.Module):
    def __init__(self, w, conv, bn):
        super().__init__()
        scale, shift = fold_bn(w, str(bn))
        self.weight = param(w[f"{conv}.weight"] * scale.view(1, -1, 1, 1))
        self.bias = param(w[f"{conv}.bias"] * scale + shift)

    def forward(self, x):
        return F.relu(F.conv_transpose2d(x, self.weight, self.bias, stride=2, padding=1, output_padding=1))


class Lama(nn.Module):
    """big-lama's generator by layer index, as in the checkpoint: 1 stem (reflect pad 3, 7x7), 2..4 downsampling
    (reflect pad 1, 3x3 stride 2; 4 makes the global branch), 5.. residual FFC blocks, then the join, three
    (ConvTranspose, BN, ReLU) triples, reflect pad, 7x7 head, sigmoid."""

    def __init__(self, w):
        super().__init__()
        ngf = w["1.ffc.convl2l.weight"].shape[0]
        glob = ngf * 8 * 3 // 4
        self.stem = ConvBN(w["1.ffc.convl2l.weight"], *fold_bn(w, "1.bn_l"), pad=3)
        self.downs = nn.ModuleList(
            ConvBN(w[f"{i + 2}.ffc.convl2l.weight"], *fold_bn(w, f"{i + 2}.bn_l"), stride=2, pad=1) for i in range(3)
        )
        self.down_g = ConvBN(w["4.ffc.convl2g.weight"], *fold_bn(w, "4.bn_g"), stride=2, pad=1)
        self.blocks = nn.ModuleList()
        i = 5
        while f"{i}.conv1.ffc.convl2l.weight" in w:
            self.blocks.append(nn.ModuleList([BlockLayer(w, f"{i}.conv1", glob), BlockLayer(w, f"{i}.conv2", glob)]))
            i += 1
        at = i + 1
        self.ups = nn.ModuleList()
        for _ in range(3):
            self.ups.append(Up(w, at, at + 1))
            at += 3
        self.head_w = param(w[f"{at + 1}.weight"])
        self.head_b = param(w[f"{at + 1}.bias"])

    def forward(self, x):
        """x (1, 4, h, w): the masked picture then the hole -> (1, 3, h, w) in 0..1."""
        x = self.stem(x)
        x = self.downs[0](x)
        x = self.downs[1](x)
        g = self.down_g(x)
        x = self.downs[2](x)
        for b1, b2 in self.blocks:
            l, gg = b1(x, g)
            l, gg = b2(l, gg)
            x, g = x + l, g + gg
        y = torch.cat([x, g], 1)
        for u in self.ups:
            y = u(y)
        y = F.conv2d(F.pad(y, (3, 3, 3, 3), mode="reflect"), self.head_w, self.head_b)
        return torch.sigmoid(y)


def check_input(side):
    """A made-up picture (smooth gradients and stripes) with a round hole in the middle, as the model takes it."""
    yy, xx = np.mgrid[0:side, 0:side].astype(np.float32) / side
    rgb = np.stack([0.3 + 0.5 * xx, 0.2 + 0.4 * yy, 0.5 + 0.3 * np.sin(12 * xx + 5 * yy)])
    hole = (((xx - 0.5) ** 2 + (yy - 0.5) ** 2) < 0.04).astype(np.float32)[None]
    return np.concatenate([rgb * (1 - hole), hole])[None].astype(np.float32), hole[0] > 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--models", required=True, type=pathlib.Path, help="the shared models folder (read only)")
    ap.add_argument("--out", required=True, type=pathlib.Path)
    ap.add_argument("--sides", default=",".join(map(str, SIDES)), help="the square input sides, multiples of 8")
    ap.add_argument("--install", action="store_true", help="also copy the .mlpackage into <models>/lama/coreml/")
    a = ap.parse_args()
    sides = sorted({int(s) for s in a.sides.split(",")})
    if not sides or any(s < 32 or s % 8 for s in sides):
        sys.exit("--sides: multiples of 8, at least 32")

    import coremltools as ct

    src = a.models / "lama" / SOURCE
    if digest(src) != SOURCE_SHA256:
        sys.exit(f"{src} is not the pinned {SOURCE}")
    model = Lama(load_generator(src)).eval()
    a.out.mkdir(parents=True, exist_ok=True)
    parts = {}
    for side in sides:
        t = time.time()
        example = torch.zeros(1, 4, side, side)
        with torch.no_grad():
            traced = torch.jit.trace(model, example)
        ml = ct.convert(
            traced,
            convert_to="mlprogram",
            inputs=[ct.TensorType(name="input", shape=example.shape, dtype=np.float32)],
            outputs=[ct.TensorType(name="fill", dtype=np.float32)],
            compute_precision=ct.precision.FLOAT16,
            minimum_deployment_target=ct.target.macOS15,
            compute_units=ct.ComputeUnit.CPU_AND_NE,
        )
        part = a.out / f"lama-{side}.mlpackage"
        shutil.rmtree(part, ignore_errors=True)
        ml.save(str(part))
        parts[side] = part
        print(f"side {side}: converted in {time.time() - t:.0f} s")

    desc = ct.utils.MultiFunctionDescriptor()
    for side, part in parts.items():
        desc.add_function(str(part), src_function_name="main", target_function_name=f"side{side}")
    desc.default_function_name = f"side{sides[-1]}"
    package = a.out / "lama.mlpackage"
    shutil.rmtree(package, ignore_errors=True)
    ct.utils.save_multifunction(desc, str(package))
    for part in parts.values():
        shutil.rmtree(part, ignore_errors=True)

    # Check each function against the float32 PyTorch network (on the GPU: the Neural Engine's first load takes
    # seconds per side; the Rust test then checks the app's path on the Neural Engine against candle).
    checks = {}
    for side in sides:
        x, hole = check_input(side)
        with torch.no_grad():
            ref = model(torch.from_numpy(x)).numpy()
        fn = ct.models.MLModel(str(package), function_name=f"side{side}", compute_units=ct.ComputeUnit.CPU_AND_GPU)
        t = time.time()
        got = np.asarray(fn.predict({"input": x})["fill"], np.float32)
        took = time.time() - t
        d = np.abs(got - ref)[0][:, hole] * 255
        checks[f"side{side}"] = {"holeMaxDiff255": round(float(d.max()), 2), "holeMeanDiff255": round(float(d.mean()), 3), "ms": round(took * 1e3)}
        print(f"side {side}: Core ML vs PyTorch fp32 in the hole (0..255): max {d.max():.2f}, mean {d.mean():.3f}")

    manifest = {
        "model": "lama",
        "package": package.name,
        "functions": [
            {"name": f"side{s}", "input": {"name": "input", "shape": [1, 4, s, s]}, "output": {"name": "fill", "shape": [1, 3, s, s]}}
            for s in sides
        ],
        "precision": "float16",
        "source": {"file": SOURCE, "sha256": SOURCE_SHA256},
        "license": LICENSE,
        "files": files_of(package),
        "check": checks,
        "tools": {
            "python": platform.python_version(),
            "torch": torch.__version__,
            "coremltools": ct.__version__,
            "numpy": np.__version__,
            "macos": platform.mac_ver()[0],
            "machine": platform.machine(),
        },
    }
    (a.out / "lama-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    if a.install:
        dest = a.models / "lama" / "coreml" / package.name
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.rmtree(dest, ignore_errors=True)
        shutil.copytree(package, dest)
        print(f"installed {dest}")


if __name__ == "__main__":
    main()
