// Viewport: sky + shaded mesh + wireframe into an HDR target (plus a subsurface
// scattering buffer), transparency (weighted blended OIT), then post passes.
// Also the 2D UV view, drawn straight to its display target.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x: exposure multiplier, y: env rotation (radians), z: env max lod, w: background lod
    params: vec4<f32>,
    // x: 3D view mode, y: environment opacity, z: 2D view mode, w: solo channel (index in Channel::ALL)
    view: vec4<f32>,
    // 2D view: ndc = uv * xy + zw
    uv_xform: vec4<f32>,
    sh: array<vec4<f32>, 9>,
    // x: tone map (0 ACES, 1 AgX, 2 Neutral, 3 linear, 4 OCIO LUT), y: bloom strength,
    // z: vignette, w: emissive intensity
    post: vec4<f32>,
    // x: alpha mode (0 opaque, 1 cutout, 2 dithered, 3 blended), y: cutoff,
    // z: parallax depth, w: frame number
    extra: vec4<f32>,
    // x: bloom threshold, y: weight of this frame in the accumulated image
    accum: vec4<f32>,
    // x: scattering radius (world units), y: pixels per world unit at distance 1,
    // z: scattering on
    sss: vec4<f32>,
    // OCIO display LUT shaper: x: log2 min, y: log2 max, z: LUT size
    lut: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var env_tex: texture_2d<f32>;
@group(0) @binding(2) var env_smp: sampler;
// The OCIO display transform baked into a 3D LUT (log2 shaper on the input).
@group(0) @binding(3) var display_lut: texture_3d<f32>;

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
// Any other channel, picked by g.view.w.
const MODE_CHANNEL: u32 = 9u;

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

// ---------------------------------------------------------------- mesh

// Composited texture set tile (see compositor.wgsl): core t0..t3, groups 1 and 2 t0..t2.
@group(1) @binding(0) var set_t0: texture_2d<f32>;
@group(1) @binding(1) var set_t1: texture_2d<f32>;
@group(1) @binding(2) var set_t2: texture_2d<f32>;
@group(1) @binding(3) var set_smp: sampler;
// Baked ambient occlusion (white if none) and the mesh map shown in MODE_MESH_MAP.
@group(1) @binding(4) var set_ao: texture_2d<f32>;
@group(1) @binding(5) var set_preview: texture_2d<f32>;
@group(1) @binding(6) var set_t3: texture_2d<f32>;
@group(1) @binding(7) var b_t0: texture_2d<f32>;
@group(1) @binding(8) var b_t1: texture_2d<f32>;
@group(1) @binding(9) var b_t2: texture_2d<f32>;
@group(1) @binding(10) var c_t0: texture_2d<f32>;
@group(1) @binding(11) var c_t1: texture_2d<f32>;
@group(1) @binding(12) var c_t2: texture_2d<f32>;

struct Material {
    // xy: UV origin of the UDIM tile
    tile: vec4<f32>,
    // x: shader model (0 metal/rough, 1 ASM, 2 OpenPBR)
    model: vec4<f32>,
};
@group(1) @binding(13) var<uniform> M: Material;

// Texture coordinate of a mesh UV within this tile (texture row 0 holds v = 1).
fn image_uv(uv: vec2<f32>) -> vec2<f32> {
    let t = uv - M.tile.xy;
    return vec2(t.x, 1.0 - t.y);
}

struct Surface {
    base: vec3<f32>,
    metal: f32,
    rough: f32,
    normal_ts: vec3<f32>,
    height: f32,
    emissive: vec3<f32>,
    opacity: f32,
    spec_level: f32,
    spec_color: vec3<f32>,
    aniso: f32,
    aniso_angle: f32,
    transmission: f32,
    scatter: f32,
    scatter_color: vec3<f32>,
    coat: f32,
    coat_color: vec3<f32>,
    coat_rough: f32,
    sheen: f32,
    sheen_color: vec3<f32>,
    sheen_rough: f32,
    ao: f32,
};

