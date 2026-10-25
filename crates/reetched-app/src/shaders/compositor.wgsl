// Layer-stack compositor. Every pass renders the mesh in UV space into four
// rgba16float targets (a "channel set"). Channels are evaluated in groups of
// seven (CHANNEL_GROUPS in reetched-doc); a pass reads and writes one group.
//
// Group 0, the core channels (entry points without a suffix):
//   t0 = base color (linear rgb), metallic
//   t1 = normal (tangent space xyz), roughness
//   t2 = height, coverage (1 inside UV islands), filled (1 = has a value, incl. dilation), opacity
//   t3 = emissive (linear rgb), unused
//   slots: 0 base color, 1 metallic, 2 roughness, 3 normal, 4 height, 5 emissive, 6 opacity.
// Groups 1 and 2 (entry points ending in _ex): two colors, then five scalars:
//   t0 = slot 0 rgb, slot 2
//   t1 = slot 1 rgb, slot 3
//   t2 = slot 4, slot 5, slot 6, unused
//   t3 = coverage, filled, unused, unused

struct Proj {
    // x: mode (0 uv, 1 tri-planar, 2 planar, 3 spherical, 4 cylindrical, 5 warp grid,
    // 6 second UV set), y: uv rotation (rad), z: tri-planar exponent, w: 1 / size
    a: vec4<f32>,
    // xy: uv scale, zw: uv offset
    so: vec4<f32>,
    // xyz: projector position, w: mirror bits (1 x, 2 y, 4 z), 8 = no repeat,
    // 16 = planar onto front faces only
    origin: vec4<f32>,
    // Rows of the inverse projector rotation.
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    // Warp grid: 4×4 lattice points (row-major, row 0 at v = 0), two per vec4.
    warp: array<vec4<f32>, 8>,
};

// One mask effect (see MaskEffect in reetched-doc).
struct MaskFx {
    // x: kind (0 none, 1 generator, 2 levels, 3 color selection), y: blend, z: opacity
    head: vec4<f32>,
    // generator: level, contrast, noise, noise scale; levels: in min, in max, gamma, out min
    a: vec4<f32>,
    // generator: kind, axis, invert; levels: out max; color selection: tolerance in w
    b: vec4<f32>,
    // color selection: up to 4 colors (w = used)
    colors: array<vec4<f32>, 4>,
};

struct Params {
    // Per slot of the pass's group. xyz: uniform value, w: channel enabled
    values: array<vec4<f32>, 7>,
    // x: blend mode, y: opacity, z: has bitmap
    blend: array<vec4<f32>, 7>,
    // x: mask enabled, y: background (1 white), z: has bitmap, w: invert
    mask: vec4<f32>,
    proj: Proj,
    mask_proj: Proj,
    // x: height-to-normal strength, y: has painted mask, z: scene size (noise scale)
    misc: vec4<f32>,
    // Which mesh maps exist: x normal, y curvature, z AO, w position
    maps: vec4<f32>,
    // x thickness, y ID
    maps2: vec4<f32>,
    fx: array<MaskFx, 4>,
    // xy: UV origin of the UDIM tile being rendered
    tile: vec4<f32>,
    // The group's empty stack, packed like t0..t3 (inside the islands).
    defaults: array<vec4<f32>, 4>,
};

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var smp: sampler;
// The group's seven channel bitmaps / paint textures, by slot.
@group(0) @binding(2) var t_c0: texture_2d<f32>;
@group(0) @binding(3) var t_c1: texture_2d<f32>;
@group(0) @binding(4) var t_c2: texture_2d<f32>;
@group(0) @binding(5) var t_c3: texture_2d<f32>;
@group(0) @binding(6) var t_c4: texture_2d<f32>;
@group(0) @binding(7) var t_c5: texture_2d<f32>;
@group(0) @binding(8) var t_c6: texture_2d<f32>;
@group(0) @binding(9) var t_mask: texture_2d<f32>;
// Painted mask texels (value in r, coverage in a), same size as the texture set.
@group(0) @binding(10) var t_mask_paint: texture_2d<f32>;
// Baked mesh maps (image orientation: row 0 = v 1).
@group(0) @binding(11) var m_normal: texture_2d<f32>;
@group(0) @binding(12) var m_curvature: texture_2d<f32>;
@group(0) @binding(13) var m_ao: texture_2d<f32>;
@group(0) @binding(14) var m_position: texture_2d<f32>;
@group(0) @binding(15) var m_thickness: texture_2d<f32>;
@group(0) @binding(16) var m_id: texture_2d<f32>;

// UV within the tile being rendered.
fn tile_uv(uv: vec2<f32>) -> vec2<f32> {
    return uv - P.tile.xy;
}

fn mesh_map(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let u = tile_uv(uv);
    return textureSampleLevel(t, smp, vec2(u.x, 1.0 - u.y), 0.0);
}

// ---------------------------------------------------------------- noise

