// ACES 2.0 output transform (Hellwig 2022 JMh tonescale, chroma compression, gamut compression).
//
// Ported from OpenColorIO (src/OpenColorIO/ops/fixedfunction/ACES2/*), Copyright Contributors
// to the OpenColorIO Project, BSD-3-Clause:
//
// Redistribution and use in source and binary forms, with or without modification, are
// permitted provided that the following conditions are met: (1) redistributions of source
// code must retain the above copyright notice, this list of conditions and the following
// disclaimer; (2) redistributions in binary form must reproduce the above copyright notice,
// this list of conditions and the following disclaimer in the documentation and/or other
// materials provided with the distribution; (3) neither the name of the copyright holder nor
// the names of its contributors may be used to endorse or promote products derived from this
// software without specific prior written permission. THIS SOFTWARE IS PROVIDED BY THE
// COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES ARE
// DISCLAIMED.
//
// Everything stays in f32 like OCIO: the hue/cusp tables are built by bisection, so changing
// precision would shift table entries and drift away from OCIO's results.

use crate::builtins::{AP0, AP1, Adapt, Primaries, XYZ_E, conversion};
use crate::ops::invert;

type F3 = [f32; 3];
type M33 = [f32; 9];

const PI: f32 = std::f32::consts::PI;
const HUE_LIMIT: f32 = 360.0;
const REFERENCE_LUMINANCE: f32 = 100.0;
const L_A: f32 = 100.0;
const Y_B: f32 = 20.0;
const SURROUND: F3 = [0.9, 0.59, 0.9];
const J_SCALE: f32 = 100.0;
const CAM_NL_OFFSET: f32 = 0.2713 * 100.0;
const CAM_NL_SCALE: f32 = 4.0 * 100.0;
const CHROMA_COMPRESS: f32 = 2.4;
const CHROMA_COMPRESS_FACT: f32 = 3.3;
const CHROMA_EXPAND: f32 = 1.3;
const CHROMA_EXPAND_FACT: f32 = 0.69;
const CHROMA_EXPAND_THR: f32 = 0.5;
const SMOOTH_CUSPS: f32 = 0.12;
const SMOOTH_M: f32 = 0.27;
const CUSP_MID_BLEND: f32 = 1.3;
const FOCUS_GAIN_BLEND: f32 = 0.3;
const FOCUS_DISTANCE: f32 = 1.35;
const FOCUS_DISTANCE_SCALING: f32 = 1.75;
const COMPRESSION_THRESHOLD: f32 = 0.75;
const GAMMA_MAXIMUM: f32 = 5.0;
const GAMMA_SEARCH_STEP: f32 = 0.4;
const GAMMA_ACCURACY: f32 = 1e-5;
const CORNERS: usize = 6;
const REACH_CUSP_TOLERANCE: f32 = 1e-3;
const DISPLAY_CUSP_TOLERANCE: f32 = 1e-7;

const CAM16: Primaries = Primaries {
    r: [0.8336, 0.1735],
    g: [2.3854, -1.4659],
    b: [0.087, -0.125],
    w: [0.333, 0.333],
};

// Hue tables: one wrap entry below, 360 nominal entries, two wrap entries above.
const TABLE_SIZE: usize = 363;
const UPPER_WRAP: usize = 361;
const LAST_NOMINAL: usize = 360;

fn lerpf(a: f32, b: f32, z: f32) -> f32 {
    (b - a) * z + a
}

fn mult_f3_f33(v: F3, m: &M33) -> F3 {
    [
        v[0] * m[0] + v[1] * m[1] + v[2] * m[2],
        v[0] * m[3] + v[1] * m[4] + v[2] * m[5],
        v[0] * m[6] + v[1] * m[7] + v[2] * m[8],
    ]
}

fn mult_f33_f33(a: &M33, b: &M33) -> M33 {
    let mut r = [0.0; 9];
    for i in 0..3 {
        for j in 0..3 {
            r[i * 3 + j] = a[i * 3] * b[j] + a[i * 3 + 1] * b[3 + j] + a[i * 3 + 2] * b[6 + j];
        }
    }
    r
}

fn diag(s: F3) -> M33 {
    [s[0], 0., 0., 0., s[1], 0., 0., 0., s[2]]
}

fn m33(m: &[[f64; 3]; 3]) -> M33 {
    [0, 1, 2, 3, 4, 5, 6, 7, 8].map(|i| m[i / 3][i % 3] as f32)
}

fn invert_f33(m: &M33) -> M33 {
    let d = [0, 1, 2].map(|r| [0, 1, 2].map(|c| m[r * 3 + c] as f64));
    m33(&invert(&d))
}

fn rgb_to_xyz(p: &Primaries) -> M33 {
    m33(&conversion(p, &XYZ_E, Adapt::None))
}

fn xyz_to_rgb(p: &Primaries) -> M33 {
    m33(&invert(&conversion(p, &XYZ_E, Adapt::None)))
}

fn to_radians(v: f32) -> f32 {
    PI * v / 180.0
}

struct JMhParams {
    rgb_to_cam16_c: M33,
    cam16_c_to_rgb: M33,
    cone_to_aab: M33,
    aab_to_cone: M33,
    f_l_n: f32,
    cz: f32,
    inv_cz: f32,
    a_w_j: f32,
    inv_a_w_j: f32,
}

