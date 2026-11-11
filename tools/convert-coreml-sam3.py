#!/usr/bin/env python3
"""Convert SAM 3 to six Core ML models that run on the GPU in float32 (issue #268, fork/coreml.md).

Run once on a Mac, by hand. The app never runs Python: it finds the `.mlpackage`s this writes in
`<models>/sam3-coreml/`, compiles each once with Core ML and runs them through `crates/coreml`
(`crates/segment/src/coreml.rs`). The weights are the `model.safetensors` already in `<models>/sam3/`, read only;
nothing is downloaded and nothing is written into that folder. The candle port in `crates/segment` stays the
reference and the fallback.

SAM 3's licence (the SAM License) is not on LightCraft's licence allowlist: the converted files stay on the Mac that
made them and are never published or uploaded.

The network is Hugging Face transformers' PyTorch SAM 3 (`models/sam3`, `models/sam3_tracker`, Apache-2.0), split
into the parts the app runs at different times:

    part        runs                    inputs -> outputs
    encoder     once per photo          pixels [1,3,1008,1008] -> backbone [1,1024,72,72]
    clickfeat   once per photo          backbone -> image [1,256,72,72], s0 [1,32,288,288], s1 [1,64,144,144]
    clickdec    per click               image, s0, s1, points [1,1,65,2], labels [1,1,65] (int32)
                                          -> masks [1,4,288,288], iou [1,4], obj [1,1]
    textfeat    once per photo          backbone -> l0 [1,256,288,288], l1 [1,256,144,144], l2 [1,256,72,72]
    textscore   per prompt              l2, ids [1,32] (int32), mask [1,32] (int32)
                                          -> logits [1,200], presence [1,1], boxes [1,200,4] (x0,y0,x1,y1),
                                             queries [1,200,256], memory [1,5184,256], text [1,32,256]
    textmask    per prompt that matches l0, l1, memory, text, mask, queries -> masks [1,200,288,288], semantic [1,1,288,288]

Clicks: one fixed list of 65 slots (the app's 64 clicks and the "not a point" that closes the list). `points` are
in the 1008 px input; `labels` are 1 (include), 0 (exclude), -1 (the "not a point" after the last click) and -10
(an unused slot). Unused slots are masked out of every attention that reads the click tokens (the tokens' self
attention and the image's attention to them); every other step works on each token alone, so the masks equal the
masks of the clicks alone. Padding with "not a point" tokens instead changed masks badly (IoU down to 0.0), and one
model per click count would plan the GPU again for every new count.

Prompts: `textscore` scores the 200 queries; the app runs `textmask` only when one scores above its threshold, as
candle skips its pixel decoder.

Converter gaps patched here (coremltools 9.0 with torch 2.15 and numpy 2.5):
  1. `new_ones` has no Core ML rule (transformers' attention-mask builders call it): a constant of ones.
  2. coremltools' `_cast` calls `int()` on a one-element array, an error since numpy 2.5: such a constant is folded
     first.
  CLIP's causal-mask builder is also replaced by the same rule written as a plain tensor (`causal_mask`).

    uv venv -p 3.12 venv && VIRTUAL_ENV=venv uv pip install -r tools/requirements-coreml-convert.txt "transformers==5.22.2"
    venv/bin/python -I tools/convert-coreml-sam3.py --models ~/Library/Application\\ Support/ArtCraft/models

Writes `<out>/sam3-<part>.mlpackage` (default `<models>/sam3-coreml/`) and `<out>/sam3-coreml-manifest.json` (each
file's size and SHA-256, the check of each part against PyTorch, the tool versions). `--parts` converts only some.
"""

import argparse
import hashlib
import json
import os
import pathlib
import platform
import shutil
import sys
import time

import numpy as np
import torch

torch.set_grad_enabled(False)

SIDE = 1008
SLOTS = 65  # 64 clicks (crates/engine/src/segment.rs MAX_CLICKS) + the "not a point" that closes the list
PAD, NOT_A_POINT = -10, -1
PARTS = ["encoder", "clickfeat", "clickdec", "textfeat", "textscore", "textmask"]
LICENSE = "SAM License (facebook/sam3): not on LightCraft's allowlist, never published"


# ---- coremltools gaps ---------------------------------------------------------------------------------------------