fn hash3(p: vec3<f32>) -> f32 {
    let q = fract(p * 0.3183099 + vec3(0.1, 0.2, 0.3)) * 17.0;
    return fract(q.x * q.y * q.z * (q.x + q.y + q.z));
}

fn value_noise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let n = mix(
        mix(mix(hash3(i), hash3(i + vec3(1.0, 0.0, 0.0)), u.x),
            mix(hash3(i + vec3(0.0, 1.0, 0.0)), hash3(i + vec3(1.0, 1.0, 0.0)), u.x), u.y),
        mix(mix(hash3(i + vec3(0.0, 0.0, 1.0)), hash3(i + vec3(1.0, 0.0, 1.0)), u.x),
            mix(hash3(i + vec3(0.0, 1.0, 1.0)), hash3(i + vec3(1.0, 1.0, 1.0)), u.x), u.y),
        u.z);
    return n;
}

fn fbm(p: vec3<f32>) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0; i < 5; i++) {
        sum += value_noise(q) * amp;
        q = q * 2.03 + vec3(17.1, 9.3, 4.7);
        amp *= 0.5;
    }
    return sum / 0.96875;
}

// ---------------------------------------------------------------- mask effects

fn generator_value(fx: MaskFx, uv: vec2<f32>, pos: vec3<f32>, n: vec3<f32>) -> f32 {
    let noise_p = pos / max(P.misc.z, 1e-5) * fx.a.w;
    var v = 0.0;
    switch u32(fx.b.x) {
        case 0u: { v = (select(0.5, mesh_map(m_curvature, uv).r, P.maps.y > 0.5) - 0.5) * 2.0; }
        case 1u: { v = (0.5 - select(0.5, mesh_map(m_curvature, uv).r, P.maps.y > 0.5)) * 2.0; }
        case 2u: { v = 1.0 - select(1.0, mesh_map(m_ao, uv).r, P.maps.z > 0.5); }
        case 3u: {
            let p = select(vec3(0.5), mesh_map(m_position, uv).rgb, P.maps.w > 0.5);
            let axis = u32(fx.b.y);
            v = select(select(p.z, p.y, axis == 1u), p.x, axis == 0u);
        }
        case 4u: { v = 1.0 - select(1.0, mesh_map(m_thickness, uv).r, P.maps2.x > 0.5); }
        case 6u: {
            // Light: horizontal angle in turns (b.y), vertical -1..1 (b.w).
            let h = fx.b.y * 6.2831853;
            let e = fx.b.w * 1.5707963;
            let l = vec3(cos(e) * sin(h), sin(e), cos(e) * cos(h));
            v = dot(normalize(n), l) * 0.5 + 0.5;
        }
        default: { v = fbm(noise_p); }
    }
    if fx.a.z > 0.0 && u32(fx.b.x) != 5u && u32(fx.b.x) != 6u {
        v += (fbm(noise_p) - 0.5) * fx.a.z;
    }
    var m = smoothstep(fx.a.x - fx.a.y, fx.a.x + fx.a.y, v);
    if fx.b.z > 0.5 {
        m = 1.0 - m;
    }
    return m;
}

fn effect_value(fx: MaskFx, below: f32, uv: vec2<f32>, pos: vec3<f32>, n: vec3<f32>) -> f32 {
    switch u32(fx.head.x) {
        case 1u: { return generator_value(fx, uv, pos, n); }
        case 2u: {
            let t = clamp((below - fx.a.x) / max(fx.a.y - fx.a.x, 1e-5), 0.0, 1.0);
            return mix(fx.a.w, fx.b.x, pow(t, 1.0 / max(fx.a.z, 1e-3)));
        }
        case 3u: {
            if P.maps2.y < 0.5 {
                return 0.0;
            }
            let id = mesh_map(m_id, uv).rgb;
            var sel = 0.0;
            for (var k = 0; k < 4; k++) {
                let c = fx.colors[k];
                if c.w > 0.5 {
                    let tol = max(fx.b.w, 1e-3);
                    sel = max(sel, 1.0 - smoothstep(tol * 0.5, tol, distance(id, c.rgb)));
                }
            }
            return sel;
        }
        default: { return below; }
    }
}

fn mask_blend(mode: u32, a: f32, b: f32, opacity: f32) -> f32 {
    switch mode {
        case 1u: { return mix(a, a * b, opacity); }
        case 2u: { return clamp(a + b * opacity, 0.0, 1.0); }
        case 3u: { return clamp(a - b * opacity, 0.0, 1.0); }
        case 4u: { return mix(a, min(a, b), opacity); }
        case 5u: { return mix(a, max(a, b), opacity); }
        default: { return mix(a, b, opacity); }
    }
}