struct ToneScale {
    n: f32,
    n_r: f32,
    g: f32,
    t_1: f32,
    c_t: f32,
    s_2: f32,
    m_2: f32,
    forward_limit: f32,
    inverse_limit: f32,
    log_peak: f32,
}

struct Shared {
    limit_j_max: f32,
    model_gamma_inv: f32,
    reach_m_table: Vec<f32>,
}

#[derive(Clone, Copy)]
struct Resolved {
    limit_j_max: f32,
    model_gamma_inv: f32,
    reach_max_m: f32,
}

struct Chroma {
    sat: f32,
    sat_thr: f32,
    compr: f32,
    scale: f32,
}

struct Gamut {
    mid_j: f32,
    focus_dist: f32,
    lower_hull_gamma_inv: f32,
    search_range: [i32; 2],
    hue_table: Vec<f32>,
    cusp_table: Vec<F3>,
}

struct HueParams {
    gamma_bottom_inv: f32,
    jm_cusp: [f32; 2],
    gamma_top_inv: f32,
    focus_j: f32,
    analytical_threshold: f32,
}

fn compress_fwd_nl(rc: f32) -> f32 {
    let f = rc.powf(0.42);
    f / (CAM_NL_OFFSET + f)
}

fn compress_inv_nl(ra: f32) -> f32 {
    let ra = ra.min(0.99);
    (CAM_NL_OFFSET * ra / (1.0 - ra)).powf(1.0 / 0.42)
}

fn post_adaptation_fwd(v: f32) -> f32 {
    compress_fwd_nl(v.abs()).copysign(v)
}

fn post_adaptation_inv(v: f32) -> f32 {
    compress_inv_nl(v.abs()).copysign(v)
}

fn achromatic_to_j(a: f32, cz: f32) -> f32 {
    J_SCALE * a.powf(cz)
}

fn j_to_achromatic(j: f32, inv_cz: f32) -> f32 {
    (j * (1.0 / J_SCALE)).powf(inv_cz)
}

fn model_gamma() -> f32 {
    SURROUND[1] * (1.48 + (Y_B / REFERENCE_LUMINANCE).sqrt())
}

impl JMhParams {
    fn new(prims: &Primaries) -> JMhParams {
        let base: M33 = [
            2.0,
            1.0,
            1.0 / 20.0,
            1.0,
            -12.0 / 11.0,
            1.0 / 11.0,
            1.0 / 9.0,
            1.0 / 9.0,
            -2.0 / 9.0,
        ];
        let m16 = xyz_to_rgb(&CAM16);
        let rgb_to_xyz_m = rgb_to_xyz(prims);
        let xyz_w = mult_f3_f33([REFERENCE_LUMINANCE; 3], &rgb_to_xyz_m);
        let y_w = xyz_w[1];
        let rgb_w = mult_f3_f33(xyz_w, &m16);
        let k = 1.0 / (5.0 * L_A + 1.0);
        let k4 = k * k * k * k;
        let f_l = 0.2 * k4 * (5.0 * L_A) + 0.1 * (1.0 - k4).powf(2.0) * (5.0 * L_A).powf(1.0 / 3.0);
        let f_l_n = f_l / REFERENCE_LUMINANCE;
        let cz = model_gamma();
        let d_rgb = [0, 1, 2].map(|i| f_l_n * y_w / rgb_w[i]);
        let rgb_aw = [0, 1, 2].map(|i| post_adaptation_fwd(d_rgb[i] * rgb_w[i]));
        let cone = mult_f33_f33(&diag([CAM_NL_SCALE; 3]), &base);
        let a_w = cone[0] * rgb_aw[0] + cone[1] * rgb_aw[1] + cone[2] * rgb_aw[2];
        let a_w_j = compress_fwd_nl(f_l);
        let rgb_to_rgb = mult_f33_f33(&xyz_to_rgb(&CAM16), &rgb_to_xyz(prims));
        let rgb_to_cam16 = mult_f33_f33(&rgb_to_rgb, &diag([REFERENCE_LUMINANCE; 3]));
        let rgb_to_cam16_c = mult_f33_f33(&diag(d_rgb), &rgb_to_cam16);
        let mut cone_to_aab = cone;
        for i in 0..9 {
            cone_to_aab[i] = if i < 3 {
                cone[i] / a_w
            } else {
                cone[i] * 43.0 * SURROUND[2]
            };
        }
        JMhParams {
            cam16_c_to_rgb: invert_f33(&rgb_to_cam16_c),
            rgb_to_cam16_c,
            aab_to_cone: invert_f33(&cone_to_aab),
            cone_to_aab,
            f_l_n,
            cz,
            inv_cz: 1.0 / cz,
            a_w_j,
            inv_a_w_j: 1.0 / a_w_j,
        }
    }

    fn rgb_to_aab(&self, rgb: F3) -> F3 {
        let m = mult_f3_f33(rgb, &self.rgb_to_cam16_c);
        mult_f3_f33(m.map(post_adaptation_fwd), &self.cone_to_aab)
    }

