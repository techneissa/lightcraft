// Mask evaluation (`lightcraft_pipeline::masks`): one component shape → `c`, then combined into
// the mask's alpha plane. Bindings: img (rgb, pre-exposure), log_l, aux (shape data), c, alpha.

// Point k of the position grid in aux (long-edge units).
fn grid_point(k: u32) -> vec2<f32> {
    return vec2<f32>(aux[2u * k], aux[2u * k + 1u]);
}

// Position of output pixel (x, y) in long-edge units through a warp, from the position grid
// (`masks::grid::PosGrid::fill`, `gpu_table`): P[19..25] = ox, oy, i0, j0, points per row, cell.
fn grid_pos(x: u32, y: u32) -> vec2<f32> {
    let cell = pu(24u);
    let gx = x + pu(19u);
    let gy = y + pu(20u);
    let s = pu(23u);
    let k = (gy / cell - pu(22u)) * s + gx / cell - pu(21u);
    let fx = (f32(gx % cell) + 0.5) / f32(cell);
    let fy = (f32(gy % cell) + 0.5) / f32(cell);
    let a = grid_point(k) + (grid_point(k + s) - grid_point(k)) * fy;
    let b = grid_point(k + 1u) + (grid_point(k + s + 1u) - grid_point(k + 1u)) * fy;
    return a + (b - a) * fx;
}

// Position of output pixel (x, y) in long-edge units: P[4..10] = affine (a, b, c, d, e, f), or with
// P[18] ≠ 0 the position grid.
fn pos(x: u32, y: u32) -> vec2<f32> {
    if (pu(18u) != 0u) {
        return grid_pos(x, y);
    }
    let px = f32(x) + 0.5;
    let py = f32(y) + 0.5;
    return vec2<f32>(pf(4u) * px + pf(6u) * py + pf(8u), pf(5u) * px + pf(7u) * py + pf(9u));
}

// P: w, h, kind, invert, affine[6], then per kind:
//   0 linear: a.xy, d.xy, |d|²         1 radial: centre.xy, cos, sin, rx, ry, feather, invert
//   (0 and 1: then P[18] = grid, P[19..25] its table: see `grid_pos`)
//   2 luminance range: lo, hi, lo feather, hi feather, ev
//   3 colour range: tol, gain, sample count (`masks::color_range::Key`s in aux, 8 values each)
//   (brush strokes: see `stroke`)
// `masks::color_range::L_FLOOR`.
const CR_L_FLOOR: f32 = 0.1;

@compute @workgroup_size(16, 16)
fn shape(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = pu(0u);
    let h = pu(1u);
    if (g.x >= w || g.y >= h) {
        return;
    }
    let i = g.y * w + g.x;
    let kind = pu(2u);
    var v = 0.0;
    if (kind == 0u) {
        let p = pos(g.x, g.y);
        let a = vec2<f32>(pf(10u), pf(11u));
        let d = vec2<f32>(pf(12u), pf(13u));
        let t = dot(p - a, d) / pf(14u);
        v = 1.0 - sstep(0.0, 1.0, t);
    } else if (kind == 1u) {
        let p = pos(g.x, g.y);
        let d = p - vec2<f32>(pf(10u), pf(11u));
        let co = pf(12u);
        let s = pf(13u);
        let x = d.x * co - d.y * s;
        let y = d.x * s + d.y * co;
        let qx = x / pf(14u);
        let qy = y / pf(15u);
        let r = sqrt(qx * qx + qy * qy);
        let f = pf(16u);
        v = 1.0 - sstep(1.0 - max(f, 0.01), 1.0, r);
        if (pu(17u) != 0u) {
            v = 1.0 - v;
        }
    } else if (kind == 2u) {
        let lo = pf(10u);
        let hi = pf(11u);
        let y = clamp((log_l[i] + pf(14u) + 8.0) / 12.0, 0.0, 1.0);
        v = sstep(lo - pf(12u), lo, y) * (1.0 - sstep(hi, hi + pf(13u), y));
    } else if (kind == 3u) {
        let tol = pf(10u);
        let gain = pf(11u);
        // `masks::color_range::pixel`, then `distance` to each key (8 values: see `color_range::Key`)
        let c = vec3<f32>(img[3u * i], img[3u * i + 1u], img[3u * i + 2u]) * gain;
        let lab = oklab(c);
        let ab = lab.yz / sqrt(lab.x * lab.x + CR_L_FLOOR * CR_L_FLOOR);
        let l = oklab(c / (1.0 + c)).x;
        var best = 3.402823e38;
        for (var k = 0u; k < pu(12u); k++) {
            let o = 8u * k;
            let d = ab - vec2<f32>(aux[o], aux[o + 1u]);
            let u = vec2<f32>(aux[o + 3u], aux[o + 4u]);
            let along = (d.x * u.x + d.y * u.y) * aux[o + 5u];
            let across = (d.y * u.x - d.x * u.y) * aux[o + 6u];
            let dl = (l - aux[o + 2u]) * aux[o + 7u];
            best = min(best, sqrt(along * along + across * across + dl * dl));
        }
        v = 1.0 - sstep(tol * 0.5, tol, best);
    }
    if (pu(3u) != 0u) {
        v = 1.0 - v;
    }
    c[i] = v;
}