fn surface(uv: vec2<f32>) -> Surface {
    let t = image_uv(uv);
    let a = textureSample(set_t0, set_smp, t);
    let b = textureSample(set_t1, set_smp, t);
    let c = textureSample(set_t2, set_smp, t);
    let d = textureSample(set_t3, set_smp, t);
    let b0 = textureSample(b_t0, set_smp, t);
    let b1 = textureSample(b_t1, set_smp, t);
    let b2 = textureSample(b_t2, set_smp, t);
    let c0 = textureSample(c_t0, set_smp, t);
    let c1 = textureSample(c_t1, set_smp, t);
    let c2 = textureSample(c_t2, set_smp, t);
    var s: Surface;
    s.base = a.rgb;
    s.metal = a.a;
    s.rough = b.a;
    s.normal_ts = normalize(b.xyz);
    s.height = c.x;
    s.emissive = d.rgb;
    s.opacity = c.w;
    // Group 1: specular color, scattering color, specular level, anisotropy, angle,
    // transmission, scattering.
    s.spec_color = b0.rgb;
    s.scatter_color = b1.rgb;
    s.spec_level = b0.a;
    s.aniso = b1.a;
    s.aniso_angle = b2.x;
    s.transmission = b2.y;
    s.scatter = b2.z;
    // Group 2: coat color, sheen color, coat, coat roughness, sheen, sheen roughness, AO.
    s.coat_color = c0.rgb;
    s.sheen_color = c1.rgb;
    s.coat = c0.a;
    s.coat_rough = c1.a;
    s.sheen = c2.x;
    s.sheen_rough = c2.y;
    s.ao = c2.z;
    return s;
}

// A channel by its index in Channel::ALL (for solo views), and whether it's a color.
fn channel_value(s: Surface, i: u32) -> vec4<f32> {
    switch i {
        case 7u: { return vec4(vec3(s.spec_level), 0.0); }
        case 8u: { return vec4(s.spec_color, 1.0); }
        case 9u: { return vec4(vec3(s.aniso), 0.0); }
        case 10u: { return vec4(vec3(s.aniso_angle), 0.0); }
        case 11u: { return vec4(vec3(s.transmission), 0.0); }
        case 12u: { return vec4(vec3(s.scatter), 0.0); }
        case 13u: { return vec4(s.scatter_color, 1.0); }
        case 14u: { return vec4(vec3(s.coat), 0.0); }
        case 15u: { return vec4(s.coat_color, 1.0); }
        case 16u: { return vec4(vec3(s.coat_rough), 0.0); }
        case 17u: { return vec4(vec3(s.sheen), 0.0); }
        case 18u: { return vec4(s.sheen_color, 1.0); }
        case 19u: { return vec4(vec3(s.sheen_rough), 0.0); }
        default: { return vec4(vec3(s.ao), 0.0); }
    }
}

// Solo-channel output, linear (encoded for display afterwards). Color channels are
// already linear; data channels are shown as raw values (0.5 = 50% grey on screen).
fn solo(mode: u32, s: Surface, n: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    switch mode {
        case MODE_EMISSIVE: { return s.emissive; }
        case MODE_OPACITY: { return srgb_to_linear(vec3(s.opacity)); }
        case MODE_ROUGHNESS: { return srgb_to_linear(vec3(s.rough)); }
        case MODE_METALLIC: { return srgb_to_linear(vec3(s.metal)); }
        case MODE_NORMAL: { return srgb_to_linear(n * 0.5 + 0.5); }
        case MODE_UV: { return srgb_to_linear(vec3(fract(uv), 0.0)); }
        case MODE_CHANNEL: {
            let v = channel_value(s, u32(g.view.w));
            return select(srgb_to_linear(v.rgb), v.rgb, v.w > 0.5);
        }
        default: { return s.base; }
    }
}

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
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) uv1: vec2<f32>,
};

@vertex
fn vs_mesh(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = g.view_proj * vec4(v.pos, 1.0);
    o.world = v.pos;
    o.normal = v.normal;
    o.uv = v.uv;
    o.tangent = v.tangent;
    o.uv1 = v.uv1;
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
    var d = 0.5 - textureSampleLevel(set_t2, set_smp, image_uv(cur), 0.0).x;
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
        d = 0.5 - textureSampleLevel(set_t2, set_smp, image_uv(cur), 0.0).x;
    }
    // Interpolate between the last two steps.
    let after = layer - d;
    let w = after / max(after + prev_gap, 1e-5);
    return mix(cur, prev_cur, w);
}

// Split-sum environment BRDF (Karis' analytic EnvBRDFApprox): F0 scale and bias.
// ponytail: analytic fit instead of a BRDF LUT; swap in a LUT if speculars drift
// from Painter's reference renders.
fn env_brdf(rough: f32, nv: f32) -> vec2<f32> {
    let rr = rough * vec4(-1.0, -0.0275, -0.572, 0.022) + vec4(1.0, 0.0425, 1.04, -0.04);
    let a004 = min(rr.x * rr.x, exp2(-9.28 * nv)) * rr.x + rr.y;
    return vec2(-1.04, 1.04) * a004 + rr.zw;
}