    fn aab_to_jmh(&self, aab: F3) -> F3 {
        if aab[0] <= 0.0 {
            return [0.0; 3];
        }
        let j = achromatic_to_j(aab[0], self.cz);
        let m = (aab[1] * aab[1] + aab[2] * aab[2]).sqrt();
        let h = 180.0 * aab[2].atan2(aab[1]) / PI;
        [j, m, if h < 0.0 { h + HUE_LIMIT } else { h }]
    }

    fn rgb_to_jmh(&self, rgb: F3) -> F3 {
        self.aab_to_jmh(self.rgb_to_aab(rgb))
    }

    fn jmh_to_aab_cs(&self, jmh: F3, cos_hr: f32, sin_hr: f32) -> F3 {
        [
            j_to_achromatic(jmh[0], self.inv_cz),
            jmh[1] * cos_hr,
            jmh[1] * sin_hr,
        ]
    }

    fn aab_to_rgb(&self, aab: F3) -> F3 {
        let a = mult_f3_f33(aab, &self.aab_to_cone);
        mult_f3_f33(a.map(post_adaptation_inv), &self.cam16_c_to_rgb)
    }

    fn jmh_to_rgb(&self, jmh: F3) -> F3 {
        let h = to_radians(jmh[2]);
        self.aab_to_rgb(self.jmh_to_aab_cs(jmh, h.cos(), h.sin()))
    }

    fn a_to_y(&self, a: f32) -> f32 {
        compress_inv_nl(self.a_w_j * a) / self.f_l_n
    }

    fn j_to_y(&self, abs_j: f32) -> f32 {
        self.a_to_y(j_to_achromatic(abs_j, self.inv_cz))
    }

    fn y_to_j_abs(&self, abs_y: f32) -> f32 {
        let ra = compress_fwd_nl(abs_y * self.f_l_n);
        achromatic_to_j(ra * self.inv_a_w_j, self.cz)
    }

    fn y_to_j(&self, y: f32) -> f32 {
        self.y_to_j_abs(y.abs()).copysign(y)
    }
}

impl ToneScale {
    fn new(peak: f32) -> ToneScale {
        let n = peak;
        let n_r = 100.0f32;
        let g = 1.15f32;
        let c = 0.18f32;
        let c_d = 10.013f32;
        let w_g = 0.14f32;
        let t_1 = 0.04f32;
        let r_hit_min = 128.0f32;
        let r_hit_max = 896.0f32;
        let r_hit =
            r_hit_min + (r_hit_max - r_hit_min) * ((n / n_r).ln() / (10000.0f32 / 100.0).ln());
        let m_0 = n / n_r;
        let m_1 = 0.5 * (m_0 + (m_0 * (m_0 + 4.0 * t_1)).sqrt());
        let u = ((r_hit / m_1) / ((r_hit / m_1) + 1.0)).powf(g);
        let m = m_1 / u;
        let w_i = (n / 100.0).ln() / 2.0f32.ln();
        let c_t = c_d / n_r * (1.0 + w_i * w_g);
        let g_ip = 0.5 * (c_t + (c_t * (c_t + 4.0 * t_1)).sqrt());
        let g_ipp2 = -(m_1 * (g_ip / m).powf(1.0 / g)) / ((g_ip / m).powf(1.0 / g) - 1.0);
        let w_2 = c / g_ipp2;
        let s_2 = w_2 * m_1 * REFERENCE_LUMINANCE;
        let u_2 = ((r_hit / m_1) / ((r_hit / m_1) + w_2)).powf(g);
        let m_2 = m_1 / u_2;
        ToneScale {
            n,
            n_r,
            g,
            t_1,
            c_t,
            s_2,
            m_2,
            forward_limit: 8.0 * r_hit,
            inverse_limit: n / (u_2 * n_r),
            log_peak: (n / n_r).log10(),
        }
    }

    fn fwd(&self, y: f32) -> f32 {
        let f = self.m_2 * (y / (y + self.s_2)).powf(self.g);
        (f * f / (f + self.t_1)).max(0.0) * self.n_r
    }

    fn inv(&self, y: f32) -> f32 {
        let z = (y / REFERENCE_LUMINANCE).min(self.inverse_limit).max(0.0);
        let f = (z + (z * (4.0 * self.t_1 + z)).sqrt()) / 2.0;
        self.s_2 / ((self.m_2 / f).powf(1.0 / self.g) - 1.0)
    }
}

fn chroma_compress_norm(cos1: f32, sin1: f32, scale: f32) -> f32 {
    let cos2 = 2.0 * cos1 * cos1 - 1.0;
    let sin2 = 2.0 * cos1 * sin1;
    let cos3 = 4.0 * cos1 * cos1 * cos1 - 3.0 * cos1;
    let sin3 = 3.0 * sin1 - 4.0 * sin1 * sin1 * sin1;
    let m = 11.34072 * cos1
        + 16.46899 * cos2
        + 7.88380 * cos3
        + 14.66441 * sin1
        + -6.37224 * sin2
        + 9.19364 * sin3
        + 77.12896;
    m * scale
}

fn toe_fwd(x: f32, limit: f32, k1_in: f32, k2_in: f32) -> f32 {
    if x > limit {
        return x;
    }
    let k2 = k2_in.max(0.001);
    let k1 = (k1_in * k1_in + k2 * k2).sqrt();
    let k3 = (limit + k1) / (limit + k2);
    let minus_b = k3 * x - k1;
    let minus_ac = k2 * k3 * x;
    0.5 * (minus_b + (minus_b * minus_b + 4.0 * minus_ac).sqrt())
}