// The stack so far.
@group(1) @binding(0) var s0: texture_2d<f32>;
@group(1) @binding(1) var s1: texture_2d<f32>;
@group(1) @binding(2) var s2: texture_2d<f32>;
@group(1) @binding(3) var s3: texture_2d<f32>;
// Folder passes: the folder's content.
@group(2) @binding(0) var o0: texture_2d<f32>;
@group(2) @binding(1) var o1: texture_2d<f32>;
@group(2) @binding(2) var o2: texture_2d<f32>;
@group(2) @binding(3) var o3: texture_2d<f32>;

struct Out {
    @location(0) t0: vec4<f32>,
    @location(1) t1: vec4<f32>,
    @location(2) t2: vec4<f32>,
    @location(3) t3: vec4<f32>,
};

// ---------------------------------------------------------------- mesh in UV space

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) uv1: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // The mesh's UVs (absolute, UDIM tiles included).
    @location(2) uv: vec2<f32>,
    @location(3) uv1: vec2<f32>,
};

@vertex
fn vs_uv(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = vec4(tile_uv(v.uv) * 2.0 - 1.0, 0.0, 1.0);
    o.world = v.pos;
    o.normal = v.normal;
    o.uv = v.uv;
    o.uv1 = v.uv1;
    return o;
}

// ---------------------------------------------------------------- projections

fn uv_xform(t: vec2<f32>, pr: Proj) -> vec2<f32> {
    let s = t * pr.so.xy;
    let c = cos(pr.a.y);
    let sn = sin(pr.a.y);
    return vec2(c * s.x - sn * s.y, sn * s.x + c * s.y) + pr.so.zw;
}

fn local_dir(d: vec3<f32>, pr: Proj) -> vec3<f32> {
    return vec3(dot(pr.r0.xyz, d), dot(pr.r1.xyz, d), dot(pr.r2.xyz, d));
}

fn cross2(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
}

// Where `p` sits in the quad a (0,0), b (1,0), c (1,1), d (0,1); outside [0,1]² if not in it.
// (Inigo Quilez's inverse bilinear interpolation.)
fn inverse_bilinear(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>, d: vec2<f32>) -> vec2<f32> {
    let e = b - a;
    let f = d - a;
    let g = a - b + c - d;
    let h = p - a;
    let k2 = cross2(g, f);
    let k1 = cross2(e, f) + cross2(h, g);
    let k0 = cross2(h, e);
    if abs(k2) < 1e-7 {
        let v = -k0 / k1;
        let den = e + g * v;
        let u = select(h.y - f.y * v, h.x - f.x * v, abs(den.x) > abs(den.y)) /
            select(den.y, den.x, abs(den.x) > abs(den.y));
        return vec2(u, v);
    }
    let w2 = k1 * k1 - 4.0 * k0 * k2;
    if w2 < 0.0 {
        return vec2(-1.0);
    }
    let w = sqrt(w2);
    let ik2 = 0.5 / k2;
    var v = (-k1 - w) * ik2;
    if v < 0.0 || v > 1.0 {
        v = (-k1 + w) * ik2;
    }
    let den = e + g * v;
    let u = select(h.y - f.y * v, h.x - f.x * v, abs(den.x) > abs(den.y)) /
        select(den.y, den.x, abs(den.x) > abs(den.y));
    return vec2(u, v);
}

fn warp_point(pr: Proj, i: u32, j: u32) -> vec2<f32> {
    let k = j * 4u + i;
    let q = pr.warp[k / 2u];
    return select(q.zw, q.xy, k % 2u == 0u);
}

// Warp grid: the image is stretched over a 4×4 lattice placed in UV space (repeating
// per unit tile); finds the image coordinate of `t`.
fn warp_uv(t: vec2<f32>, pr: Proj) -> vec2<f32> {
    let base = floor(t);
    let f = t - base;
    var found = f;
    for (var j = 0u; j < 3u; j++) {
        for (var i = 0u; i < 3u; i++) {
            let q = inverse_bilinear(f, warp_point(pr, i, j), warp_point(pr, i + 1u, j),
                warp_point(pr, i + 1u, j + 1u), warp_point(pr, i, j + 1u));
            if all(q >= vec2(-1e-4)) && all(q <= vec2(1.0 + 1e-4)) {
                found = (vec2(f32(i), f32(j)) + clamp(q, vec2(0.0), vec2(1.0))) / 3.0;
            }
        }
    }
    // ponytail: points outside every cell keep their unwarped coordinate.
    return base + found;
}

// A non-repeating image covers nothing outside 0..1.
fn clipped(t0: vec2<f32>, pr: Proj) -> bool {
    let outside = any(t0 < vec2(0.0)) || any(t0 > vec2(1.0));
    return (u32(pr.origin.w) & 8u) != 0u && outside;
}

