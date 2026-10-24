// Progressive path tracer: one thread per pixel, `size.w` paths per thread per frame,
// added to an accumulation buffer and averaged into an rgba16float image.
//
// Output: rgb = linear HDR radiance, a = coverage, i.e. 1 where a surface was hit and
// env_opacity where the path saw the sky. The viewport composites with
// mix(background, rgb, a), so rgb is accumulated weighted by alpha
// (sum(c * a) / sum(a)); that keeps sky/mesh edge pixels exact under that mix.
//
// Shading: metal/roughness. GGX specular (height-correlated Smith, Schlick Fresnel,
// sampled with visible normals) plus Lambert diffuse weighted by 1 - F. Lobes are
// picked by their estimated contribution. Environment light by next event estimation
// from the environment CDF (env.wgsl) and by BSDF sampled rays that escape, combined
// with the power heuristic.

struct Frame {
    inv_view_proj: mat4x4<f32>,
    // w: 1 for an orthographic camera
    eye: vec4<f32>,
    // width, height, seed, samples per frame
    size: vec4<u32>,
    // aperture radius, focus distance, env rotation (radians), env opacity
    lens: vec4<f32>,
    // max bounces, indirect clamp (0 = off), background lod, samples accumulated before
    misc: vec4<f32>,
    // Hidden material layers, one bit each.
    hidden: array<vec4<u32>, 8>,
};

// Both children of an inner node; count 0 = idx is a node, else a leaf of `count`
// triangle records from idx (see bvh.rs).
struct Node {
    lmin: vec3<f32>,
    lidx: u32,
    lmax: vec3<f32>,
    lcount: u32,
    rmin: vec3<f32>,
    ridx: u32,
    rmax: vec3<f32>,
    rcount: u32,
};

struct Tri {
    v0: vec3<f32>,
    id: u32,
    e1: vec3<f32>,
    layer: u32,
    e2: vec3<f32>,
    pad: u32,
};

@group(0) @binding(0) var<uniform> F: Frame;
@group(0) @binding(1) var<storage, read_write> accum: array<vec4<f32>>;
@group(0) @binding(2) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(3) var env_tex: texture_2d<f32>;
@group(0) @binding(4) var env_smp: sampler;
@group(0) @binding(5) var<storage, read> env_cdf: array<f32>;

@group(1) @binding(0) var<storage, read> nodes: array<Node>;
@group(1) @binding(1) var<storage, read> tris: array<Tri>;
// Per vertex: normal xyz, uv, tangent xyzw (9 floats).
@group(1) @binding(2) var<storage, read> verts: array<f32>;
@group(1) @binding(3) var<storage, read> indices: array<u32>;

// Material layers, channels packed as in compositor.wgsl.
@group(2) @binding(0) var mat_t0: texture_2d_array<f32>;
@group(2) @binding(1) var mat_t1: texture_2d_array<f32>;
@group(2) @binding(2) var mat_t2: texture_2d_array<f32>;
@group(2) @binding(3) var mat_t3: texture_2d_array<f32>;
@group(2) @binding(4) var mat_smp: sampler;

const PI: f32 = 3.14159265;
const INF: f32 = 1e30;
const NO_HIT: u32 = 0xffffffffu;
const STACK: u32 = 64u;
// Environment CDF grid (env.wgsl).
const GW: u32 = 1024u;
const GH: u32 = 512u;
const GN: u32 = GW * GH;

// ---------------------------------------------------------------- random numbers

var<private> rng: u32;

fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

fn rand() -> f32 {
    rng = pcg(rng);
    return f32(rng >> 8u) * (1.0 / 16777216.0);
}

fn rand2() -> vec2<f32> {
    return vec2(rand(), rand());
}

// ---------------------------------------------------------------- traversal

struct Hit {
    t: f32,
    // Triangle record, NO_HIT on a miss.
    tri: u32,
    u: f32,
    v: f32,
};

fn hidden(layer: u32) -> bool {
    let word = F.hidden[min(layer, 255u) / 128u][(layer / 32u) % 4u];
    return ((word >> (layer % 32u)) & 1u) != 0u;
}

