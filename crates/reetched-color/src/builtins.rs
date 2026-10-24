// BuiltinTransform styles.
//
// Ported from OpenColorIO (src/OpenColorIO/transforms/builtins/*, ops/gradingrgbcurve),
// Copyright Contributors to the OpenColorIO Project, BSD-3-Clause.
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
// Where OCIO samples a function into a 1D LUT (half-float domain or 4096 entries) this port
// evaluates the function analytically instead; the difference is far below test tolerance.

// Constants are copied verbatim from OCIO / the ACES CTL.
#![allow(clippy::excessive_precision)]

use std::sync::Arc;

use anyhow::{Result, bail};

use crate::aces2::Aces2;
use crate::ops::{GammaKind, LogParams, M3, Op, invert, mul, mul_v};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Primaries {
    pub r: [f64; 2],
    pub g: [f64; 2],
    pub b: [f64; 2],
    pub w: [f64; 2],
}

const fn prim(r: [f64; 2], g: [f64; 2], b: [f64; 2], w: [f64; 2]) -> Primaries {
    Primaries { r, g, b, w }
}

const D65: [f64; 2] = [0.3127, 0.3290];
const D60: [f64; 2] = [0.32168, 0.33767];
pub(crate) const XYZ_E: Primaries = prim([1., 0.], [0., 1.], [0., 0.], [1. / 3., 1. / 3.]);
pub(crate) const AP0: Primaries = prim([0.7347, 0.2653], [0.0, 1.0], [0.0001, -0.0770], D60);
pub(crate) const AP1: Primaries = prim([0.713, 0.293], [0.165, 0.830], [0.128, 0.044], D60);
const REC709: Primaries = prim([0.64, 0.33], [0.30, 0.60], [0.15, 0.06], D65);
const REC709_D60: Primaries = prim([0.64, 0.33], [0.30, 0.60], [0.15, 0.06], D60);
const REC2020: Primaries = prim([0.708, 0.292], [0.170, 0.797], [0.131, 0.046], D65);
const REC2020_D60: Primaries = prim([0.708, 0.292], [0.170, 0.797], [0.131, 0.046], D60);
const P3_DCI: Primaries = prim(
    [0.680, 0.320],
    [0.265, 0.690],
    [0.150, 0.060],
    [0.314, 0.351],
);
const P3_D65: Primaries = prim([0.680, 0.320], [0.265, 0.690], [0.150, 0.060], D65);
const P3_D60: Primaries = prim([0.680, 0.320], [0.265, 0.690], [0.150, 0.060], D60);
const APPLE_WG: Primaries = prim([0.725, 0.301], [0.221, 0.814], [0.068, -0.076], D65);
const CANON_CGAMUT: Primaries = prim([0.7400, 0.2700], [0.1700, 1.1400], [0.0800, -0.1000], D65);

const WHITE_D60_XYZ: [f64; 3] = [0.95264607456985, 1., 1.00882518435159];
const WHITE_D65_XYZ: [f64; 3] = [0.95045592705167, 1., 1.08905775075988];
const WHITE_DCI_XYZ: [f64; 3] = [0.89458689458689, 1., 0.95441595441595];

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Adapt {
    None,
    Bradford,
    Cat02,
}

/// RGB to XYZ (Y of white = 1) from xy chromaticities.
pub(crate) fn rgb2xyz(p: &Primaries) -> M3 {
    let xyz = |c: [f64; 2]| [c[0], c[1], 1. - c[0] - c[1]];
    let (r, g, b) = (xyz(p.r), xyz(p.g), xyz(p.b));
    let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let w = [p.w[0] / p.w[1], 1.0, (1.0 - p.w[0] - p.w[1]) / p.w[1]];
    let gains = mul_v(&invert(&m), w);
    let mut out = m;
    for row in out.iter_mut() {
        for i in 0..3 {
            row[i] *= gains[i];
        }
    }
    out
}

fn vonkries(src: [f64; 3], dst: [f64; 3], method: Adapt) -> M3 {
    let cone: M3 = if method == Adapt::Cat02 {
        [
            [0.7328, 0.4296, -0.1624],
            [-0.7036, 1.6975, 0.0061],
            [0.0030, 0.0136, 0.9834],
        ]
    } else {
        [
            [0.8951, 0.2664, -0.1614],
            [-0.7502, 1.7135, 0.0367],
            [0.0389, -0.0685, 1.0296],
        ]
    };
    let (s, d) = (mul_v(&cone, src), mul_v(&cone, dst));
    let scale = [
        [d[0] / s[0], 0., 0.],
        [0., d[1] / s[1], 0.],
        [0., 0., d[2] / s[2]],
    ];
    mul(&invert(&cone), &mul(&scale, &cone))
}

/// OCIO `build_conversion_matrix`; white overrides are XYZ values.
fn conversion_w(
    src: &Primaries,
    dst: &Primaries,
    src_w: Option<[f64; 3]>,
    dst_w: Option<[f64; 3]>,
    method: Adapt,
) -> M3 {
    let src_rgb2xyz = rgb2xyz(src);
    let dst_rgb2xyz = rgb2xyz(dst);
    let dst_xyz2rgb = invert(&dst_rgb2xyz);
    let same_white = src_w.is_none() && dst_w.is_none() && src.w == dst.w;
    if same_white || method == Adapt::None {
        return mul(&dst_xyz2rgb, &src_rgb2xyz);
    }
    let ones = [1.0; 3];
    let sw = src_w.unwrap_or_else(|| mul_v(&src_rgb2xyz, ones));
    let dw = dst_w.unwrap_or_else(|| mul_v(&dst_rgb2xyz, ones));
    mul(&dst_xyz2rgb, &mul(&vonkries(sw, dw, method), &src_rgb2xyz))
}

pub(crate) fn conversion(src: &Primaries, dst: &Primaries, method: Adapt) -> M3 {
    conversion_w(src, dst, None, None, method)
}

pub(crate) fn to_xyz_d65(src: &Primaries, method: Adapt) -> M3 {
    conversion_w(src, &XYZ_E, None, Some(WHITE_D65_XYZ), method)
}

