// SCRATCH for issue #438, never merged: a mask plane packed into u32 words and read back, as storing it would.
// P: n (values), at (the plane's first value in `plane`). Two u16 or f16 a word, four u8.

@compute @workgroup_size(256)
fn pack_u16(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let k = lin_index(g, nw);
    let n = pu(0u);
    if (2u * k >= n) {
        return;
    }
    let at = pu(1u) + 2u * k;
    var v = vec2<f32>(plane[at], 0.0);
    if (2u * k + 1u < n) {
        v.y = plane[at + 1u];
    }
    words[k] = pack2x16unorm(v);
}

@compute @workgroup_size(256)
fn unpack_u16(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    plane[pu(1u) + i] = unpack2x16unorm(words[i / 2u])[i % 2u];
}

@compute @workgroup_size(256)
fn pack_f16(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let k = lin_index(g, nw);
    let n = pu(0u);
    if (2u * k >= n) {
        return;
    }
    let at = pu(1u) + 2u * k;
    var v = vec2<f32>(plane[at], 0.0);
    if (2u * k + 1u < n) {
        v.y = plane[at + 1u];
    }
    words[k] = pack2x16float(v);
}

@compute @workgroup_size(256)
fn unpack_f16(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    plane[pu(1u) + i] = unpack2x16float(words[i / 2u])[i % 2u];
}

@compute @workgroup_size(256)
fn pack_u8(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let k = lin_index(g, nw);
    let n = pu(0u);
    if (4u * k >= n) {
        return;
    }
    var v = vec4<f32>(0.0);
    for (var j = 0u; j < 4u; j++) {
        if (4u * k + j < n) {
            v[j] = plane[pu(1u) + 4u * k + j];
        }
    }
    words[k] = pack4x8unorm(v);
}

@compute @workgroup_size(256)
fn unpack_u8(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    plane[pu(1u) + i] = unpack4x8unorm(words[i / 4u])[i % 4u];
}