// Möller–Trumbore: vec3(t, u, v), t <= 0 on a miss.
fn hit_tri(r: Tri, o: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    let p = cross(d, r.e2);
    let det = dot(r.e1, p);
    if det == 0.0 {
        return vec3(-1.0);
    }
    let inv = 1.0 / det;
    let s = o - r.v0;
    let u = dot(s, p) * inv;
    let q = cross(s, r.e1);
    let v = dot(d, q) * inv;
    let t = dot(r.e2, q) * inv;
    if u < 0.0 || v < 0.0 || u + v > 1.0 {
        return vec3(-1.0);
    }
    return vec3(t, u, v);
}

// Entry distance into a box, INF if the ray misses it before `best`.
fn slab(lo: vec3<f32>, hi: vec3<f32>, o: vec3<f32>, inv: vec3<f32>, best: f32) -> f32 {
    let t0 = (lo - o) * inv;
    let t1 = (hi - o) * inv;
    let a = min(t0, t1);
    let b = max(t0, t1);
    let near = max(max(a.x, a.y), max(a.z, 0.0));
    // Slightly widened exit (Ize 2013) so boxes flush with a triangle don't miss.
    let far = min(min(b.x, b.y), min(b.z, best)) * 1.0000004;
    return select(INF, near, near <= far);
}

// Opacity of a hit (t2.w), for shadow rays.
fn opacity_at(r: Tri, u: f32, v: f32) -> f32 {
    let uv = interp_uv(r.id, u, v);
    return textureSampleLevel(mat_t2, mat_smp, uv, r.layer, 0.0).w;
}

// Closest hit along o + t d, t < tmax. In shadow mode it multiplies the transmittance
// (1 - opacity) of every surface on the ray instead, returned in .u (.tri stays
// NO_HIT); it stops as soon as that's ~0, so opaque blockers end it at once.
fn traverse(o: vec3<f32>, d: vec3<f32>, tmax: f32, shadow: bool) -> Hit {
    let dd = select(d, select(vec3(1e-12), vec3(-1e-12), d < vec3(0.0)), abs(d) < vec3(1e-12));
    let inv = 1.0 / dd;
    var hit = Hit(tmax, NO_HIT, 1.0, 0.0);
    var stack: array<u32, STACK>;
    var sp = 0u;
    var cur = 0u;
    loop {
        let n = nodes[cur];
        var tl = slab(n.lmin, n.lmax, o, inv, hit.t);
        var tr = slab(n.rmin, n.rmax, o, inv, hit.t);
        // Leaves are tested right away; only inner children are walked.
        for (var side = 0u; side < 2u; side++) {
            let count = select(n.rcount, n.lcount, side == 0u);
            let tbox = select(tr, tl, side == 0u);
            if count == 0u || tbox >= hit.t {
                continue;
            }
            let first = select(n.ridx, n.lidx, side == 0u);
            for (var i = first; i < first + count; i++) {
                let r = tris[i];
                let h = hit_tri(r, o, d);
                if h.x <= 0.0 || h.x >= hit.t || hidden(r.layer) {
                    continue;
                }
                if shadow {
                    hit.u *= 1.0 - opacity_at(r, h.y, h.z);
                    if hit.u < 1e-3 {
                        hit.u = 0.0;
                        return hit;
                    }
                } else {
                    hit = Hit(h.x, i, h.y, h.z);
                }
            }
            if side == 0u { tl = INF; } else { tr = INF; }
        }
        let gl = tl < hit.t;
        let gr = tr < hit.t;
        if gl && gr {
            let near_left = tl <= tr;
            // ponytail: a full stack drops the farther subtree; 64 entries covers
            // BVH depth 64, far beyond what PLOC builds for millions of triangles.
            if sp < STACK {
                stack[sp] = select(n.lidx, n.ridx, near_left);
                sp++;
            }
            cur = select(n.ridx, n.lidx, near_left);
        } else if gl {
            cur = n.lidx;
        } else if gr {
            cur = n.ridx;
        } else if sp > 0u {
            sp--;
            cur = stack[sp];
        } else {
            break;
        }
    }
    return hit;
}

// ---------------------------------------------------------------- surfaces

fn vert(i: u32, k: u32) -> f32 {
    return verts[i * 9u + k];
}

