// The display fit's two ends (`render::fit_on_device`): the finished picture's packed RGBA8 as three floats a pixel in 0..255, the way
// the CPU's `fit_for_display` filters it, and back, rounded and clamped, with alpha 255 (the render's alpha is 255 everywhere).
// Bindings: px (packed RGBA8), f (3 floats a pixel), out (packed RGBA8).

// P[0] = pixels.
@compute @workgroup_size(256)
fn unpack_rgb8(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let p = px[i];
    f[3u * i] = f32(p & 255u);
    f[3u * i + 1u] = f32((p >> 8u) & 255u);
    f[3u * i + 2u] = f32((p >> 16u) & 255u);
}

// P[0] = pixels.
@compute @workgroup_size(256)
fn pack_rgb8(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    // (`floor(v + 0.5)`: `round` goes to even on a half, the CPU's rounds away from zero)
    let r = u32(floor(clamp(f[3u * i], 0.0, 255.0) + 0.5));
    let gg = u32(floor(clamp(f[3u * i + 1u], 0.0, 255.0) + 0.5));
    let b = u32(floor(clamp(f[3u * i + 2u], 0.0, 255.0) + 0.5));
    out[i] = r | (gg << 8u) | (b << 16u) | (255u << 24u);
}