fn from_xyz_d65(dst: &Primaries, method: Adapt) -> M3 {
    conversion_w(&XYZ_E, dst, Some(WHITE_D65_XYZ), None, method)
}

/// Scalar curves OCIO bakes into 1D LUTs; evaluated analytically here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Curve {
    /// Linear nits/100 to ST-2084 code value, mirrored.
    LinToPq,
    /// Scene linear to BT.2100 HLG, mirrored.
    LinToHlg,
    AppleLogToLin,
    /// ACEScc code value to linear AP1 (input clamped to the LUT range like OCIO).
    AcesccToLin,
    CLog2ToLin,
    CLog3ToLin,
    /// ADX channel-independent density to relative log exposure.
    AdxCidToRle,
    /// ACES 1.0 cinema roll-off of white (0.918 for D60 sim, 0.908 for D65 sim).
    RollWhite {
        white: f64,
    },
}

mod pq {
    pub const M1: f64 = 0.25 * 2610. / 4096.;
    pub const M2: f64 = 128. * 2523. / 4096.;
    pub const C2: f64 = 32. * 2413. / 4096.;
    pub const C3: f64 = 32. * 2392. / 4096.;
    pub const C1: f64 = C3 - C2 + 1.;
}

mod hlg {
    pub const A: f64 = 0.17883277;
    pub const E_MAX: f64 = 3.;
    pub const B: f64 = (1. - 4. * A) * E_MAX / 12.;
    pub const E_SCALE: f64 = 3. / E_MAX;
    pub const E_BREAK: f64 = E_MAX / 12.;
    pub fn c() -> f64 {
        (12. / E_MAX).ln() * A + 0.5 - A * (4. * A).ln()
    }
}

mod apple {
    pub const R0: f64 = -0.05641088;
    pub const RT: f64 = 0.01;
    pub const C: f64 = 47.28711236;
    pub const BETA: f64 = 0.00964052;
    pub const GAMMA: f64 = 0.08550479;
    pub const DELTA: f64 = 0.69336945;
}

const ADX_LUT: [[f64; 2]; 11] = [
    [-0.190, -6.000000000],
    [0.010, -2.721718645],
    [0.028, -2.521718645],
    [0.054, -2.321718645],
    [0.095, -2.121718645],
    [0.145, -1.921718645],
    [0.220, -1.721718645],
    [0.300, -1.521718645],
    [0.400, -1.321718645],
    [0.500, -1.121718645],
    [0.600, -0.926545676714876],
];

const HALF_MAX: f64 = 65504.0;

impl Curve {
    pub fn fwd(&self, x: f64) -> f64 {
        match *self {
            Curve::LinToPq => {
                let x = x.clamp(-HALF_MAX, HALF_MAX);
                let y = (x * 0.01).abs().powf(pq::M1);
                let n = ((pq::C1 + pq::C2 * y) / (1. + pq::C3 * y))
                    .max(0.)
                    .powf(pq::M2);
                n.copysign(x)
            }
            Curve::LinToHlg => {
                let x = x.clamp(-HALF_MAX, HALF_MAX);
                let e = x.abs();
                let y = if e < hlg::E_BREAK {
                    (e * hlg::E_SCALE).sqrt()
                } else {
                    hlg::A * (e - hlg::B).ln() + hlg::c()
                };
                y.copysign(x)
            }
            Curve::AppleLogToLin => {
                let x = x.clamp(-HALF_MAX, HALF_MAX);
                let pt = apple::C * (apple::RT - apple::R0).powi(2);
                if x >= pt {
                    2f64.powf((x - apple::DELTA) / apple::GAMMA) - apple::BETA
                } else if x >= 0. {
                    (x / apple::C).sqrt() + apple::R0
                } else {
                    apple::R0
                }
            }
            Curve::AcesccToLin => {
                let x = x.clamp(-0.36, 1.5);
                if x < (9.72 - 15.0) / 17.52 {
                    (2f64.powf(x * 17.52 - 9.72) - 2f64.powi(-16)) * 2.0
                } else {
                    2f64.powf(x * 17.52 - 9.72)
                }
            }
            Curve::CLog2ToLin => {
                let x = x.clamp(0., 1.);
                let out = if x < 0.092864125 {
                    -(10f64.powf((0.092864125 - x) / 0.24136077) - 1.) / 87.099375
                } else {
                    (10f64.powf((x - 0.092864125) / 0.24136077) - 1.) / 87.099375
                };
                out * 0.9
            }
            Curve::CLog3ToLin => {
                let x = x.clamp(0., 1.);
                let out = if x < 0.097465473 {
                    -(10f64.powf((0.12783901 - x) / 0.36726845) - 1.) / 14.98325
                } else if x <= 0.15277891 {
                    (x - 0.12512219) / 1.9754798
                } else {
                    (10f64.powf((x - 0.12240537) / 0.36726845) - 1.) / 14.98325
                };
                out * 0.9
            }
            Curve::AdxCidToRle => {
                let x = x.clamp(-HALF_MAX, HALF_MAX);
                let l = &ADX_LUT;
                if x < l[0][0] {
                    let slope = (l[1][1] - l[0][1]) / (l[1][0] - l[0][0]);
                    (l[0][1] - slope * (l[0][0] - x)).max(-10.)
                } else if x <= l[10][0] {
                    let i = l.iter().position(|e| x < e[0]).unwrap_or(10).max(1);
                    let t = (x - l[i - 1][0]) / (l[i][0] - l[i - 1][0]);
                    l[i - 1][1] * (1. - t) + l[i][1] * t
                } else {
                    let ref_pt = (7120. - 1520.) / 8000. * (100. / 55.) - 0.18f64.log10();
                    ((100. / 55.) * x - ref_pt).min(4.8162678)
                }
            }
            Curve::RollWhite { white } => {
                let x = x.clamp(-HALF_MAX, HALF_MAX);
                let (x0, x1) = (-1.0, -0.5);
                let (y0, y1) = (-white, x1);
                let m1 = x1 - x0;
                let a = y0 - y1 + m1;
                let b = 2. * (y1 - y0) - m1;
                let c = y0;
                let t = (-x - x0) / (x1 - x0);
                if t < 0. {
                    -(t * b + c)
                } else if t > 1. {
                    x
                } else {
                    -((t * a + b) * t + c)
                }
            }
        }
    }