def patch_coremltools():
    from coremltools.converters.mil import Builder as mb
    from coremltools.converters.mil.frontend.torch import ops as ct_ops
    from coremltools.converters.mil.frontend.torch.ops import _get_inputs
    from coremltools.converters.mil.frontend.torch.torch_op_registry import register_torch_op
    from coremltools.converters.mil.frontend.torch.utils import NUM_TO_NUMPY_DTYPE

    @register_torch_op
    def new_ones(context, node):
        """Patch 1: `x.new_ones(size, dtype=…)` with a constant size is a constant of ones."""
        inputs = _get_inputs(context, node)
        size = inputs[1]
        if size.val is None:
            raise NotImplementedError("new_ones with a size known only at run time")
        dt = inputs[2].val if len(inputs) > 2 and inputs[2] is not None else None
        npdt = NUM_TO_NUMPY_DTYPE.get(dt, np.float32) if dt is not None else np.float32
        val = np.ones(tuple(int(s) for s in np.atleast_1d(size.val)) if np.ndim(size.val) else (), dtype=npdt)
        if npdt == np.float64:
            val = val.astype(np.float32)
        if npdt == np.int64:
            val = val.astype(np.int32)
        context.add(mb.const(val=val, name=node.name))

    original = ct_ops._cast

    def cast(context, node, dtype, dtype_name):
        """Patch 2: fold a one-element constant before `int()` (an error on arrays since numpy 2.5)."""
        x = _get_inputs(context, node, expected=1)[0]
        if x.can_be_folded_to_const() and np.size(x.val) == 1 and not isinstance(x.val, dtype):
            context.add(mb.const(val=dtype(np.asarray(x.val).reshape(-1)[0]), name=node.name), node.name)
            return
        original(context, node, dtype, dtype_name)

    ct_ops._cast = cast


def causal_mask(config, inputs_embeds, attention_mask, past_key_values=None, **kw):
    """CLIP's causal + padding mask as a plain additive tensor (transformers' builder uses `new_ones`). Same rule: a
    token sees itself and earlier tokens, never padding."""
    n = inputs_embeds.shape[1]
    causal = torch.tril(torch.ones(n, n))[None, None]
    keep = causal * attention_mask.to(torch.float32)[:, None, None, :]
    return (1.0 - keep) * -1e4


# ---- the network --------------------------------------------------------------------------------------------------


def load(weights):
    """The detector and the tracker with transformers' default configs (the checkpoint has no config.json); every
    tensor the parts use must load."""
    from safetensors import safe_open
    from transformers import Sam3Config, Sam3Model, Sam3TrackerConfig, Sam3TrackerModel
    import transformers.models.clip.modeling_clip as clip

    clip.create_causal_mask = causal_mask

    def state(prefixes):
        sd = {}
        with safe_open(weights, "pt") as f:
            for k in f.keys():
                for old, new in prefixes:
                    if k.startswith(old):
                        sd[new + k[len(old):]] = f.get_tensor(k)
        return sd

    cfg = Sam3Config()
    cfg._attn_implementation = "sdpa"
    det = Sam3Model(cfg).eval()
    missing, unexpected = det.load_state_dict(state([("detector_model.", "")]), strict=False)
    missing = [k for k in missing if not k.startswith("geometry_encoder.")]
    if missing or unexpected:
        sys.exit(f"detector weights: missing {missing[:5]}, unexpected {unexpected[:5]}")
    tcfg = Sam3TrackerConfig()
    tcfg._attn_implementation = "sdpa"
    trk = Sam3TrackerModel(tcfg).eval()
    missing, unexpected = trk.load_state_dict(
        state([("tracker_model.", ""), ("tracker_neck.", "vision_encoder.neck."), ("detector_model.vision_encoder.backbone.", "vision_encoder.backbone.")]),
        strict=False,
    )
    missing = [k for k in missing if not k.startswith("vision_encoder.backbone.")]
    video_only = ("memory_", "mask_downsample", "object_pointer_proj", "temporal_positional", "no_memory_positional", "no_object_pointer", "occlusion_spatial")
    unexpected = [k for k in unexpected if not k.startswith(video_only)]
    if missing or unexpected:
        sys.exit(f"tracker weights: missing {missing[:5]}, unexpected {unexpected[:5]}")
    return det, trk