fn toe_inv(x: f32, limit: f32, k1_in: f32, k2_in: f32) -> f32 {
    if x > limit {
        return x;
    }
    let k2 = k2_in.max(0.001);
    let k1 = (k1_in * k1_in + k2 * k2).sqrt();
    let k3 = (limit + k1) / (limit + k2);
    (x * x + k1 * x) / (k3 * (x + k2))
}

fn chroma_compress_fwd(jmh: F3, j_ts: f32, m_norm: f32, r: &Resolved, c: &Chroma) -> F3 {
    let (j, m, h) = (jmh[0], jmh[1], jmh[2]);
    let mut m_cp = m;
    if m != 0.0 {
        let nj = j_ts / r.limit_j_max;
        let snj = (1.0 - nj).max(0.0);
        let limit = nj.powf(r.model_gamma_inv) * r.reach_max_m / m_norm;
        m_cp = m * (j_ts / j).powf(r.model_gamma_inv);
        m_cp /= m_norm;
        m_cp = limit
            - toe_fwd(
                limit - m_cp,
                limit - 0.001,
                snj * c.sat,
                (nj * nj + c.sat_thr).sqrt(),
            );
        m_cp = toe_fwd(m_cp, limit, nj * c.compr, snj);
        m_cp *= m_norm;
    }
    [j_ts, m_cp, h]
}

fn chroma_compress_inv(jmh: F3, j: f32, m_norm: f32, r: &Resolved, c: &Chroma) -> F3 {
    let (j_ts, m_cp, h) = (jmh[0], jmh[1], jmh[2]);
    let mut m = m_cp;
    if m_cp != 0.0 {
        let nj = j_ts / r.limit_j_max;
        let snj = (1.0 - nj).max(0.0);
        let limit = nj.powf(r.model_gamma_inv) * r.reach_max_m / m_norm;
        m = m_cp / m_norm;
        m = toe_inv(m, limit, nj * c.compr, snj);
        m = limit
            - toe_inv(
                limit - m,
                limit - 0.001,
                snj * c.sat,
                (nj * nj + c.sat_thr).sqrt(),
            );
        m *= m_norm;
        m *= (j_ts / j).powf(-r.model_gamma_inv);
    }
    [j, m, h]
}

fn corner(c: usize) -> F3 {
    [
        ((c + 1) % CORNERS < 3) as u32 as f32,
        ((c + 5) % CORNERS < 3) as u32 as f32,
        ((c + 3) % CORNERS < 3) as u32 as f32,
    ]
}

/// Rotate so the lowest hue sits at [1], then add wrap entries at [0] and [7].
fn cycle<T: Copy>(rgb: &[T; CORNERS], jmh: &[F3; CORNERS]) -> ([T; 8], [F3; 8]) {
    let mut min_index = 0;
    for i in 0..CORNERS {
        if jmh[i][2] < jmh[min_index][2] {
            min_index = i;
        }
    }
    let mut r = [rgb[0]; 8];
    let mut j = [jmh[0]; 8];
    for i in 0..CORNERS {
        r[i + 1] = rgb[(i + min_index) % CORNERS];
        j[i + 1] = jmh[(i + min_index) % CORNERS];
    }
    r[0] = r[CORNERS];
    r[CORNERS + 1] = r[1];
    j[0] = j[CORNERS];
    j[CORNERS + 1] = j[1];
    j[0][2] -= HUE_LIMIT;
    j[CORNERS + 1][2] += HUE_LIMIT;
    (r, j)
}

fn reach_corners(p: &JMhParams, limit_j: f32, maximum_source: f32) -> [F3; 8] {
    let limit_a = j_to_achromatic(limit_j, p.inv_cz);
    let mut tmp = [[0.0; 3]; CORNERS];
    for (i, t) in tmp.iter_mut().enumerate() {
        let v = corner(i);
        let (mut lower, mut upper) = (0.0f32, maximum_source);
        while upper - lower > REACH_CUSP_TOLERANCE {
            let test = (lower + upper) / 2.0;
            let a = p.rgb_to_aab(v.map(|c| c * test))[0];
            if a < limit_a {
                lower = test;
            } else {
                upper = test;
            }
            if a == limit_a {
                break;
            }
        }
        *t = p.rgb_to_jmh(v.map(|c| c * upper));
    }
    cycle(&[(); CORNERS], &tmp).1
}

fn limiting_corners(p: &JMhParams, peak: f32) -> ([F3; 8], [F3; 8]) {
    let rgb = [0, 1, 2, 3, 4, 5].map(|i| corner(i).map(|c| c * (peak / REFERENCE_LUMINANCE)));
    let jmh = rgb.map(|c| p.rgb_to_jmh(c));
    cycle(&rgb, &jmh)
}

fn sorted_hues(reach: &[F3; 8], display: &[F3; 8]) -> Vec<f32> {
    let mut out = Vec::new();
    let (mut ri, mut di) = (1, 1);
    while ri < CORNERS + 1 || di < CORNERS + 1 {
        let (rh, dh) = (reach[ri][2], display[di][2]);
        if rh == dh {
            out.push(rh);
            ri += 1;
            di += 1;
        } else if rh < dh {
            out.push(rh);
            ri += 1;
        } else {
            out.push(dh);
            di += 1;
        }
    }
    out
}