    pub fn inv(&self, y: f64) -> f64 {
        match *self {
            Curve::LinToPq => {
                let x = y.abs().powf(1. / pq::M2);
                let l = ((x - pq::C1).max(0.) / (pq::C2 - pq::C3 * x)).powf(1. / pq::M1) * 100.;
                l.copysign(y)
            }
            Curve::LinToHlg => {
                let e = y.abs();
                let x = if e < 0.5 {
                    e * e / hlg::E_SCALE
                } else {
                    hlg::B + ((e - hlg::c()) / hlg::A).exp()
                };
                x.copysign(y)
            }
            Curve::AppleLogToLin => {
                if y >= apple::RT {
                    apple::DELTA + apple::GAMMA * (y + apple::BETA).log2()
                } else if y > apple::R0 {
                    apple::C * (y - apple::R0).powi(2)
                } else {
                    0.
                }
            }
            Curve::AcesccToLin => {
                let arg = if y < 2f64.powi(-15) {
                    2f64.powi(-16) + y * 0.5
                } else {
                    y
                };
                // Below the curve's range the LUT inverse returns its domain start.
                if arg <= 0. {
                    -0.36
                } else {
                    ((arg.log2() + 9.72) / 17.52).clamp(-0.36, 1.5)
                }
            }
            Curve::CLog2ToLin => {
                let v = y / 0.9;
                let x = if v < 0. {
                    0.092864125 - 0.24136077 * (1. - 87.099375 * v).log10()
                } else {
                    0.092864125 + 0.24136077 * (87.099375 * v + 1.).log10()
                };
                x.clamp(0., 1.)
            }
            Curve::CLog3ToLin => {
                let v = y / 0.9;
                let x = if v < -0.014 {
                    0.12783901 - 0.36726845 * (1. - 14.98325 * v).log10()
                } else if v <= 0.014 {
                    1.9754798 * v + 0.12512219
                } else {
                    0.12240537 + 0.36726845 * (14.98325 * v + 1.).log10()
                };
                x.clamp(0., 1.)
            }
            Curve::AdxCidToRle | Curve::RollWhite { .. } => self.bisect(y),
        }
    }

    // ponytail: numeric inverse for the rarely inverted ADX / cinema roll-off curves.
    fn bisect(&self, y: f64) -> f64 {
        let (mut lo, mut hi) = (-HALF_MAX, HALF_MAX);
        let (y_lo, y_hi) = (self.fwd(lo), self.fwd(hi));
        let y = y.clamp(y_lo, y_hi);
        // Like a LUT inverse, values on a flat end map to the end of the flat run nearest
        // the curve's sloped part.
        let top = y >= y_hi;
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            let f = self.fwd(mid);
            if if top { f < y } else { f <= y } {
                lo = mid
            } else {
                hi = mid
            }
        }
        0.5 * (lo + hi)
    }
}

/// Monotonic quadratic B-spline curve (OCIO GradingBSplineCurve, RGB curve type).
pub(crate) struct BSpline {
    knots: Vec<f32>,
    a: Vec<f32>,
    b: Vec<f32>,
    c: Vec<f32>,
}

impl BSpline {
    pub fn new(pts: &[[f32; 2]], slopes: &[f32]) -> BSpline {
        let mut slopes = slopes.to_vec();
        let mut s = Self::fit(pts, &slopes);
        if Self::adjust_slopes(pts, &mut slopes, &s.knots) {
            s = Self::fit(pts, &slopes);
        }
        s
    }

    fn fit(pts: &[[f32; 2]], slopes: &[f32]) -> BSpline {
        let mut s = BSpline {
            knots: vec![pts[0][0]],
            a: vec![],
            b: vec![],
            c: vec![],
        };
        for i in 0..pts.len() - 1 {
            let (xi, yi) = (pts[i][0], pts[i][1]);
            let (xi1, yi1) = (pts[i + 1][0], pts[i + 1][1]);
            let dx = xi1 - xi;
            let secant = (yi1 - yi) / dx;
            if ((slopes[i] + slopes[i + 1]) - 2. * secant).abs() < 1e-6 {
                s.c.push(yi);
                s.b.push(slopes[i]);
                s.a.push(0.5 * (slopes[i + 1] - slopes[i]) / dx);
            } else {
                let aa = slopes[i] - secant;
                let bb = slopes[i + 1] - secant;
                let ksi = if aa * bb >= 0. {
                    (xi + xi1) * 0.5
                } else if aa.abs() > bb.abs() {
                    xi1 + aa * dx / (slopes[i + 1] - slopes[i])
                } else {
                    xi + bb * dx / (slopes[i + 1] - slopes[i])
                };
                let s_bar =
                    (2. * secant - slopes[i + 1]) + (slopes[i + 1] - slopes[i]) * (ksi - xi) / dx;
                let eta = (s_bar - slopes[i]) / (ksi - xi);
                s.c.push(yi);
                s.b.push(slopes[i]);
                s.a.push(0.5 * eta);
                s.c.push(yi + slopes[i] * (ksi - xi) + 0.5 * eta * (ksi - xi) * (ksi - xi));
                s.b.push(s_bar);
                s.a.push(0.5 * (slopes[i + 1] - s_bar) / (xi1 - ksi));
                s.knots.push(ksi);
            }
            s.knots.push(xi1);
        }
        s
    }