fn interp_uv(id: u32, u: f32, v: f32) -> vec2<f32> {
    let i0 = indices[id * 3u];
    let i1 = indices[id * 3u + 1u];
    let i2 = indices[id * 3u + 2u];
    let w = 1.0 - u - v;
    let uv = vec2(vert(i0, 3u), vert(i0, 4u)) * w + vec2(vert(i1, 3u), vert(i1, 4u)) * u
        + vec2(vert(i2, 3u), vert(i2, 4u)) * v;
    // Texture row 0 holds v = 1 of the tile; the repeat sampler wraps the tile origin.
    return vec2(uv.x, 1.0 - uv.y);
}

struct Surface {
    p: vec3<f32>,
    // Geometric and shading normals, both facing the incoming ray.
    ng: vec3<f32>,
    n: vec3<f32>,
    diffuse: vec3<f32>,
    f0: vec3<f32>,
    // GGX alpha squared.
    a2: f32,
    emissive: vec3<f32>,
    opacity: f32,
};

fn surface(o: vec3<f32>, d: vec3<f32>, h: Hit) -> Surface {
    let r = tris[h.tri];
    let i0 = indices[r.id * 3u];
    let i1 = indices[r.id * 3u + 1u];
    let i2 = indices[r.id * 3u + 2u];
    let w = 1.0 - h.u - h.v;
    let nv = vec3(vert(i0, 0u), vert(i0, 1u), vert(i0, 2u)) * w
        + vec3(vert(i1, 0u), vert(i1, 1u), vert(i1, 2u)) * h.u
        + vec3(vert(i2, 0u), vert(i2, 1u), vert(i2, 2u)) * h.v;
    let tv = vec4(vert(i0, 5u), vert(i0, 6u), vert(i0, 7u), vert(i0, 8u)) * w
        + vec4(vert(i1, 5u), vert(i1, 6u), vert(i1, 7u), vert(i1, 8u)) * h.u
        + vec4(vert(i2, 5u), vert(i2, 6u), vert(i2, 7u), vert(i2, 8u)) * h.v;
    let uv = interp_uv(r.id, h.u, h.v);
    let layer = r.layer;
    let c0 = textureSampleLevel(mat_t0, mat_smp, uv, layer, 0.0);
    let c1 = textureSampleLevel(mat_t1, mat_smp, uv, layer, 0.0);
    let c2 = textureSampleLevel(mat_t2, mat_smp, uv, layer, 0.0);
    let c3 = textureSampleLevel(mat_t3, mat_smp, uv, layer, 0.0);

    var s: Surface;
    s.p = o + d * h.t;
    var ng = normalize(cross(r.e1, r.e2));
    let front = dot(ng, d) < 0.0;
    // MikkTSpace frame from the interpolated normal, as viewport.wgsl does.
    var n = select(ng, normalize(nv), dot(nv, nv) > 1e-12);
    var t = tv.xyz - n * dot(n, tv.xyz);
    t = select(normalize(t), vec3(1.0, 0.0, 0.0), dot(t, t) < 1e-12);
    let b = cross(n, t) * select(1.0, -1.0, tv.w < 0.0);
    let ts = select(vec3(0.0, 0.0, 1.0), c1.xyz, dot(c1.xyz, c1.xyz) > 1e-8);
    n = normalize(t * ts.x + b * ts.y + n * ts.z);
    if !front {
        ng = -ng;
        n = -n;
    }
    // A shading normal facing away from the viewer (strong normal maps at grazing
    // angles) has no valid reflection; tilt it back until the viewer just sees it.
    let v = -d;
    let nvd = dot(n, v);
    if nvd < 0.01 {
        n = normalize(n + v * (0.01 - nvd));
    }
    s.ng = ng;
    s.n = n;
    let metal = clamp(c0.a, 0.0, 1.0);
    let base = max(c0.rgb, vec3(0.0));
    s.diffuse = base * (1.0 - metal);
    s.f0 = mix(vec3(0.04), base, metal);
    // ponytail: roughness floored at 0.02 (alpha 4e-4) so a mirror stays a finite
    // GGX lobe; true delta reflection would need its own path.
    let rough = clamp(c1.a, 0.02, 1.0);
    s.a2 = rough * rough * rough * rough;
    s.emissive = max(c3.rgb, vec3(0.0));
    s.opacity = clamp(c2.w, 0.0, 1.0);
    return s;
}

// A point just off the surface on the side `dir` leaves to, so the next ray doesn't
// hit the triangle it starts on.
fn offset(p: vec3<f32>, ng: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    let eps = 1e-4 * (1.0 + max(max(abs(p.x), abs(p.y)), abs(p.z)));
    return p + ng * select(-eps, eps, dot(dir, ng) > 0.0);
}