fn sample_proj(t: texture_2d<f32>, pr: Proj, in: VsOut, n: vec3<f32>) -> vec4<f32> {
    let mode = u32(pr.a.x);
    if mode == 0u || mode == 5u || mode == 6u {
        // Images have v = 1 at the top row; our UVs put v = 0 at the bottom.
        var t0 = uv_xform(select(in.uv, in.uv1, mode == 6u), pr);
        if mode == 5u {
            t0 = warp_uv(t0, pr);
        }
        let s = textureSample(t, smp, vec2(t0.x, 1.0 - t0.y));
        return select(s, vec4(0.0), clipped(t0, pr));
    }
    let pos = in.world;
    // Fill symmetry: mirror the position (and normal) across the chosen planes.
    let bits = u32(pr.origin.w);
    var mp = pos;
    var mn = n;
    for (var i = 0u; i < 3u; i++) {
        if (bits & (1u << i)) != 0u && mp[i] < 0.0 {
            mp[i] = -mp[i];
            mn[i] = -mn[i];
        }
    }
    let q = local_dir((mp - pr.origin.xyz) * pr.a.w, pr);
    if mode == 2u {
        let t0 = uv_xform(q.xy + 0.5, pr);
        let s = textureSample(t, smp, vec2(t0.x, 1.0 - t0.y));
        let back = (bits & 16u) != 0u && local_dir(mn, pr).z < 0.0;
        return select(s, vec4(0.0), clipped(t0, pr) || back);
    }
    if mode == 3u || mode == 4u {
        // Around the projector's Y axis; u wraps once around.
        let u = atan2(q.x, q.z) / 6.2831853 + 0.5;
        var v = q.y + 0.5;
        if mode == 3u {
            v = 1.0 - acos(clamp(q.y / max(length(q), 1e-6), -1.0, 1.0)) / 3.14159265;
        }
        let t0 = uv_xform(vec2(u, v), pr);
        // Level 0: atan2 jumps at the seam, which would pick the smallest mip there.
        // ponytail: no mipmapping for these two; use analytic gradients if aliasing shows.
        return textureSampleLevel(t, smp, vec2(t0.x, 1.0 - t0.y), 0.0);
    }
    var w = pow(abs(local_dir(mn, pr)), vec3(pr.a.z));
    w /= max(w.x + w.y + w.z, 1e-5);
    let x = textureSample(t, smp, uv_xform(q.zy, pr) * vec2(1.0, -1.0));
    let y = textureSample(t, smp, uv_xform(q.xz, pr) * vec2(1.0, -1.0));
    let z = textureSample(t, smp, uv_xform(q.xy, pr) * vec2(1.0, -1.0));
    return x * w.x + y * w.y + z * w.z;
}

fn mask_value(in: VsOut, n: vec3<f32>, p: vec2<i32>) -> f32 {
    if P.mask.x < 0.5 {
        return 1.0;
    }
    var m = P.mask.y;
    if P.mask.z > 0.5 {
        m = sample_proj(t_mask, P.mask_proj, in, n).r;
    }
    for (var i = 0u; i < 4u; i++) {
        let fx = P.fx[i];
        if u32(fx.head.x) == 0u {
            continue;
        }
        m = mask_blend(u32(fx.head.y), m, effect_value(fx, m, in.uv, in.world, n), fx.head.z);
    }
    if P.misc.y > 0.5 {
        let painted = textureLoad(t_mask_paint, p, 0);
        m = mix(m, painted.r, painted.a);
    }
    if P.mask.w > 0.5 {
        m = 1.0 - m;
    }
    return m;
}

// ---------------------------------------------------------------- blend modes
// Indices follow BlendMode::ALL in reetched-doc.
// ponytail: formulas follow the W3C compositing spec / common Photoshop definitions;
// verify each against Painter golden exports before calling them exact.

fn rgb2hsv(c: vec3<f32>) -> vec3<f32> {
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    let d = mx - mn;
    var h = 0.0;
    if d > 1e-6 {
        if mx == c.r {
            h = (c.g - c.b) / d + select(0.0, 6.0, c.g < c.b);
        } else if mx == c.g {
            h = (c.b - c.r) / d + 2.0;
        } else {
            h = (c.r - c.g) / d + 4.0;
        }
        h /= 6.0;
    }
    return vec3(h, select(0.0, d / mx, mx > 1e-6), mx);
}

fn hsv2rgb(c: vec3<f32>) -> vec3<f32> {
    let k = vec3(0.0, 2.0 / 3.0, 1.0 / 3.0);
    let p = abs(fract(vec3(c.x) + k) * 6.0 - 3.0);
    return c.z * mix(vec3(1.0), clamp(p - 1.0, vec3(0.0), vec3(1.0)), c.y);
}

fn overlay(d: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    return select(1.0 - 2.0 * (1.0 - d) * (1.0 - s), 2.0 * d * s, d < vec3(0.5));
}

fn color_burn(d: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    return 1.0 - min(vec3(1.0), (1.0 - d) / max(s, vec3(1e-4)));
}

fn color_dodge(d: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    return min(vec3(1.0), d / max(1.0 - s, vec3(1e-4)));
}

fn soft_light(d: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    let dd = select(sqrt(d), ((16.0 * d - 12.0) * d + 4.0) * d, d <= vec3(0.25));
    return select(d + (2.0 * s - 1.0) * (dd - d), d - (1.0 - 2.0 * s) * d * (1.0 - d), s <= vec3(0.5));
}