    fn adjust_slopes(pts: &[[f32; 2]], slopes: &mut [f32], knots: &[f32]) -> bool {
        let mut done = false;
        let mut i = 0;
        for &ksi in knots {
            if pts[i][0] != ksi {
                let (xi, yi) = (pts[i][0], pts[i][1]);
                let (xi1, yi1) = (pts[i + 1][0], pts[i + 1][1]);
                let s_bar =
                    (2. * (yi1 - yi) - (ksi - xi) * slopes[i] - (xi1 - ksi) * slopes[i + 1])
                        / (xi1 - xi);
                if s_bar < 0. {
                    done = true;
                    let secant = (yi1 - yi) / (xi1 - xi);
                    let blend = ((ksi - xi) * slopes[i] + (xi1 - ksi) * slopes[i + 1]) / (xi1 - xi);
                    let aim = (0.01 * 0.5 * (slopes[i] + slopes[i + 1])).min(secant);
                    let adjust = (2. * secant - aim) / blend;
                    slopes[i] *= adjust;
                    slopes[i + 1] *= adjust;
                }
                i += 1;
            }
        }
        done
    }

    pub fn eval(&self, x: f32) -> f32 {
        let (k, n) = (&self.knots, self.a.len());
        let (start, end) = (k[0], k[k.len() - 1]);
        if x <= start {
            return (x - start) * self.b[0] + self.c[0];
        }
        if x >= end {
            let t = end - k[k.len() - 2];
            let (a, b, c) = (self.a[n - 1], self.b[n - 1], self.c[n - 1]);
            return (x - end) * (2. * a * t + b) + (a * t + b) * t + c;
        }
        let i = (0..k.len() - 2)
            .find(|&i| x < k[i + 1])
            .unwrap_or(k.len() - 2);
        let t = x - k[i];
        (self.a[i] * t + self.b[i]) * t + self.c[i]
    }

    pub fn eval_rev(&self, y: f32) -> f32 {
        let (k, n) = (&self.knots, self.a.len());
        let (start, end) = (k[0], k[k.len() - 1]);
        let t_end = end - k[k.len() - 2];
        let (an, bn, cn) = (self.a[n - 1], self.b[n - 1], self.c[n - 1]);
        let end_y = (an * t_end + bn) * t_end + cn;
        if y <= self.c[0] {
            let b = self.b[0];
            return if b.abs() < 1e-5 {
                start
            } else {
                (y - self.c[0]) / b + start
            };
        }
        if y >= end_y {
            let slope = 2. * an * t_end + bn;
            return if slope.abs() < 1e-5 {
                end
            } else {
                (y - end_y) / slope + end
            };
        }
        let i = (0..k.len() - 2)
            .find(|&i| y < self.c[i + 1])
            .unwrap_or(k.len() - 2);
        let (a, b) = (self.a[i], self.b[i]);
        let c0 = self.c[i] - y;
        let denom = (b * b - 4. * a * c0).sqrt() + b;
        if denom.abs() < 1e-5 {
            return if b.abs() < 1e-5 { k[i] } else { k[i] - c0 / b };
        }
        k[i] + (-2. * c0) / denom
    }
}

fn bspline(pts: &[[f32; 2]], slopes: &[f32]) -> Op {
    Op::BSpline {
        curve: Arc::new(BSpline::new(pts, slopes)),
        inverse: false,
    }
}

fn log10_op(inverse: bool) -> Op {
    Op::Log {
        base: 10.,
        p: [LogParams::PLAIN; 3],
        inverse,
    }
}

fn curve(c: Curve) -> Op {
    Op::Curve {
        curve: c,
        inverse: false,
    }
}

fn gamma(kind: GammaKind, g: f64, offset: f64) -> Op {
    // Display builtins are "reverse" gammas: linear in, encoded out.
    Op::Gamma {
        kind,
        inverse: true,
        gamma: [g; 3],
        offset: [offset; 3],
    }
}

fn rrt_preamble(ops: &mut Vec<Op>) {
    ops.push(Op::Glow {
        gain: 0.05,
        mid: 0.08,
        inverse: false,
    });
    ops.push(Op::RedMod {
        v03: false,
        inverse: false,
    });
    ops.push(Op::clamp(Some(0.), None));
    ops.push(Op::matrix(conversion(&AP0, &AP1, Adapt::None)));
    ops.push(Op::clamp(Some(0.), None));
    ops.push(Op::matrix([
        [0.970889148671, 0.026963270632, 0.002147580696],
        [0.010889148671, 0.986963270632, 0.002147580696],
        [0.010889148671, 0.026963270632, 0.962147580696],
    ]));
}

fn sdr_tonecurve(ops: &mut Vec<Op>) {
    ops.push(log10_op(false));
    ops.push(bspline(
        &[
            [-5.26017743, -4.],
            [-3.75502745, -3.57868829],
            [-2.24987747, -1.82131329],
            [-0.74472749, 0.68124124],
            [1.06145248, 2.87457742],
            [2.86763245, 3.83406206],
            [4.67381243, 4.],
        ],
        &[0., 0.55982688, 1.77532247, 1.55, 0.8787017, 0.18374463, 0.],
    ));
    ops.push(bspline(
        &[
            [-2.54062362, -1.69897000],
            [-2.08035721, -1.58843500],
            [-1.62009080, -1.35350000],
            [-1.15982439, -1.04695000],
            [-0.69955799, -0.65640000],
            [-0.23929158, -0.22141000],
            [0.22097483, 0.22814402],
            [0.68124124, 0.68124124],
            [1.01284632, 0.99142189],
            [1.34445140, 1.25800000],
            [1.67605648, 1.44995000],
            [2.00766156, 1.55910000],
            [2.33926665, 1.62260000],
            [2.67087173, 1.66065457],
            [3.00247681, 1.68124124],
        ],
        &[
            0., 0.4803088, 0.5405565, 0.79149813, 0.9055625, 0.98460368, 0.96884766, 1.,
            0.87078346, 0.73702127, 0.42068113, 0.23763206, 0.14535362, 0.08416378, 0.04,
        ],
    ));
    ops.push(log10_op(true));
    let (white, black) = (48., 0.02);
    let s = 1. / (white - black);
    ops.push(Op::scale_offset([s; 3], [-black * s; 3]));
}

