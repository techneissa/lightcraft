#!/usr/bin/env python3
"""Convert the people-parsing SegFormers (face parser B0, clothes parser B3) to Core ML models (run on the GPU).

Run once on a Mac, by hand (issue #105, fork/coreml.md, fork/people-parsing.md). The app never runs Python: it finds
the `.mlpackage` this writes in the shared models folder, compiles it once with Core ML and runs it through
`crates/coreml`. Convert from the `model.safetensors` already in that folder (read only); never download the weights
a second time. The candle port in `crates/parsing/src/segformer.rs` stays the reference; this network is the same
one, written again in PyTorch from the same published tensor names, with the same layer norms (eps 1e-6), the same
batch norm fold and the same steps after the network, so both give the same masks.

    uv venv -p 3.12 venv && VIRTUAL_ENV=venv uv pip install -r tools/requirements-coreml-convert.txt
    M=~/Library/Application\\ Support/ArtCraft/models
    venv/bin/python tools/convert-coreml-parsing.py --model segformer-b0 --models "$M" --out out/ --install
    venv/bin/python tools/convert-coreml-parsing.py --model segformer-b3 --models "$M" --out out/ --install

The face parser (`face-parsing-segformer-b0.mlpackage`) takes `pixels` [B, 3, 512, 512] (ImageNet-normalised face
crops, B = 1 to 8) and gives `groups` [B, 10, 512, 512]: the class probabilities after candle's bilinear resize and
softmax, summed into the ten face groups of `crates/parsing/src/face.rs` (Person, Skin, Neck, Hair, Brows, Eyes, Lips,
Mouth, Hat, Others).

The clothes parser (`human-parsing-segformer-b3.mlpackage`) takes `pixels` [1, 4, H, W]: the ImageNet-normalised
photo in the top-left corner of a fixed canvas (one of `BODY_CANVASES`), and a fourth channel that is 1 over the
photo and 0 over the padding. It gives `probs` [1, 18, H, W], the class probabilities over the canvas; the top-left
part the photo covers equals candle's run on the photo alone. Every layer that looks at neighbours is told where the
photo ends: convolutions see zeros past it (as candle's zero padding), attention ignores the padding's keys, and the
decoder's bilinear resizes clamp at the photo's edge (a resize of the masked map divided by the resize of the mask).
The canvas sides and the photo's sides are multiples of 32, so every stride lines up with the photo's edge.

Writes `out/<model>.mlpackage`, `out/<model>.mlmodelc` (compiled, for a quick local check) and
`out/<model>-manifest.json` (each file's size and SHA-256, the tool versions). `--install` also copies the
`.mlpackage` into `<models>/<model>/coreml/`.
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
from safetensors.torch import load_file

HEADS = [1, 2, 5, 8]
SR = [8, 4, 2, 1]
PATCH = [7, 3, 3, 3]
STRIDE = [4, 2, 2, 2]
LN_EPS = 1e-6
BN_EPS = 1e-5
# Added to the attention scores of the padding's keys: exp() of it is 0 in float16.
MASKED = -1e4

FACE_SIDE = 512
FACE_BATCHES = list(range(1, 9))
# CelebAMask-HQ classes in the SegFormer fine-tune's order, and the groups of `face.rs` they add to.
FACE_NAMES = [
    "background", "hair", "hat", "ear_r", "skin", "l_brow", "r_brow", "l_eye", "r_eye", "eye_g",
    "nose", "u_lip", "l_lip", "mouth", "cloth", "neck", "neck_l", "l_ear", "r_ear",
]
FACE_GROUPS = ["person", "skin", "neck", "hair", "brows", "eyes", "lips", "mouth", "hat", "others"]
GROUP_OF = {
    "skin": "skin", "nose": "skin", "l_ear": "skin", "r_ear": "skin", "neck": "neck", "hair": "hair",
    "l_brow": "brows", "r_brow": "brows", "l_eye": "eyes", "r_eye": "eyes", "u_lip": "lips", "l_lip": "lips",
    "mouth": "mouth", "hat": "hat", "eye_g": "others", "ear_r": "others", "neck_l": "others",
}

# (height, width) canvases of the clothes parser. A run picks the smallest that holds the photo's grid
# (`crates/parsing/src/body.rs` `grid`: long side 768 to 1536, both sides multiples of 32).
def up32(v):
    return -(-v // 32) * 32


BODY_CANVASES = []
for long in (768, 1024, 1280, 1536):
    for short in sorted({long, up32(long * 3 // 4), up32(long * 2 // 3), long // 2}, reverse=True):
        BODY_CANVASES.append((short, long))
        if short != long:
            BODY_CANVASES.append((long, short))

MODELS = {
    "segformer-b0": {
        "folder": "face-parsing-segformer-b0",
        "sha256": "420a3e21e4ee9eea8e912c06ef3b9ca683c67570c1908aa837915751666665de",
        "classes": 19,
        "kind": "face",
        "license": "NVIDIA Source Code License for SegFormer (non-commercial); fine-tuned on CelebAMask-HQ (non-commercial research)",
    },
    "segformer-b3": {
        "folder": "human-parsing-segformer-b3",
        "sha256": "f70ae566c5773fb335796ebaa8acc924ac25eb97222c2b2967d44d2fc11568e6",
        "classes": 18,
        "kind": "body",
        "license": "MIT per the model card (sayeed99/segformer_b3_clothes); MiT-b3 encoder under the NVIDIA Source Code License for SegFormer (non-commercial)",
    },
    # the BiSeNet set's clothes parser: the same network with fewer blocks per stage (read from the weights)
    "segformer-b2": {
        "folder": "human-parsing-segformer-b2",
        "sha256": "8f86fd90c567afd4370b3cc3a7e81ed767a632b2832a738331af660acc0c4c68",
        "classes": 18,
        "kind": "body",
        "license": "MIT per the model card (mattmdjaga/segformer_b2_clothes); MiT-b2 encoder under the NVIDIA Source Code License for SegFormer (non-commercial)",
    },
}


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def linear(w, name):
    m = torch.nn.Linear(w[name + ".weight"].shape[1], w[name + ".weight"].shape[0])
    m.weight.data, m.bias.data = w[name + ".weight"].float(), w[name + ".bias"].float()
    return m


def layer_norm(w, name):
    m = torch.nn.LayerNorm(w[name + ".weight"].shape[0], eps=LN_EPS)
    m.weight.data, m.bias.data = w[name + ".weight"].float(), w[name + ".bias"].float()
    return m


def conv(w, name, stride=1, padding=0, groups=1):
    k = w[name + ".weight"].float()
    m = torch.nn.Conv2d(k.shape[1] * groups, k.shape[0], k.shape[2], stride=stride, padding=padding, groups=groups)
    m.weight.data, m.bias.data = k, w[name + ".bias"].float()
    return m


def pool(mask, k):
    """The mask at a `k` times coarser grid (exact: the photo's edge lines up with every window)."""
    return mask if k == 1 else F.max_pool2d(mask, k, k)


def resize_masked(x, mask, scale):
    """candle's bilinear resize (half-pixel centres, edge clamped) of the photo part of `x`, `scale` times larger:
    resizing the masked map and dividing by the resized mask clamps at the photo's edge, as resizing the photo alone
    would. `mask` None: the whole canvas is the photo."""
    up = lambda t: F.interpolate(t, scale_factor=scale, mode="bilinear", align_corners=False)
    if mask is None:
        return up(x)
    return up(x * mask) / up(mask).clamp(min=1e-2)


class Block(torch.nn.Module):
    def __init__(self, w, p, stage, c):
        super().__init__()
        a = p + "attention.self."
        self.ln1 = layer_norm(w, p + "layer_norm_1")
        self.q, self.k, self.v = linear(w, a + "query"), linear(w, a + "key"), linear(w, a + "value")
        self.out = linear(w, p + "attention.output.dense")
        self.sr_k = SR[stage]
        if self.sr_k > 1:
            self.sr = conv(w, a + "sr", stride=self.sr_k)
            self.sr_ln = layer_norm(w, a + "layer_norm")
        self.heads, self.c = HEADS[stage], c
        self.d = c // self.heads
        self.scale = float(self.d) ** -0.5
        self.ln2 = layer_norm(w, p + "layer_norm_2")
        self.fc1 = linear(w, p + "mlp.dense1")
        self.dw = conv(w, p + "mlp.dwconv.dwconv", padding=1, groups=4 * c)
        self.fc2 = linear(w, p + "mlp.dense2")

    def attention(self, x, h, w, mask):
        # sizes known when the model is built are Python numbers, so the traced graph holds no casts of them
        b, n = x.shape[0], x.shape[1]
        split = lambda t: t.reshape(b, -1, self.heads, self.d).transpose(1, 2)
        q = split(self.q(x))
        if self.sr_k > 1:
            img = x.transpose(1, 2).reshape(b, self.c, h, w)
            kv = self.sr_ln(self.sr(img).flatten(2).transpose(1, 2))
        else:
            kv = x
        k, v = split(self.k(kv)), split(self.v(kv))
        scores = (q @ k.transpose(-1, -2)) * self.scale
        if mask is not None:
            keys = pool(mask, self.sr_k).flatten(2).unsqueeze(1)  # B x 1 x 1 x M
            scores = scores + (1.0 - keys) * MASKED
        ctx = (torch.softmax(scores, dim=-1) @ v).transpose(1, 2).reshape(b, n, self.c)
        return self.out(ctx)

    def forward(self, x, h, w, mask):
        x = x + self.attention(self.ln1(x), h, w, mask)
        y = self.fc1(self.ln2(x))
        img = y.transpose(1, 2).reshape(y.shape[0], 4 * self.c, h, w)
        if mask is not None:
            img = img * mask  # the depthwise conv sees zeros past the photo
        y = F.gelu(self.dw(img).flatten(2).transpose(1, 2))
        return x + self.fc2(y)


class Segformer(torch.nn.Module):
    """`SegformerForSemanticSegmentation` as `segformer.rs` reads it; `masked`: the photo sits in a canvas (4th input
    channel = where it is)."""

    def __init__(self, w, masked):
        super().__init__()
        self.masked = masked
        enc = "segformer.encoder."
        self.embeds, self.embed_lns, self.stages, self.lns = (torch.nn.ModuleList() for _ in range(4))
        for i in range(4):
            pe = f"{enc}patch_embeddings.{i}."
            c = w[pe + "proj.weight"].shape[0]
            self.embeds.append(conv(w, pe + "proj", stride=STRIDE[i], padding=PATCH[i] // 2))
            self.embed_lns.append(layer_norm(w, pe + "layer_norm"))
            depth = 0
            while f"{enc}block.{i}.{depth}.layer_norm_1.weight" in w:
                depth += 1
            self.stages.append(torch.nn.ModuleList(Block(w, f"{enc}block.{i}.{j}.", i, c) for j in range(depth)))
            self.lns.append(layer_norm(w, f"{enc}layer_norm.{i}"))
        head = "decode_head."
        proj = "linear_c" if head + "linear_c.0.proj.weight" in w else "linear_projections"
        self.proj = torch.nn.ModuleList(linear(w, f"{head}{proj}.{i}.proj") for i in range(4))
        # linear_fuse (no bias) with its batch norm folded in, as nn.rs does
        fw = w[head + "linear_fuse.weight"].float()
        g, beta = w[head + "batch_norm.weight"].float(), w[head + "batch_norm.bias"].float()
        mean, var = w[head + "batch_norm.running_mean"].float(), w[head + "batch_norm.running_var"].float()
        scale = g / torch.sqrt(var + BN_EPS)
        self.fuse = torch.nn.Conv2d(fw.shape[1], fw.shape[0], 1)
        self.fuse.weight.data = fw * scale.reshape(-1, 1, 1, 1)
        self.fuse.bias.data = -mean * scale + beta
        self.classifier = conv(w, head + "classifier")

    def forward(self, x):
        """Logits at a quarter of the input's size, and the mask there (None when not masked)."""
        mask = None
        if self.masked:
            mask0 = x[:, 3:4]
            x = x[:, :3] * mask0
        maps, masks = [], []
        for i in range(4):
            if self.masked:
                mask = pool(mask0, 2 ** (i + 2))
            e = self.embeds[i](x)
            b, c, h, w = e.shape
            t = self.embed_lns[i](e.flatten(2).transpose(1, 2))
            for blk in self.stages[i]:
                t = blk(t, h, w, mask)
            x = self.lns[i](t).transpose(1, 2).reshape(b, c, h, w)
            if mask is not None:
                x = x * mask  # the next patch embedding sees zeros past the photo
            maps.append(x)
            masks.append(mask)
        ups = []
        for i in (3, 2, 1, 0):
            m = maps[i]
            b, c, h, w = m.shape
            t = self.proj[i](m.flatten(2).transpose(1, 2)).transpose(1, 2).reshape(b, -1, h, w)
            ups.append(t if i == 0 else resize_masked(t, masks[i], 2 ** i))
        fused = torch.relu(self.fuse(torch.cat(ups, 1)))
        return self.classifier(fused), masks[0]


class FaceParser(torch.nn.Module):
    """Face crops -> the ten face groups' probabilities at 512 x 512 (`face.rs` `parse_batch`)."""

    def __init__(self, w):
        super().__init__()
        self.net = Segformer(w, masked=False)
        m = torch.zeros(len(FACE_GROUPS), len(FACE_NAMES))
        for c, name in enumerate(FACE_NAMES):
            if name != "background":
                m[FACE_GROUPS.index("person"), c] = 1.0
            if name in GROUP_OF:
                m[FACE_GROUPS.index(GROUP_OF[name]), c] = 1.0
        self.register_buffer("groups", m.reshape(len(FACE_GROUPS), len(FACE_NAMES), 1, 1))

    def forward(self, pixels):
        logits, _ = self.net(pixels)
        probs = torch.softmax(resize_masked(logits, None, 4), dim=1)
        return F.conv2d(probs, self.groups)


class BodyParser(torch.nn.Module):
    """A photo in a canvas -> the clothes classes' probabilities over the canvas (`body.rs` `run`)."""

    def __init__(self, w):
        super().__init__()
        self.net = Segformer(w, masked=True)

    def forward(self, pixels):
        logits, mask = self.net(pixels)
        return torch.softmax(resize_masked(logits, mask, 4), dim=1)


def files_of(package):
    return [
        {"path": str(p.relative_to(package)), "bytes": p.stat().st_size, "sha256": digest(p)}
        for p in sorted(package.rglob("*"))
        if p.is_file()
    ]


def body_input(h, w, ph, pw, seed):
    x = torch.zeros(1, 4, h, w)
    x[:, :3, :ph, :pw] = torch.randn(1, 3, ph, pw, generator=torch.Generator().manual_seed(seed))
    x[:, 3, :ph, :pw] = 1.0
    return x


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--model", required=True, choices=sorted(MODELS))
    ap.add_argument("--models", required=True, type=pathlib.Path, help="the shared models folder (read only)")
    ap.add_argument("--out", required=True, type=pathlib.Path)
    ap.add_argument("--install", action="store_true", help="also copy the .mlpackage into <models>/<model>/coreml/")
    a = ap.parse_args()

    import coremltools as ct

    spec = MODELS[a.model]
    name = spec["folder"]
    weights = a.models / name / "model.safetensors"
    if digest(weights) != spec["sha256"]:
        sys.exit(f"{weights} is not the pinned {a.model} model.safetensors")
    w = load_file(str(weights))
    if spec["kind"] == "face":
        model = FaceParser(w).eval()
        example = torch.zeros(1, 3, FACE_SIDE, FACE_SIDE)
        shapes = ct.EnumeratedShapes(shapes=[[b, 3, FACE_SIDE, FACE_SIDE] for b in FACE_BATCHES], default=[1, 3, FACE_SIDE, FACE_SIDE])
        output = "groups"
        # the GPU, as the app loads it: as fast as the Neural Engine (11 against 12 ms a face on an M4 Pro) and about
        # ten times closer to float32
        units = ct.ComputeUnit.CPU_AND_GPU
    else:
        model = BodyParser(w).eval()
        h0, w0 = BODY_CANVASES[0]
        example = body_input(h0, w0, h0, w0, 1)
        shapes = ct.EnumeratedShapes(shapes=[[1, 4, h, w] for h, w in BODY_CANVASES], default=[1, 4, h0, w0])
        output = "probs"
        # the GPU: the Neural Engine compiler takes over 15 minutes for one large canvas (its first attention is
        # 98,304 x 1,536), and the app loads this model on the GPU too
        units = ct.ComputeUnit.CPU_AND_GPU
    with torch.no_grad():
        traced = torch.jit.trace(model, example)
    ml = ct.convert(
        traced,
        convert_to="mlprogram",
        inputs=[ct.TensorType(name="pixels", shape=shapes, dtype=np.float32)],
        outputs=[ct.TensorType(name=output, dtype=np.float16)],
        compute_precision=ct.precision.FLOAT16,
        minimum_deployment_target=ct.target.macOS13,
        compute_units=units,
    )
    ml.short_description = f"{a.model} people parser for LightCraft (fork/people-parsing.md)"
    ml.license = spec["license"]
    a.out.mkdir(parents=True, exist_ok=True)
    package = a.out / f"{name}.mlpackage"
    compiled = a.out / f"{name}.mlmodelc"
    for old in (package, compiled):
        shutil.rmtree(old, ignore_errors=True)
    ml.save(str(package))
    shutil.move(str(ct.utils.compile_model(str(package))), str(compiled))

    # Check against the float32 PyTorch reference (what the Rust test then checks against candle).
    if spec["kind"] == "face":
        checks = [torch.randn(2, 3, FACE_SIDE, FACE_SIDE, generator=torch.Generator().manual_seed(7))]
        valid = [None]
    else:
        # a photo grid inside a larger canvas: only its part is compared, to the photo run alone
        ph, pw = 512, 704
        ch, cw = next((h, w) for h, w in sorted(BODY_CANVASES, key=lambda s: s[0] * s[1]) if h >= ph and w >= pw)
        checks = [body_input(ch, cw, ph, pw, 7)]
        valid = [(ph, pw)]
    report = []
    for x, v in zip(checks, valid):
        with torch.no_grad():
            if v is None:
                ref = model(x).numpy()
            else:
                ref = model(body_input(*v, *v, 7)).numpy()
        t = time.time()
        got = ml.predict({"pixels": x.numpy()})[output].astype(np.float32)
        took = time.time() - t
        if v is not None:
            got = got[:, :, : v[0], : v[1]]
        agree = float((ref.argmax(1) == got.argmax(1)).mean())
        diff = float(np.abs(ref - got).max())
        print(f"Core ML vs PyTorch fp32 {list(x.shape)}: argmax agreement {agree:.5f}, max |diff| {diff:.4f}, {took * 1e3:.0f} ms")
        report.append({"shape": list(x.shape), "argmaxAgreement": round(agree, 5), "maxDiff": round(diff, 4)})

    manifest = {
        "model": name,
        "package": package.name,
        "input": {"name": "pixels", "shapes": [list(s.shape) for s in shapes.shapes]},
        "output": {"name": output},
        "precision": "float16",
        "source": {"file": "model.safetensors", "sha256": spec["sha256"]},
        "license": spec["license"],
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
    (a.out / f"{name}-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest["files"], indent=2))
    if a.install:
        dest = a.models / name / "coreml" / package.name
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.rmtree(dest, ignore_errors=True)
        shutil.copytree(package, dest)
        print(f"installed {dest}")


if __name__ == "__main__":
    main()