// ---------------------------------------------------------------- BSDF

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3(0.2126, 0.7152, 0.0722));
}

fn fresnel(f0: vec3<f32>, c: f32) -> vec3<f32> {
    let k = pow(1.0 - clamp(c, 0.0, 1.0), 5.0);
    return f0 + (vec3(1.0) - f0) * k;
}

fn ggx_d(nh: f32, a2: f32) -> f32 {
    let k = nh * nh * (a2 - 1.0) + 1.0;
    return a2 / (PI * k * k);
}

fn smith_g1(c: f32, a2: f32) -> f32 {
    return 2.0 * c / (c + sqrt(a2 + (1.0 - a2) * c * c));
}

// Probability of sampling the specular lobe: its share of the expected reflection.
fn spec_prob(s: Surface, nv: f32) -> f32 {
    let fs = luminance(fresnel(s.f0, nv));
    let fd = luminance(s.diffuse) * (1.0 - fs);
    if fd <= 0.0 {
        return 1.0;
    }
    return clamp(fs / (fs + fd), 0.1, 0.9);
}

// BSDF × cos for light from l, and the pdf of sampling l.
fn bsdf(s: Surface, v: vec3<f32>, l: vec3<f32>) -> vec4<f32> {
    let nl = dot(s.n, l);
    let nv = dot(s.n, v);
    if nl <= 0.0 || nv <= 0.0 || dot(s.ng, l) <= 0.0 {
        return vec4(0.0);
    }
    let h = normalize(v + l);
    // Clamped to 1 too: with a near-mirror alpha, 1 - nh² is down at f32 precision.
    let nh = clamp(dot(s.n, h), 0.0, 1.0);
    let f = fresnel(s.f0, dot(v, h));
    let d = ggx_d(nh, s.a2);
    // Height-correlated Smith visibility, G2 / (4 nl nv).
    let vis = 0.5 / (nl * sqrt(nv * nv * (1.0 - s.a2) + s.a2) + nv * sqrt(nl * nl * (1.0 - s.a2) + s.a2));
    let value = (f * d * vis + s.diffuse / PI * (vec3(1.0) - f)) * nl;
    // Visible-normal sampling: pdf(l) = G1(v) D / (4 nv).
    let pdf_spec = smith_g1(nv, s.a2) * d / (4.0 * nv);
    let ps = spec_prob(s, nv);
    return vec4(value, ps * pdf_spec + (1.0 - ps) * nl / PI);
}

// Orthonormal basis around n (Duff et al. 2017); columns t, b, n.
fn basis(n: vec3<f32>) -> mat3x3<f32> {
    let sg = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (sg + n.z);
    let b = n.x * n.y * a;
    return mat3x3(
        vec3(1.0 + sg * n.x * n.x * a, sg * b, -sg * n.x),
        vec3(b, sg + n.y * n.y * a, -n.y),
        n,
    );
}

// GGX visible normal (Dupuy & Benyoub 2023, spherical caps), in the local frame.
fn sample_vndf(v: vec3<f32>, alpha: f32, u: vec2<f32>) -> vec3<f32> {
    let vs = normalize(vec3(v.xy * alpha, v.z));
    let phi = 2.0 * PI * u.x;
    let z = (1.0 - u.y) * (1.0 + vs.z) - vs.z;
    let st = sqrt(clamp(1.0 - z * z, 0.0, 1.0));
    let hs = vec3(st * cos(phi), st * sin(phi), z) + vs;
    return normalize(vec3(hs.xy * alpha, max(hs.z, 0.0)));
}

fn sample_bsdf(s: Surface, v: vec3<f32>) -> vec3<f32> {
    let m = basis(s.n);
    let u = rand2();
    if rand() < spec_prob(s, dot(s.n, v)) {
        let vl = vec3(dot(v, m[0]), dot(v, m[1]), dot(v, m[2]));
        let h = m * sample_vndf(vl, sqrt(s.a2), u);
        return reflect(-v, h);
    }
    // Cosine-weighted hemisphere.
    let r = sqrt(u.x);
    let phi = 2.0 * PI * u.y;
    return m * vec3(r * cos(phi), r * sin(phi), sqrt(max(1.0 - u.x, 0.0)));
}

