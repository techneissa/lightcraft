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
