// Low-level CPU ops. The math of the individual renderers follows OpenColorIO's CPU ops
// (Copyright Contributors to the OpenColorIO Project, BSD-3-Clause) so results agree with it:
// everything runs on f32 RGB triples, parameters are converted the way OCIO converts them.

use std::sync::Arc;

use crate::aces2::Aces2;
use crate::builtins::{BSpline, Curve};

pub(crate) type M3 = [[f64; 3]; 3];

pub(crate) fn mul(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    r
}

pub(crate) fn mul_v(m: &M3, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

pub(crate) fn invert(m: &M3) -> M3 {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let co = [e * i - f * h, f * g - d * i, d * h - e * g];
    let det = a * co[0] + b * co[1] + c * co[2];
    let k = 1.0 / det;
    [
        [co[0] * k, (c * h - b * i) * k, (b * f - c * e) * k],
        [co[1] * k, (a * i - c * g) * k, (c * d - a * f) * k],
        [co[2] * k, (b * g - a * h) * k, (a * e - b * d) * k],
    ]
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum GammaKind {
    /// Negative values clamp to zero.
    Basic,
    Mirror,
    PassThru,
    /// Power with a linear toe (sRGB-like), negatives follow the linear segment.
    Moncurve,
    MoncurveMirror,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum EcStyle {
    Linear,
    Video,
    Log,
}

/// Per channel parameters of a log op (OCIO `LogOpData`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct LogParams {
    pub log_slope: f64,
    pub log_offset: f64,
    pub lin_slope: f64,
    pub lin_offset: f64,
    /// Camera style: linear segment below this linear value.
    pub lin_break: Option<f64>,
    pub linear_slope: Option<f64>,
}

impl LogParams {
    pub const PLAIN: LogParams = LogParams {
        log_slope: 1.,
        log_offset: 0.,
        lin_slope: 1.,
        lin_offset: 0.,
        lin_break: None,
        linear_slope: None,
    };
}

pub(crate) struct Lut1D {
    /// Values per channel, sampled uniformly over [0, 1].
    pub ch: [Vec<f32>; 3],
    inv: [InvChannel; 3],
}

struct InvChannel {
    /// Monotonic, always increasing (negated for decreasing LUTs).
    tbl: Vec<f32>,
    start: usize,
    end: usize,
    flip: f32,
}

impl Lut1D {
    pub fn new(ch: [Vec<f32>; 3]) -> Lut1D {
        let inv = [0, 1, 2].map(|c| InvChannel::new(&ch[c]));
        Lut1D { ch, inv }
    }
}

impl InvChannel {
    // Mirrors Lut1DOpData::initializeFromForward: force monotonicity and find the flat ends.
    fn new(v: &[f32]) -> InvChannel {
        let n = v.len();
        let increasing = v[0] < v[n - 1];
        let flip = if increasing { 1.0 } else { -1.0 };
        let mut tbl: Vec<f32> = v.iter().map(|x| x * flip).collect();
        for i in 1..n {
            if tbl[i] < tbl[i - 1] {
                tbl[i] = tbl[i - 1];
            }
        }
        let mut end = n - 1;
        while end > 0 && tbl[end - 1] == tbl[n - 1] {
            end -= 1;
        }
        let mut start = 0;
        while start < end && tbl[start + 1] == tbl[0] {
            start += 1;
        }
        InvChannel {
            tbl,
            start,
            end,
            flip,
        }
    }

    // Port of OCIO's FindLutInv.
    fn find(&self, val: f32) -> f32 {
        let lut = &self.tbl[self.start..=self.end];
        let cv = (val * self.flip).max(lut[0]).min(lut[lut.len() - 1]);
        let lo = lut.partition_point(|&x| x < cv).saturating_sub(1);
        let hi = (lo + 1).min(lut.len() - 1);
        let delta = if lut[hi] > lut[lo] {
            (cv - lut[lo]) / (lut[hi] - lut[lo])
        } else {
            0.0
        };
        (lo as f32 + self.start as f32 + delta) / (self.tbl.len() - 1) as f32
    }
}

/// CSP prelut: OCIO's (rsr) cubic Hermite interpolator through non-uniform points.
pub(crate) struct CspCurve {
    stims: Vec<f32>,
    params: Vec<[f32; 5]>,
    min_value: f32,
    max_value: f32,
}

impl CspCurve {
    pub fn new(x: &[f32], v: &[f32]) -> CspCurve {
        let n = x.len();
        let params = (0..n - 1)
            .map(|i| {
                let (f0, f1) = (v[i], v[i + 1]);
                let inv_delta = 1.0 / (x[i + 1] - x[i]);
                if n == 2 {
                    return [inv_delta, f0, f1 - f0, 0.0, 0.0];
                }
                let delta = x[i + 1] - x[i];
                if i == 0 {
                    let dfdx1 = (v[i + 2] - f0) / (1.0 + (x[i + 2] - x[i + 1]) / delta);
                    [
                        inv_delta,
                        f0,
                        -2.0 * f0 + 2.0 * f1 - dfdx1,
                        f0 - f1 + dfdx1,
                        0.0,
                    ]
                } else if i == n - 2 {
                    let dfdx0 = (f1 - v[i - 1]) / (1.0 + (x[i] - x[i - 1]) / delta);
                    [inv_delta, f0, dfdx0, -f0 + f1 - dfdx0, 0.0]
                } else {
                    let dfdx0 = (f1 - v[i - 1]) / (1.0 + (x[i] - x[i - 1]) / delta);
                    let dfdx1 = (v[i + 2] - f0) / (1.0 + (x[i + 2] - x[i + 1]) / delta);
                    [
                        inv_delta,
                        f0,
                        dfdx0,
                        -3.0 * f0 - 2.0 * dfdx0 + 3.0 * f1 - dfdx1,
                        2.0 * f0 + dfdx0 - 2.0 * f1 + dfdx1,
                    ]
                }
            })
            .collect();
        CspCurve {
            stims: x.to_vec(),
            params,
            min_value: v[0],
            max_value: v[n - 1],
        }
    }

    fn eval(&self, x: f32) -> f32 {
        let s = &self.stims;
        if x < s[0] {
            return self.min_value;
        }
        if x > s[s.len() - 1] {
            return self.max_value;
        }
        let i = (s.partition_point(|&v| v <= x).max(1) - 1).min(s.len() - 2);
        let [inv_delta, a, b, c, d] = self.params[i];
        let z = (x - s[i]) * inv_delta;
        a + z * (b + z * (c + d * z))
    }

    // ponytail: bisection inverse, assumes a monotonic prelut.
    fn inv(&self, y: f32) -> f32 {
        let (mut lo, mut hi) = (self.stims[0], self.stims[self.stims.len() - 1]);
        let rising = self.max_value >= self.min_value;
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if (self.eval(mid) < y) == rising {
                lo = mid
            } else {
                hi = mid
            }
        }
        0.5 * (lo + hi)
    }
}

pub(crate) struct Lut3D {
    pub n: usize,
    /// Red changes fastest.
    pub data: Vec<[f32; 3]>,
}

impl Lut3D {
    fn at(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.data[r + self.n * (g + self.n * b)]
    }

    pub fn trilinear(&self, rgb: [f32; 3]) -> [f32; 3] {
        trilinear(self.n, &self.data, rgb)
    }

    pub fn tetrahedral(&self, rgb: [f32; 3]) -> [f32; 3] {
        let (i, f) = cell(self.n, rgb);
        let p = |r, g, b| self.at(i[0] + r, i[1] + g, i[2] + b);
        let (fr, fg, fb) = (f[0], f[1], f[2]);
        let c000 = p(0, 0, 0);
        let c111 = p(1, 1, 1);
        let mut out = [0.0; 3];
        // Pick the tetrahedron containing the point, then blend its four corners.
        let (a, b, w) = if fr > fg {
            if fg > fb {
                (p(1, 0, 0), p(1, 1, 0), [fr, fg, fb])
            } else if fr > fb {
                (p(1, 0, 0), p(1, 0, 1), [fr, fb, fg])
            } else {
                (p(0, 0, 1), p(1, 0, 1), [fb, fr, fg])
            }
        } else if fb > fg {
            (p(0, 0, 1), p(0, 1, 1), [fb, fg, fr])
        } else if fb > fr {
            (p(0, 1, 0), p(0, 1, 1), [fg, fb, fr])
        } else {
            (p(0, 1, 0), p(1, 1, 0), [fg, fr, fb])
        };
        for c in 0..3 {
            out[c] = (1.0 - w[0]) * c000[c]
                + (w[0] - w[1]) * a[c]
                + (w[1] - w[2]) * b[c]
                + w[2] * c111[c];
        }
        out
    }
}

fn cell(n: usize, rgb: [f32; 3]) -> ([usize; 3], [f32; 3]) {
    let max = (n - 1) as f32;
    let mut i = [0; 3];
    let mut f = [0.0; 3];
    for c in 0..3 {
        // NaN maps to 0 like OCIO.
        let x = (rgb[c] * max).max(0.0).min(max);
        let lo = (x.floor() as usize).min(n - 2);
        i[c] = lo;
        f[c] = x - lo as f32;
    }
    (i, f)
}

/// Trilinear lookup in a red-fastest `n`³ table over [0, 1]³.
pub(crate) fn trilinear(n: usize, data: &[[f32; 3]], rgb: [f32; 3]) -> [f32; 3] {
    let (i, f) = cell(n, rgb);
    let at = |r: usize, g: usize, b: usize| data[(i[0] + r) + n * ((i[1] + g) + n * (i[2] + b))];
    [0, 1, 2].map(|c| {
        let v = |r, g, b| at(r, g, b)[c];
        let x00 = v(0, 0, 0) + (v(1, 0, 0) - v(0, 0, 0)) * f[0];
        let x10 = v(0, 1, 0) + (v(1, 1, 0) - v(0, 1, 0)) * f[0];
        let x01 = v(0, 0, 1) + (v(1, 0, 1) - v(0, 0, 1)) * f[0];
        let x11 = v(0, 1, 1) + (v(1, 1, 1) - v(0, 1, 1)) * f[0];
        let y0 = x00 + (x10 - x00) * f[1];
        let y1 = x01 + (x11 - x01) * f[1];
        y0 + (y1 - y0) * f[2]
    })
}

#[derive(Clone)]
pub(crate) enum Op {
    Matrix {
        m: M3,
        off: [f64; 3],
    },
    Range {
        min_in: Option<f64>,
        max_in: Option<f64>,
        min_out: Option<f64>,
        max_out: Option<f64>,
    },
    Gamma {
        kind: GammaKind,
        inverse: bool,
        gamma: [f64; 3],
        offset: [f64; 3],
    },
    Log {
        base: f64,
        p: [LogParams; 3],
        inverse: bool,
    },
    Cdl {
        slope: [f64; 3],
        offset: [f64; 3],
        power: [f64; 3],
        sat: f64,
        clamp: bool,
        inverse: bool,
    },
    ExposureContrast {
        style: EcStyle,
        exposure: f64,
        contrast: f64,
        gamma: f64,
        pivot: f64,
        log_step: f64,
        log_mid: f64,
        inverse: bool,
    },
    Lut1D {
        lut: Arc<Lut1D>,
        inverse: bool,
    },
    Lut3D {
        lut: Arc<Lut3D>,
        tetra: bool,
        inverse: bool,
    },
    Prelut {
        ch: Arc<[CspCurve; 3]>,
        inverse: bool,
    },
    Curve {
        curve: Curve,
        inverse: bool,
    },
    BSpline {
        curve: Arc<BSpline>,
        inverse: bool,
    },
    RedMod {
        v03: bool,
        inverse: bool,
    },
    Glow {
        gain: f32,
        mid: f32,
        inverse: bool,
    },
    DarkToDim {
        gamma: f32,
    },
    GamutComp13 {
        lim: [f32; 3],
        thr: [f32; 3],
        power: f32,
        inverse: bool,
    },
    Aces2 {
        t: Arc<Aces2>,
        inverse: bool,
    },
    Rec2100Surround {
        gamma: f32,
        inverse: bool,
    },
    RgbToHsv {
        inverse: bool,
    },
}

impl Op {
    pub fn scale_offset(scale: [f64; 3], off: [f64; 3]) -> Op {
        Op::Matrix {
            m: [[scale[0], 0., 0.], [0., scale[1], 0.], [0., 0., scale[2]]],
            off,
        }
    }

    pub fn scale(s: f64) -> Op {
        Op::scale_offset([s; 3], [0.; 3])
    }

    pub fn matrix(m: M3) -> Op {
        Op::Matrix { m, off: [0.; 3] }
    }

    /// Clamp to [lo, hi]; `None` leaves that side open.
    pub fn clamp(lo: Option<f64>, hi: Option<f64>) -> Op {
        Op::Range {
            min_in: lo,
            max_in: hi,
            min_out: lo,
            max_out: hi,
        }
    }

    pub fn inverse(&self) -> Op {
        let mut op = self.clone();
        match &mut op {
            Op::Matrix { m, off } => {
                let mi = invert(m);
                let o = mul_v(&mi, *off);
                *m = mi;
                *off = [-o[0], -o[1], -o[2]];
            }
            Op::Range {
                min_in,
                max_in,
                min_out,
                max_out,
            } => {
                std::mem::swap(min_in, min_out);
                std::mem::swap(max_in, max_out);
            }
            Op::DarkToDim { gamma } => {
                // OCIO uses the exact CTL constant for the inverse rather than 1 / 0.9811.
                *gamma = if *gamma == 0.9811 {
                    1.019_264_1
                } else {
                    0.9811
                };
            }
            Op::Gamma { inverse, .. }
            | Op::Log { inverse, .. }
            | Op::Cdl { inverse, .. }
            | Op::ExposureContrast { inverse, .. }
            | Op::Lut1D { inverse, .. }
            | Op::Lut3D { inverse, .. }
            | Op::Prelut { inverse, .. }
            | Op::Curve { inverse, .. }
            | Op::BSpline { inverse, .. }
            | Op::RedMod { inverse, .. }
            | Op::Glow { inverse, .. }
            | Op::GamutComp13 { inverse, .. }
            | Op::Aces2 { inverse, .. }
            | Op::Rec2100Surround { inverse, .. }
            | Op::RgbToHsv { inverse } => *inverse = !*inverse,
        }
        op
    }

    pub fn apply(&self, p: &mut [f32; 3]) {
        match self {
            Op::Matrix { m, off } => {
                // f32 in OCIO's summation order: tiny values near zero then keep the same sign
                // as OCIO, which matters before a power function.
                let [r, g, b] = *p;
                let row = |i: usize| {
                    r * m[i][0] as f32 + g * m[i][1] as f32 + b * m[i][2] as f32 + off[i] as f32
                };
                *p = [row(0), row(1), row(2)];
            }
            Op::Range {
                min_in,
                max_in,
                min_out,
                max_out,
            } => {
                let (scale, offset) = range_scale_offset(*min_in, *max_in, *min_out, *max_out);
                let (scale, offset) = (scale as f32, offset as f32);
                let lo = min_out.map_or(f32::NEG_INFINITY, |v| v as f32);
                let hi = max_out.map_or(f32::INFINITY, |v| v as f32);
                for v in p.iter_mut() {
                    *v = (*v * scale + offset).max(lo).min(hi);
                }
            }
            Op::Gamma {
                kind,
                inverse,
                gamma,
                offset,
            } => {
                for c in 0..3 {
                    p[c] = apply_gamma(*kind, *inverse, gamma[c], offset[c], p[c]);
                }
            }
            Op::Log {
                base,
                p: lp,
                inverse,
            } => {
                for c in 0..3 {
                    p[c] = apply_log(*base, &lp[c], *inverse, p[c]);
                }
            }
            Op::Cdl {
                slope,
                offset,
                power,
                sat,
                clamp,
                inverse,
            } => apply_cdl(p, slope, offset, power, *sat, *clamp, *inverse),
            Op::ExposureContrast {
                style,
                exposure,
                contrast,
                gamma,
                pivot,
                log_step,
                log_mid,
                inverse,
            } => apply_ec(
                p, *style, *exposure, *contrast, *gamma, *pivot, *log_step, *log_mid, *inverse,
            ),
            Op::Lut1D { lut, inverse } => {
                for (c, v) in p.iter_mut().enumerate() {
                    *v = if *inverse {
                        lut.inv[c].find(*v)
                    } else {
                        lut1d(&lut.ch[c], *v)
                    };
                }
            }
            Op::Lut3D {
                lut,
                tetra,
                inverse,
            } => {
                *p = if *inverse {
                    lut3d_inverse(lut, *tetra, *p)
                } else if *tetra {
                    lut.tetrahedral(*p)
                } else {
                    lut.trilinear(*p)
                };
            }
            Op::Prelut { ch, inverse } => {
                for (v, curve) in p.iter_mut().zip(ch.iter()) {
                    *v = if *inverse {
                        curve.inv(*v)
                    } else {
                        curve.eval(*v)
                    };
                }
            }
            Op::Curve { curve, inverse } => {
                for v in p.iter_mut() {
                    let x = *v as f64;
                    *v = if *inverse { curve.inv(x) } else { curve.fwd(x) } as f32;
                }
            }
            Op::BSpline { curve, inverse } => {
                for v in p.iter_mut() {
                    *v = if *inverse {
                        curve.eval_rev(*v)
                    } else {
                        curve.eval(*v)
                    };
                }
            }
            Op::RedMod { v03, inverse } => red_mod(p, *v03, *inverse),
            Op::Glow { gain, mid, inverse } => glow(p, *gain, *mid, *inverse),
            Op::DarkToDim { gamma } => {
                let y =
                    (0.272_228_72 * p[0] + 0.674_081_77 * p[1] + 0.053_689_517 * p[2]).max(1e-10);
                let k = y.powf(gamma - 1.0);
                p.iter_mut().for_each(|v| *v *= k);
            }
            Op::GamutComp13 {
                lim,
                thr,
                power,
                inverse,
            } => gamut_comp13(p, lim, thr, *power, *inverse),
            Op::Aces2 { t, inverse } => *p = if *inverse { t.inv(*p) } else { t.fwd(*p) },
            Op::Rec2100Surround { gamma, inverse } => {
                let min_lum = if *inverse { 1e-4f32.powf(*gamma) } else { 1e-4 };
                let g = if *inverse { 1.0 / gamma } else { *gamma };
                let y = (0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2])
                    .abs()
                    .max(min_lum);
                let k = y.powf(g - 1.0);
                p.iter_mut().for_each(|v| *v *= k);
            }
            Op::RgbToHsv { inverse } => {
                *p = if *inverse {
                    hsv_to_rgb(*p)
                } else {
                    rgb_to_hsv(*p)
                }
            }
        }
    }
}