fn hdr_tonecurve(ops: &mut Vec<Op>, y_max: f64) {
    ops.push(log10_op(false));
    let lo: [[f32; 2]; 4] = [
        [0., -4.],
        [0., -3.57868829],
        [0., -1.82131329],
        [0., 0.68124124],
    ];
    let (xs, hi, s): ([f32; 4], [[f32; 2]; 3], [f32; 2]) = match y_max as u32 {
        1000 => (
            [-5.60050155, -4.09535157, -2.59020159, -1.08505161],
            [
                [0.22347059, 2.22673503],
                [1.53199279, 2.87906206],
                [2.84051500, 3.0],
            ],
            [0.81219728, 0.1848466],
        ),
        2000 => (
            [-5.59738488, -4.09223490, -2.58708492, -1.08193494],
            [
                [0.37639718, 2.42130131],
                [1.83472930, 3.16609199],
                [3.29306142, 3.30103],
            ],
            [0.83637009, 0.18505799],
        ),
        4000 => (
            [-5.59503319, -4.08988322, -2.58473324, -1.07958326],
            [
                [0.52855878, 2.61625839],
                [2.13670081, 3.45351273],
                [3.74484285, 3.60205999],
            ],
            [0.85652519, 0.18474395],
        ),
        _ => (
            [-5.37852506, -3.87337508, -2.36822510, -0.86307513],
            [
                [-0.03557710, 1.60464482],
                [0.79192092, 1.96008059],
                [1.61941895, 2.03342376],
            ],
            [0.68179646, 0.17726487],
        ),
    };
    let mut pts: Vec<[f32; 2]> = (0..4).map(|i| [xs[i], lo[i][1]]).collect();
    pts.extend_from_slice(&hi);
    ops.push(bspline(
        &pts,
        &[0., 0.55982688, 1.77532247, 1.55, s[0], s[1], 0.],
    ));
    ops.push(log10_op(true));
    let y_min = 0.0001;
    let sc = 1. / (y_max - y_min);
    ops.push(Op::scale_offset([sc; 3], [-y_min * sc; 3]));
}

fn video_adjust(ops: &mut Vec<Op>) {
    ops.push(Op::DarkToDim { gamma: 0.9811 });
    ops.push(Op::matrix([
        [0.949056010175, 0.047185723607, 0.003758266219],
        [0.019056010175, 0.977185723607, 0.003758266219],
        [0.019056010175, 0.047185723607, 0.933758266219],
    ]));
}

fn sdr_primary_clamp(ops: &mut Vec<Op>, lim: &Primaries) {
    ops.push(Op::matrix(conversion(&AP1, lim, Adapt::Bradford)));
    ops.push(Op::clamp(Some(0.), Some(1.)));
    ops.push(Op::matrix(rgb2xyz(lim)));
}

fn hdr_primary_clamp(ops: &mut Vec<Op>, lim: &Primaries) {
    ops.push(Op::matrix(conversion(&AP1, lim, Adapt::None)));
    ops.push(Op::clamp(Some(0.), Some(1.)));
    ops.push(Op::matrix(rgb2xyz(lim)));
    ops.push(Op::matrix(vonkries(
        WHITE_D60_XYZ,
        WHITE_D65_XYZ,
        Adapt::Bradford,
    )));
}

fn aces2_output(
    ops: &mut Vec<Op>,
    peak: f32,
    lim: Primaries,
    enc: Primaries,
    linear_scale: f32,
    scale_white: bool,
) {
    let to_ap1 = conversion(&AP0, &AP1, Adapt::None);
    ops.push(Op::matrix(to_ap1));
    let upper = 8.0f32 * (128.0 + 768.0 * ((peak / 100.0).ln() / (10000.0f32 / 100.0).ln()));
    ops.push(Op::clamp(Some(0.), Some(upper as f64)));
    ops.push(Op::matrix(invert(&to_ap1)));
    ops.push(Op::Aces2 {
        t: Arc::new(Aces2::new(peak, &lim)),
        inverse: false,
    });
    ops.push(Op::clamp(Some(0.), Some((peak / 100.0) as f64)));
    if scale_white {
        let w = mul_v(&conversion(&lim, &enc, Adapt::None), [1.; 3]);
        ops.push(Op::scale(1. / w[0].max(w[1]).max(w[2])));
    }
    if linear_scale != 1.0 {
        ops.push(Op::scale(linear_scale as f64));
    }
    ops.push(Op::matrix(to_xyz_d65(&lim, Adapt::None)));
}

fn adx(ops: &mut Vec<Op>, scale: f64, offset: f64) {
    ops.push(Op::scale_offset([scale; 3], [offset; 3]));
    ops.push(Op::matrix([
        [0.75573, 0.22197, 0.02230],
        [0.05901, 0.96928, -0.02829],
        [0.16134, 0.07406, 0.76460],
    ]));
    ops.push(curve(Curve::AdxCidToRle));
    ops.push(log10_op(true));
    ops.push(Op::matrix([
        [0.72286, 0.12630, 0.15084],
        [0.11923, 0.76418, 0.11659],
        [0.01427, 0.08213, 0.90359],
    ]));
}

fn hlg_1000(ops: &mut Vec<Op>) {
    ops.push(Op::matrix(from_xyz_d65(&REC2020, Adapt::None)));
    let gamma: f64 = 1.2;
    ops.push(Op::scale(100.));
    ops.push(Op::scale(hlg::E_MAX.powf(gamma) / 1000.));
    ops.push(Op::Rec2100Surround {
        gamma: (1. / gamma) as f32,
        inverse: false,
    });
    ops.push(curve(Curve::LinToHlg));
}

