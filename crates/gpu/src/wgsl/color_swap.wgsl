// Recolor (fork, issue #460): `lightcraft_pipeline::color_swap`, step by step. An entry's words are in the parameter
// block from `b` (`SwapK::words`: From L, C, hue cos, hue sin, saturation; the hue, saturation and lightness
// half-widths; where the fade starts; the To colour in OkLab; Intensity; Keep Lightness; how neutral the To colour is).

// `SwapK::membership` for a pixel of OkLab `lab`: the weight of the whole change and of a change of hue alone.
fn swap_member(b: u32, lab: vec3<f32>) -> vec2<f32> {
    let c = length(lab.yz);
    let h = ok_hue(lab.y, lab.z);
    let coloured = rc_smooth(SW_FROM0, SW_FROM1, pf(b + 1u));
    let dh_angle = abs(wrap_angle(h - ok_hue(pf(b + 2u), pf(b + 3u))));
    let dh = dh_angle / pf(b + 5u) * coloured;
    let ds = (c / max(lab.x, 0.05) - pf(b + 4u)) / pf(b + 6u);
    let dl = (lab.x - pf(b)) / pf(b + 7u);
    let d = sqrt(dh * dh + ds * ds + dl * dl);
    let gate = 1.0 - coloured * (1.0 - rc_smooth(SW_PIX0, SW_PIX1, c));
    let full = (1.0 - rc_smooth(pf(b + 8u), 1.0, d)) * gate;
    let wh = pf(b + 5u);
    let loose = 1.0 - rc_smooth(wh * 1.2, wh * 2.0 + 0.1, dh_angle);
    let weak = rc_smooth(SW_WEAK0, SW_WEAK1, c) * (1.0 - rc_smooth(SW_WEAK2, SW_WEAK3, c));
    let in_light = 1.0 - rc_smooth(0.5, 1.0, abs(dl));
    return vec2<f32>(full, max(full, loose * weak * coloured * in_light));
}

// `SwapK::apply` (`recolor::shade` with this membership).
fn swap_one(b: u32, lab: vec3<f32>) -> vec3<f32> {
    let k0 = min(pf(b + 12u), 1.0);
    if (k0 <= 0.0) {
        return lab;
    }
    let member = swap_member(b, lab);
    if (member.y <= 0.0) {
        return lab;
    }
    let k = k0 * member.x;
    let k_hue = k0 * max(member.y - member.x, 0.0);
    if (k <= 0.0 && k_hue <= 0.0) {
        return lab;
    }
    let rl = pf(b);
    let rc = pf(b + 1u);
    let hr = ok_hue(pf(b + 2u), pf(b + 3u));
    let c = length(lab.yz);
    let h = ok_hue(lab.y, lab.z);
    let tl = vec3<f32>(pf(b + 9u), pf(b + 10u), pf(b + 11u));
    let lt = tl.x;
    let ct = length(tl.yz);
    let ht = ok_hue(tl.y, tl.z);
    var l1 = lab.x;
    if (pf(b + 13u) == 0.0) {
        l1 = recolor_lightness(lab.x, rl, lt);
    }
    let coloured = rc_smooth(RC_GREY0, RC_GREY1, rc);
    let grey = select(1.0, clamp(l1 / lt, 0.0, 1.0), lt > 1e-6) * (1.0 - rc_smooth(lt, max(lt, 0.95) + 0.05, l1));
    let ends = rc_smooth(RC_DARK0, RC_DARK1, l1) * (1.0 - rc_smooth(RC_LIGHT0, RC_LIGHT1, l1));
    let c1 = ct * (grey + (recolor_chroma_ratio(c, rc) - grey) * coloured) * ends;
    let keep = coloured * rc_smooth(RC_GREY0, RC_GREY1, c);
    let h1 = ht + clamp(wrap_angle(h - hr) * keep, -RC_MAX_HUE, RC_MAX_HUE);
    let nlab = vec3<f32>(l1, c1 * cos(h1), c1 * sin(h1));
    // (to a neutral To colour a hue-only change takes the new chroma too: `SwapK::neutral`, `recolor::shade`)
    let own = vec3<f32>(l1, c * cos(h1), c * sin(h1));
    let turned = own + (nlab - own) * pf(b + 14u);
    return lab + (nlab - lab) * k + (turned - lab) * k_hue;
}

// `SwapK::view` of display colour `d`.
fn swap_view(b: u32, d: vec3<f32>) -> vec3<f32> {
    let lab = oklab(d);
    let w = swap_member(b, lab).y;
    let dim = vec3<f32>(lab.x * SW_DIM, 0.0, 0.0);
    var o = dim + (lab - dim) * w;
    let glow = 4.0 * w * (1.0 - w) * SW_GLOW;
    let accent = vec3<f32>(clamp(pf(b + 9u), 0.7, 0.95), pf(b + 10u), pf(b + 11u));
    o = o + (accent - o) * glow;
    return oklab_inv(o);
}

// `ColorSwaps::apply`: the entries in order, then the range view of the one being viewed.
fn color_swaps(d0: vec3<f32>) -> vec3<f32> {
    let n = pu(F_NSW);
    let view = pu(F_SWV) != 0u;
    if (n == 0u && !view) {
        return d0;
    }
    var d = d0;
    if (n > 0u) {
        let lab0 = oklab(d0);
        var lab = lab0;
        for (var k = 0u; k < n; k++) {
            lab = swap_one(F_SW + k * SWAP_WORDS, lab);
        }
        if (any(lab != lab0)) {
            d = oklab_inv(lab);
        }
    }
    if (view) {
        return swap_view(F_SWV_W, d);
    }
    return d;
}