/// RangeOpData's affine part: both bounds give a scale, one bound only an offset.
pub(crate) fn range_scale_offset(
    min_in: Option<f64>,
    max_in: Option<f64>,
    min_out: Option<f64>,
    max_out: Option<f64>,
) -> (f64, f64) {
    match (min_in, max_in, min_out, max_out) {
        (Some(a), Some(b), Some(c), Some(d)) => {
            let s = (d - c) / (b - a);
            (s, c - s * a)
        }
        (Some(a), _, Some(c), _) => (1.0, c - a),
        (_, Some(b), _, Some(d)) => (1.0, d - b),
        _ => (1.0, 0.0),
    }
}

fn apply_gamma(kind: GammaKind, inverse: bool, g: f64, off: f64, x: f32) -> f32 {
    match kind {
        GammaKind::Basic | GammaKind::Mirror | GammaKind::PassThru => {
            let e = (if inverse { 1.0 / g } else { g }) as f32;
            match kind {
                GammaKind::Basic => x.max(0.0).powf(e),
                GammaKind::Mirror => x.abs().powf(e).copysign(x),
                _ if x > 0.0 => x.powf(e),
                _ => x,
            }
        }
        GammaKind::Moncurve | GammaKind::MoncurveMirror => {
            // GammaOpUtils: break point and slope of the linear toe are implied by gamma/offset.
            const EPS: f64 = 1e-6;
            let gm = g.max(1.0 + EPS);
            let o = off.max(EPS);
            let mirror = kind == GammaKind::MoncurveMirror;
            let ax = if mirror { x.abs() } else { x };
            let y = if !inverse {
                let brk = (o / (gm - 1.0)) as f32;
                let slope = ((gm - 1.0) / o * (o * gm / ((gm - 1.0) * (1.0 + o))).powf(gm)) as f32;
                if ax <= brk {
                    ax * slope
                } else {
                    (ax * (1.0 / (1.0 + o)) as f32 + (o / (1.0 + o)) as f32).powf(gm as f32)
                }
            } else {
                let brk = ((o * gm) / ((gm - 1.0) * (1.0 + o))).powf(gm) as f32;
                let slope = (((gm - 1.0) / o).powf(gm - 1.0) * ((1.0 + o) / gm).powf(gm)) as f32;
                if ax <= brk {
                    ax * slope
                } else {
                    ax.powf((1.0 / gm) as f32) * (1.0 + o) as f32 - o as f32
                }
            };
            if mirror { y.copysign(x) } else { y }
        }
    }
}

