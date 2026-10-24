// Viewport: sky + PBR mesh + wireframe into an HDR target, then tone mapping.
// Also the 2D UV view, drawn straight to its display target.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x: exposure multiplier, y: env rotation (radians), z: env max lod, w: background lod
    params: vec4<f32>,
    // x: 3D view mode, y: environment opacity, z: 2D view mode
    view: vec4<f32>,
    // 2D view: ndc = uv * xy + zw
    uv_xform: vec4<f32>,
    sh: array<vec4<f32>, 9>,
    // x: tone map (0 ACES, 1 AgX, 2 Neutral, 3 linear), y: bloom strength, z: vignette,
    // w: emissive intensity
    post: vec4<f32>,
    // x: alpha mode (0 opaque, 1 cutout, 2 blend), y: cutoff, z: parallax depth, w: frame
    extra: vec4<f32>,
    // x: bloom threshold, y: weight of this frame in the accumulated image
    accum: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var env_tex: texture_2d<f32>;
@group(0) @binding(2) var env_smp: sampler;

const PI: f32 = 3.14159265;
// Viewport background, linear (#262626 like Painter).
const BACKGROUND: vec3<f32> = vec3(0.0194, 0.0194, 0.0194);

// View modes; keep in sync with ViewMode in viewport.rs.
const MODE_MATERIAL: u32 = 0u;
const MODE_BASE_COLOR: u32 = 1u;
const MODE_ROUGHNESS: u32 = 2u;
const MODE_METALLIC: u32 = 3u;
const MODE_NORMAL: u32 = 4u;
const MODE_UV: u32 = 5u;
const MODE_MESH_MAP: u32 = 6u;
const MODE_EMISSIVE: u32 = 7u;
const MODE_OPACITY: u32 = 8u;

// Rotates a world direction into the environment's frame.
fn env_frame(d: vec3<f32>) -> vec3<f32> {
    let c = cos(g.params.y);
    let s = sin(g.params.y);
    return vec3(c * d.x - s * d.z, d.y, s * d.x + c * d.z);
}

fn env(d: vec3<f32>, lod: f32) -> vec3<f32> {
    let r = env_frame(d);
    let uv = vec2(atan2(r.z, r.x) / (2.0 * PI) + 0.5, acos(clamp(r.y, -1.0, 1.0)) / PI);
    return textureSampleLevel(env_tex, env_smp, uv, lod).rgb;
}

// Same basis order and constants as env.rs.
fn sh_irradiance(n_world: vec3<f32>) -> vec3<f32> {
    let n = env_frame(n_world);
    var e = g.sh[0].rgb * 0.282095;
    e += g.sh[1].rgb * 0.488603 * n.y;
    e += g.sh[2].rgb * 0.488603 * n.z;
    e += g.sh[3].rgb * 0.488603 * n.x;
    e += g.sh[4].rgb * 1.092548 * n.x * n.y;
    e += g.sh[5].rgb * 1.092548 * n.y * n.z;
    e += g.sh[6].rgb * 0.315392 * (3.0 * n.z * n.z - 1.0);
    e += g.sh[7].rgb * 1.092548 * n.x * n.z;
    e += g.sh[8].rgb * 0.546274 * (n.x * n.x - n.y * n.y);
    return max(e, vec3(0.0));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let c0 = max(c, vec3(0.0));
    return select(1.055 * pow(c0, vec3(1.0 / 2.4)) - 0.055, c0 * 12.92, c0 <= vec3(0.0031308));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}

// Solo-channel output, linear (encoded for display afterwards). Base color is already
// linear; data channels are shown as raw values (0.5 roughness = 50% grey on screen).
fn solo(mode: u32, s: Surface, n: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    let base = s.base;
    let rough = s.rough;
    let metal = s.metal;
    switch mode {
        case MODE_EMISSIVE: { return s.emissive; }
        case MODE_OPACITY: { return srgb_to_linear(vec3(s.opacity)); }
        case MODE_ROUGHNESS: { return srgb_to_linear(vec3(rough)); }
        case MODE_METALLIC: { return srgb_to_linear(vec3(metal)); }
        case MODE_NORMAL: { return srgb_to_linear(n * 0.5 + 0.5); }
        case MODE_UV: { return srgb_to_linear(vec3(fract(uv), 0.0)); }
        default: { return base; }
    }
}

// ---------------------------------------------------------------- mesh