fn build_hue_table(sorted: &[f32]) -> Vec<f32> {
    let mut table = vec![0.0f32; TABLE_SIZE];
    let mut samples = [0u32; 2 * CORNERS + 2];
    let mut last = u32::MAX;
    let mut min_index = if sorted[0] == 0.0 { 0 } else { 1 };
    for i in 0..sorted.len() {
        let mut nominal = ((sorted[i] * (360.0 / HUE_LIMIT)).round() as u32)
            .max(min_index)
            .min(359);
        if last == nominal {
            if i > 1 && samples[i - 2] != samples[i - 1].wrapping_sub(1) {
                samples[i - 1] = samples[i - 1].wrapping_sub(1);
            } else {
                nominal += 1;
            }
        }
        samples[i] = nominal.min(359);
        last = nominal;
        min_index = nominal;
    }
    let mut fill = |count: u32, lower: f32, upper: f32, base: usize| {
        let delta = (upper - lower) / count as f32;
        for i in 0..count as usize {
            table[base + i] = lower + i as f32 * delta;
        }
    };
    let mut total = 0u32;
    fill(samples[0], 0.0, sorted[0], 1);
    total += samples[0];
    for i in 1..sorted.len() {
        let n = samples[i].wrapping_sub(samples[i - 1]);
        fill(n, sorted[i - 1], sorted[i], total as usize + 1);
        total = total.wrapping_add(n);
    }
    fill(
        360 - total,
        sorted[sorted.len() - 1],
        HUE_LIMIT,
        total as usize + 1,
    );
    table[0] = table[LAST_NOMINAL] - HUE_LIMIT;
    table[UPPER_WRAP] = table[1] + HUE_LIMIT;
    table[UPPER_WRAP + 1] = table[2] + HUE_LIMIT;
    table
}

fn display_cusp(
    hue: f32,
    rgb_c: &[F3; 8],
    jmh_c: &[F3; 8],
    p: &JMhParams,
    prev: &mut [f32; 2],
) -> [f32; 2] {
    let upper = (1..8).find(|&i| jmh_c[i][2] > hue).unwrap_or(1);
    let lower = upper - 1;
    if jmh_c[lower][2] == hue {
        return [jmh_c[lower][0], jmh_c[lower][1]];
    }
    let (lo_c, hi_c) = (rgb_c[lower], rgb_c[upper]);
    let lerp3 = |t: f32| [0, 1, 2].map(|i| lerpf(lo_c[i], hi_c[i], t));
    let mut lower_t = if upper as f32 == prev[0] {
        prev[1]
    } else {
        0.0
    };
    let mut upper_t = 1.0f32;
    while upper_t - lower_t > DISPLAY_CUSP_TOLERANCE {
        let t = (lower_t + upper_t) / 2.0;
        let jmh = p.rgb_to_jmh(lerp3(t));
        if jmh[2] < jmh_c[lower][2] {
            upper_t = t;
        } else if jmh[2] >= jmh_c[upper][2] {
            lower_t = t;
        } else if jmh[2] > hue {
            upper_t = t;
        } else {
            lower_t = t;
        }
    }
    let t = (lower_t + upper_t) / 2.0;
    let jmh = p.rgb_to_jmh(lerp3(t));
    *prev = [upper as f32, t];
    [jmh[0], jmh[1]]
}

fn reach_m_table(p: &JMhParams, limit_j_max: f32) -> Vec<f32> {
    let mut t = vec![0.0f32; TABLE_SIZE];
    for i in 0..360 {
        let hue = i as f32;
        let (mut low, mut high) = (0.0f32, 50.0f32);
        let mut outside = false;
        let below_zero = |m: f32| p.jmh_to_rgb([limit_j_max, m, hue]).iter().any(|&c| c < 0.0);
        while !outside && high < 1300.0 {
            outside = below_zero(high);
            if !outside {
                low = high;
                high += 50.0;
            }
        }
        while high - low > 1e-2 {
            let m = (high + low) / 2.0;
            if below_zero(m) { high = m } else { low = m }
        }
        t[i + 1] = high;
    }
    t[0] = t[LAST_NOMINAL];
    t[UPPER_WRAP] = t[1];
    t[UPPER_WRAP + 1] = t[2];
    t
}

fn focus_gain(j: f32, threshold: f32, limit_j_max: f32, focus_dist: f32) -> f32 {
    let mut gain = limit_j_max * focus_dist;
    if j > threshold {
        let adj = ((limit_j_max - threshold) / (limit_j_max - j).max(0.0001)).log10();
        gain *= adj * adj + 1.0;
    }
    gain
}

fn solve_j_intersect(j: f32, m: f32, focus_j: f32, max_j: f32, slope_gain: f32) -> f32 {
    let m_scaled = m / slope_gain;
    let a = m_scaled / focus_j;
    if j < focus_j {
        let b = 1.0 - m_scaled;
        let c = -j;
        let root = (b * b - 4.0 * a * c).sqrt();
        -2.0 * c / (b + root)
    } else {
        let b = -(1.0 + m_scaled + max_j * a);
        let c = max_j * m_scaled + j;
        let root = (b * b - 4.0 * a * c).sqrt();
        -2.0 * c / (b - root)
    }
}