fn blend(mode: u32, d: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 4u: { return d * s; }
        case 5u: { return d / max(s, vec3(1e-4)); }
        case 6u: { return s / max(d, vec3(1e-4)); }
        case 7u: { return min(d, s); }
        case 8u: { return max(d, s); }
        case 9u: { return d + s; }
        case 10u: { return d - s; }
        case 11u: { return s - d; }
        case 12u: { return abs(d - s); }
        case 13u: { return d + s - 2.0 * d * s; }
        case 14u: { return d + (s - 0.5) * 2.0; }
        case 15u: { return overlay(d, s); }
        case 16u: { return 1.0 - (1.0 - d) * (1.0 - s); }
        case 17u: { return d + s - 1.0; }
        case 18u: { return color_burn(d, s); }
        case 19u: { return color_dodge(d, s); }
        case 20u: { return soft_light(d, s); }
        case 21u: { return overlay(s, d); }
        case 22u: { return select(color_dodge(d, 2.0 * (s - 0.5)), color_burn(d, 2.0 * s), s < vec3(0.5)); }
        case 23u: { return d + 2.0 * s - 1.0; }
        case 24u: { return select(max(d, 2.0 * s - 1.0), min(d, 2.0 * s), s < vec3(0.5)); }
        case 25u: { let a = rgb2hsv(d); return hsv2rgb(vec3(rgb2hsv(s).x, a.y, a.z)); }
        case 26u: { let a = rgb2hsv(d); return hsv2rgb(vec3(a.x, rgb2hsv(s).y, a.z)); }
        case 27u: { let b = rgb2hsv(s); return hsv2rgb(vec3(b.x, b.y, rgb2hsv(d).z)); }
        case 28u: { let a = rgb2hsv(d); return hsv2rgb(vec3(a.x, a.y, rgb2hsv(s).z)); }
        default: { return s; } // Normal, Replace
    }
}

// Reoriented normal mapping (Barré-Brisebois & Hill): `detail` on top of `base`.
fn rnm(base: vec3<f32>, detail: vec3<f32>) -> vec3<f32> {
    let t = base + vec3(0.0, 0.0, 1.0);
    let u = detail * vec3(-1.0, -1.0, 1.0);
    return normalize(t * dot(t, u) / t.z - u);
}

fn composite(mode: u32, d: vec3<f32>, s: vec3<f32>, a: f32, is_normal: bool) -> vec3<f32> {
    if mode == 2u {
        return d; // Disable
    }
    if is_normal {
        var b = s;
        switch mode {
            case 29u: { b = normalize(vec3(d.xy + s.xy, d.z * s.z)); }
            case 30u: { b = rnm(d, s); }
            case 31u: { b = rnm(s, d); }
            case 0u, 1u, 3u: { b = s; }
            default: { b = blend(mode, d, s); }
        }
        return normalize(mix(d, b, a));
    }
    return mix(d, blend(mode, d, s), a);
}

fn mode_of(i: u32) -> u32 {
    return u32(P.blend[i].x);
}

fn on(i: u32) -> bool {
    return P.values[i].w > 0.5;
}

// A fill layer's value for slot `i` and how much of it there is: its bitmap
// through the projection (alpha = coverage, like Painter), or its uniform value.
fn fill_value(i: u32, t: texture_2d<f32>, in: VsOut, n: vec3<f32>) -> vec4<f32> {
    if P.blend[i].z > 0.5 {
        return sample_proj(t, P.proj, in, n);
    }
    return vec4(P.values[i].xyz, 1.0);
}

// A fill layer onto `d` in slot `i`.
fn fill_slot(i: u32, t: texture_2d<f32>, d: vec3<f32>, in: VsOut, n: vec3<f32>, m: f32) -> vec3<f32> {
    if !on(i) {
        return d;
    }
    let v = fill_value(i, t, in, n);
    return composite(mode_of(i), d, v.rgb, P.blend[i].y * m * v.a, false);
}

// A paint layer's texels onto `d` in slot `i`.
fn paint_slot(i: u32, t: texture_2d<f32>, d: vec3<f32>, p: vec2<i32>, m: f32, is_normal: bool) -> vec3<f32> {
    if P.blend[i].z < 0.5 {
        return d;
    }
    let s = textureLoad(t, p, 0);
    if s.a <= 0.0 {
        return d;
    }
    let v = select(s.rgb, normalize(s.xyz), is_normal);
    return composite(mode_of(i), d, v, s.a * P.blend[i].y * m, is_normal);
}

// Folders: children were composited on top of the layers below (`s`), giving `o`.
// ponytail: Normal folders are not isolated (same as Passthrough); verify against Painter.
fn folder_slot(i: u32, d: vec3<f32>, s: vec3<f32>, a: f32, is_normal: bool) -> vec3<f32> {
    let mode = mode_of(i);
    if mode <= 1u {
        let r = mix(d, s, a);
        return select(r, normalize(r), is_normal);
    }
    return composite(mode, d, s, a, is_normal);
}