// Composited texture set (see compositor.wgsl): t0 = base rgb + metallic,
// t1 = tangent normal xyz + roughness, t2 = height, coverage, filled.
@group(1) @binding(0) var set_t0: texture_2d<f32>;
@group(1) @binding(1) var set_t1: texture_2d<f32>;
@group(1) @binding(2) var set_t2: texture_2d<f32>;
@group(1) @binding(3) var set_smp: sampler;
// Baked ambient occlusion (white if none) and the mesh map shown in MODE_MESH_MAP.
@group(1) @binding(4) var set_ao: texture_2d<f32>;
@group(1) @binding(5) var set_preview: texture_2d<f32>;
// t3: emissive rgb.
@group(1) @binding(6) var set_t3: texture_2d<f32>;

fn image_uv(uv: vec2<f32>) -> vec2<f32> {
    return vec2(uv.x, 1.0 - uv.y);
}

struct Surface {
    base: vec3<f32>,
    metal: f32,
    rough: f32,
    normal_ts: vec3<f32>,
    height: f32,
    emissive: vec3<f32>,
    opacity: f32,
};

fn surface(uv: vec2<f32>) -> Surface {
    // Texture row 0 holds v = 1.
    let t = vec2(uv.x, 1.0 - uv.y);
    let a = textureSample(set_t0, set_smp, t);
    let b = textureSample(set_t1, set_smp, t);
    let c = textureSample(set_t2, set_smp, t);
    let d = textureSample(set_t3, set_smp, t);
    return Surface(a.rgb, a.a, b.a, normalize(b.xyz), c.x, d.rgb, c.w);
}

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
    @location(3) tangent: vec4<f32>,
};

@vertex
fn vs_mesh(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = g.view_proj * vec4(v.pos, 1.0);
    o.world = v.pos;
    o.normal = v.normal;
    o.uv = v.uv;
    o.tangent = v.tangent;
    return o;
}

// Interleaved gradient noise (Jimenez), varied per frame.
fn ign(p: vec2<f32>, frame: f32) -> f32 {
    let q = p + frame * 5.588238;
    return fract(52.9829189 * fract(dot(q, vec2(0.06711056, 0.00583715))));
}

// Parallax occlusion mapping: walks the height field along the view ray in
// tangent space and returns the UV where it's hit.
fn parallax_uv(uv: vec2<f32>, v_ts: vec3<f32>, depth: f32) -> vec2<f32> {
    let steps = 24.0;
    // Height in -0.5..0.5 → depth below the top in 0..1.
    let shift = v_ts.xy / max(v_ts.z, 0.15) * depth / steps;
    var cur = uv;
    var layer = 0.0;
    var d = 0.5 - textureSampleLevel(set_t2, set_smp, vec2(cur.x, 1.0 - cur.y), 0.0).x;
    var prev_cur = cur;
    var prev_gap = 0.0;
    for (var i = 0; i < 24; i++) {
        if layer >= d {
            break;
        }
        prev_cur = cur;
        prev_gap = d - layer;
        cur -= shift;
        layer += 1.0 / steps;
        d = 0.5 - textureSampleLevel(set_t2, set_smp, vec2(cur.x, 1.0 - cur.y), 0.0).x;
    }
    // Interpolate between the last two steps.
    let after = layer - d;
    let w = after / max(after + prev_gap, 1e-5);
    return mix(cur, prev_cur, w);
}

@fragment
fn fs_pbr(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // MikkTSpace frame: bitangent = sign * cross(normal, tangent).
    var geo_n = normalize(in.normal);
    var t = in.tangent.xyz - geo_n * dot(geo_n, in.tangent.xyz);
    t = select(normalize(t), vec3(1.0, 0.0, 0.0), dot(t, t) < 1e-8);
    let b = cross(geo_n, t) * in.tangent.w;
    var uv = in.uv;
    if g.extra.z > 0.0 && u32(g.view.x) == MODE_MATERIAL {
        let v = normalize(g.eye.xyz - in.world);
        uv = parallax_uv(in.uv, vec3(dot(v, t), dot(v, b), dot(v, geo_n)), g.extra.z);
    }
    let s = surface(uv);
    // Opacity: alpha test, or stochastic transparency that accumulation smooths out.
    let alpha_mode = u32(g.extra.x);
    if alpha_mode == 1u && s.opacity < g.extra.y {
        discard;
    }
    if alpha_mode == 2u && s.opacity <= ign(in.clip.xy, g.extra.w) {
        discard;
    }
    var n = normalize(t * s.normal_ts.x + b * s.normal_ts.y + geo_n * s.normal_ts.z);
    if !front { n = -n; }
    let base = s.base;
    let rough = clamp(s.rough, 0.02, 1.0);
    let metal = s.metal;

    let mode = u32(g.view.x);
    if mode == MODE_MESH_MAP {
        return vec4(srgb_to_linear(textureSample(set_preview, set_smp, image_uv(uv)).rgb), 1.0);
    }
    if mode != MODE_MATERIAL {
        return vec4(solo(mode, s, n, uv), 1.0);
    }
    let ao = textureSample(set_ao, set_smp, image_uv(uv)).r;

    let v = normalize(g.eye.xyz - in.world);
    let f0 = mix(vec3(0.04), base, metal);
    let nv = max(dot(n, v), 1e-4);

    // ponytail: analytic split-sum (Karis' mobile EnvBRDFApprox) instead of a BRDF LUT;
    // swap in a LUT if speculars drift from Painter's reference renders.
    let rr = rough * vec4(-1.0, -0.0275, -0.572, 0.022) + vec4(1.0, 0.0425, 1.04, -0.04);
    let a004 = min(rr.x * rr.x, exp2(-9.28 * nv)) * rr.x + rr.y;
    let ab = vec2(-1.04, 1.04) * a004 + rr.zw;

    let spec = env(reflect(-v, n), rough * g.params.z) * (f0 * ab.x + ab.y);
    let diffuse = sh_irradiance(n) * base * (1.0 - metal);
    // Painter multiplies baked AO into the ambient lighting.
    return vec4((diffuse + spec) * ao + s.emissive * g.post.w, 1.0);
}