// Combine `c` into mask plane P[2]: P[3] = 0 copy, 1 add (max), 2 subtract, 3 intersect.
@compute @workgroup_size(256)
fn combine(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    let n = pu(0u);
    if (i >= n) {
        return;
    }
    let j = pu(2u) * n + i;
    let op = pu(3u);
    let a = alpha[j];
    let v = c[i];
    if (op == 0u) {
        alpha[j] = v;
    } else if (op == 1u) {
        alpha[j] = max(a, v);
    } else if (op == 2u) {
        alpha[j] = a * (1.0 - v);
    } else {
        alpha[j] = a * v;
    }
}

// Finish mask plane P[2]: P[3] = invert, P[4] = amount (applied when ≠ 1).
@compute @workgroup_size(256)
fn finalize(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    let n = pu(0u);
    if (i >= n) {
        return;
    }
    let j = pu(2u) * n + i;
    var a = alpha[j];
    if (pu(3u) != 0u) {
        a = 1.0 - a;
    }
    let amt = pf(4u);
    if (abs(amt - 1.0) > 1e-6) {
        a = a * amt;
    }
    alpha[j] = a;
}

// One brush stroke stamped inside its bounding box into `c` (`masks::brush`'s `stamp`): one dab is
// five words in aux (x, y, r, hard, flow), pen pressure already applied. P: w, h, (unused 2..9),
// box x0, y0, x1, y1 (10..13), dab count, density, erase (14..16): the stroke is combined into `c`
// (max, or subtract when erasing). (Auto Mask strokes are stamped on the CPU.)
//
// Each 16 × 16 tile first picks, 256 dabs at a time, the dabs that reach it into workgroup memory:
// a pixel then reads the few dabs near it, not every dab of the stroke. A stroke's coverage is a
// product over its dabs, so the order of the picked ones does not matter.
var<workgroup> near_dabs: array<u32, 256>;
var<workgroup> near_count: atomic<u32>;
var<workgroup> dab_count: u32;

@compute @workgroup_size(16, 16)
fn stroke(@builtin(global_invocation_id) g: vec3<u32>, @builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wid: vec3<u32>) {
    let w = pu(0u);
    let h = pu(1u);
    let x = pu(10u) + g.x;
    let y = pu(11u) + g.y;
    let inside = x < pu(12u) && y < pu(13u) && x < w && y < h;
    let tx0 = f32(pu(10u) + wid.x * 16u);
    let ty0 = f32(pu(11u) + wid.y * 16u);
    let px = f32(x) + 0.5;
    let py = f32(y) + 0.5;
    if (li == 0u) {
        dab_count = pu(14u);
    }
    workgroupBarrier();
    let cnt = workgroupUniformLoad(&dab_count);
    var keep = 1.0;
    for (var base = 0u; base < cnt; base += 256u) {
        if (li == 0u) {
            atomicStore(&near_count, 0u);
        }
        workgroupBarrier();
        let j = base + li;
        if (j < cnt) {
            let o = 5u * j;
            let r = aux[o + 2u];
            let dx = aux[o];
            let dy = aux[o + 1u];
            // (the tile's pixel centres lie within [tx0 + 0.5, tx0 + 15.5]: a dab farther than r misses them all)
            if (dx + r >= tx0 && dx - r <= tx0 + 16.0 && dy + r >= ty0 && dy - r <= ty0 + 16.0) {
                let k = atomicAdd(&near_count, 1u);
                near_dabs[k] = j;
            }
        }
        workgroupBarrier();
        let m = atomicLoad(&near_count);
        if (inside) {
            for (var q = 0u; q < m; q++) {
                let o = 5u * near_dabs[q];
                let dx = px - aux[o];
                let dy = py - aux[o + 1u];
                let r = aux[o + 2u];
                let dd = sqrt(dx * dx + dy * dy);
                if (dd > r) {
                    continue;
                }
                var a = 1.0;
                if (dd > aux[o + 3u]) {
                    a = 1.0 - sstep(aux[o + 3u], r, dd);
                }
                keep *= 1.0 - a * aux[o + 4u];
            }
        }
        // (the next batch overwrites the list)
        workgroupBarrier();
    }
    if (inside) {
        let i = y * w + x;
        let sa = min(1.0 - keep, bitcast<f32>(pu(15u)));
        if (pu(16u) != 0u) {
            c[i] = c[i] * (1.0 - sa);
        } else {
            c[i] = max(c[i], sa);
        }
    }
}