fn apply_log(base: f64, p: &LogParams, inverse: bool, x: f32) -> f32 {
    let log2_base = (base as f32).log2();
    let klog = (p.log_slope / (base as f32).log2() as f64) as f32;
    let (m, b, kb) = (p.lin_slope as f32, p.lin_offset as f32, p.log_offset as f32);
    let lin_to_log = |x: f32| (x * m + b).max(f32::MIN_POSITIVE).log2() * klog + kb;
    let log_to_lin =
        |y: f32| (((y - kb) * (log2_base / p.log_slope as f32)).exp2() - b) * (1.0 / m);
    let Some(lin_break) = p.lin_break else {
        return if inverse {
            log_to_lin(x)
        } else {
            lin_to_log(x)
        };
    };
    // LogUtils: linear segment that meets the log curve at lin_break.
    let linear_slope = p.linear_slope.map_or_else(
        || {
            (p.log_slope * p.lin_slope / ((p.lin_slope * lin_break + p.lin_offset) * base.ln()))
                as f32
        },
        |s| s as f32,
    );
    let log_break = ((p.lin_slope * lin_break + p.lin_offset) as f32).log2()
        * (p.log_slope as f32 / log2_base)
        + p.log_offset as f32;
    let linear_offset = log_break - linear_slope * lin_break as f32;
    if inverse {
        if x < log_break {
            (x - linear_offset) * (1.0 / linear_slope)
        } else {
            log_to_lin(x)
        }
    } else if x < lin_break as f32 {
        linear_slope * x + linear_offset
    } else {
        lin_to_log(x)
    }
}

