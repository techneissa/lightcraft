// Rectangles of images (issue #111: a render recomputes only the tiles an edit changed). Bindings: src, dst.

// Copy an RW × RH rectangle of NC interleaved channels from `src` (rows of SW pixels, at SX, SY) to
// `dst` (rows of DW pixels, at DX, DY). P: nc, sw, sx, sy, dw, dx, dy, rw, rh.
@compute @workgroup_size(16, 16)
fn rect_copy(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= pu(7u) || g.y >= pu(8u)) {
        return;
    }
    let nc = pu(0u);
    let i = ((pu(3u) + g.y) * pu(1u) + pu(2u) + g.x) * nc;
    let j = ((pu(6u) + g.y) * pu(4u) + pu(5u) + g.x) * nc;
    for (var c = 0u; c < nc; c++) {
        dst[j + c] = src[i + c];
    }
}

// Clear an RW × RH rectangle of NC channels of `dst` (rows of DW pixels, at DX, DY) to zero words: a packed RGBA8 pixel
// cleared has alpha 0, which the incomplete-image checks catch when no later work writes it. P: nc, dw, dx, dy, rw, rh.
@compute @workgroup_size(16, 16)
fn rect_clear(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= pu(4u) || g.y >= pu(5u)) {
        return;
    }
    let nc = pu(0u);
    let j = ((pu(3u) + g.y) * pu(1u) + pu(2u) + g.x) * nc;
    for (var c = 0u; c < nc; c++) {
        dst[j + c] = 0.0;
    }
}