class Encoder(torch.nn.Module):
    def __init__(self, det):
        super().__init__()
        self.vit = det.vision_encoder.backbone

    def forward(self, pixels):
        return self.vit(pixels).last_hidden_state.view(1, 72, 72, 1024).permute(0, 3, 1, 2)


class ClickFeatures(torch.nn.Module):
    """The tracker neck's three finest levels, with the decoder's 1x1 projections of the two finest (candle's
    `Sam3::prepare_clicks`; like candle, no `no_memory_embedding`)."""

    def __init__(self, trk):
        super().__init__()
        self.layers = torch.nn.ModuleList(list(trk.vision_encoder.neck.fpn_layers)[:3])
        self.conv_s0 = trk.mask_decoder.conv_s0
        self.conv_s1 = trk.mask_decoder.conv_s1

    def forward(self, backbone):
        l0, l1, l2 = (layer(backbone) for layer in self.layers)
        return l2, self.conv_s0(l0), self.conv_s1(l1)


def attend(a, q, k, v, bias=None):
    """`Sam3TrackerAttention` on [1, L, C] with an optional additive key mask [1, 1, 1, Lk]."""
    h, d = a.num_attention_heads, a.head_dim
    lq, lk = q.shape[1], k.shape[1]
    q = a.q_proj(q).view(1, lq, h, d).transpose(1, 2)
    k = a.k_proj(k).view(1, lk, h, d).transpose(1, 2)
    v = a.v_proj(v).view(1, lk, h, d).transpose(1, 2)
    w = (q @ k.transpose(2, 3)) * a.scaling
    if bias is not None:
        w = w + bias
    o = (torch.softmax(w, dim=-1) @ v).transpose(1, 2).reshape(1, lq, h * d)
    return a.o_proj(o)


class ClickDecoder(torch.nn.Module):
    """Prompt encoder + mask decoder for up to 64 clicks in fixed slots, unused slots masked out; all four masks
    (the app picks one, as candle does)."""

    def __init__(self, trk):
        super().__init__()
        self.pe = trk.prompt_encoder
        self.md = trk.mask_decoder
        image_pe = trk.get_image_wide_positional_embeddings()  # [1, 256, 72, 72]
        self.register_buffer("image_pe", image_pe.flatten(2).transpose(1, 2).contiguous())

    def embed(self, points, labels):
        pe = self.pe
        lab = labels.to(torch.float32).view(1, -1, 1)  # [1, S, 1]
        c = (points.view(1, -1, 2) + 0.5) / SIDE
        c = (2 * c - 1) @ pe.shared_embedding.positional_embedding
        c = 2 * torch.pi * c
        e = torch.cat([torch.sin(c), torch.cos(c)], -1)  # [1, S, 256]
        w = pe.point_embed.weight
        e = torch.where(lab == NOT_A_POINT, pe.not_a_point_embed.weight[None], e)
        e = e + (lab == 0).to(e.dtype) * w[0] + (lab == 1).to(e.dtype) * w[1]
        e = torch.where(lab == PAD, torch.zeros_like(e), e)
        return e

    def forward(self, image, s0, s1, points, labels):
        md = self.md
        t = md.transformer
        sparse = self.embed(points, labels)  # [1, S, 256]
        out_tokens = torch.cat([md.obj_score_token.weight, md.iou_token.weight, md.mask_tokens.weight], 0)[None]
        tokens = torch.cat([out_tokens, sparse], 1)  # [1, 6 + S, 256]
        unused = (labels.to(torch.float32).view(1, 1, 1, -1) == PAD).to(torch.float32)
        bias = torch.cat([torch.zeros(1, 1, 1, out_tokens.shape[1]), unused * -1e4], -1)  # [1, 1, 1, 6 + S]
        keys = (image + self.pe.no_mask_embed.weight.reshape(1, -1, 1, 1)).flatten(2).transpose(1, 2)  # [1, 5184, 256]
        kpe = self.image_pe
        q, k = tokens, keys
        for layer in t.layers:
            if layer.skip_first_layer_pe:
                q = attend(layer.self_attn, q, q, q, bias)
            else:
                qq = q + tokens
                q = q + attend(layer.self_attn, qq, qq, q, bias)
            q = layer.layer_norm1(q)
            q = layer.layer_norm2(q + attend(layer.cross_attn_token_to_image, q + tokens, k + kpe, k))
            q = layer.layer_norm3(q + layer.mlp(q))
            k = layer.layer_norm4(k + attend(layer.cross_attn_image_to_token, k + kpe, q + tokens, q, bias))
        q = t.layer_norm_final_attn(q + attend(t.final_attn_token_to_image, q + tokens, k + kpe, k))
        img = k.transpose(1, 2).reshape(1, 256, 72, 72)
        up = md.upscale_conv1(img) + s1
        up = md.activation(md.upscale_layer_norm(up))
        up = md.activation(md.upscale_conv2(up) + s0)
        hyper = torch.stack([md.output_hypernetworks_mlps[i](q[:, 2 + i, :]) for i in range(4)], 1)  # [1, 4, 32]
        masks = (hyper @ up.view(1, 32, 288 * 288)).view(1, 4, 288, 288)
        iou = md.iou_prediction_head(q[:, 1, :]).view(1, 4)
        obj = md.pred_obj_score_head(q[:, 0, :]).view(1, 1)
        return masks, iou, obj