@vertex
fn vs_wire(v: VsIn) -> @builtin(position) vec4<f32> {
    // Nudge toward the camera so the wire wins the depth test against its own surface.
    let to_eye = g.eye.xyz - v.pos;
    return g.view_proj * vec4(v.pos + to_eye * 0.002, 1.0);
}

@fragment
fn fs_wire() -> @location(0) vec4<f32> {
    return vec4(0.02, 0.02, 0.02, 1.0);
}

// ---------------------------------------------------------------- full-screen passes

struct Fullscreen {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> Fullscreen {
    let p = vec2(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var o: Fullscreen;
    o.clip = vec4(p, 0.0, 1.0);
    o.ndc = p;
    return o;
}

// Alpha carries how much environment shows through over the flat background.
@fragment
fn fs_sky(in: Fullscreen) -> @location(0) vec4<f32> {
    if u32(g.view.x) != MODE_MATERIAL {
        return vec4(0.0);
    }
    let far = g.inv_view_proj * vec4(in.ndc, 1.0, 1.0);
    let dir = normalize(far.xyz / far.w - g.eye.xyz);
    return vec4(env(dir, g.params.w), g.view.y);
}

// Post passes: two inputs and a sampler.
@group(1) @binding(0) var post_a: texture_2d<f32>;
@group(1) @binding(1) var post_b: texture_2d<f32>;
@group(1) @binding(2) var post_smp: sampler;

// Progressive anti-aliasing: average the jittered frames while nothing changes.
@fragment
fn fs_accumulate(in: Fullscreen) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip.xy);
    return mix(textureLoad(post_b, p, 0), textureLoad(post_a, p, 0), g.accum.y);
}

fn post_uv(in: Fullscreen) -> vec2<f32> {
    return in.ndc * vec2(0.5, -0.5) + 0.5;
}

// Bloom: bright parts of the (exposed) image at quarter resolution.
@fragment
fn fs_bloom_down(in: Fullscreen) -> @location(0) vec4<f32> {
    let uv = post_uv(in);
    let texel = 1.0 / vec2<f32>(textureDimensions(post_a));
    var c = vec3(0.0);
    for (var y = -1; y <= 1; y += 2) {
        for (var x = -1; x <= 1; x += 2) {
            let s = textureSampleLevel(post_a, post_smp, uv + vec2(f32(x), f32(y)) * texel, 0.0);
            c += s.rgb * s.a;
        }
    }
    c *= 0.25 * g.params.x;
    let bright = max(c.r, max(c.g, c.b));
    let knee = 0.5;
    let soft = clamp(bright - g.accum.x + knee, 0.0, 2.0 * knee);
    let amount = max(soft * soft / (4.0 * knee), bright - g.accum.x) / max(bright, 1e-4);
    return vec4(c * max(amount, 0.0), 1.0);
}

fn blur(in: Fullscreen, dir: vec2<f32>) -> vec4<f32> {
    let uv = post_uv(in);
    let step = dir / vec2<f32>(textureDimensions(post_a)) * 1.5;
    let w = array<f32, 5>(0.227027, 0.1945946, 0.1216216, 0.054054, 0.016216);
    var c = textureSampleLevel(post_a, post_smp, uv, 0.0).rgb * w[0];
    for (var i = 1; i < 5; i++) {
        c += textureSampleLevel(post_a, post_smp, uv + step * f32(i), 0.0).rgb * w[i];
        c += textureSampleLevel(post_a, post_smp, uv - step * f32(i), 0.0).rgb * w[i];
    }
    return vec4(c, 1.0);
}