/// Ops for a BuiltinTransform style, forward direction.
pub(crate) fn builtin_ops(style: &str) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let o = &mut ops;
    // ACES 2.0 output transforms: (peak nits, limiting primaries, encoding primaries, linear scale, scale white).
    let aces2 = |s: &str| -> Option<(f32, Primaries, Primaries, f32, bool)> {
        let rest = s
            .strip_prefix("ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - ")?
            .strip_suffix("_2.0")?;
        Some(match rest {
            "SDR-100nit-REC709" => (100., REC709, REC709, 1., false),
            "SDR-100nit-P3-D65" => (100., P3_D65, P3_D65, 1., false),
            "HDR-108nit-P3-D65" => (225., P3_D65, P3_D65, 0.48, false),
            "HDR-300nit-P3-D65" => (625., P3_D65, P3_D65, 0.48, false),
            "HDR-500nit-P3-D65" => (500., P3_D65, P3_D65, 1., false),
            "HDR-1000nit-P3-D65" => (1000., P3_D65, P3_D65, 1., false),
            "HDR-2000nit-P3-D65" => (2000., P3_D65, P3_D65, 1., false),
            "HDR-4000nit-P3-D65" => (4000., P3_D65, P3_D65, 1., false),
            "HDR-500nit-REC2020" => (500., REC2020, REC2020, 1., false),
            "HDR-1000nit-REC2020" => (1000., REC2020, REC2020, 1., false),
            "HDR-2000nit-REC2020" => (2000., REC2020, REC2020, 1., false),
            "HDR-4000nit-REC2020" => (4000., REC2020, REC2020, 1., false),
            "SDR-100nit-REC709-D60-in-REC709-D65" => (100., REC709_D60, REC709, 1., true),
            "SDR-100nit-REC709-D60-in-P3-D65" => (100., REC709_D60, P3_D65, 1., true),
            "SDR-100nit-REC709-D60-in-REC2020-D65" => (100., REC709_D60, REC2020, 1., true),
            "SDR-100nit-P3-D60-in-P3-D65" => (100., P3_D60, P3_D65, 1., true),
            "SDR-100nit-P3-D60-in-XYZ-E" => (100., P3_D60, XYZ_E, 1., false),
            "HDR-108nit-P3-D60-in-P3-D65" => (225., P3_D60, P3_D65, 0.48, true),
            "HDR-300nit-P3-D60-in-XYZ-E" => (625., P3_D60, XYZ_E, 0.48, true),
            "HDR-500nit-P3-D60-in-P3-D65" => (500., P3_D60, P3_D65, 1., true),
            "HDR-1000nit-P3-D60-in-P3-D65" => (1000., P3_D60, P3_D65, 1., true),
            "HDR-2000nit-P3-D60-in-P3-D65" => (2000., P3_D60, P3_D65, 1., true),
            "HDR-4000nit-P3-D60-in-P3-D65" => (4000., P3_D60, P3_D65, 1., true),
            "HDR-500nit-P3-D60-in-REC2020-D65" => (500., P3_D60, REC2020, 1., true),
            "HDR-1000nit-P3-D60-in-REC2020-D65" => (1000., P3_D60, REC2020, 1., true),
            "HDR-2000nit-P3-D60-in-REC2020-D65" => (2000., P3_D60, REC2020, 1., true),
            "HDR-4000nit-P3-D60-in-REC2020-D65" => (4000., P3_D60, REC2020, 1., true),
            "HDR-500nit-REC2020-D60-in-REC2020-D65" => (500., REC2020_D60, REC2020, 1., true),
            "HDR-1000nit-REC2020-D60-in-REC2020-D65" => (1000., REC2020_D60, REC2020, 1., true),
            "HDR-2000nit-REC2020-D60-in-REC2020-D65" => (2000., REC2020_D60, REC2020, 1., true),
            "HDR-4000nit-REC2020-D60-in-REC2020-D65" => (4000., REC2020_D60, REC2020, 1., true),
            _ => return None,
        })
    };
    if let Some((peak, lim, enc, scale, white)) = aces2(style) {
        aces2_output(o, peak, lim, enc, scale, white);
        return Ok(ops);
    }
    let ap1_to_xyz = || Op::matrix(to_xyz_d65(&AP1, Adapt::Bradford));
    match style {
        "UTILITY - ACES-AP0_to_CIE-XYZ-D65_BFD" => {
            o.push(Op::matrix(to_xyz_d65(&AP0, Adapt::Bradford)))
        }
        "UTILITY - ACES-AP1_to_CIE-XYZ-D65_BFD" => o.push(ap1_to_xyz()),
        "UTILITY - ACES-AP1_to_LINEAR-REC709_BFD" => {
            o.push(Op::matrix(conversion(&AP1, &REC709, Adapt::Bradford)))
        }
        "CURVE - ACEScct-LOG_to_LINEAR" | "ACEScct_to_ACES2065-1" => {
            let p = LogParams {
                log_slope: 1. / 17.52,
                log_offset: 9.72 / 17.52,
                lin_slope: 1.,
                lin_offset: 0.,
                lin_break: Some(0.0078125),
                linear_slope: None,
            };
            o.push(Op::Log {
                base: 2.,
                p: [p; 3],
                inverse: true,
            });
            if style == "ACEScct_to_ACES2065-1" {
                o.push(Op::matrix(conversion(&AP1, &AP0, Adapt::None)));
            }
        }
        "ACEScc_to_ACES2065-1" => {
            o.push(curve(Curve::AcesccToLin));
            o.push(Op::matrix(conversion(&AP1, &AP0, Adapt::None)));
            o.push(Op::clamp(Some(0.), None));
        }
        "ACEScg_to_ACES2065-1" => o.push(Op::matrix(conversion(&AP1, &AP0, Adapt::None))),
        "ACESproxy10i_to_ACES2065-1" => {
            o.push(Op::Range {
                min_in: Some(64. / 1023.),
                max_in: Some(940. / 1023.),
                min_out: Some((64. - 425.) / 50. - 2.5),
                max_out: Some((940. - 425.) / 50. - 2.5),
            });
            o.push(Op::Log {
                base: 2.,
                p: [LogParams::PLAIN; 3],
                inverse: true,
            });
            o.push(Op::matrix(conversion(&AP1, &AP0, Adapt::None)));
        }
        "ADX10_to_ACES2065-1" => adx(o, 1023. / 500., -95. / 500.),
        "ADX16_to_ACES2065-1" => adx(o, 65535. / 8000., -1520. / 8000.),
        "ACES-LMT - BLUE_LIGHT_ARTIFACT_FIX" => o.push(Op::matrix([
            [0.9404372683, -0.0183068787, 0.0778696104],
            [0.0083786969, 0.8286599939, 0.1629613092],
            [0.0005471261, -0.0008833746, 1.0003362486],
        ])),
        "ACES-LMT - ACES 1.3 Reference Gamut Compression" => {
            let m = conversion(&AP0, &AP1, Adapt::None);
            o.push(Op::matrix(m));
            o.push(Op::GamutComp13 {
                lim: [1.147, 1.264, 1.312],
                thr: [0.815, 0.803, 0.880],
                power: 1.2,
                inverse: false,
            });
            o.push(Op::matrix(invert(&m)));
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-CINEMA_1.0" => {
            rrt_preamble(o);
            sdr_tonecurve(o);
            o.push(ap1_to_xyz());
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0" => {
            rrt_preamble(o);
            sdr_tonecurve(o);
            video_adjust(o);
            o.push(ap1_to_xyz());
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-CINEMA-REC709lim_1.1" => {
            rrt_preamble(o);
            sdr_tonecurve(o);
            sdr_primary_clamp(o, &REC709);
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO-REC709lim_1.1" => {
            rrt_preamble(o);
            sdr_tonecurve(o);
            video_adjust(o);
            sdr_primary_clamp(o, &REC709);
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO-P3lim_1.1" => {
            rrt_preamble(o);
            sdr_tonecurve(o);
            video_adjust(o);
            sdr_primary_clamp(o, &P3_D65);
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-CINEMA-D60sim-D65_1.1" => {
            rrt_preamble(o);
            sdr_tonecurve(o);
            o.push(Op::clamp(None, Some(1.)));
            o.push(Op::scale(0.964));
            o.push(Op::matrix(rgb2xyz(&AP1)));
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO-D60sim-D65_1.0" => {
            rrt_preamble(o);
            sdr_tonecurve(o);
            o.push(Op::clamp(None, Some(1.)));
            o.push(Op::scale(0.955));
            video_adjust(o);
            o.push(Op::matrix(rgb2xyz(&AP1)));
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-CINEMA-D60sim-DCI_1.0" => {
            rrt_preamble(o);
            sdr_tonecurve(o);
            o.push(curve(Curve::RollWhite { white: 0.918 }));
            o.push(Op::clamp(None, Some(0.918)));
            o.push(Op::scale(0.96));
            o.push(Op::matrix(rgb2xyz(&AP1)));
            o.push(Op::matrix(vonkries(
                WHITE_DCI_XYZ,
                WHITE_D65_XYZ,
                Adapt::Bradford,
            )));
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-CINEMA-D65sim-DCI_1.1" => {
            rrt_preamble(o);
            sdr_tonecurve(o);
            o.push(curve(Curve::RollWhite { white: 0.908 }));
            o.push(Op::clamp(None, Some(0.908)));
            o.push(Op::scale(0.9575));
            o.push(ap1_to_xyz());
            o.push(Op::matrix(vonkries(
                WHITE_DCI_XYZ,
                WHITE_D65_XYZ,
                Adapt::Bradford,
            )));
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-1000nit-15nit-REC2020lim_1.1"
        | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-1000nit-15nit-P3lim_1.1"
        | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-2000nit-15nit-REC2020lim_1.1"
        | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-2000nit-15nit-P3lim_1.1"
        | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-4000nit-15nit-REC2020lim_1.1"
        | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-4000nit-15nit-P3lim_1.1"
        | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-CINEMA-108nit-7.2nit-P3lim_1.1" => {
            let nits = if style.contains("108nit") {
                108.
            } else if style.contains("1000nit") {
                1000.
            } else if style.contains("2000nit") {
                2000.
            } else {
                4000.
            };
            let lim = if style.contains("REC2020lim") {
                REC2020
            } else {
                P3_D65
            };
            rrt_preamble(o);
            hdr_tonecurve(o, nits);
            hdr_primary_clamp(o, &lim);
            o.push(Op::scale(nits * 0.01));
        }
        "DISPLAY - CIE-XYZ-D65_to_REC.1886-REC.709"
        | "DISPLAY - CIE-XYZ-D65_to_REC.1886-REC.709 - MIRROR NEGS" => {
            o.push(Op::matrix(from_xyz_d65(&REC709, Adapt::None)));
            o.push(gamma(
                if style.ends_with("NEGS") {
                    GammaKind::Mirror
                } else {
                    GammaKind::Basic
                },
                2.4,
                0.,
            ));
        }
        "DISPLAY - CIE-XYZ-D65_to_REC.1886-REC.2020"
        | "DISPLAY - CIE-XYZ-D65_to_REC.1886-REC.2020 - MIRROR NEGS" => {
            o.push(Op::matrix(from_xyz_d65(&REC2020, Adapt::None)));
            o.push(gamma(
                if style.ends_with("NEGS") {
                    GammaKind::Mirror
                } else {
                    GammaKind::Basic
                },
                2.4,
                0.,
            ));
        }
        "DISPLAY - CIE-XYZ-D65_to_G2.2-REC.709"
        | "DISPLAY - CIE-XYZ-D65_to_G2.2-REC.709 - MIRROR NEGS" => {
            o.push(Op::matrix(from_xyz_d65(&REC709, Adapt::None)));
            o.push(gamma(
                if style.ends_with("NEGS") {
                    GammaKind::Mirror
                } else {
                    GammaKind::Basic
                },
                2.2,
                0.,
            ));
        }
        "DISPLAY - CIE-XYZ-D65_to_sRGB" | "DISPLAY - CIE-XYZ-D65_to_sRGB - MIRROR NEGS" => {
            o.push(Op::matrix(from_xyz_d65(&REC709, Adapt::None)));
            let kind = if style.ends_with("NEGS") {
                GammaKind::MoncurveMirror
            } else {
                GammaKind::Moncurve
            };
            o.push(gamma(kind, 2.4, 0.055));
        }
        "DISPLAY - CIE-XYZ-D65_to_G2.6-P3-DCI-BFD" => {
            o.push(Op::matrix(from_xyz_d65(&P3_DCI, Adapt::Bradford)));
            o.push(gamma(GammaKind::Basic, 2.6, 0.));
        }
        "DISPLAY - CIE-XYZ-D65_to_G2.6-P3-D65"
        | "DISPLAY - CIE-XYZ-D65_to_G2.6-P3-D65 - MIRROR NEGS" => {
            o.push(Op::matrix(from_xyz_d65(&P3_D65, Adapt::None)));
            o.push(gamma(
                if style.ends_with("NEGS") {
                    GammaKind::Mirror
                } else {
                    GammaKind::Basic
                },
                2.6,
                0.,
            ));
        }
        "DISPLAY - CIE-XYZ-D65_to_G2.6-P3-D60-BFD" => {
            o.push(Op::matrix(from_xyz_d65(&P3_D60, Adapt::Bradford)));
            o.push(gamma(GammaKind::Basic, 2.6, 0.));
        }
        "DISPLAY - CIE-XYZ-D65_to_DCDM-D65" => {
            o.push(Op::scale(48.0 / 52.37));
            o.push(gamma(GammaKind::Basic, 2.6, 0.));
        }
        "DISPLAY - CIE-XYZ-D65_to_DisplayP3" | "DISPLAY - CIE-XYZ-D65_to_DisplayP3-HDR" => {
            o.push(Op::matrix(from_xyz_d65(&P3_D65, Adapt::None)));
            o.push(gamma(GammaKind::MoncurveMirror, 2.4, 0.055));
        }
        "CURVE - ST-2084_to_LINEAR" => o.push(Op::Curve {
            curve: Curve::LinToPq,
            inverse: true,
        }),
        "CURVE - LINEAR_to_ST-2084" | "DISPLAY - CIE-XYZ-D65_to_ST2084-DCDM-D65" => {
            o.push(curve(Curve::LinToPq))
        }
        "DISPLAY - CIE-XYZ-D65_to_REC.2100-PQ" => {
            o.push(Op::matrix(from_xyz_d65(&REC2020, Adapt::None)));
            o.push(curve(Curve::LinToPq));
        }
        "DISPLAY - CIE-XYZ-D65_to_ST2084-P3-D65" => {
            o.push(Op::matrix(from_xyz_d65(&P3_D65, Adapt::None)));
            o.push(curve(Curve::LinToPq));
        }
        "CURVE - HLG-OETF-INVERSE" => o.push(Op::Curve {
            curve: Curve::LinToHlg,
            inverse: true,
        }),
        "CURVE - HLG-OETF" => o.push(curve(Curve::LinToHlg)),
        "DISPLAY - CIE-XYZ-D65_to_REC.2100-HLG-1000nit" => hlg_1000(o),
        "APPLE_LOG_to_ACES2065-1"
        | "APPLE_LOG-APPLEWG_to_ACES2065-1"
        | "CURVE - APPLE_LOG_to_LINEAR" => {
            o.push(curve(Curve::AppleLogToLin));
            if style.starts_with("APPLE_LOG-APPLEWG") {
                o.push(Op::matrix(conversion(&APPLE_WG, &AP0, Adapt::Bradford)));
            } else if style.starts_with("APPLE") {
                o.push(Op::matrix(conversion(&REC2020, &AP0, Adapt::Bradford)));
            }
        }
        "CANON_CLOG2-CGAMUT_to_ACES2065-1" | "CURVE - CANON_CLOG2_to_LINEAR" => {
            o.push(curve(Curve::CLog2ToLin));
            if style.starts_with("CANON") {
                o.push(Op::matrix(conversion(&CANON_CGAMUT, &AP0, Adapt::Cat02)));
            }
        }
        "CANON_CLOG3-CGAMUT_to_ACES2065-1" | "CURVE - CANON_CLOG3_to_LINEAR" => {
            o.push(curve(Curve::CLog3ToLin));
            if style.starts_with("CANON") {
                o.push(Op::matrix(conversion(&CANON_CGAMUT, &AP0, Adapt::Cat02)));
            }
        }
        "IDENTITY" => {}
        // ponytail: ARRI/Sony/RED/Panasonic camera builtins are not ported; the ACES configs
        // express those cameras with LogCameraTransforms, which are supported.
        _ => bail!("unsupported BuiltinTransform style '{style}'"),
    }
    Ok(ops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_invert() {
        let curves = [
            (Curve::LinToPq, vec![0.001, 0.18, 1.0, 50.0]),
            (Curve::LinToHlg, vec![0.001, 0.05, 0.18, 1.0, 5.0]),
            (Curve::AppleLogToLin, vec![0.0, 0.05, 0.3, 0.7]),
            (Curve::AcesccToLin, vec![-0.3, 0.0, 0.4, 1.2]),
            (Curve::CLog2ToLin, vec![0.05, 0.1, 0.5, 0.9]),
            (Curve::CLog3ToLin, vec![0.05, 0.1, 0.13, 0.5, 0.9]),
            (Curve::AdxCidToRle, vec![-0.3, 0.1, 0.5, 0.8]),
            (Curve::RollWhite { white: 0.918 }, vec![0.1, 0.6, 0.9]),
        ];
        for (c, xs) in curves {
            for x in xs {
                let back = c.inv(c.fwd(x));
                assert!((back - x).abs() < 1e-7, "{c:?} {x} -> {back}");
            }
        }
    }

    #[test]
    fn bspline_roundtrip() {
        let s = BSpline::new(
            &[
                [-5.26017743, -4.],
                [-3.75502745, -3.57868829],
                [-2.24987747, -1.82131329],
                [-0.74472749, 0.68124124],
            ],
            &[0., 0.55982688, 1.77532247, 1.55],
        );
        // The start slope is 0 (flat below the first knot), so stay above it.
        for x in [-5.0f32, -4.0, -2.0, -1.0, 0.5] {
            assert!((s.eval_rev(s.eval(x)) - x).abs() < 1e-4);
        }
    }
}
