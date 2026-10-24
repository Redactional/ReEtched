// Layer-stack compositor. Every pass renders the mesh in UV space into three
// rgba16float targets (a "channel set"):
//   t0 = base color (linear rgb), metallic
//   t1 = normal (tangent space xyz), roughness
//   t2 = height, coverage (1 inside UV islands), filled (1 = has a value, incl. dilation), 0
// Channel order everywhere: 0 base color, 1 metallic, 2 roughness, 3 normal, 4 height.

struct Proj {
    // x: mode (0 uv, 1 tri-planar, 2 planar), y: uv rotation (rad), z: tri-planar exponent, w: 1 / size
    a: vec4<f32>,
    // xy: uv scale, zw: uv offset
    so: vec4<f32>,
    origin: vec4<f32>,
    // Rows of the inverse projector rotation.
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
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
    // xyz: uniform value, w: channel enabled
    values: array<vec4<f32>, 5>,
    // x: blend mode, y: opacity, z: has bitmap
    blend: array<vec4<f32>, 5>,
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
};

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var t_base: texture_2d<f32>;
@group(0) @binding(3) var t_metal: texture_2d<f32>;
@group(0) @binding(4) var t_rough: texture_2d<f32>;
@group(0) @binding(5) var t_normal: texture_2d<f32>;
@group(0) @binding(6) var t_height: texture_2d<f32>;
@group(0) @binding(7) var t_mask: texture_2d<f32>;
// Painted mask texels (value in r, coverage in a), same size as the texture set.
@group(0) @binding(8) var t_mask_paint: texture_2d<f32>;
// Baked mesh maps (image orientation: row 0 = v 1).
@group(0) @binding(9) var m_normal: texture_2d<f32>;
@group(0) @binding(10) var m_curvature: texture_2d<f32>;
@group(0) @binding(11) var m_ao: texture_2d<f32>;
@group(0) @binding(12) var m_position: texture_2d<f32>;
@group(0) @binding(13) var m_thickness: texture_2d<f32>;
@group(0) @binding(14) var m_id: texture_2d<f32>;

fn mesh_map(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(t, smp, vec2(uv.x, 1.0 - uv.y), 0.0);
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

fn generator_value(fx: MaskFx, uv: vec2<f32>, pos: vec3<f32>) -> f32 {
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
        default: { v = fbm(noise_p); }
    }
    if fx.a.z > 0.0 && u32(fx.b.x) != 5u {
        v += (fbm(noise_p) - 0.5) * fx.a.z;
    }
    var m = smoothstep(fx.a.x - fx.a.y, fx.a.x + fx.a.y, v);
    if fx.b.z > 0.5 {
        m = 1.0 - m;
    }
    return m;
}