@fragment
fn fs_blur_h(in: Fullscreen) -> @location(0) vec4<f32> {
    return blur(in, vec2(1.0, 0.0));
}

@fragment
fn fs_blur_v(in: Fullscreen) -> @location(0) vec4<f32> {
    return blur(in, vec2(0.0, 1.0));
}

// Narkowicz's ACES fit.
fn aces(x: vec3<f32>) -> vec3<f32> {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3(0.0), vec3(1.0));
}

// AgX (Troy Sobotka), with the common polynomial fit of its base curve. Returns linear.
fn agx(c: vec3<f32>) -> vec3<f32> {
    let inset = mat3x3<f32>(
        vec3(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
        vec3(0.0784335999999992, 0.878468636469772, 0.0784336),
        vec3(0.0792237451477643, 0.0791661274605434, 0.879142973793104),
    );
    let outset = mat3x3<f32>(
        vec3(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
        vec3(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
        vec3(-0.0990297440797205, -0.0989611768448433, 1.15107367264116),
    );
    var v = inset * max(c, vec3(1e-10));
    v = (clamp(log2(v), vec3(-12.47393), vec3(4.026069)) + 12.47393) / 16.5;
    let v2 = v * v;
    let v4 = v2 * v2;
    v = 15.5 * v4 * v2 - 40.14 * v4 * v + 31.96 * v4 - 6.868 * v2 * v + 0.4298 * v2 + 0.1191 * v - 0.00232;
    v = outset * v;
    return pow(max(v, vec3(0.0)), vec3(2.2));
}

// Khronos PBR Neutral.
fn neutral(c: vec3<f32>) -> vec3<f32> {
    let start = 0.76;
    let desat = 0.15;
    let x = min(c.r, min(c.g, c.b));
    let offset = select(0.04, x - 6.25 * x * x, x < 0.08);
    var col = c - offset;
    let peak = max(col.r, max(col.g, col.b));
    if peak < start {
        return col;
    }
    let d = 1.0 - start;
    let new_peak = 1.0 - d * d / (peak + d - start);
    col *= new_peak / peak;
    let gray = 1.0 - 1.0 / (desat * (peak - new_peak) + 1.0);
    return mix(col, vec3(new_peak), gray);
}

fn tone_map(mode: u32, c: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 1u: { return agx(c); }
        case 2u: { return neutral(c); }
        case 3u: { return clamp(c, vec3(0.0), vec3(1.0)); }
        default: { return aces(c); }
    }
}

@fragment
fn fs_tonemap(in: Fullscreen) -> @location(0) vec4<f32> {
    let c = textureLoad(post_a, vec2<i32>(in.clip.xy), 0);
    var rgb = c.rgb;
    if u32(g.view.x) == MODE_MATERIAL {
        let bloom = textureSampleLevel(post_b, post_smp, post_uv(in), 0.0).rgb;
        rgb = tone_map(u32(g.post.x), rgb * g.params.x + bloom * g.post.y);
        rgb *= 1.0 - g.post.z * smoothstep(0.5, 1.5, length(in.ndc));
    }
    // egui shows the display texture as-is, so encode to sRGB here.
    return vec4(linear_to_srgb(mix(BACKGROUND, rgb, c.a)), 1.0);
}

// ---------------------------------------------------------------- 2D UV view

@vertex
fn vs_uv(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = vec4(v.uv * g.uv_xform.xy + g.uv_xform.zw, 0.0, 1.0);
    o.world = v.pos;
    o.normal = v.normal;
    o.uv = v.uv;
    o.tangent = v.tangent;
    return o;
}

@fragment
fn fs_uv(in: VsOut) -> @location(0) vec4<f32> {
    // Unlit: "Material" mode shows base color, like Painter's 2D view.
    let mode = u32(g.view.z);
    if mode == MODE_MESH_MAP {
        return vec4(textureSample(set_preview, set_smp, image_uv(in.uv)).rgb, 1.0);
    }
    let s = surface(in.uv);
    // The 2D view shows the tangent-space normal map itself.
    let c = solo(mode, s, s.normal_ts, in.uv);
    return vec4(linear_to_srgb(c), 1.0);
}

@vertex
fn vs_uv_wire(v: VsIn) -> @builtin(position) vec4<f32> {
    return vec4(v.uv * g.uv_xform.xy + g.uv_xform.zw, 0.0, 1.0);
}

@fragment
fn fs_uv_wire() -> @location(0) vec4<f32> {
    return vec4(0.1, 0.1, 0.1, 1.0); // already display-encoded
}