fn smin_scaled(a: f32, b: f32, scale_reference: f32) -> f32 {
    let s = SMOOTH_CUSPS * scale_reference;
    let h = (s - (a - b).abs()).max(0.0) / s;
    a.min(b) - h * h * h * s * (1.0 / 6.0)
}

fn compression_slope(intersect_j: f32, focus_j: f32, limit_j_max: f32, slope_gain: f32) -> f32 {
    let d = if intersect_j < focus_j {
        intersect_j
    } else {
        limit_j_max - intersect_j
    };
    d * (intersect_j - focus_j) / (focus_j * slope_gain)
}

fn line_boundary_m(
    j_axis: f32,
    slope: f32,
    inv_gamma: f32,
    j_max: f32,
    m_max: f32,
    j_ref: f32,
) -> f32 {
    let shifted = j_ref * (j_axis / j_ref).powf(inv_gamma);
    shifted * m_max / (j_max - slope * m_max)
}

fn gamut_boundary_m(
    jm_cusp: [f32; 2],
    j_max: f32,
    top_inv: f32,
    bottom_inv: f32,
    j_src: f32,
    slope: f32,
    j_cusp: f32,
) -> f32 {
    let lower = line_boundary_m(j_src, slope, bottom_inv, jm_cusp[0], jm_cusp[1], j_cusp);
    let upper = line_boundary_m(
        j_max - j_src,
        -slope,
        top_inv,
        j_max - jm_cusp[0],
        jm_cusp[1],
        j_max - j_cusp,
    );
    smin_scaled(lower, upper, jm_cusp[1])
}

fn remap_m(m: f32, gamut_m: f32, reach_m: f32, invert: bool) -> f32 {
    let proportion = (gamut_m / reach_m).max(COMPRESSION_THRESHOLD);
    let threshold = proportion * gamut_m;
    if m <= threshold || proportion >= 1.0 {
        return m;
    }
    let m_off = m - threshold;
    let gamut_off = gamut_m - threshold;
    let reach_off = reach_m - threshold;
    let scale = reach_off / ((reach_off / gamut_off) - 1.0);
    let nd = m_off / scale;
    threshold
        + if invert {
            if nd >= 1.0 {
                scale
            } else {
                scale * -(nd / (nd - 1.0))
            }
        } else {
            scale * nd / (1.0 + nd)
        }
}

fn compute_focus_j(cusp_j: f32, mid_j: f32, limit_j_max: f32) -> f32 {
    lerpf(
        cusp_j,
        mid_j,
        (CUSP_MID_BLEND - cusp_j / limit_j_max).min(1.0),
    )
}

struct GammaTest {
    test_jmh: F3,
    j_src: f32,
    slope: f32,
    j_cusp: f32,
}

impl Gamut {
    fn new(
        peak: f32,
        p_in: &JMhParams,
        p_lim: &JMhParams,
        ts: &ToneScale,
        sh: &Shared,
        reach: &JMhParams,
    ) -> Gamut {
        let mid_j = p_in.y_to_j(ts.c_t * REFERENCE_LUMINANCE);
        let focus_dist = FOCUS_DISTANCE + FOCUS_DISTANCE * FOCUS_DISTANCE_SCALING * ts.log_peak;
        let lower_hull_gamma_inv = 1.0 / (1.14 + 0.07 * ts.log_peak);

        let reach_c = reach_corners(reach, sh.limit_j_max, ts.forward_limit);
        let (lim_rgb, lim_jmh) = limiting_corners(p_lim, peak);
        let hue_table = build_hue_table(&sorted_hues(&reach_c, &lim_jmh));
        let mut cusp = vec![[0.0f32; 3]; TABLE_SIZE];
        let mut prev = [0.0f32; 2];
        for i in 1..UPPER_WRAP {
            let jm = display_cusp(hue_table[i], &lim_rgb, &lim_jmh, p_lim, &mut prev);
            cusp[i] = [jm[0], jm[1] * (1.0 + SMOOTH_M * SMOOTH_CUSPS), hue_table[i]];
        }
        cusp[0] = [cusp[LAST_NOMINAL][0], cusp[LAST_NOMINAL][1], hue_table[0]];
        cusp[UPPER_WRAP] = [cusp[1][0], cusp[1][1], hue_table[UPPER_WRAP]];
        cusp[UPPER_WRAP + 1] = [cusp[2][0], cusp[2][1], hue_table[UPPER_WRAP + 1]];

        let mut search_range = [0i32, 1i32];
        for (i, c) in cusp.iter().enumerate().take(UPPER_WRAP).skip(1) {
            let delta = i as i32 - (1 + c[2] as u32) as i32;
            search_range[0] = search_range[0].min(delta);
            search_range[1] = search_range[1].max(delta + 1);
        }

        let mut g = Gamut {
            mid_j,
            focus_dist,
            lower_hull_gamma_inv,
            search_range,
            hue_table,
            cusp_table: cusp,
        };
        g.upper_hull_gamma(peak, sh.limit_j_max, p_lim);
        g
    }