// Directional albedo of a Charlie sheen lobe.
// ponytail: a rough fit of Estevez & Kulla's curves, not their table.
fn sheen_albedo(nv: f32, r: f32) -> f32 {
    return mix(0.14, 0.42, r) * (0.6 + 0.4 * pow(1.0 - nv, 3.0));
}

struct Shaded {
    color: vec3<f32>,
    // Diffuse light that scatters under the surface (blurred in screen space).
    sss: vec3<f32>,
    alpha: f32,
};

// The frame and the shaded surface of one fragment.
fn shade(in: VsOut, front: bool) -> Shaded {
    // MikkTSpace frame: bitangent = sign * cross(normal, tangent).
    let geo_n = normalize(in.normal);
    var t = in.tangent.xyz - geo_n * dot(geo_n, in.tangent.xyz);
    t = select(normalize(t), vec3(1.0, 0.0, 0.0), dot(t, t) < 1e-8);
    let b = cross(geo_n, t) * in.tangent.w;
    var uv = in.uv;
    if g.extra.z > 0.0 && u32(g.view.x) == MODE_MATERIAL {
        let v = normalize(g.eye.xyz - in.world);
        uv = parallax_uv(in.uv, vec3(dot(v, t), dot(v, b), dot(v, geo_n)), g.extra.z);
    }
    let s = surface(uv);
    var n = normalize(t * s.normal_ts.x + b * s.normal_ts.y + geo_n * s.normal_ts.z);
    if !front { n = -n; }

    var out: Shaded;
    out.alpha = s.opacity;
    out.sss = vec3(0.0);
    let mode = u32(g.view.x);
    if mode == MODE_MESH_MAP {
        out.color = srgb_to_linear(textureSample(set_preview, set_smp, image_uv(uv)).rgb);
        out.alpha = 1.0;
        return out;
    }
    if mode != MODE_MATERIAL {
        out.color = solo(mode, s, n, uv);
        out.alpha = 1.0;
        return out;
    }

    let model = u32(M.model.x);
    let base = s.base;
    let rough = clamp(s.rough, 0.02, 1.0);
    let metal = s.metal;
    // Painter multiplies baked AO into the ambient lighting; the AO channel too.
    let ao = textureSample(set_ao, set_smp, image_uv(uv)).r * s.ao;
    let v = normalize(g.eye.xyz - in.world);
    let nv = max(dot(n, v), 1e-4);

    // Specular reflectance at normal (f0) and grazing (f90) incidence.
    var f0 = mix(vec3(0.04), base, metal);
    var f90 = vec3(1.0);
    if model == 1u {
        // ASM: specular level 0.5 = 4 %, edge color tints metals at grazing angles.
        f0 = mix(vec3(0.08 * s.spec_level), base, metal);
        f90 = mix(vec3(1.0), s.spec_color, metal);
    } else if model == 2u {
        // OpenPBR: specular weight (level 0.5 = weight 1) and color scale the dielectric
        // Fresnel; the color also tints metals' edges (F82 tint).
        let w = 2.0 * s.spec_level;
        f0 = mix(vec3(0.04 * w) * s.spec_color, base, metal);
        f90 = mix(vec3(w) * s.spec_color, s.spec_color, metal);
    }

    // Anisotropy bends the reflection toward the brushing direction.
    var spec_n = n;
    if model != 0u && s.aniso > 0.0 {
        let a = s.aniso_angle * 2.0 * PI;
        let ta = normalize(t * cos(a) + b * sin(a));
        let ba = normalize(cross(n, ta));
        let at = cross(ba, v);
        let an = cross(at, ba);
        spec_n = normalize(mix(n, an, s.aniso * clamp(5.0 * rough, 0.0, 1.0)));
    }
    let ab = env_brdf(rough, nv);
    let spec = env(reflect(-v, spec_n), rough * g.params.z) * (f0 * ab.x + f90 * ab.y);

    var diffuse = sh_irradiance(n) * base * (1.0 - metal);
    var sss = vec3(0.0);
    var sheen = vec3(0.0);
    var coat_spec = vec3(0.0);
    var coat_tint = vec3(1.0);
    if model != 0u {
        // Transmission: thin-walled refraction of the environment, tinted by base color.
        let tr = s.transmission * (1.0 - metal);
        let refracted = env(refract(-v, n, 1.0 / 1.5), rough * g.params.z) * base;
        diffuse = mix(diffuse, refracted * (1.0 - metal), tr);
        // Scattering: this share of the diffuse light spreads under the surface.
        let sc = s.scatter * (1.0 - metal) * (1.0 - tr);
        sss = sh_irradiance(n) * base * s.scatter_color * sc;
        diffuse *= 1.0 - sc;
        // Sheen (fuzz) on top of the base.
        let e = sheen_albedo(nv, s.sheen_rough);
        sheen = s.sheen * s.sheen_color * sh_irradiance(n) * e;
        let sheen_loss = 1.0 - s.sheen * e * max(s.sheen_color.r, max(s.sheen_color.g, s.sheen_color.b));
        diffuse *= sheen_loss;
        sss *= sheen_loss;
        // Coat: a clear (or tinted) glossy layer over everything below.
        if s.coat > 0.0 {
            let cr = clamp(s.coat_rough, 0.02, 1.0);
            let cab = env_brdf(cr, nv);
            let cf = 0.04 * cab.x + cab.y;
            coat_spec = env(reflect(-v, n), cr * g.params.z) * cf * s.coat;
            coat_tint = mix(vec3(1.0), s.coat_color, s.coat) * (1.0 - s.coat * cf);
        }
    }
    let under = (diffuse + spec + sheen) * coat_tint;
    out.color = (under + coat_spec) * ao + s.emissive * g.post.w;
    out.sss = sss * coat_tint * ao * g.sss.z;
    if g.sss.z < 0.5 {
        out.color += sss * coat_tint * ao;
    }
    return out;
}