// ---------------------------------------------------------------- environment

// World direction → the environment's frame (env_frame in viewport.wgsl) and back.
fn to_env(d: vec3<f32>) -> vec3<f32> {
    let c = cos(F.lens.z);
    let s = sin(F.lens.z);
    return vec3(c * d.x - s * d.z, d.y, s * d.x + c * d.z);
}

fn from_env(r: vec3<f32>) -> vec3<f32> {
    let c = cos(F.lens.z);
    let s = sin(F.lens.z);
    return vec3(c * r.x + s * r.z, r.y, -s * r.x + c * r.z);
}

fn env_uv(r: vec3<f32>) -> vec2<f32> {
    return vec2(atan2(r.z, r.x) / (2.0 * PI) + 0.5, acos(clamp(r.y, -1.0, 1.0)) / PI);
}

fn env(d: vec3<f32>, lod: f32) -> vec3<f32> {
    let c = textureSampleLevel(env_tex, env_smp, env_uv(to_env(d)), lod).rgb;
    return select(vec3(0.0), c, c == c);
}

fn env_total() -> f32 {
    return env_cdf[2u * GN + GH];
}

// Solid-angle pdf of env_sample picking world direction d.
fn env_pdf(d: vec3<f32>) -> f32 {
    let r = to_env(d);
    let uv = env_uv(r);
    let x = min(u32(uv.x * f32(GW)), GW - 1u);
    let y = min(u32(uv.y * f32(GH)), GH - 1u);
    let sin_t = sqrt(max(1.0 - r.y * r.y, 0.0));
    if sin_t < 1e-6 {
        return 0.0;
    }
    return env_cdf[y * GW + x] * f32(GN) / (env_total() * 2.0 * PI * PI * sin_t);
}

// First index in cdf[base .. base + n) above u.
fn search(base: u32, n: u32, u: f32) -> u32 {
    var lo = 0u;
    var hi = n - 1u;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        if env_cdf[base + mid] > u {
            hi = mid;
        } else {
            lo = mid + 1u;
        }
    }
    return lo;
}

// Where u fell between cdf[i - 1] and cdf[i], in 0..1 (reuses the random number).
fn remap(base: u32, i: u32, u: f32) -> f32 {
    let lo = select(0.0, env_cdf[base + i - 1u], i > 0u);
    let hi = env_cdf[base + i];
    return clamp((u - lo) / max(hi - lo, 1e-12), 0.0, 0.999999);
}

// A world direction from the environment CDF.
fn env_sample(u: vec2<f32>) -> vec3<f32> {
    let y = search(2u * GN, GH, u.y);
    let fy = remap(2u * GN, y, u.y);
    let row = GN + y * GW;
    let x = search(row, GW, u.x);
    let fx = remap(row, x, u.x);
    let phi = ((f32(x) + fx) / f32(GW) - 0.5) * 2.0 * PI;
    let theta = (f32(y) + fy) / f32(GH) * PI;
    let st = sin(theta);
    return from_env(vec3(st * cos(phi), cos(theta), st * sin(phi)));
}

fn power_heuristic(a: f32, b: f32) -> f32 {
    let a2 = a * a;
    return a2 / max(a2 + b * b, 1e-30);
}

// ---------------------------------------------------------------- paths

var<private> radiance: vec3<f32>;

// Adds light that arrived after `scatters` surface interactions; past the first the
// sample counts as indirect and its luminance is clamped (fireflies).
fn add(c: vec3<f32>, scatters: u32) {
    var x = c;
    let limit = F.misc.y;
    if scatters >= 2u && limit > 0.0 {
        let l = luminance(x);
        if l > limit {
            x *= limit / l;
        }
    }
    radiance += x;
}

fn unproject(ndc: vec2<f32>, z: f32) -> vec3<f32> {
    let p = F.inv_view_proj * vec4(ndc, z, 1.0);
    return p.xyz / p.w;
}

