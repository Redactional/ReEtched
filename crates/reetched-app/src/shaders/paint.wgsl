// Brush strokes. Dabs accumulate into a stroke mask (R16Float, texture-set sized),
// rendered by rasterising the mesh in UV space. The apply pass then blends the
// brush color into a paint texture using that mask × stroke opacity, so flow
// builds up within a stroke but never exceeds the stroke opacity (Painter's behaviour).

const MAX_DABS: u32 = 64u;

struct Dab {
    // xyz: world center, w: world radius
    center_radius: vec4<f32>,
    // xyz: brush right axis, w: flow
    right_flow: vec4<f32>,
    // xyz: brush up axis, w: angle (radians)
    up_angle: vec4<f32>,
    // xyz: view direction (eye → center), w: flags (1 = depth test, 2 = UV space)
    view_flags: vec4<f32>,
    // xy: UV center, z: UV radius
    uv: vec4<f32>,
};

struct Splat {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x: dab count, y: hardness, z: backface cosine limit (< -1.5 = off), w: has alpha
    params: vec4<f32>,
    dabs: array<Dab, 64>,
};

@group(0) @binding(0) var<uniform> S: Splat;
@group(0) @binding(1) var depth_tex: texture_depth_2d;
@group(0) @binding(2) var alpha_tex: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;

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

// Brush footprint at `d` (brush space, unit radius).
fn shape(d: vec2<f32>, angle: f32) -> f32 {
    let r = length(d);
    if r >= 1.0 {
        return 0.0;
    }
    let h = min(S.params.y, 0.999);
    var a = 1.0 - smoothstep(h, 1.0, r);
    if S.params.w > 0.5 {
        let c = cos(angle);
        let s = sin(angle);
        let q = vec2(c * d.x - s * d.y, s * d.x + c * d.y);
        a *= textureSampleLevel(alpha_tex, smp, q * vec2(0.5, -0.5) + 0.5, 0.0).r;
    }
    return a;
}

// Is `world` the surface the camera sees (not hidden behind other geometry)?
fn visible(world: vec3<f32>, radius: f32) -> bool {
    let clip = S.view_proj * vec4(world, 1.0);
    if clip.w <= 0.0 {
        return false;
    }
    let ndc = clip.xyz / clip.w;
    if abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0 {
        return false;
    }
    let size = vec2<f32>(textureDimensions(depth_tex));
    let px = clamp(vec2<i32>((vec2(ndc.x, -ndc.y) * 0.5 + 0.5) * size), vec2(0), vec2<i32>(size) - 1);
    let depth = textureLoad(depth_tex, px, 0);
    let p = S.inv_view_proj * vec4(ndc.xy, depth, 1.0);
    let surface = p.xyz / p.w;
    return distance(world, S.eye.xyz) <= distance(surface, S.eye.xyz) + radius * 0.05 + 1e-4;
}

@fragment
fn fs_splat(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    var acc = 0.0;
    for (var i = 0u; i < u32(S.params.x); i++) {
        let dab = S.dabs[i];
        let flags = u32(dab.view_flags.w);
        var a = 0.0;
        if (flags & 2u) != 0u {
            a = shape((in.uv - dab.uv.xy) / dab.uv.z, dab.up_angle.w);
        } else {
            let v = in.world - dab.center_radius.xyz;
            let r = dab.center_radius.w;
            // Limit the brush to a sphere so it doesn't reach far-away surfaces behind it.
            if dot(v, v) > r * r * 2.25 {
                continue;
            }
            if S.params.z > -1.5 && dot(n, -dab.view_flags.xyz) < S.params.z {
                continue;
            }
            a = shape(vec2(dot(v, dab.right_flow.xyz), dot(v, dab.up_angle.xyz)) / r, dab.up_angle.w);
            if a > 0.0 && (flags & 1u) != 0u && !visible(in.world, r) {
                a = 0.0;
            }
        }
        acc += a * dab.right_flow.w * (1.0 - acc);
    }
    // Blended as dst + src * (1 - dst): coverage keeps building up across dabs.
    return vec4(acc, 0.0, 0.0, acc);
}

// ---------------------------------------------------------------- apply

struct Apply {
    // rgb: brush value, w: 1 = normal channel (renormalize)
    color: vec4<f32>,
    // x: stroke opacity, y: erase
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> A: Apply;
@group(0) @binding(1) var before: texture_2d<f32>;
@group(0) @binding(2) var stroke_mask: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4(p, 0.0, 1.0);
}

@fragment
fn fs_apply(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(frag.xy);
    let b = textureLoad(before, p, 0);
    let m = textureLoad(stroke_mask, p, 0).r * A.params.x;
    if A.params.y > 0.5 {
        return vec4(b.rgb, b.a * (1.0 - m));
    }
    let a = m + b.a * (1.0 - m);
    if a <= 0.0 {
        return b;
    }
    var rgb = (A.color.rgb * m + b.rgb * b.a * (1.0 - m)) / a;
    if A.color.w > 0.5 {
        rgb = normalize(rgb);
    }
    return vec4(rgb, a);
}