fn apply_cdl(
    p: &mut [f32; 3],
    slope: &[f64; 3],
    offset: &[f64; 3],
    power: &[f64; 3],
    sat: f64,
    clamp: bool,
    inverse: bool,
) {
    let rcp = |x: f64| 1.0 / (x as f32).max(1e-6);
    let clamp01 = |p: &mut [f32; 3]| {
        if clamp {
            p.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
        }
    };
    let pow = |p: &mut [f32; 3], e: [f32; 3]| {
        clamp01(p);
        for c in 0..3 {
            p[c] = if clamp || p[c] >= 0.0 {
                p[c].powf(e[c])
            } else {
                p[c]
            };
            if p[c].is_nan() {
                p[c] = 0.0;
            }
        }
    };
    let saturate = |p: &mut [f32; 3], s: f32| {
        let luma = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
        p.iter_mut().for_each(|v| *v = luma + s * (*v - luma));
    };
    if !inverse {
        for c in 0..3 {
            p[c] = p[c] * slope[c] as f32 + offset[c] as f32;
        }
        pow(p, power.map(|v| v as f32));
        saturate(p, sat as f32);
        clamp01(p);
    } else {
        clamp01(p);
        saturate(p, rcp(sat));
        pow(p, power.map(rcp));
        for c in 0..3 {
            p[c] = (p[c] - offset[c] as f32) * rcp(slope[c]);
        }
        clamp01(p);
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_ec(
    p: &mut [f32; 3],
    style: EcStyle,
    exposure: f64,
    contrast: f64,
    gamma: f64,
    pivot: f64,
    log_step: f64,
    log_mid: f64,
    inverse: bool,
) {
    const VIDEO_OETF_POWER: f64 = 1.0 / 1.83;
    let contrast = (contrast * gamma).max(0.001) as f32;
    let pivot = pivot.max(0.001);
    match style {
        EcStyle::Linear | EcStyle::Video => {
            let (pivot, gain) = if style == EcStyle::Video {
                (
                    (pivot as f32).powf(VIDEO_OETF_POWER as f32),
                    2f32.powf(exposure as f32).powf(VIDEO_OETF_POWER as f32),
                )
            } else {
                (pivot as f32, 2f32.powf(exposure as f32))
            };
            for v in p.iter_mut() {
                *v = if contrast == 1.0 {
                    if inverse {
                        *v * (1.0 / gain)
                    } else {
                        *v * gain
                    }
                } else if inverse {
                    (*v * (1.0 / pivot)).max(0.0).powf(1.0 / contrast) * (pivot * (1.0 / gain))
                } else {
                    (*v * (gain / pivot)).max(0.0).powf(contrast) * pivot
                };
            }
        }
        EcStyle::Log => {
            let pivot = ((pivot / 0.18).log2() * log_step + log_mid).max(0.0) as f32;
            let e = (exposure * log_step) as f32;
            for v in p.iter_mut() {
                *v = if inverse {
                    (*v - pivot) / contrast + pivot - e
                } else {
                    (*v + e - pivot) * contrast + pivot
                };
            }
        }
    }
}

fn lut1d(v: &[f32], x: f32) -> f32 {
    let max = (v.len() - 1) as f32;
    let i = (x * max).max(0.0).min(max);
    let lo = i.floor() as usize;
    let hi = i.ceil() as usize;
    let d = hi as f32 - i;
    v[lo] * d + v[hi] * (1.0 - d)
}

// ponytail: 3D LUT inverse is a damped Newton solve on the forward LUT, not OCIO's exact
// inverse; it converges for smooth, invertible LUTs and returns the best guess otherwise.
fn lut3d_inverse(lut: &Lut3D, tetra: bool, target: [f32; 3]) -> [f32; 3] {
    let f = |x: [f32; 3]| {
        if tetra {
            lut.tetrahedral(x)
        } else {
            lut.trilinear(x)
        }
    };
    let mut x = target.map(|v| v.clamp(0.0, 1.0));
    let h = 0.5 / (lut.n - 1) as f32;
    for _ in 0..30 {
        let y = f(x);
        let err = [y[0] - target[0], y[1] - target[1], y[2] - target[2]];
        if err.iter().all(|e| e.abs() < 1e-6) {
            break;
        }
        let mut j = [[0f64; 3]; 3];
        for c in 0..3 {
            let mut xp = x;
            let mut xm = x;
            xp[c] = (x[c] + h).min(1.0);
            xm[c] = (x[c] - h).max(0.0);
            let (yp, ym) = (f(xp), f(xm));
            let dx = (xp[c] - xm[c]).max(1e-9);
            for r in 0..3 {
                j[r][c] = ((yp[r] - ym[r]) / dx) as f64;
            }
        }
        let det = j[0][0] * (j[1][1] * j[2][2] - j[1][2] * j[2][1])
            - j[0][1] * (j[1][0] * j[2][2] - j[1][2] * j[2][0])
            + j[0][2] * (j[1][0] * j[2][1] - j[1][1] * j[2][0]);
        if det.abs() < 1e-12 {
            break;
        }
        let step = mul_v(&invert(&j), err.map(|e| e as f64));
        for c in 0..3 {
            x[c] = (x[c] - step[c] as f32).clamp(0.0, 1.0);
        }
    }
    x
}

// FixedFunctionOpCPU: ACES 1.x red modifier (hue-weighted red desaturation).
fn red_mod(p: &mut [f32; 3], v03: bool, inverse: bool) {
    let (one_minus_scale, inv_width) = if v03 {
        (1.0 - 0.85, 1.909_859_3)
    } else {
        (1.0 - 0.82, 1.697_652_7)
    };
    let pivot = 0.03f32;
    let [mut red, mut grn, mut blu] = *p;
    let f_h = hue_weight(red, grn, blu, inv_width);
    if f_h > 0.0 {
        let new_red = if inverse {
            let min_chan = grn.min(blu);
            let a = f_h * one_minus_scale - 1.0;
            let b = red - f_h * (pivot + min_chan) * one_minus_scale;
            let c = f_h * pivot * min_chan * one_minus_scale;
            (-b - (b * b - 4.0 * a * c).sqrt()) / (2.0 * a)
        } else {
            let f_s = sat_weight(red, grn, blu, 1e-2);
            red + f_h * f_s * (pivot - red) * one_minus_scale
        };
        if v03 {
            // RedMod03 also restores the hue by moving the middle channel.
            if grn >= blu {
                let hue_fac = (grn - blu) / (red - blu).max(1e-10);
                grn = hue_fac * (new_red - blu) + blu;
            } else {
                let hue_fac = (blu - grn) / (red - grn).max(1e-10);
                blu = hue_fac * (new_red - grn) + grn;
            }
        }
        red = new_red;
    }
    *p = [red, grn, blu];
}

fn sat_weight(r: f32, g: f32, b: f32, noise_limit: f32) -> f32 {
    let mn = r.min(g.min(b));
    let mx = r.max(g.max(b));
    (mx.max(1e-10) - mn.max(1e-10)) / noise_limit.max(mx)
}

fn hue_weight(r: f32, g: f32, b: f32, inv_width: f32) -> f32 {
    let a = 2.0 * r - (g + b);
    let bb = 1.732_050_8 * (g - b);
    let hue = bb.atan2(a);
    let knot = hue * inv_width + 2.0;
    let j = knot as i32;
    const M: [[f32; 4]; 4] = [
        [0.25, 0.00, 0.00, 0.00],
        [-0.75, 0.75, 0.75, 0.25],
        [0.75, -1.50, 0.00, 1.00],
        [-0.25, 0.75, -0.75, 0.25],
    ];
    if (0..4).contains(&j) {
        let t = knot - j as f32;
        let c = M[j as usize];
        c[3] + t * (c[2] + t * (c[1] + t * c[0]))
    } else {
        0.0
    }
}

fn glow(p: &mut [f32; 3], gain: f32, mid: f32, inverse: bool) {
    let [r, g, b] = *p;
    let chroma = (b * (b - g) + g * (g - r) + r * (r - b)).sqrt();
    let yc = (b + g + r + 1.75 * chroma) / 3.0;
    let sat = sat_weight(r, g, b, 1e-2);
    let x = (sat - 0.4) * 5.0;
    let sign = 1f32.copysign(x);
    let t = (1.0 - 0.5 * sign * x).max(0.0);
    let s = (1.0 + sign * (1.0 - t * t)) * 0.5;
    let gain = gain * s;
    let k = if !inverse {
        if yc >= mid * 2.0 {
            0.0
        } else if yc <= mid * 2.0 / 3.0 {
            gain
        } else {
            gain * (mid / yc - 0.5)
        }
    } else if yc >= mid * 2.0 {
        0.0
    } else if yc <= (1.0 + gain) * mid * 2.0 / 3.0 {
        -gain / (1.0 + gain)
    } else {
        gain * (mid / yc - 0.5) / (gain * 0.5 - 1.0)
    };
    p.iter_mut().for_each(|v| *v *= 1.0 + k);
}

fn gamut_comp13(p: &mut [f32; 3], lim: &[f32; 3], thr: &[f32; 3], power: f32, inverse: bool) {
    let ach = p[0].max(p[1].max(p[2]));
    for c in 0..3 {
        let (l, t) = (lim[c], thr[c]);
        let scale = (l - t) / (((1.0 - t) / (l - t)).powf(-power) - 1.0).powf(1.0 / power);
        if ach == 0.0 {
            p[c] = 0.0;
            continue;
        }
        let dist = (ach - p[c]) / ach.abs();
        if dist < t {
            continue;
        }
        let nd = (dist - t) / scale;
        let pw = nd.powf(power);
        let cd = if !inverse {
            t + scale * nd / (1.0 + pw).powf(1.0 / power)
        } else if dist >= t + scale {
            dist
        } else {
            t + scale * (-(pw / (pw - 1.0))).powf(1.0 / power)
        };
        p[c] = ach - cd * ach.abs();
    }
}

fn rgb_to_hsv([red, grn, blu]: [f32; 3]) -> [f32; 3] {
    let mn = red.min(grn).min(blu);
    let mx = red.max(grn).max(blu);
    let mut val = mx;
    let mut sat = 0.0;
    let mut hue = 0.0;
    if mn != mx {
        let delta = mx - mn;
        if mx != 0.0 {
            sat = delta / mx;
        }
        hue = if red == mx {
            (grn - blu) / delta
        } else if grn == mx {
            2.0 + (blu - red) / delta
        } else {
            4.0 + (red - grn) / delta
        };
        if hue < 0.0 {
            hue += 6.0;
        }
        hue *= 1.0 / 6.0;
    }
    if mn < 0.0 {
        val += mn;
    }
    if -mn > mx {
        sat = (mx - mn) / -mn;
    }
    [hue, sat, val]
}

fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let hue = (h - h.floor()) * 6.0;
    let sat = s.clamp(0.0, 1.999);
    let r = ((hue - 3.0).abs() - 1.0).clamp(0.0, 1.0);
    let g = (2.0 - (hue - 2.0).abs()).clamp(0.0, 1.0);
    let b = (2.0 - (hue - 4.0).abs()).clamp(0.0, 1.0);
    let mut mx = v;
    let mut mn = v * (1.0 - sat);
    if sat > 1.0 {
        mn = v * (1.0 - sat) / (2.0 - sat);
        mx = v - mn;
    }
    if v < 0.0 {
        mn = v / (2.0 - sat);
        mx = v - mn;
    }
    let d = mx - mn;
    [r * d + mn, g * d + mn, b * d + mn]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut1d_inverse_roundtrip() {
        let v: Vec<f32> = (0..17).map(|i| (i as f32 / 16.0).powf(2.2)).collect();
        let lut = Arc::new(Lut1D::new([v.clone(), v.clone(), v]));
        let fwd = Op::Lut1D {
            lut,
            inverse: false,
        };
        let inv = fwd.inverse();
        let mut p = [0.1, 0.5, 0.93];
        fwd.apply(&mut p);
        inv.apply(&mut p);
        for (a, b) in p.iter().zip([0.1, 0.5, 0.93]) {
            assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        }
    }

    #[test]
    fn lut3d_inverse_roundtrip() {
        let n = 9;
        let mut data = Vec::new();
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let v = [r, g, b].map(|i| i as f32 / (n - 1) as f32);
                    data.push([v[0] * 0.8 + v[1] * 0.2, v[1].powf(1.5), v[2] * 0.9 + 0.05]);
                }
            }
        }
        let fwd = Op::Lut3D {
            lut: Arc::new(Lut3D { n, data }),
            tetra: true,
            inverse: false,
        };
        let mut p = [0.3, 0.6, 0.2];
        fwd.apply(&mut p);
        fwd.inverse().apply(&mut p);
        for (a, b) in p.iter().zip([0.3, 0.6, 0.2]) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
    }

    #[test]
    fn matrix_inverse() {
        let op = Op::Matrix {
            m: [[2., 1., 0.], [0., 1., 3.], [1., 0., 1.]],
            off: [0.1, 0.2, 0.3],
        };
        let mut p = [0.5, -0.25, 2.0];
        op.apply(&mut p);
        op.inverse().apply(&mut p);
        assert!(
            (p[0] - 0.5).abs() < 1e-5 && (p[1] + 0.25).abs() < 1e-5 && (p[2] - 2.0).abs() < 1e-5
        );
    }
}