class ClickReference(torch.nn.Module):
    """The clicks alone, no unused slots, through transformers' own two-way transformer (the trial's decoder, equal
    to candle on 18 of 18 clicks): the check of `ClickDecoder`."""

    def __init__(self, trk):
        super().__init__()
        self.pe = trk.prompt_encoder
        self.md = trk.mask_decoder
        self.register_buffer("image_pe", trk.get_image_wide_positional_embeddings())

    def forward(self, image, s0, s1, points, labels):
        pe, md = self.pe, self.md
        c = (points + 0.5) / SIDE
        c = torch.cat([c, torch.zeros(1, 1, 1, 2)], 2)
        c = 2 * torch.pi * ((2 * c - 1) @ pe.shared_embedding.positional_embedding)
        e = torch.cat([torch.sin(c), torch.cos(c)], -1)
        sparse = torch.cat([e[:, :, :-1] + pe.point_embed(labels.long()), pe.not_a_point_embed.weight[None, None]], 2)
        out_tokens = torch.cat([md.obj_score_token.weight, md.iou_token.weight, md.mask_tokens.weight], 0)[None, None]
        img = image + pe.no_mask_embed.weight.reshape(1, -1, 1, 1)
        q, k = md.transformer(torch.cat([out_tokens, sparse], 2), img, self.image_pe, None)
        img = k.transpose(2, 3).reshape(1, 256, 72, 72)
        up = md.activation(md.upscale_layer_norm(md.upscale_conv1(img) + s1))
        up = md.activation(md.upscale_conv2(up) + s0)
        hyper = torch.stack([md.output_hypernetworks_mlps[i](q[:, :, 2 + i, :]) for i in range(4)], 2)
        masks = (hyper @ up.view(1, 1, 32, 288 * 288)).view(1, 4, 288, 288)
        return masks, md.iou_prediction_head(q[:, :, 1, :]).view(1, 4), md.pred_obj_score_head(q[:, :, 0, :]).view(1, 1)


class TextFeatures(torch.nn.Module):
    def __init__(self, det):
        super().__init__()
        self.layers = torch.nn.ModuleList(list(det.vision_encoder.neck.fpn_layers)[:3])

    def forward(self, backbone):
        return tuple(layer(backbone) for layer in self.layers)


def position(det, side):
    return det.vision_encoder.neck.position_encoding((1, 256, side, side), torch.device("cpu"), torch.float32)


class TextScore(torch.nn.Module):
    """CLIP text encoder, DETR encoder and decoder, box head and dot-product scoring for one prompt (no masks)."""

    def __init__(self, det):
        super().__init__()
        self.det = det
        self.register_buffer("pos2", position(det, 72))

    def forward(self, l2, ids, mask):
        from transformers.models.sam3.modeling_sam3 import box_cxcywh_to_xyxy, inverse_sigmoid

        det = self.det
        text = det.get_text_features(input_ids=ids.long(), attention_mask=mask.long(), return_dict=True).pooler_output
        text_mask = mask.bool()
        enc = det.detr_encoder(vision_features=[l2], text_features=text, vision_pos_embeds=[self.pos2], text_mask=text_mask)
        dec = det.detr_decoder(
            vision_features=enc.last_hidden_state,
            text_features=enc.text_features,
            vision_pos_encoding=enc.pos_embeds_flattened,
            text_mask=text_mask,
            spatial_shapes=enc.spatial_shapes,
        )
        hidden = dec.intermediate_hidden_states
        offsets = det.detr_decoder.box_head(hidden)
        boxes = box_cxcywh_to_xyxy((inverse_sigmoid(dec.reference_boxes) + offsets).sigmoid())
        logits = det.dot_product_scoring(decoder_hidden_states=hidden, text_features=enc.text_features, text_mask=text_mask).squeeze(-1)
        return logits[-1], dec.presence_logits[-1], boxes[-1], hidden[-1], enc.last_hidden_state, text


