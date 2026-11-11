#!/usr/bin/env python3
"""Convert Sapiens2-seg 0.4B (the Sapiens2 people-parts set's body-part parser) to two Core ML models.

Run once on a Mac, by hand (issue #216, fork/coreml.md). The app never runs Python: it finds the two `.mlpackage`s
this writes in the shared models folder, compiles them once with Core ML and runs them through `crates/coreml`.
Convert from the `model.safetensors` already in that folder (read only); never download the weights a second time.
The candle port in `crates/parsing/src/sapiens.rs` stays the reference; this is the same network written again in
PyTorch from the same tensor names (MLX layout: kernels out, kh, kw, in).

    uv venv -p 3.12 venv && VIRTUAL_ENV=venv uv pip install -r tools/requirements-coreml-convert.txt
    M=~/Library/Application\\ Support/ArtCraft/models
    venv/bin/python -I tools/convert-coreml-sapiens2.py --models "$M" --out out/ --install

Core ML plans a model again whenever its input shape changes (issue #200), and Sapiens2 runs on a box around each
person, each of another size. So each of the two models takes one fixed shape, and a call holds several people:

* `body-parsing-sapiens2-0.4b-backbone.mlpackage`, the transformer, takes `pixels` [1, TOKENS, 906]: one row per
  token, of up to 9 passes packed one after another. A row is the patch's 16 x 16 x 3 ImageNet-normalised pixels
  (row, column, channel), then 9 channels choosing one of the class and storage tokens instead (one-hot; all 0 for a
  patch), then the rotary table's sin and cos for the patch (64 each; 0 and 1 for the class and storage tokens, which
  are not turned), then the pass's number. A token attends only to the tokens of its own pass, so each pass gives what
  it gives alone; rows past the last pass are padding with a number of their own. It gives `tokens` [1, TOKENS, 1024]
  after the last norm. Float16, on the Neural Engine.
* `body-parsing-sapiens2-0.4b-head-<H>x<W>.mlpackage`, the decoder head, one for each canvas of `HEAD_CANVASES`
  (H x W patches; a call takes the smallest that holds its passes), takes `pixels` [1, 1024 + 9, H, W]:
  the passes' patch features laid out on a canvas of patches, each pass's grid in its own rectangle with at least one
  empty patch between two of them, then 9 channels marking which pass each patch belongs to (one-hot; all 0 for empty
  patches). Every instance norm takes each pass's own mean and variance, and every layer sees zeros past a pass's
  edge, so each pass's part of the canvas equals the head run on that pass alone. It gives `logits`
  [1, 29, 16 H, 16 W]. Float32 on the GPU: the instance norms sum over up to 1.5 million pixels, and float16 was no
  faster. Its time follows the canvas (on an M4 Pro under load: 200 ms for 48 x 48, 540 ms for 96 x 64), so most
  photos (a whole photo's pass of 3:2 either way round is 48 x 32) take the small one.

Writes `out/<model>.mlpackage`, `out/<model>.mlmodelc` (compiled, for a quick local check) and
`out/<model>-manifest.json` (each file's size and SHA-256, the check, the tool versions). `--install` also copies the
`.mlpackage`s into `<models>/body-parsing-sapiens2-0.4b/coreml/`.

The Neural Engine was tried for the backbone and dropped: 1.43 s a call against 0.75 s on the GPU, and 97 % of pixels
the same class as float32 against 99.9 % (its float16 norms and attention drift).
"""

import argparse
import hashlib
import json
import math
import pathlib
import platform
import shutil
import sys
import time

import numpy as np
import torch
import torch.nn.functional as F
from safetensors.torch import load_file