// ---------------------------------------------------------------- core channels (group 0)

struct Ch {
    base: vec3<f32>,
    metal: f32,
    rough: f32,
    normal: vec3<f32>,
    height: f32,
    coverage: f32,
    filled: f32,
    emissive: vec3<f32>,
    opacity: f32,
};

fn unpack(a: vec4<f32>, b: vec4<f32>, c: vec4<f32>, d: vec4<f32>) -> Ch {
    return Ch(a.rgb, a.a, b.a, b.xyz, c.x, c.y, c.z, d.rgb, c.w);
}

fn load_s(p: vec2<i32>) -> Ch {
    return unpack(textureLoad(s0, p, 0), textureLoad(s1, p, 0), textureLoad(s2, p, 0), textureLoad(s3, p, 0));
}

fn load_o(p: vec2<i32>) -> Ch {
    return unpack(textureLoad(o0, p, 0), textureLoad(o1, p, 0), textureLoad(o2, p, 0), textureLoad(o3, p, 0));
}

fn pack(c: Ch) -> Out {
    var o: Out;
    o.t0 = vec4(clamp(c.base, vec3(0.0), vec3(1.0)), clamp(c.metal, 0.0, 1.0));
    o.t1 = vec4(normalize(c.normal), clamp(c.rough, 0.0, 1.0));
    o.t2 = vec4(c.height, c.coverage, c.filled, clamp(c.opacity, 0.0, 1.0));
    o.t3 = vec4(clamp(c.emissive, vec3(0.0), vec3(1.0)), 1.0);
    return o;
}

// The empty stack (Channel::empty_value in reetched-doc, sent as P.defaults).
fn empty() -> Ch {
    return unpack(P.defaults[0], P.defaults[1], P.defaults[2], P.defaults[3]);
}

// The empty stack, with the baked normal map (if any) as the base normal, like Painter.
@fragment
fn fs_init(in: VsOut) -> Out {
    var c = empty();
    if P.maps.x > 0.5 {
        c.normal = normalize(mesh_map(m_normal, in.uv).xyz * 2.0 - 1.0);
    }
    return pack(c);
}

@fragment
fn fs_fill(in: VsOut) -> Out {
    let p = vec2<i32>(in.clip.xy);
    var c = load_s(p);
    let n = normalize(in.normal);
    let m = mask_value(in, n, p);

    c.base = fill_slot(0u, t_c0, c.base, in, n, m);
    c.metal = fill_slot(1u, t_c1, vec3(c.metal), in, n, m).x;
    c.rough = fill_slot(2u, t_c2, vec3(c.rough), in, n, m).x;
    if on(3u) {
        var v = P.values[3].xyz;
        var a = 1.0;
        // Normal bitmaps are OpenGL-convention tangent-space maps.
        if P.blend[3].z > 0.5 {
            let s = sample_proj(t_c3, P.proj, in, n);
            v = s.rgb * 2.0 - 1.0;
            a = s.a;
        }
        if a > 0.0 {
            c.normal = composite(mode_of(3u), c.normal, normalize(v), P.blend[3].y * m * a, true);
        }
    }
    if on(4u) {
        var v = P.values[4].x;
        var a = 1.0;
        // ponytail: bitmap mid-grey = height 0, matching export's +0.5; revisit with Painter goldens.
        if P.blend[4].z > 0.5 {
            let s = sample_proj(t_c4, P.proj, in, n);
            v = s.r - 0.5;
            a = s.a;
        }
        c.height = composite(mode_of(4u), vec3(c.height), vec3(v), P.blend[4].y * m * a, false).x;
    }
    c.emissive = fill_slot(5u, t_c5, c.emissive, in, n, m);
    c.opacity = fill_slot(6u, t_c6, vec3(c.opacity), in, n, m).x;
    c.coverage = 1.0;
    c.filled = 1.0;
    return pack(c);
}

// Paint layers: painted texels (color + coverage alpha) per channel, same size as the set.
@fragment
fn fs_paint(in: VsOut) -> Out {
    let p = vec2<i32>(in.clip.xy);
    var c = load_s(p);
    let m = mask_value(in, normalize(in.normal), p);
    c.base = paint_slot(0u, t_c0, c.base, p, m, false);
    c.metal = paint_slot(1u, t_c1, vec3(c.metal), p, m, false).x;
    c.rough = paint_slot(2u, t_c2, vec3(c.rough), p, m, false).x;
    c.normal = paint_slot(3u, t_c3, c.normal, p, m, true);
    c.height = paint_slot(4u, t_c4, vec3(c.height), p, m, false).x;
    c.emissive = paint_slot(5u, t_c5, c.emissive, p, m, false);
    c.opacity = paint_slot(6u, t_c6, vec3(c.opacity), p, m, false).x;
    c.coverage = 1.0;
    c.filled = 1.0;
    return pack(c);
}