class TextMask(torch.nn.Module):
    """The mask decoder: the encoder memory attends to the prompt, a pixel decoder up to 288², per-query masks."""

    def __init__(self, det):
        super().__init__()
        self.md = det.mask_decoder

    def forward(self, l0, l1, memory, text, mask, queries):
        l2 = torch.zeros(1, 256, 72, 72)  # only its shape is read: the memory takes its place
        out = self.md(decoder_queries=queries, backbone_features=[l0, l1, l2], encoder_hidden_states=memory, prompt_features=text, prompt_mask=mask.bool())
        return out.pred_masks, out.semantic_seg


class TextWhole(torch.nn.Module):
    """transformers' own detector for one prompt (the check of `TextScore` + `TextMask`)."""

    def __init__(self, det):
        super().__init__()
        self.det = det
        for i, s in enumerate((288, 144, 72, 36)):
            self.register_buffer(f"pos{i}", position(det, s))

    def forward(self, l0, l1, l2, ids, mask):
        from transformers.models.sam3.modeling_sam3 import Sam3VisionEncoderOutput

        l3 = torch.zeros(1, 256, 36, 36)
        ve = Sam3VisionEncoderOutput(fpn_hidden_states=(l0, l1, l2, l3), fpn_position_encoding=(self.pos0, self.pos1, self.pos2, self.pos3))
        o = self.det(vision_embeds=ve, input_ids=ids.long(), attention_mask=mask.long())
        return o.pred_masks, o.pred_logits, o.presence_logits, o.pred_boxes, o.semantic_seg


def tokens(model_dir, text):
    from transformers import CLIPTokenizer

    tok = CLIPTokenizer(str(model_dir / "vocab.json"), str(model_dir / "merges.txt"))
    e = tok(text, padding="max_length", max_length=32, truncation=True, return_tensors="pt")
    return e.input_ids.to(torch.int32), e.attention_mask.to(torch.int32)


def clicks(points):
    """`points` [(x, y, include)] in 0..1 as the fixed slots: (points [1,1,65,2], labels [1,1,65])."""
    p = torch.zeros(1, 1, SLOTS, 2)
    lab = torch.full((1, 1, SLOTS), PAD, dtype=torch.int32)
    for i, (x, y, inc) in enumerate(points):
        p[0, 0, i] = torch.tensor([x * SIDE, y * SIDE])
        lab[0, 0, i] = 1 if inc else 0
    lab[0, 0, len(points)] = NOT_A_POINT
    return p, lab


# ---- checks in PyTorch --------------------------------------------------------------------------------------------