FOLDER = "body-parsing-sapiens2-0.4b"
SHA256 = "b387d5de2fb0e4bec87671956ba4836de1c91f02cd2aea0118c2fa149c1e0948"
LICENSE = "Sapiens2 License (Meta): use and redistribution with the licence; forbids surveillance, biometric processing and identifying people"
PATCH = 16
HEAD_DIM = 64
PREFIX = 9  # the class token and 8 storage tokens
# Tokens a backbone call holds: the largest pass (`crops.rs` `input_size`: at most 1.05 x 1024 x 768 / 256 = 3225
# patches) with its prefix, rounded up to a multiple of 64.
TOKENS = 3264
# The head's canvases in patches (height, width), smallest first (`crates/parsing/src/coreml/sapiens.rs` keeps the
# same): one that holds a whole photo's pass of 3:2 either way round, and a tall one for the boxes around people.
HEAD_CANVASES = [(48, 48), (96, 64)]
MAX_PASSES = 9
PX = PATCH * PATCH * 3
ROW = PX + PREFIX + 2 * HEAD_DIM + 1
EPS = 1e-6
IN_EPS = 1e-5
# Added to the attention scores of another pass's keys: exp() of it is 0 in float16.
MASKED = -1e4


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def rms(x, w):
    """candle's rms_norm (x / sqrt(mean(x^2) + eps) * w), computed on x divided by its largest value so the squares
    never overflow float16 (a ViT's residual stream holds values in the hundreds)."""
    s = x.abs().amax(-1, keepdim=True).clamp(min=1e-2)
    xn = x / s
    return xn * torch.rsqrt((xn * xn).mean(-1, keepdim=True) + EPS / (s * s)) * w


def rotate(x, sin, cos):
    d = HEAD_DIM // 2
    return x * cos + torch.cat([-x[..., d:], x[..., :d]], -1) * sin


class Block(torch.nn.Module):
    def __init__(self, w, p, c):
        super().__init__()
        a = p + "attn."
        f = lambda n: w[n].float()
        self.ln1, self.ln2 = f(p + "ln1.weight"), f(p + "ln2.weight")
        self.q_norm, self.k_norm = f(a + "q_norm.weight"), f(a + "k_norm.weight")
        lin = lambda n: (f(n + ".weight"), f(n + ".bias"))
        self.wq, self.wk, self.wv, self.proj = lin(a + "wq"), lin(a + "wk"), lin(a + "wv"), lin(a + "proj")
        self.gamma = f(a + "gamma.weight") if a + "gamma.weight" in w else None
        self.w12, self.w3 = lin(p + "ffn.w12"), lin(p + "ffn.w3")
        self.hidden = self.w12[0].shape[0] // 2
        self.heads = c // HEAD_DIM
        self.kv = self.wk[0].shape[0] // HEAD_DIM
        self.c = c

    def attention(self, x, sin, cos, bias):
        # sizes are Python numbers known when the model is built (-1 for the tokens), so the traced graph holds no
        # casts of shapes, which the converter can't take
        split = lambda t, h: t.reshape(1, -1, h, HEAD_DIM).transpose(1, 2)
        q = rms(split(F.linear(x, *self.wq), self.heads), self.q_norm)
        k = rms(split(F.linear(x, *self.wk), self.kv), self.k_norm)
        v = split(F.linear(x, *self.wv), self.kv)
        if self.kv != self.heads:
            r = self.heads // self.kv
            # each key/value head serves `r` neighbouring query heads (PyTorch's repeat_interleave)
            k = k[:, :, None].expand(-1, -1, r, -1, -1).reshape(1, self.heads, -1, HEAD_DIM)
            v = v[:, :, None].expand(-1, -1, r, -1, -1).reshape(1, self.heads, -1, HEAD_DIM)
        q, k = rotate(q, sin, cos), rotate(k, sin, cos)
        scores = (q @ k.transpose(-1, -2)) * (HEAD_DIM ** -0.5)
        if bias is not None:
            scores = scores + bias
        ctx = (torch.softmax(scores, -1) @ v).transpose(1, 2).reshape(1, -1, self.c)
        out = F.linear(ctx, *self.proj)
        return out * self.gamma if self.gamma is not None else out

    def forward(self, x, sin, cos, bias):
        x = x + self.attention(rms(x, self.ln1), sin, cos, bias)
        y = F.linear(rms(x, self.ln2), *self.w12)
        h = self.hidden
        return x + F.linear(F.silu(y[..., :h]) * y[..., h:], *self.w3)