struct SceneOut {
    @location(0) color: vec4<f32>,
    @location(1) sss: vec4<f32>,
};

@fragment
fn fs_pbr(in: VsOut, @builtin(front_facing) front: bool) -> SceneOut {
    let s = shade(in, front);
    // Opacity: alpha test, stochastic transparency that accumulation smooths out, or
    // (blended mode) only the opaque texels here — fs_oit draws the rest.
    let alpha_mode = u32(g.extra.x);
    if alpha_mode == 1u && s.alpha < g.extra.y {
        discard;
    }
    if alpha_mode == 2u && s.alpha <= ign(in.clip.xy, g.extra.w) {
        discard;
    }
    if alpha_mode == 3u && s.alpha < 0.999 {
        discard;
    }
    var o: SceneOut;
    o.color = vec4(s.color, 1.0);
    o.sss = vec4(s.sss, 1.0);
    return o;
}

// Weighted blended order-independent transparency (McGuire & Bavoil): accumulate
// premultiplied color with a depth weight, and the product of (1 - alpha).
struct OitOut {
    @location(0) accum: vec4<f32>,
    @location(1) reveal: vec4<f32>,
};

@fragment
fn fs_oit(in: VsOut, @builtin(front_facing) front: bool) -> OitOut {
    let s = shade(in, front);
    if s.alpha >= 0.999 || s.alpha <= 0.001 {
        discard;
    }
    let a = s.alpha;
    let z = in.clip.z;
    let w = clamp(a * max(1e-2, 3e3 * pow(1.0 - z, 3.0)), 1e-2, 3e3);
    var o: OitOut;
    o.accum = vec4((s.color + s.sss) * a, a) * w;
    o.reveal = vec4(a);
    return o;
}

@vertex
fn vs_wire(v: VsIn) -> @builtin(position) vec4<f32> {
    // Nudge toward the camera so the wire wins the depth test against its own surface.
    let to_eye = g.eye.xyz - v.pos;
    return g.view_proj * vec4(v.pos + to_eye * 0.002, 1.0);
}

