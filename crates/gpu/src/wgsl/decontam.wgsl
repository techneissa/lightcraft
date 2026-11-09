
// Fork (`fork/perf-decontam-gpu.md`): the finish stage at a decontaminated mask's soft edge, a port of
// `lightcraft_pipeline::decontam::pixel` (keep in step with it). Appended to `finish.wgsl`.
//
// `F_DC_N` decontaminated masks, each a descriptor of `DC_DESC` words in `aux` at `F_DC_OFF`, in mask order: the
// mask's index (u32 bits), its Amount (0..1), where its edge starts in `masks` (rows, then pixels; u32 bits), and
// its layers (1: scene edits, 2: colour edits). The edge (`decontam::Edge::device_words`): `height + 1` row starts,
// then `DC_PX` words a pixel (column, selected colour, unselected colour).

// The index of pixel `(x, y)` in edge `s`'s pixels, or -1 when it isn't one of them.
fn dc_find(s: u32, x: u32, y: u32) -> i32 {
    let d = pu(F_DC_OFF) + s * DC_DESC;
    let rows = bitcast<u32>(aux[d + 2u]);
    var lo = bitcast<u32>(masks[rows + y]);
    var hi = bitcast<u32>(masks[rows + y + 1u]);
    let px = bitcast<u32>(aux[d + 3u]);
    // (a row holds at most a row of pixels: the search ends)
    for (var step = 0u; step < 32u && lo < hi; step++) {
        let mid = lo + (hi - lo) / 2u;
        let mx = bitcast<u32>(masks[px + mid * DC_PX]);
        if (mx == x) {
            return i32(mid);
        }
        if (mx < x) {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    return -1;
}

// One decontaminated mask's hit at a pixel (`decontam::pixel::Hit`): `k` its mask, `a` its alpha, `t` its Amount,
// its layers, and the pixel's two colours.
struct DcHit {
    ok: bool,
    k: u32,
    a: f32,
    t: f32,
    layers: u32,
    sel: vec3<f32>,
    unsel: vec3<f32>,
}

fn dc_hit(s: u32, x: u32, y: u32, i: u32, n: u32) -> DcHit {
    var h: DcHit;
    let d = pu(F_DC_OFF) + s * DC_DESC;
    h.k = bitcast<u32>(aux[d]);
    h.t = aux[d + 1u];
    h.layers = bitcast<u32>(aux[d + 4u]);
    h.a = masks[h.k * n + i];
    if (!(h.a > DC_E0 && h.a < DC_E1) || h.t <= 0.0 || h.layers == 0u) {
        return h;
    }
    let j = dc_find(s, x, y);
    if (j < 0) {
        return h;
    }
    let o = bitcast<u32>(aux[d + 3u]) + u32(j) * DC_PX;
    h.sel = vec3<f32>(masks[o + 1u], masks[o + 2u], masks[o + 3u]);
    h.unsel = vec3<f32>(masks[o + 4u], masks[o + 5u], masks[o + 6u]);
    h.ok = true;
    return h;
}

// The pixel's scene colour `c` (every mask at its own alpha, terms `lt`) with each hit's scene edits made apart, and
// the change of its log luminance (`decontam::pixel::scene`).
fn dc_scene(c: vec3<f32>, x: u32, y: u32, i: u32, n: u32, lt: Local) -> vec4<f32> {
    var out = c;
    for (var s = 0u; s < pu(F_DC_N); s++) {
        let h = dc_hit(s, x, y, i, n);
        if (!h.ok || (h.layers & 1u) == 0u) {
            continue;
        }
        let su = scene_pixel(h.sel, i, n, local_terms(i, n, h.k, 1.0)).xyz;
        let uu = scene_pixel(h.unsel, i, n, local_terms(i, n, h.k, 0.0)).xyz;
        // what the mask's edit does to each colour wholly on (off) beyond what it does at the pixel's alpha
        let sa = scene_pixel(h.sel, i, n, lt).xyz;
        let ua = scene_pixel(h.unsel, i, n, lt).xyz;
        out = out + h.t * (h.a * (su - sa) + (1.0 - h.a) * (uu - ua));
    }
    var shift = 0.0;
    if (any(out != c)) {
        shift = log2(max(lum2020(out), 1e-7)) - log2(max(lum2020(c), 1e-7));
        // (a non-finite change is none)
        shift = select(0.0, shift, abs(shift) <= 1e30);
    }
    return vec4<f32>(out, shift);
}

// The pixel's colour edits with each hit's made apart (`decontam::pixel::colour`): `pre` is the composite's display
// colour before them, `d` after them (every mask at its own alpha), `g` the tone stage's gain.
fn dc_colour(pre: vec3<f32>, d: vec3<f32>, g: f32, x: u32, y: u32, i: u32, n: u32) -> vec3<f32> {
    var out = d;
    for (var s = 0u; s < pu(F_DC_N); s++) {
        let h = dc_hit(s, x, y, i, n);
        if (!h.ok || (h.layers & 2u) == 0u) {
            continue;
        }
        let on = local_terms(i, n, h.k, 1.0);
        let off = local_terms(i, n, h.k, 0.0);
        let su = scene_pixel(h.sel, i, n, on).xyz;
        let uu = scene_pixel(h.unsel, i, n, off).xyz;
        // the two colours as the composite's tone stage draws them, corrected to mix back to it exactly
        var ds = to_display(su * g);
        var du = to_display(uu * g);
        let a = h.a;
        let norm = a * a + (1.0 - a) * (1.0 - a);
        let r = (pre - (a * ds + (1.0 - a) * du)) / norm;
        ds = max(ds + a * r, vec3<f32>(0.0));
        du = max(du + (1.0 - a) * r, vec3<f32>(0.0));
        let cs = colour_pixel(ds, i, n, on, h.k, 1.0);
        let cu = colour_pixel(du, i, n, off, h.k, 0.0);
        out = out + h.t * (a * cs + (1.0 - a) * cu - d);
    }
    return out;
}