def check_torch(det, trk, model_dir):
    """The new wrappers against transformers' own code on made-up features: the click slots for 1, 2, 3 and 5 clicks
    (with excludes), and the split prompt model."""
    g = torch.Generator().manual_seed(268)
    backbone = torch.randn(1, 1024, 72, 72, generator=g)
    image, s0, s1 = ClickFeatures(trk)(backbone)
    dec, ref = ClickDecoder(trk), ClickReference(trk)
    sets = [[(0.4, 0.5, True)], [(0.4, 0.5, True), (0.7, 0.2, False)], [(0.4, 0.5, True), (0.7, 0.2, False), (0.1, 0.9, True)],
            [(0.4, 0.5, True), (0.7, 0.2, False), (0.1, 0.9, True), (0.5, 0.5, False), (0.9, 0.9, True)]]
    out = {}
    for pts in sets:
        p, lab = clicks(pts)
        m, i, o = dec(image, s0, s1, p, lab)
        n = len(pts)
        rp = torch.tensor([[[[x * SIDE, y * SIDE] for x, y, _ in pts]]])
        rl = torch.tensor([[[1 if inc else 0 for _, _, inc in pts]]], dtype=torch.int32)
        rm, ri, ro = ref(image, s0, s1, rp, rl)
        diff = float((m[0] - rm[0]).abs().max())
        flips = int(((m[0] > 0) != (rm[0] > 0)).sum())
        out[f"clicks{n}"] = {"masks_max_diff": diff, "cells_flipped": flips, "iou_max_diff": float((i - ri).abs().max())}
        print(f"click slots, {n} clicks: masks max diff {diff:.2e}, {flips} cells flipped", flush=True)
        if flips:
            sys.exit("the click slots change the masks")
    l0, l1, l2 = TextFeatures(det)(backbone)
    ids, mask = tokens(model_dir, "person")
    whole = TextWhole(det)(l0, l1, l2, ids, mask)
    logits, presence, boxes, queries, memory, text = TextScore(det)(l2, ids, mask)
    masks, semantic = TextMask(det)(l0, l1, memory, text, mask, queries)
    for name, a, b in [("masks", masks, whole[0]), ("logits", logits, whole[1]), ("presence", presence, whole[2]), ("boxes", boxes, whole[3]), ("semantic", semantic, whole[4])]:
        d = float((a - b).abs().max())
        out[f"prompt-{name}"] = d
        print(f"split prompt model, {name}: max diff {d:.2e}", flush=True)
        if d > 1e-3:
            sys.exit(f"the split prompt model differs in {name}")
    print("memory", tuple(memory.shape), "text", tuple(text.shape), "queries", tuple(queries.shape), flush=True)
    return out


# ---- conversion ---------------------------------------------------------------------------------------------------


def examples(det, trk, model_dir):
    """Each part: (module, example inputs, input names, output names, how)."""
    g = torch.Generator().manual_seed(1)
    backbone = torch.randn(1, 1024, 72, 72, generator=g)
    image, s0, s1 = ClickFeatures(trk)(backbone)
    l0, l1, l2 = TextFeatures(det)(backbone)
    ids, mask = tokens(model_dir, "person")
    _, _, _, queries, memory, text = TextScore(det)(l2, ids, mask)
    p, lab = clicks([(0.4, 0.5, True), (0.7, 0.2, False)])
    return {
        "encoder": (lambda: Encoder(det), (torch.randn(1, 3, SIDE, SIDE, generator=g).clamp(-1, 1),), ["pixels"], ["backbone"], "export"),
        "clickfeat": (lambda: ClickFeatures(trk), (backbone,), ["backbone"], ["image", "s0", "s1"], "export"),
        "clickdec": (lambda: ClickDecoder(trk), (image, s0, s1, p, lab), ["image", "s0", "s1", "points", "labels"], ["masks", "iou", "obj"], "export"),
        "textfeat": (lambda: TextFeatures(det), (backbone,), ["backbone"], ["l0", "l1", "l2"], "export"),
        "textscore": (lambda: TextScore(det), (l2, ids, mask), ["l2", "ids", "mask"], ["logits", "presence", "boxes", "queries", "memory", "text"], "trace"),
        "textmask": (lambda: TextMask(det), (l0, l1, memory, text, mask, queries), ["l0", "l1", "memory", "text", "mask", "queries"], ["masks", "semantic"], "trace"),
    }


def convert(part, make, ex, names, outs, how, out_dir):
    import coremltools as ct

    m = make().eval()
    t = time.time()
    if how == "export":
        prog = torch.export.export(m, ex).run_decompositions({})
        graph = prog.graph_module.graph
        for node in list(graph.nodes):  # coremltools rejects alias nodes (as tools/convert-coreml-birefnet.py)
            if node.op == "call_function" and "alias" in str(node.target):
                node.replace_all_uses_with(node.args[0])
                graph.erase_node(node)
        prog.graph_module.recompile()
        io = {}
    else:
        prog = torch.jit.trace(m, ex, check_trace=False, strict=False)
        io = {
            "inputs": [ct.TensorType(name=n, shape=e.shape, dtype=np.int32 if e.dtype == torch.int32 else np.float32) for n, e in zip(names, ex)],
            "outputs": [ct.TensorType(name=o, dtype=np.float32) for o in outs],
        }
    trace_s = time.time() - t
    t = time.time()
    mlm = ct.convert(prog, **io, convert_to="mlprogram", minimum_deployment_target=ct.target.macOS14, compute_precision=ct.precision.FLOAT32, skip_model_load=True)
    convert_s = time.time() - t
    spec = mlm.get_spec()
    have_in = [i.name for i in spec.description.input]
    if have_in != names:
        sys.exit(f"{part}: inputs {have_in}, wanted {names}")
    have_out = [o.name for o in spec.description.output]
    if len(have_out) != len(outs):
        sys.exit(f"{part}: {len(have_out)} outputs, wanted {outs}")
    if have_out != outs:  # torch.export names outputs after their last op: give them the app's names, in order
        for old, new in zip(have_out, outs):
            ct.utils.rename_feature(spec, old, new)
        mlm = ct.models.MLModel(spec, weights_dir=mlm.weights_dir, skip_model_load=True)
    path = out_dir / f"sam3-{part}.mlpackage"
    if path.exists():
        shutil.rmtree(path)
    mlm.save(str(path))
    return path, round(trace_s, 1), round(convert_s, 1)