class Backbone(torch.nn.Module):
    """The packed transformer: `pixels` [1, TOKENS, ROW] -> tokens [1, TOKENS, C]."""

    def __init__(self, w):
        super().__init__()
        bb = "backbone."
        k = w[bb + "patch_embed.projection.weight"].float()  # c, 16, 16, 3
        self.c = k.shape[0]
        self.patch_w = k.reshape(self.c, PX)
        self.patch_b = w[bb + "patch_embed.projection.bias"].float()
        self.prefix = torch.cat([w[bb + "cls_token"].float(), w[bb + "storage_tokens"].float()], 1)[0]  # 9 x c
        assert self.prefix.shape[0] == PREFIX
        depth = 0
        while f"{bb}blocks.{depth}.ln1.weight" in w:
            depth += 1
        self.blocks = torch.nn.ModuleList(Block(w, f"{bb}blocks.{i}.", self.c) for i in range(depth))
        self.ln = w[bb + "ln1.weight"].float()

    def forward(self, pixels):
        px = pixels[..., :PX]
        pre = pixels[..., PX : PX + PREFIX]
        sin = pixels[..., PX + PREFIX : PX + PREFIX + HEAD_DIM].unsqueeze(1)
        cos = pixels[..., PX + PREFIX + HEAD_DIM : PX + PREFIX + 2 * HEAD_DIM].unsqueeze(1)
        seg = pixels[..., ROW - 1 :]  # 1 x N x 1
        is_patch = 1.0 - pre.sum(-1, keepdim=True)
        x = F.linear(px, self.patch_w, self.patch_b) * is_patch + pre @ self.prefix
        bias = (seg - seg.transpose(1, 2)).abs().clamp(max=1.0).unsqueeze(1) * MASKED
        for blk in self.blocks:
            x = blk(x, sin, cos, bias)
        return rms(x, self.ln)


def seg_norm(y, m, c, h, w):
    """Instance norm (no scale, PyTorch's eps) of each pass on its own: `y` [1, c, h, w], `m` [1, S, h, w] marks the
    passes' pixels. Pixels of no pass come out 0."""
    yf, mf = y.reshape(c, h * w), m.reshape(MAX_PASSES, h * w)
    n = mf.sum(-1).clamp(min=1.0)
    mean = (yf @ mf.t()) / n
    d = yf - mean @ mf
    var = ((d * d) @ mf.t()) / n
    return (d * (torch.rsqrt(var + IN_EPS) @ mf)).reshape(1, c, h, w)