// One path through the pixel; rgb radiance and alpha (coverage).
fn path(pixel: vec2<u32>) -> vec4<f32> {
    let size = vec2<f32>(F.size.xy);
    let px = vec2<f32>(pixel) + rand2();
    let ndc = vec2(px.x / size.x * 2.0 - 1.0, 1.0 - px.y / size.y * 2.0);
    let near = unproject(ndc, 0.0);
    var d = normalize(unproject(ndc, 1.0) - near);
    // Perspective rays start at the eye, orthographic ones on the near plane.
    let ortho = F.eye.w > 0.5;
    var o = select(F.eye.xyz, near, ortho);
    if F.lens.x > 0.0 {
        // Thin lens: aim every lens point at where the pinhole ray meets the focus plane.
        let fwd = normalize(unproject(vec2(0.0), 1.0) - unproject(vec2(0.0), 0.0));
        let right = normalize(unproject(vec2(1.0, 0.0), 0.0) - unproject(vec2(-1.0, 0.0), 0.0));
        let up = cross(right, fwd);
        let focus = o + d * (F.lens.y - dot(o - F.eye.xyz, fwd)) / dot(d, fwd);
        let u = rand2();
        let r = F.lens.x * sqrt(u.x);
        o += (right * cos(2.0 * PI * u.y) + up * sin(2.0 * PI * u.y)) * r;
        d = normalize(focus - o);
    }

    radiance = vec3(0.0);
    var throughput = vec3(1.0);
    var alpha = F.lens.w;
    var bounce = 0u;
    var crossings = 0u;
    var last_pdf = 0.0;
    let max_bounces = u32(F.misc.x);
    let env_ok = env_total() > 0.0;
    loop {
        let h = traverse(o, d, INF, false);
        if h.tri == NO_HIT {
            if bounce == 0u {
                radiance += throughput * env(d, F.misc.z);
            } else {
                var w = 1.0;
                if env_ok {
                    w = power_heuristic(last_pdf, env_pdf(d));
                }
                add(throughput * env(d, 0.0) * w, bounce);
            }
            break;
        }
        let s = surface(o, d, h);
        // Cutout: pass through with probability 1 - opacity, without a bounce.
        // ponytail: after 32 crossings the surface counts as opaque.
        if crossings < 32u && rand() >= s.opacity {
            crossings++;
            o = offset(s.p, s.ng, d);
            continue;
        }
        if bounce == 0u {
            alpha = 1.0;
        }
        add(throughput * s.emissive, bounce);
        if bounce >= max_bounces {
            break;
        }
        let v = -d;

        // Next event estimation: one direction from the environment CDF.
        if env_ok {
            let l = env_sample(rand2());
            let f = bsdf(s, v, l);
            if f.w > 0.0 {
                let pl = env_pdf(l);
                let vis = traverse(offset(s.p, s.ng, l), l, INF, true).u;
                if vis > 0.0 && pl > 0.0 {
                    add(throughput * f.rgb * env(l, 0.0) * (vis * power_heuristic(pl, f.w) / pl), bounce + 1u);
                }
            }
        }

        // Continue along a BSDF sample.
        let l = sample_bsdf(s, v);
        let f = bsdf(s, v, l);
        if f.w <= 0.0 {
            break;
        }
        throughput *= f.rgb / f.w;
        last_pdf = f.w;
        bounce++;
        o = offset(s.p, s.ng, l);
        d = l;
        // Russian roulette once paths have had a few bounces.
        if bounce >= 3u {
            let q = clamp(max(throughput.x, max(throughput.y, throughput.z)), 0.05, 0.95);
            if rand() > q {
                break;
            }
            throughput /= q;
        }
    }
    return vec4(radiance, alpha);
}

@compute @workgroup_size(8, 8)
fn trace(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= F.size.x || id.y >= F.size.y {
        return;
    }
    let pixel = id.y * F.size.x + id.x;
    var sum = vec4(0.0);
    for (var i = 0u; i < F.size.w; i++) {
        rng = pcg(pixel ^ pcg(F.size.z + i * 0x68bc21ebu));
        let c = path(id.xy);
        // A NaN or inf sample would poison the pixel for good; drop it.
        let ok = all(c == c) && all(abs(c) < vec4(1e30));
        let cs = select(vec4(0.0, 0.0, 0.0, 1.0), c, ok);
        sum += vec4(cs.rgb * cs.a, cs.a);
    }
    var total = sum;
    if F.misc.w > 0.0 {
        total += accum[pixel];
    }
    accum[pixel] = total;
    let n = F.misc.w + f32(F.size.w);
    textureStore(out_tex, id.xy, vec4(total.rgb / max(total.a, 1e-8), total.a / n));
}