@fragment
fn fs_folder(in: VsOut) -> Out {
    let p = vec2<i32>(in.clip.xy);
    var c = load_s(p);
    let o = load_o(p);
    let m = mask_value(in, normalize(in.normal), p);
    c.base = folder_slot(0u, c.base, o.base, P.blend[0].y * m, false);
    c.metal = folder_slot(1u, vec3(c.metal), vec3(o.metal), P.blend[1].y * m, false).x;
    c.rough = folder_slot(2u, vec3(c.rough), vec3(o.rough), P.blend[2].y * m, false).x;
    c.normal = folder_slot(3u, c.normal, o.normal, P.blend[3].y * m, true);
    c.height = folder_slot(4u, vec3(c.height), vec3(o.height), P.blend[4].y * m, false).x;
    c.emissive = folder_slot(5u, c.emissive, o.emissive, P.blend[5].y * m, false);
    c.opacity = folder_slot(6u, vec3(c.opacity), vec3(o.opacity), P.blend[6].y * m, false).x;
    c.coverage = 1.0;
    c.filled = 1.0;
    return pack(c);
}

// ---------------------------------------------------------------- extra channels (groups 1, 2)

struct Ex {
    v0: vec3<f32>,
    v1: vec3<f32>,
    // Slots 2..6.
    s: array<f32, 5>,
    coverage: f32,
    filled: f32,
};

fn unpack_ex(a: vec4<f32>, b: vec4<f32>, c: vec4<f32>, d: vec4<f32>) -> Ex {
    return Ex(a.rgb, b.rgb, array<f32, 5>(a.a, b.a, c.x, c.y, c.z), d.x, d.y);
}

fn load_se(p: vec2<i32>) -> Ex {
    return unpack_ex(textureLoad(s0, p, 0), textureLoad(s1, p, 0), textureLoad(s2, p, 0), textureLoad(s3, p, 0));
}

fn load_oe(p: vec2<i32>) -> Ex {
    return unpack_ex(textureLoad(o0, p, 0), textureLoad(o1, p, 0), textureLoad(o2, p, 0), textureLoad(o3, p, 0));
}

fn pack_ex(e: Ex) -> Out {
    var o: Out;
    let c = vec3(0.0);
    let one = vec3(1.0);
    o.t0 = vec4(clamp(e.v0, c, one), clamp(e.s[0], 0.0, 1.0));
    o.t1 = vec4(clamp(e.v1, c, one), clamp(e.s[1], 0.0, 1.0));
    o.t2 = vec4(clamp(e.s[2], 0.0, 1.0), clamp(e.s[3], 0.0, 1.0), clamp(e.s[4], 0.0, 1.0), 0.0);
    o.t3 = vec4(e.coverage, e.filled, 0.0, 0.0);
    return o;
}

fn empty_ex() -> Ex {
    return unpack_ex(P.defaults[0], P.defaults[1], P.defaults[2], P.defaults[3]);
}

@fragment
fn fs_init_ex(in: VsOut) -> Out {
    return pack_ex(empty_ex());
}

@fragment
fn fs_fill_ex(in: VsOut) -> Out {
    let p = vec2<i32>(in.clip.xy);
    var e = load_se(p);
    let n = normalize(in.normal);
    let m = mask_value(in, n, p);
    e.v0 = fill_slot(0u, t_c0, e.v0, in, n, m);
    e.v1 = fill_slot(1u, t_c1, e.v1, in, n, m);
    e.s[0] = fill_slot(2u, t_c2, vec3(e.s[0]), in, n, m).x;
    e.s[1] = fill_slot(3u, t_c3, vec3(e.s[1]), in, n, m).x;
    e.s[2] = fill_slot(4u, t_c4, vec3(e.s[2]), in, n, m).x;
    e.s[3] = fill_slot(5u, t_c5, vec3(e.s[3]), in, n, m).x;
    e.s[4] = fill_slot(6u, t_c6, vec3(e.s[4]), in, n, m).x;
    e.coverage = 1.0;
    e.filled = 1.0;
    return pack_ex(e);
}

@fragment
fn fs_paint_ex(in: VsOut) -> Out {
    let p = vec2<i32>(in.clip.xy);
    var e = load_se(p);
    let m = mask_value(in, normalize(in.normal), p);
    e.v0 = paint_slot(0u, t_c0, e.v0, p, m, false);
    e.v1 = paint_slot(1u, t_c1, e.v1, p, m, false);
    e.s[0] = paint_slot(2u, t_c2, vec3(e.s[0]), p, m, false).x;
    e.s[1] = paint_slot(3u, t_c3, vec3(e.s[1]), p, m, false).x;
    e.s[2] = paint_slot(4u, t_c4, vec3(e.s[2]), p, m, false).x;
    e.s[3] = paint_slot(5u, t_c5, vec3(e.s[3]), p, m, false).x;
    e.s[4] = paint_slot(6u, t_c6, vec3(e.s[4]), p, m, false).x;
    e.coverage = 1.0;
    e.filled = 1.0;
    return pack_ex(e);
}