def check_coreml(path, module, ex, outs):
    """The package on the GPU against PyTorch on the example inputs: max |diff| per output."""
    import coremltools as ct

    ml = ct.models.MLModel(str(path), compute_units=ct.ComputeUnit.CPU_AND_GPU)
    names = [i.name for i in ml.get_spec().description.input]
    feed = {n: e.numpy().astype(np.int32 if e.dtype == torch.int32 else np.float32) for n, e in zip(names, ex)}
    got = ml.predict(feed)
    want = module(*ex)
    want = want if isinstance(want, tuple) else (want,)
    res = {}
    for name, w in zip(outs, want):
        g = np.asarray(got[name]).reshape(w.shape)
        res[name] = float(np.abs(g - w.numpy()).max())
    return res


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--models", type=pathlib.Path, default=pathlib.Path.home() / "Library/Application Support/ArtCraft/models")
    ap.add_argument("--out", type=pathlib.Path, help="default: <models>/sam3-coreml")
    ap.add_argument("--parts", default=",".join(PARTS))
    ap.add_argument("--no-check", action="store_true", help="skip the checks against PyTorch")
    a = ap.parse_args()
    model_dir = a.models / "sam3"
    weights = model_dir / "model.safetensors"
    if not weights.is_file():
        sys.exit(f"no {weights}")
    out_dir = a.out or a.models / "sam3-coreml"
    if out_dir.resolve() == model_dir.resolve():
        sys.exit("the packages never go into the sam3 folder")
    out_dir.mkdir(parents=True, exist_ok=True)
    parts = [p for p in a.parts.split(",") if p]
    if any(p not in PARTS for p in parts):
        sys.exit(f"parts: {PARTS}")
    patch_coremltools()
    import coremltools as ct
    import transformers

    t = time.time()
    det, trk = load(weights)
    print(f"weights loaded in {time.time() - t:.0f} s", flush=True)
    manifest_path = out_dir / "sam3-coreml-manifest.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {"parts": {}}
    manifest["license"] = LICENSE
    manifest["source"] = {"file": "sam3/model.safetensors", "bytes": weights.stat().st_size}
    if not a.no_check:
        manifest["torch_checks"] = check_torch(det, trk, model_dir)
    ex = examples(det, trk, model_dir)
    for part in parts:
        make, inputs, names, outs, how = ex[part]
        t = time.time()
        path, trace_s, convert_s = convert(part, make, inputs, names, outs, how, out_dir)
        files = [{"path": str(p.relative_to(out_dir)), "bytes": p.stat().st_size, "sha256": digest(p)} for p in sorted(path.rglob("*")) if p.is_file()]
        entry = {"inputs": names, "outputs": outs, "trace_s": trace_s, "convert_s": convert_s, "bytes": sum(f["bytes"] for f in files), "files": files}
        if not a.no_check:
            entry["coreml_vs_torch_max_diff"] = check_coreml(path, make().eval(), inputs, outs)
        manifest["parts"][part] = entry
        print(f"{part}: {entry['bytes'] / 1e6:.0f} MB in {time.time() - t:.0f} s, check {entry.get('coreml_vs_torch_max_diff')}", flush=True)
    manifest["tools"] = {
        "python": platform.python_version(),
        "torch": torch.__version__,
        "coremltools": ct.__version__,
        "transformers": transformers.__version__,
        "numpy": np.__version__,
        "macos": platform.mac_ver()[0],
        "machine": platform.machine(),
    }
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"wrote {out_dir}", flush=True)


if __name__ == "__main__":
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")
    main()