@fragment
fn fs_wire() -> SceneOut {
    var o: SceneOut;
    o.color = vec4(0.02, 0.02, 0.02, 1.0);
    o.sss = vec4(0.0);
    return o;
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
fn fs_sky(in: Fullscreen) -> SceneOut {
    var o: SceneOut;
    o.sss = vec4(0.0);
    if u32(g.view.x) != MODE_MATERIAL {
        o.color = vec4(0.0);
        return o;
    }
    let far = g.inv_view_proj * vec4(in.ndc, 1.0, 1.0);
    let dir = normalize(far.xyz / far.w - g.eye.xyz);
    o.color = vec4(env(dir, g.params.w), g.view.y);
    return o;
}

// Post passes: up to three inputs, a sampler and the scene depth.
@group(1) @binding(0) var post_a: texture_2d<f32>;
@group(1) @binding(1) var post_b: texture_2d<f32>;
@group(1) @binding(2) var post_smp: sampler;
@group(1) @binding(3) var post_c: texture_2d<f32>;
@group(1) @binding(4) var post_depth: texture_depth_2d;

// Resolves the transparent layer over the opaque image (blended onto it).
@fragment
fn fs_oit_resolve(in: Fullscreen) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip.xy);
    let accum = textureLoad(post_a, p, 0);
    let reveal = textureLoad(post_b, p, 0).r;
    if reveal >= 0.999 {
        discard;
    }
    let avg = accum.rgb / max(accum.a, 1e-5);
    // Blend state: color = avg * (1 - reveal) + dst * reveal.
    return vec4(avg, 1.0 - reveal);
}

// Screen-space subsurface scattering: a depth-aware Gaussian blur of the light
// that scatters, sized by the scattering radius in world units.
fn sss_blur(in: Fullscreen, dir: vec2<f32>) -> vec4<f32> {
    let p = vec2<i32>(in.clip.xy);
    let size = vec2<i32>(textureDimensions(post_a));
    let center = textureLoad(post_a, p, 0);
    let d0 = textureLoad(post_depth, p, 0);
    if d0 >= 1.0 {
        return center;
    }
    let ndc = vec2(in.ndc.x, in.ndc.y);
    let wp = g.inv_view_proj * vec4(ndc, d0, 1.0);
    let dist = distance(wp.xyz / wp.w, g.eye.xyz);
    let radius = g.sss.x * g.sss.y / max(dist, 1e-4);
    let step = max(radius / 6.0, 0.5);
    var sum = center.rgb;
    var wsum = 1.0;
    for (var i = 1; i <= 6; i++) {
        for (var sgn = -1.0; sgn <= 1.0; sgn += 2.0) {
            let q = clamp(p + vec2<i32>(dir * sgn * f32(i) * step), vec2(0), size - 1);
            let dq = textureLoad(post_depth, q, 0);
            let x = f32(i) / 3.0;
            // Don't bleed across depth discontinuities (other objects, silhouettes).
            let w = exp(-x * x * 0.5) * exp(-abs(dq - d0) * 3000.0);
            sum += textureLoad(post_a, q, 0).rgb * w;
            wsum += w;
        }
    }
    return vec4(sum / wsum, 1.0);
}

@fragment
fn fs_sss_h(in: Fullscreen) -> @location(0) vec4<f32> {
    return sss_blur(in, vec2(1.0, 0.0));
}

@fragment
fn fs_sss_v(in: Fullscreen) -> @location(0) vec4<f32> {
    return sss_blur(in, vec2(0.0, 1.0));
}

// Progressive anti-aliasing (and depth of field): average the jittered frames while
// nothing changes. a = this frame, b = the history, c = blurred scattering.
@fragment
fn fs_accumulate(in: Fullscreen) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip.xy);
    var frame = textureLoad(post_a, p, 0);
    frame = vec4(frame.rgb + textureLoad(post_c, p, 0).rgb, frame.a);
    return mix(textureLoad(post_b, p, 0), frame, g.accum.y);
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

// The OCIO display transform: scene-linear Rec.709 → display-encoded (not linear).
fn ocio_lut(c: vec3<f32>) -> vec3<f32> {
    let lo = g.lut.x;
    let hi = g.lut.y;
    let n = g.lut.z;
    let shaped = clamp((log2(max(c, vec3(1e-10)) / 0.18) - lo) / (hi - lo), vec3(0.0), vec3(1.0));
    // Sample texel centers.
    let uvw = (shaped * (n - 1.0) + 0.5) / n;
    return textureSampleLevel(display_lut, env_smp, uvw, 0.0).rgb;
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
        rgb = rgb * g.params.x + bloom * g.post.y;
        let vignette = 1.0 - g.post.z * smoothstep(0.5, 1.5, length(in.ndc));
        if u32(g.post.x) == 4u {
            // The LUT covers the whole display transform, background included.
            let shown = mix(BACKGROUND, rgb, c.a) * vignette;
            return vec4(ocio_lut(shown), 1.0);
        }
        rgb = tone_map(u32(g.post.x), rgb) * vignette;
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
    o.uv1 = v.uv1;
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