class Head(torch.nn.Module):
    """The decoder head on a canvas of passes: [1, C + 9, H, W] -> logits [1, 29, 16 H, 16 W]."""

    def __init__(self, w, c, size):
        super().__init__()
        h = "decode_head."
        self.c = c
        self.size = size
        self.deconvs = []
        while f"{h}deconv_layers.{len(self.deconvs) * 3}.weight" in w:
            k = w[f"{h}deconv_layers.{len(self.deconvs) * 3}.weight"].float()  # out, 4, 4, in
            self.deconvs.append(k.permute(3, 0, 1, 2).contiguous())  # in, out, kh, kw
        self.convs = []
        while f"{h}conv_layers.{len(self.convs) * 3}.weight" in w:
            n = f"{h}conv_layers.{len(self.convs) * 3}"
            self.convs.append((w[n + ".weight"].float().permute(0, 3, 1, 2).contiguous(), w[n + ".bias"].float()))
        self.deconv_out = [int(k.shape[1]) for k in self.deconvs]
        self.conv_out = [int(k.shape[0]) for k, _ in self.convs]
        self.seg = (w[h + "conv_seg.weight"].float().permute(0, 3, 1, 2).contiguous(), w[h + "conv_seg.bias"].float())

    def forward(self, pixels):
        y = pixels[:, : self.c]
        m = pixels[:, self.c :]
        y = y * m.sum(1, keepdim=True)
        # (the canvas's size is fixed, so every size here is a Python number)
        h, w = self.size
        for k, c in zip(self.deconvs, self.deconv_out):
            y = F.conv_transpose2d(y, k, stride=2, padding=1)
            h, w = 2 * h, 2 * w
            # each mask pixel to 2 x 2 (a nearest resize to a fixed size; a scale factor traces to shape casts)
            m = m.reshape(MAX_PASSES, h // 2, 1, w // 2, 1).expand(-1, -1, 2, -1, 2).reshape(1, MAX_PASSES, h, w)
            y = F.silu(seg_norm(y, m, c, h, w))
        for (k, b), c in zip(self.convs, self.conv_out):
            y = F.silu(seg_norm(F.conv2d(y, k, b), m, c, h, w))
        return F.conv2d(y, *self.seg)


# ---- the reference: one pass alone, as sapiens.rs runs it ----


def bf16(x):
    return torch.tensor(np.float32(x)).to(torch.bfloat16).float().item()


def rope_table(periods, h, w):
    """`sapiens.rs` `rope_table`: (sin, cos), each h*w x 64, rounded to bf16 at each step as trained."""
    q = len(periods)
    d = q * 4
    sin, cos = np.zeros((h * w, d), np.float32), np.zeros((h * w, d), np.float32)
    two_pi = bf16(2.0 * math.pi)
    coord = lambda i, n: bf16(2.0 * bf16(bf16(i + 0.5) / n) - 1.0)
    for y in range(h):
        for x in range(w):
            row = y * w + x
            for axis, c in enumerate((coord(y, h), coord(x, w))):
                for k, p in enumerate(periods):
                    a = bf16(bf16(np.float32(two_pi) * np.float32(c)) / np.float32(p))
                    s, co = bf16(math.sin(np.float32(a))), bf16(math.cos(np.float32(a)))
                    for half in range(2):
                        at = half * (d // 2) + axis * q + k
                        sin[row, at], cos[row, at] = s, co
    return sin, cos


def instance_norm(y):
    mean = y.mean((2, 3), keepdim=True)
    var = ((y - mean) ** 2).mean((2, 3), keepdim=True)
    return (y - mean) / torch.sqrt(var + IN_EPS)


def reference(bb, head, x, periods):
    """Logits [1, 29, H, W] for one normalised [1, 3, H, W] input, in float32, as `Sapiens2::forward`."""
    _, _, h, w = x.shape
    gh, gw = h // PATCH, w // PATCH
    patches = x.reshape(1, 3, gh, PATCH, gw, PATCH).permute(0, 2, 4, 3, 5, 1).reshape(1, gh * gw, PX)
    t = torch.cat([bb.prefix[None], F.linear(patches, bb.patch_w, bb.patch_b)], 1)
    s, c = rope_table(periods, gh, gw)
    sin = torch.cat([torch.zeros(PREFIX, HEAD_DIM), torch.from_numpy(s)])[None, None]
    cos = torch.cat([torch.ones(PREFIX, HEAD_DIM), torch.from_numpy(c)])[None, None]
    for blk in bb.blocks:
        t = blk(t, sin, cos, None)
    t = rms(t, bb.ln)
    y = t[:, PREFIX:].transpose(1, 2).reshape(1, bb.c, gh, gw)
    for k in head.deconvs:
        y = F.silu(instance_norm(F.conv_transpose2d(y, k, stride=2, padding=1)))
    for k, b in head.convs:
        y = F.silu(instance_norm(F.conv2d(y, k, b)))
    return F.conv2d(y, *head.seg)


def pack(xs, periods):
    """The backbone's input for passes `xs` (each [1, 3, H, W]) and where each one's patches are in it."""
    rows = np.zeros((1, TOKENS, ROW), np.float32)
    rows[0, :, ROW - 1] = 1000.0  # padding: a pass of its own
    rows[0, :, PX + PREFIX + HEAD_DIM : PX + PREFIX + 2 * HEAD_DIM] = 1.0
    at, spans = 0, []
    for k, x in enumerate(xs):
        _, _, h, w = x.shape
        gh, gw = h // PATCH, w // PATCH
        for j in range(PREFIX):
            rows[0, at + j, PX + j] = 1.0
        rows[0, at : at + PREFIX + gh * gw, ROW - 1] = k
        p = x.reshape(1, 3, gh, PATCH, gw, PATCH).permute(0, 2, 4, 3, 5, 1).reshape(gh * gw, PX).numpy()
        s, c = rope_table(periods, gh, gw)
        b = at + PREFIX
        rows[0, b : b + gh * gw, :PX] = p
        rows[0, b : b + gh * gw, PX + PREFIX : PX + PREFIX + HEAD_DIM] = s
        rows[0, b : b + gh * gw, PX + PREFIX + HEAD_DIM : PX + PREFIX + 2 * HEAD_DIM] = c
        spans.append((b, gh, gw))
        at = b + gh * gw
    assert at <= TOKENS
    return rows, spans


def canvas(tokens, spans, places, c, size):
    """The head's input: each pass's tokens at its place (row, column) on a canvas of `size` patches."""
    out = np.zeros((1, c + MAX_PASSES, *size), np.float32)
    for k, ((b, gh, gw), (r, col)) in enumerate(zip(spans, places)):
        out[0, :c, r : r + gh, col : col + gw] = tokens[0, b : b + gh * gw].T.reshape(c, gh, gw)
        out[0, c + k, r : r + gh, col : col + gw] = 1.0
    return out


def files_of(package):
    return [
        {"path": str(p.relative_to(package)), "bytes": p.stat().st_size, "sha256": digest(p)}
        for p in sorted(package.rglob("*"))
        if p.is_file()
    ]


def save(ml, out, name):
    package, compiled = out / f"{name}.mlpackage", out / f"{name}.mlmodelc"
    for old in (package, compiled):
        shutil.rmtree(old, ignore_errors=True)
    ml.save(str(package))
    import coremltools as ct

    shutil.move(str(ct.utils.compile_model(str(package))), str(compiled))
    return package


def photo_like(h, w, seed):
    """A smooth made-up picture (normalised), closer to a photo than noise."""
    g = torch.Generator().manual_seed(seed)
    small = torch.randn(1, 3, max(h // 32, 2), max(w // 32, 2), generator=g)
    return F.interpolate(small, size=(h, w), mode="bilinear", align_corners=False) + 0.1 * torch.randn(1, 3, h, w, generator=g)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--models", required=True, type=pathlib.Path, help="the shared models folder (read only)")
    ap.add_argument("--out", required=True, type=pathlib.Path)
    ap.add_argument("--install", action="store_true", help="also copy the .mlpackages into <models>/<folder>/coreml/")
    ap.add_argument("--only", choices=["backbone"] + [f"head-{h}x{w}" for h, w in HEAD_CANVASES], help="convert only this one")
    a = ap.parse_args()

    import coremltools as ct

    weights = a.models / FOLDER / "model.safetensors"
    if digest(weights) != SHA256:
        sys.exit(f"{weights} is not the pinned Sapiens2 model.safetensors")
    w = load_file(str(weights))
    periods = [float(p) for p in w["backbone.rope_embed.periods"].float()]
    bb = Backbone(w).eval()
    heads = {f"head-{h}x{w_}": Head(w, bb.c, (h, w_)).eval() for h, w_ in HEAD_CANVASES}
    a.out.mkdir(parents=True, exist_ok=True)
    tools = {
        "python": platform.python_version(),
        "torch": torch.__version__,
        "coremltools": ct.__version__,
        "numpy": np.__version__,
        "macos": platform.mac_ver()[0],
        "machine": platform.machine(),
    }

    # two passes in one call: a portrait-shaped whole photo and a smaller box (shapes as `crops.rs` makes them); the
    # small canvas holds the first alone
    xs = [photo_like(768, 512, 1), photo_like(384, 192, 2)]
    rows, spans = pack(xs, periods)
    with torch.no_grad():
        tokens32 = bb(torch.from_numpy(rows)).numpy()
    layouts = {"head-48x48": [(0, 0)], "head-96x64": [(0, 0), (0, 512 // PATCH + 1)]}
    made = {}
    for part in ["backbone", *heads]:
        if a.only and a.only != part:
            continue
        name = f"{FOLDER}-{part}"
        t = time.time()
        if part == "backbone":
            example = torch.from_numpy(rows)
            model, output = bb, "tokens"
            # the GPU: the Neural Engine was slower and further from float32 (see above)
            precision, units = ct.precision.FLOAT16, ct.ComputeUnit.CPU_AND_GPU
        else:
            model, output = heads[part], "logits"
            n = len(layouts[part])
            example = torch.from_numpy(canvas(tokens32, spans[:n], layouts[part], bb.c, model.size))
            precision, units = ct.precision.FLOAT32, ct.ComputeUnit.CPU_AND_GPU
        shape = tuple(example.shape)
        with torch.no_grad():
            traced = torch.jit.trace(model, example)
        ml = ct.convert(
            traced,
            convert_to="mlprogram",
            inputs=[ct.TensorType(name="pixels", shape=shape, dtype=np.float32)],
            outputs=[ct.TensorType(name=output, dtype=np.float16 if precision == ct.precision.FLOAT16 else np.float32)],
            compute_precision=precision,
            minimum_deployment_target=ct.target.macOS14,
            compute_units=units,
        )
        ml.short_description = f"Sapiens2 0.4B {part} for LightCraft (fork/coreml.md)"
        ml.license = LICENSE
        package = save(ml, a.out, name)
        print(f"{name}: converted in {time.time() - t:.0f} s")
        made[part] = (ml, package, name, shape, output, precision)

    # Check against the float32 reference, each pass alone (what the Rust test then checks against candle): the
    # backbone on Core ML (or PyTorch when not converted now), then each head on Core ML (or PyTorch).
    report = {}
    with torch.no_grad():
        refs = [reference(bb, heads["head-96x64"], x, periods).numpy() for x in xs]
    if "backbone" in made:
        tokens = made["backbone"][0].predict({"pixels": rows})["tokens"].astype(np.float32)
    else:
        tokens = tokens32
    for part, places in layouts.items():
        n = len(places)
        x = canvas(tokens, spans[:n], places, bb.c, heads[part].size)
        if part in made:
            logits = made[part][0].predict({"pixels": x})["logits"].astype(np.float32)
        else:
            with torch.no_grad():
                logits = heads[part](torch.from_numpy(x)).numpy()
        for xi, ref, (b, gh, gw), (r, col) in zip(xs, refs, spans, places):
            got = logits[:, :, r * PATCH : (r + gh) * PATCH, col * PATCH : (col + gw) * PATCH]
            agree = float((ref.argmax(1) == got.argmax(1)).mean())
            diff = float(np.abs(ref - got).max())
            print(f"{part}, Core ML vs PyTorch fp32 {list(xi.shape)}: argmax agreement {agree:.5f}, max |diff| {diff:.4f} (logits up to {np.abs(ref).max():.1f})")
            report.setdefault(part, []).append({"shape": list(xi.shape), "argmaxAgreement": round(agree, 5), "maxDiff": round(diff, 4)})

    for part, (ml, package, name, shape, output, precision) in made.items():
        manifest = {
            "model": name,
            "package": package.name,
            "input": {"name": "pixels", "shape": list(shape)},
            "output": {"name": output},
            "precision": "float16" if precision == ct.precision.FLOAT16 else "float32",
            "source": {"file": "model.safetensors", "sha256": SHA256},
            "license": LICENSE,
            "files": files_of(package),
            "check": report,
            "tools": tools,
        }
        (a.out / f"{name}-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(json.dumps(manifest["files"], indent=2))
        if a.install:
            dest = a.models / FOLDER / "coreml" / package.name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.rmtree(dest, ignore_errors=True)
            shutil.copytree(package, dest)
            print(f"installed {dest}")


if __name__ == "__main__":
    main()