@fragment
fn fs_folder_ex(in: VsOut) -> Out {
    let p = vec2<i32>(in.clip.xy);
    var e = load_se(p);
    let o = load_oe(p);
    let m = mask_value(in, normalize(in.normal), p);
    e.v0 = folder_slot(0u, e.v0, o.v0, P.blend[0].y * m, false);
    e.v1 = folder_slot(1u, e.v1, o.v1, P.blend[1].y * m, false);
    for (var k = 0u; k < 5u; k++) {
        e.s[k] = folder_slot(k + 2u, vec3(e.s[k]), vec3(o.s[k]), P.blend[k + 2u].y * m, false).x;
    }
    e.coverage = 1.0;
    e.filled = 1.0;
    return pack_ex(e);
}

// ---------------------------------------------------------------- full-texture passes

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4(p, 0.0, 1.0);
}

fn height_at(p: vec2<i32>, fallback: f32) -> f32 {
    let size = vec2<i32>(textureDimensions(s2));
    let q = clamp(p, vec2(0), size - 1);
    let t = textureLoad(s2, q, 0);
    // Outside the islands there is no height; don't let that create a cliff at the edge.
    return select(fallback, t.x, t.y > 0.5);
}

// Painter mixes the height channel into the normal (height-to-normal, then detail blend).
@fragment
fn fs_finalize(@builtin(position) frag: vec4<f32>) -> Out {
    let p = vec2<i32>(frag.xy);
    var c = load_s(p);
    if c.coverage > 0.5 {
        let h = c.height;
        let dhdu = (height_at(p + vec2(1, 0), h) - height_at(p - vec2(1, 0), h)) * 0.5;
        // Texture rows go down while v goes up.
        let dhdv = (height_at(p - vec2(0, 1), h) - height_at(p + vec2(0, 1), h)) * 0.5;
        let k = P.misc.x;
        let from_height = normalize(vec3(-dhdu * k, -dhdv * k, 1.0));
        c.normal = rnm(c.normal, from_height);
    }
    return pack(c);
}

// Grows filled texels one pixel outward so bilinear sampling doesn't pick up
// the background at UV seams. Coverage stays 0 so export still sees the true islands.
@fragment
fn fs_dilate(@builtin(position) frag: vec4<f32>) -> Out {
    let p = vec2<i32>(frag.xy);
    let c = load_s(p);
    if c.filled > 0.5 {
        return pack(c);
    }
    let size = vec2<i32>(textureDimensions(s0));
    var sum0 = vec4(0.0);
    var sum1 = vec4(0.0);
    var sum3 = vec4(0.0);
    var h = 0.0;
    var op = 0.0;
    var count = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let q = clamp(p + vec2(dx, dy), vec2(0), size - 1);
            let t2 = textureLoad(s2, q, 0);
            if t2.z > 0.5 {
                sum0 += textureLoad(s0, q, 0);
                sum1 += textureLoad(s1, q, 0);
                sum3 += textureLoad(s3, q, 0);
                h += t2.x;
                op += t2.w;
                count += 1.0;
            }
        }
    }
    if count == 0.0 {
        return pack(c);
    }
    var o: Out;
    o.t0 = sum0 / count;
    o.t1 = vec4(normalize(sum1.xyz), sum1.w / count);
    o.t2 = vec4(h / count, 0.0, 1.0, op / count);
    o.t3 = sum3 / count;
    return o;
}

// The same for groups 1 and 2, where `filled` lives in t3.y.
@fragment
fn fs_dilate_ex(@builtin(position) frag: vec4<f32>) -> Out {
    let p = vec2<i32>(frag.xy);
    let t3 = textureLoad(s3, p, 0);
    var o: Out;
    o.t0 = textureLoad(s0, p, 0);
    o.t1 = textureLoad(s1, p, 0);
    o.t2 = textureLoad(s2, p, 0);
    o.t3 = t3;
    if t3.y > 0.5 {
        return o;
    }
    let size = vec2<i32>(textureDimensions(s0));
    var sum0 = vec4(0.0);
    var sum1 = vec4(0.0);
    var sum2 = vec4(0.0);
    var count = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let q = clamp(p + vec2(dx, dy), vec2(0), size - 1);
            if textureLoad(s3, q, 0).y > 0.5 {
                sum0 += textureLoad(s0, q, 0);
                sum1 += textureLoad(s1, q, 0);
                sum2 += textureLoad(s2, q, 0);
                count += 1.0;
            }
        }
    }
    if count > 0.0 {
        o.t0 = sum0 / count;
        o.t1 = sum1 / count;
        o.t2 = sum2 / count;
        o.t3 = vec4(0.0, 1.0, 0.0, 0.0);
    }
    return o;
}
