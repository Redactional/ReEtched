// GGX-prefilters one mip level of the equirect environment (roughness = level / (levels - 1)).
// Filtered importance sampling (Křivánek & Colbert, GPU Gems 3 ch. 20) keeps sample counts low.

struct Params {
    roughness: f32,
    src_texel_solid_angle: f32,
    samples: u32,
    _pad: u32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(3) var<uniform> p: Params;

const PI: f32 = 3.14159265;

fn dir_from_uv(uv: vec2<f32>) -> vec3<f32> {
    let phi = (uv.x - 0.5) * 2.0 * PI;
    let theta = uv.y * PI;
    return vec3(sin(theta) * cos(phi), cos(theta), sin(theta) * sin(phi));
}

fn uv_from_dir(d: vec3<f32>) -> vec2<f32> {
    return vec2(atan2(d.z, d.x) / (2.0 * PI) + 0.5, acos(clamp(d.y, -1.0, 1.0)) / PI);
}

fn hammersley(i: u32, n: u32) -> vec2<f32> {
    return vec2(f32(i) / f32(n), f32(reverseBits(i)) * 2.3283064365386963e-10);
}

fn ggx_half_vector(xi: vec2<f32>, n: vec3<f32>, a: f32) -> vec3<f32> {
    let phi = 2.0 * PI * xi.x;
    let cos_t = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
    let sin_t = sqrt(1.0 - cos_t * cos_t);
    let up = select(vec3(0.0, 0.0, 1.0), vec3(1.0, 0.0, 0.0), abs(n.z) > 0.999);
    let tx = normalize(cross(up, n));
    let ty = cross(n, tx);
    return tx * (cos(phi) * sin_t) + ty * (sin(phi) * sin_t) + n * cos_t;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dst);
    if id.x >= size.x || id.y >= size.y { return; }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);

    if p.roughness < 1e-3 {
        textureStore(dst, id.xy, textureSampleLevel(src, smp, uv, 0.0));
        return;
    }

    // Assume view = normal (standard split-sum simplification).
    let n = dir_from_uv(uv);
    let a = p.roughness * p.roughness;
    var sum = vec3(0.0);
    var weight = 0.0;
    for (var i = 0u; i < p.samples; i++) {
        let h = ggx_half_vector(hammersley(i, p.samples), n, a);
        let l = 2.0 * dot(n, h) * h - n;
        let nl = dot(n, l);
        if nl > 0.0 {
            let nh = max(dot(n, h), 0.0);
            let k = nh * nh * (a * a - 1.0) + 1.0;
            let pdf = max(a * a / (PI * k * k) * 0.25, 1e-6);
            let sample_solid_angle = 1.0 / (f32(p.samples) * pdf);
            let lod = max(0.5 * log2(sample_solid_angle / p.src_texel_solid_angle) + 1.0, 0.0);
            sum += textureSampleLevel(src, smp, uv_from_dir(l), lod).rgb * nl;
            weight += nl;
        }
    }
    textureStore(dst, id.xy, vec4(sum / max(weight, 1e-4), 1.0));
}