fn effect_value(fx: MaskFx, below: f32, uv: vec2<f32>, pos: vec3<f32>) -> f32 {
    switch u32(fx.head.x) {
        case 1u: { return generator_value(fx, uv, pos); }
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
// Folder passes: the folder's content.
@group(2) @binding(0) var o0: texture_2d<f32>;
@group(2) @binding(1) var o1: texture_2d<f32>;
@group(2) @binding(2) var o2: texture_2d<f32>;

struct Ch {
    base: vec3<f32>,
    metal: f32,
    rough: f32,
    normal: vec3<f32>,
    height: f32,
    coverage: f32,
    filled: f32,
};

struct Out {
    @location(0) t0: vec4<f32>,
    @location(1) t1: vec4<f32>,
    @location(2) t2: vec4<f32>,
};

fn unpack(a: vec4<f32>, b: vec4<f32>, c: vec4<f32>) -> Ch {
    return Ch(a.rgb, a.a, b.a, b.xyz, c.x, c.y, c.z);
}

fn load_s(p: vec2<i32>) -> Ch {
    return unpack(textureLoad(s0, p, 0), textureLoad(s1, p, 0), textureLoad(s2, p, 0));
}

fn load_o(p: vec2<i32>) -> Ch {
    return unpack(textureLoad(o0, p, 0), textureLoad(o1, p, 0), textureLoad(o2, p, 0));
}

fn pack(c: Ch) -> Out {
    var o: Out;
    o.t0 = vec4(clamp(c.base, vec3(0.0), vec3(1.0)), clamp(c.metal, 0.0, 1.0));
    o.t1 = vec4(normalize(c.normal), clamp(c.rough, 0.0, 1.0));
    o.t2 = vec4(c.height, c.coverage, c.filled, 0.0);
    return o;
}

// Must match Channel::empty_value in reetched-doc.
fn empty() -> Ch {
    return Ch(vec3(0.214), 0.0, 0.5, vec3(0.0, 0.0, 1.0), 0.0, 1.0, 1.0);
}

// ---------------------------------------------------------------- mesh in UV space

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_uv(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = vec4(v.uv * 2.0 - 1.0, 0.0, 1.0);
    o.world = v.pos;
    o.normal = v.normal;
    o.uv = v.uv;
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

fn sample_proj(t: texture_2d<f32>, pr: Proj, uv: vec2<f32>, pos: vec3<f32>, n: vec3<f32>) -> vec4<f32> {
    let mode = u32(pr.a.x);
    if mode == 0u {
        // Images have v = 1 at the top row; our UVs put v = 0 at the bottom.
        let t0 = uv_xform(uv, pr);
        return textureSample(t, smp, vec2(t0.x, 1.0 - t0.y));
    }
    let q = local_dir((pos - pr.origin.xyz) * pr.a.w, pr);
    if mode == 2u {
        let t0 = uv_xform(q.xy + 0.5, pr);
        return textureSample(t, smp, vec2(t0.x, 1.0 - t0.y));
    }
    var w = pow(abs(local_dir(n, pr)), vec3(pr.a.z));
    w /= max(w.x + w.y + w.z, 1e-5);
    let x = textureSample(t, smp, uv_xform(q.zy, pr) * vec2(1.0, -1.0));
    let y = textureSample(t, smp, uv_xform(q.xz, pr) * vec2(1.0, -1.0));
    let z = textureSample(t, smp, uv_xform(q.xy, pr) * vec2(1.0, -1.0));
    return x * w.x + y * w.y + z * w.z;
}

fn mask_value(uv: vec2<f32>, pos: vec3<f32>, n: vec3<f32>, p: vec2<i32>) -> f32 {
    if P.mask.x < 0.5 {
        return 1.0;
    }
    var m = P.mask.y;
    if P.mask.z > 0.5 {
        m = sample_proj(t_mask, P.mask_proj, uv, pos, n).r;
    }
    for (var i = 0u; i < 4u; i++) {
        let fx = P.fx[i];
        if u32(fx.head.x) == 0u {
            continue;
        }
        m = mask_blend(u32(fx.head.y), m, effect_value(fx, m, uv, pos), fx.head.z);
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

// ---------------------------------------------------------------- passes

// The empty stack, with the baked normal map (if any) as the base normal, like Painter.
@fragment
fn fs_init(in: VsOut) -> Out {
    var c = empty();
    if P.maps.x > 0.5 {
        c.normal = normalize(mesh_map(m_normal, in.uv).xyz * 2.0 - 1.0);
    }
    return pack(c);
}

fn mode_of(i: u32) -> u32 {
    return u32(P.blend[i].x);
}

@fragment
fn fs_fill(in: VsOut) -> Out {
    let p = vec2<i32>(in.clip.xy);
    var c = load_s(p);
    let n = normalize(in.normal);
    let m = mask_value(in.uv, in.world, n, p);

    if P.values[0].w > 0.5 {
        var v = P.values[0].xyz;
        if P.blend[0].z > 0.5 { v = sample_proj(t_base, P.proj, in.uv, in.world, n).rgb; }
        c.base = composite(mode_of(0u), c.base, v, P.blend[0].y * m, false);
    }
    if P.values[1].w > 0.5 {
        var v = P.values[1].x;
        if P.blend[1].z > 0.5 { v = sample_proj(t_metal, P.proj, in.uv, in.world, n).r; }
        c.metal = composite(mode_of(1u), vec3(c.metal), vec3(v), P.blend[1].y * m, false).x;
    }
    if P.values[2].w > 0.5 {
        var v = P.values[2].x;
        if P.blend[2].z > 0.5 { v = sample_proj(t_rough, P.proj, in.uv, in.world, n).r; }
        c.rough = composite(mode_of(2u), vec3(c.rough), vec3(v), P.blend[2].y * m, false).x;
    }
    if P.values[3].w > 0.5 {
        var v = P.values[3].xyz;
        // Normal bitmaps are OpenGL-convention tangent-space maps.
        if P.blend[3].z > 0.5 { v = sample_proj(t_normal, P.proj, in.uv, in.world, n).rgb * 2.0 - 1.0; }
        c.normal = composite(mode_of(3u), c.normal, normalize(v), P.blend[3].y * m, true);
    }
    if P.values[4].w > 0.5 {
        var v = P.values[4].x;
        // ponytail: bitmap mid-grey = height 0, matching export's +0.5; revisit with Painter goldens.
        if P.blend[4].z > 0.5 { v = sample_proj(t_height, P.proj, in.uv, in.world, n).r - 0.5; }
        c.height = composite(mode_of(4u), vec3(c.height), vec3(v), P.blend[4].y * m, false).x;
    }
    c.coverage = 1.0;
    c.filled = 1.0;
    return pack(c);
}

// Paint layers: painted texels (color + coverage alpha) per channel, same size as the set.
@fragment
fn fs_paint(in: VsOut) -> Out {
    let p = vec2<i32>(in.clip.xy);
    var c = load_s(p);
    let m = mask_value(in.uv, in.world, normalize(in.normal), p);
    if P.blend[0].z > 0.5 {
        let t = textureLoad(t_base, p, 0);
        c.base = composite(mode_of(0u), c.base, t.rgb, t.a * P.blend[0].y * m, false);
    }
    if P.blend[1].z > 0.5 {
        let t = textureLoad(t_metal, p, 0);
        c.metal = composite(mode_of(1u), vec3(c.metal), vec3(t.r), t.a * P.blend[1].y * m, false).x;
    }
    if P.blend[2].z > 0.5 {
        let t = textureLoad(t_rough, p, 0);
        c.rough = composite(mode_of(2u), vec3(c.rough), vec3(t.r), t.a * P.blend[2].y * m, false).x;
    }
    if P.blend[3].z > 0.5 {
        let t = textureLoad(t_normal, p, 0);
        if t.a > 0.0 {
            c.normal = composite(mode_of(3u), c.normal, normalize(t.xyz), t.a * P.blend[3].y * m, true);
        }
    }
    if P.blend[4].z > 0.5 {
        let t = textureLoad(t_height, p, 0);
        c.height = composite(mode_of(4u), vec3(c.height), vec3(t.r), t.a * P.blend[4].y * m, false).x;
    }
    c.coverage = 1.0;
    c.filled = 1.0;
    return pack(c);
}

// Folders: children were composited on top of the layers below (`s`), giving `o`.
// ponytail: Normal folders are not isolated (same as Passthrough); verify against Painter.
fn folder_channel(i: u32, d: vec3<f32>, s: vec3<f32>, a: f32, is_normal: bool) -> vec3<f32> {
    let mode = mode_of(i);
    if mode <= 1u {
        let r = mix(d, s, a);
        return select(r, normalize(r), is_normal);
    }
    return composite(mode, d, s, a, is_normal);
}

@fragment
fn fs_folder(in: VsOut) -> Out {
    let p = vec2<i32>(in.clip.xy);
    var c = load_s(p);
    let o = load_o(p);
    let m = mask_value(in.uv, in.world, normalize(in.normal), p);
    c.base = folder_channel(0u, c.base, o.base, P.blend[0].y * m, false);
    c.metal = folder_channel(1u, vec3(c.metal), vec3(o.metal), P.blend[1].y * m, false).x;
    c.rough = folder_channel(2u, vec3(c.rough), vec3(o.rough), P.blend[2].y * m, false).x;
    c.normal = folder_channel(3u, c.normal, o.normal, P.blend[3].y * m, true);
    c.height = folder_channel(4u, vec3(c.height), vec3(o.height), P.blend[4].y * m, false).x;
    c.coverage = 1.0;
    c.filled = 1.0;
    return pack(c);
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
    var h = 0.0;
    var count = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let q = clamp(p + vec2(dx, dy), vec2(0), size - 1);
            let t2 = textureLoad(s2, q, 0);
            if t2.z > 0.5 {
                sum0 += textureLoad(s0, q, 0);
                sum1 += textureLoad(s1, q, 0);
                h += t2.x;
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
    o.t2 = vec4(h / count, 0.0, 1.0, 0.0);
    return o;
}
