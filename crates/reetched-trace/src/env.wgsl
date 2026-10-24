// Environment importance sampling: a piecewise-constant distribution over a fixed
// GW × GH grid of the equirect image (in the environment's own frame; rotation is
// applied when sampling). Built in three passes whenever the environment changes.
//
// data layout (f32):
//   [0, N)          cell weights: mean luminance of the cell's mip-0 texels × sin(theta)
//   [N, 2N)         per-row CDFs over x, normalized (inclusive)
//   [2N, 2N + GH)   CDF over rows, normalized (inclusive)
//   [2N + GH]       sum of all weights (0 = black environment, sampling disabled)
// with N = GW * GH.

// Keep in sync with ENV_W / ENV_H in lib.rs and trace.wgsl.
const GW: u32 = 1024u;
const GH: u32 = 512u;
const N: u32 = GW * GH;
const PI: f32 = 3.14159265;

@group(0) @binding(0) var env_tex: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> data: array<f32>;

@compute @workgroup_size(8, 8)
fn cells(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= GW || id.y >= GH {
        return;
    }
    // The block of mip-0 texels under this cell (at least one when the image is
    // smaller than the grid). Averaging, not point sampling, so a small sun keeps
    // its full weight.
    let dim = textureDimensions(env_tex, 0);
    let x0 = id.x * dim.x / GW;
    let x1 = max((id.x + 1u) * dim.x / GW, x0 + 1u);
    let y0 = id.y * dim.y / GH;
    let y1 = max((id.y + 1u) * dim.y / GH, y0 + 1u);
    var sum = 0.0;
    for (var y = y0; y < y1; y++) {
        for (var x = x0; x < x1; x++) {
            let c = textureLoad(env_tex, vec2(x, y), 0).rgb;
            sum += dot(max(c, vec3(0.0)), vec3(0.2126, 0.7152, 0.0722));
        }
    }
    let theta = (f32(id.y) + 0.5) / f32(GH) * PI;
    let w = sum / f32((x1 - x0) * (y1 - y0)) * sin(theta);
    // Drops NaN and inf texels rather than poisoning the whole CDF.
    data[id.y * GW + id.x] = select(0.0, w, w > 0.0 && w < 1e30);
}

@compute @workgroup_size(64)
fn rows(@builtin(global_invocation_id) id: vec3<u32>) {
    let y = id.x;
    if y >= GH {
        return;
    }
    let row = y * GW;
    var acc = 0.0;
    for (var x = 0u; x < GW; x++) {
        acc += data[row + x];
        data[N + row + x] = acc;
    }
    for (var x = 0u; x < GW; x++) {
        // A black row is never picked, but keep its CDF valid anyway.
        data[N + row + x] = select(f32(x + 1u) / f32(GW), data[N + row + x] / acc, acc > 0.0);
    }
    data[2u * N + y] = acc;
}

@compute @workgroup_size(1)
fn marginal() {
    var acc = 0.0;
    for (var y = 0u; y < GH; y++) {
        acc += data[2u * N + y];
        data[2u * N + y] = acc;
    }
    for (var y = 0u; y < GH; y++) {
        data[2u * N + y] = select(f32(y + 1u) / f32(GH), data[2u * N + y] / acc, acc > 0.0);
    }
    data[2u * N + GH] = acc;
}
