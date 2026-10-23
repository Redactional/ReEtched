// Viewport: sky, PBR mesh, wireframe, grid (into an HDR target), then tone mapping.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x: exposure multiplier, y: env rotation (radians), z: env max lod, w: background lod
    params: vec4<f32>,
    // x: grid plane height, y: grid cell size, z: grid extent
    grid: vec4<f32>,
    sh: array<vec4<f32>, 9>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var env_tex: texture_2d<f32>;
@group(0) @binding(2) var env_smp: sampler;

const PI: f32 = 3.14159265;

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

// ---------------------------------------------------------------- mesh

struct Material {
    base_color: vec4<f32>,   // linear
    rough_metal: vec4<f32>,  // x: roughness, y: metallic
};
@group(1) @binding(0) var<uniform> mat: Material;

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
};

@vertex
fn vs_mesh(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = g.view_proj * vec4(v.pos, 1.0);
    o.world = v.pos;
    o.normal = v.normal;
    return o;
}

@fragment
fn fs_pbr(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n = normalize(in.normal);
    if !front { n = -n; }
    let v = normalize(g.eye.xyz - in.world);
    let base = mat.base_color.rgb;
    let rough = clamp(mat.rough_metal.x, 0.02, 1.0);
    let metal = mat.rough_metal.y;
    let f0 = mix(vec3(0.04), base, metal);
    let nv = max(dot(n, v), 1e-4);

    // ponytail: analytic split-sum (Karis' mobile EnvBRDFApprox) instead of a BRDF LUT;
    // swap in a LUT if speculars drift from Painter's reference renders.
    let rr = rough * vec4(-1.0, -0.0275, -0.572, 0.022) + vec4(1.0, 0.0425, 1.04, -0.04);
    let a004 = min(rr.x * rr.x, exp2(-9.28 * nv)) * rr.x + rr.y;
    let ab = vec2(-1.04, 1.04) * a004 + rr.zw;

    let spec = env(reflect(-v, n), rough * g.params.z) * (f0 * ab.x + ab.y);
    let diffuse = sh_irradiance(n) * base * (1.0 - metal);
    return vec4(diffuse + spec, 1.0);
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

@fragment
fn fs_sky(in: Fullscreen) -> @location(0) vec4<f32> {
    let far = g.inv_view_proj * vec4(in.ndc, 1.0, 1.0);
    let dir = normalize(far.xyz / far.w - g.eye.xyz);
    return vec4(env(dir, g.params.w), 1.0);
}

@group(1) @binding(0) var hdr_tex: texture_2d<f32>;

// Narkowicz's ACES fit.
fn aces(x: vec3<f32>) -> vec3<f32> {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3(0.0), vec3(1.0));
}

@fragment
fn fs_tonemap(in: Fullscreen) -> @location(0) vec4<f32> {
    let c = textureLoad(hdr_tex, vec2<i32>(in.clip.xy), 0).rgb * g.params.x;
    return vec4(aces(c), 1.0); // target is *Srgb, so encoding happens in hardware
}

// ---------------------------------------------------------------- grid

struct GridOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_grid(@builtin(vertex_index) i: u32) -> GridOut {
    let corner = vec2(f32(i & 1u), f32(i >> 1u)) * 2.0 - 1.0;
    let world = vec3(g.eye.x + corner.x * g.grid.z, g.grid.x, g.eye.z + corner.y * g.grid.z);
    var o: GridOut;
    o.clip = g.view_proj * vec4(world, 1.0);
    o.world = world;
    return o;
}

fn grid_lines(p: vec2<f32>) -> f32 {
    let w = fwidth(p);
    let d = abs(fract(p - 0.5) - 0.5) / max(w, vec2(1e-6));
    return 1.0 - min(min(d.x, d.y), 1.0);
}

@fragment
fn fs_grid(in: GridOut) -> @location(0) vec4<f32> {
    let p = in.world.xz / g.grid.y;
    let minor = grid_lines(p);
    let major = grid_lines(p / 10.0);
    let fade = 1.0 - smoothstep(0.2, 1.0, length(in.world.xz - g.eye.xz) / g.grid.z);
    let a = max(minor * 0.2, major * 0.5) * fade;
    return vec4(vec3(0.6), a);
}