    fn upper_hull_gamma(&mut self, peak: f32, limit_j_max: f32, p_lim: &JMhParams) {
        for i in 1..UPPER_WRAP {
            let hue = self.hue_table[i];
            let jm = [self.cusp_table[i][0], self.cusp_table[i][1]];
            let data = self.gamma_test_data(jm, hue, limit_j_max);
            let fits = |gamma: f32| {
                let top_inv = 1.0 / gamma;
                data.iter().all(|d| {
                    let m = gamut_boundary_m(
                        jm,
                        limit_j_max,
                        top_inv,
                        self.lower_hull_gamma_inv,
                        d.j_src,
                        d.slope,
                        d.j_cusp,
                    );
                    let j = d.j_src + d.slope * m;
                    let rgb = p_lim.jmh_to_rgb([j, m, d.test_jmh[2]]);
                    let lim = peak / REFERENCE_LUMINANCE;
                    rgb[0] > lim || rgb[1] > lim || rgb[2] > lim
                })
            };
            let (mut low, mut high) = (0.0f32, GAMMA_SEARCH_STEP);
            let mut outside = false;
            while !outside && high < GAMMA_MAXIMUM {
                if fits(high) {
                    outside = true;
                } else {
                    low = high;
                    high += GAMMA_SEARCH_STEP;
                }
            }
            while high - low > GAMMA_ACCURACY {
                let t = (high + low) / 2.0;
                if fits(t) { high = t } else { low = t }
            }
            self.cusp_table[i][2] = 1.0 / high;
        }
        self.cusp_table[0][2] = self.cusp_table[LAST_NOMINAL][2];
        self.cusp_table[UPPER_WRAP][2] = self.cusp_table[1][2];
        self.cusp_table[UPPER_WRAP + 1][2] = self.cusp_table[2][2];
    }

    fn gamma_test_data(&self, jm: [f32; 2], hue: f32, limit_j_max: f32) -> Vec<GammaTest> {
        let threshold = lerpf(jm[0], limit_j_max, FOCUS_GAIN_BLEND);
        let focus_j = compute_focus_j(jm[0], self.mid_j, limit_j_max);
        [0.01f32, 0.1, 0.5, 0.8, 0.99]
            .iter()
            .map(|&pos| {
                let test_j = lerpf(jm[0], limit_j_max, pos);
                let gain = focus_gain(test_j, threshold, limit_j_max, self.focus_dist);
                let j_src = solve_j_intersect(test_j, jm[1], focus_j, limit_j_max, gain);
                GammaTest {
                    test_jmh: [test_j, jm[1], hue],
                    j_src,
                    slope: compression_slope(j_src, focus_j, limit_j_max, gain),
                    j_cusp: solve_j_intersect(jm[0], jm[1], focus_j, limit_j_max, gain),
                }
            })
            .collect()
    }

    fn hue_params(&self, hue: f32, r: &Resolved) -> HueParams {
        let h = &self.hue_table;
        let mut i = 1 + hue as u32 as i32;
        let mut i_lo = 0.max(i + self.search_range[0]);
        let mut i_hi = (UPPER_WRAP as i32).min(i + self.search_range[1]);
        while i_lo + 1 < i_hi {
            if hue > h[i as usize] {
                i_lo = i;
            } else {
                i_hi = i;
            }
            i = (i_lo + i_hi) / 2;
        }
        let i_hi = i_hi.max(1) as usize;
        let t = (hue - h[i_hi - 1]) / (h[i_hi] - h[i_hi - 1]);
        let (a, b) = (self.cusp_table[i_hi - 1], self.cusp_table[i_hi]);
        let cusp = [
            lerpf(a[0], b[0], t),
            lerpf(a[1], b[1], t),
            lerpf(a[2], b[2], t),
        ];
        HueParams {
            gamma_bottom_inv: self.lower_hull_gamma_inv,
            jm_cusp: [cusp[0], cusp[1]],
            gamma_top_inv: cusp[2],
            focus_j: compute_focus_j(cusp[0], self.mid_j, r.limit_j_max),
            analytical_threshold: lerpf(cusp[0], r.limit_j_max, FOCUS_GAIN_BLEND),
        }
    }

    fn compress(&self, jmh: F3, jx: f32, r: &Resolved, hp: &HueParams, invert: bool) -> F3 {
        let (j, m, h) = (jmh[0], jmh[1], jmh[2]);
        let gain = focus_gain(jx, hp.analytical_threshold, r.limit_j_max, self.focus_dist);
        let j_src = solve_j_intersect(j, m, hp.focus_j, r.limit_j_max, gain);
        let slope = compression_slope(j_src, hp.focus_j, r.limit_j_max, gain);
        let j_cusp = solve_j_intersect(
            hp.jm_cusp[0],
            hp.jm_cusp[1],
            hp.focus_j,
            r.limit_j_max,
            gain,
        );
        let gamut_m = gamut_boundary_m(
            hp.jm_cusp,
            r.limit_j_max,
            hp.gamma_top_inv,
            hp.gamma_bottom_inv,
            j_src,
            slope,
            j_cusp,
        );
        if gamut_m <= 0.0 {
            return [j, 0.0, h];
        }
        let reach_m = line_boundary_m(
            j_src,
            slope,
            r.model_gamma_inv,
            r.limit_j_max,
            r.reach_max_m,
            r.limit_j_max,
        );
        let rm = remap_m(m, gamut_m, reach_m, invert);
        [j_src + rm * slope, rm, h]
    }

    fn fwd(&self, jmh: F3, r: &Resolved) -> F3 {
        let (j, m, h) = (jmh[0], jmh[1], jmh[2]);
        if j <= 0.0 {
            return [0.0, 0.0, h];
        }
        if m <= 0.0 || j > r.limit_j_max {
            return [j, 0.0, h];
        }
        let hp = self.hue_params(h, r);
        self.compress(jmh, j, r, &hp, false)
    }

    fn inv(&self, jmh: F3, r: &Resolved) -> F3 {
        let (j, m, h) = (jmh[0], jmh[1], jmh[2]);
        if j <= 0.0 {
            return [0.0, 0.0, h];
        }
        if m <= 0.0 || j > r.limit_j_max {
            return [j, 0.0, h];
        }
        let hp = self.hue_params(h, r);
        let mut jx = j;
        if jx > hp.analytical_threshold {
            jx = self.compress(jmh, jx, r, &hp, true)[0];
        }
        self.compress(jmh, jx, r, &hp, true)
    }
}

/// ACES 2.0 display rendering: ACES2065-1 in, linear limiting-primaries RGB out (1.0 = 100 nits).
pub(crate) struct Aces2 {
    p_in: JMhParams,
    p_out: JMhParams,
    ts: ToneScale,
    shared: Shared,
    chroma: Chroma,
    gamut: Gamut,
}

impl Aces2 {
    pub fn new(peak: f32, limiting: &Primaries) -> Aces2 {
        // OCIO passes the limiting primaries through f32 op parameters.
        let r32 = |v: [f64; 2]| [v[0] as f32 as f64, v[1] as f32 as f64];
        let lim = Primaries {
            r: r32(limiting.r),
            g: r32(limiting.g),
            b: r32(limiting.b),
            w: r32(limiting.w),
        };
        let p_in = JMhParams::new(&AP0);
        let p_out = JMhParams::new(&lim);
        let ts = ToneScale::new(peak);
        let reach = JMhParams::new(&AP1);
        let limit_j_max = p_in.y_to_j(peak);
        let shared = Shared {
            limit_j_max,
            model_gamma_inv: 1.0 / model_gamma(),
            reach_m_table: reach_m_table(&reach, limit_j_max),
        };
        let chroma = Chroma {
            compr: CHROMA_COMPRESS + (CHROMA_COMPRESS * CHROMA_COMPRESS_FACT) * ts.log_peak,
            sat: (CHROMA_EXPAND - (CHROMA_EXPAND * CHROMA_EXPAND_FACT) * ts.log_peak).max(0.2),
            sat_thr: CHROMA_EXPAND_THR / ts.n,
            scale: (0.03379 * peak).powf(0.30596) - 0.45135,
        };
        let gamut = Gamut::new(peak, &p_in, &p_out, &ts, &shared, &reach);
        Aces2 {
            p_in,
            p_out,
            ts,
            shared,
            chroma,
            gamut,
        }
    }

    fn resolve(&self, hue: f32) -> Resolved {
        let base = hue as u32;
        let t = hue - base as f32;
        let i_lo = base as usize + 1;
        let tbl = &self.shared.reach_m_table;
        Resolved {
            limit_j_max: self.shared.limit_j_max,
            model_gamma_inv: self.shared.model_gamma_inv,
            reach_max_m: lerpf(tbl[i_lo], tbl[i_lo + 1], t),
        }
    }

    pub fn fwd(&self, rgb: F3) -> F3 {
        let aab = self.p_in.rgb_to_aab(rgb);
        let jmh = self.p_in.aab_to_jmh(aab);
        let r = self.resolve(jmh[2]);
        let h = to_radians(jmh[2]);
        let (cos_h, sin_h) = (h.cos(), h.sin());
        let m_norm = chroma_compress_norm(cos_h, sin_h, self.chroma.scale);
        let y = self.p_in.a_to_y(aab[0]);
        let j_ts = self.p_in.y_to_j_abs(self.ts.fwd(y)).copysign(aab[0]);
        let tonemapped = chroma_compress_fwd(jmh, j_ts, m_norm, &r, &self.chroma);
        let compressed = self.gamut.fwd(tonemapped, &r);
        self.p_out
            .aab_to_rgb(self.p_out.jmh_to_aab_cs(compressed, cos_h, sin_h))
    }

    pub fn inv(&self, rgb: F3) -> F3 {
        let compressed = self.p_out.rgb_to_jmh(rgb);
        let r = self.resolve(compressed[2]);
        let h = to_radians(compressed[2]);
        let (cos_h, sin_h) = (h.cos(), h.sin());
        let m_norm = chroma_compress_norm(cos_h, sin_h, self.chroma.scale);
        let tonemapped = self.gamut.inv(compressed, &r);
        let jt = tonemapped[0];
        let j = self
            .p_in
            .y_to_j_abs(self.ts.inv(self.p_in.j_to_y(jt.abs())))
            .copysign(jt);
        let jmh = chroma_compress_inv(tonemapped, j, m_norm, &r, &self.chroma);
        self.p_in
            .aab_to_rgb(self.p_in.jmh_to_aab_cs(jmh, cos_h, sin_h))
    }
}
