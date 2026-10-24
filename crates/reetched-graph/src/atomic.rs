//! Designer's atomic image nodes. Semantics follow Designer's documentation;
//! blur/warp/normal intensity scales are approximations (no reference renders
//! to match against yet, see docs/PLAN.md Phase 4 status).

use crate::{Image, V};
use rayon::prelude::*;
use std::{borrow::Cow, collections::HashMap, f32::consts::TAU, sync::Arc};

pub struct Cx<'a> {
    pub w: usize,
    pub h: usize,
    pub seed: i32,
    pub inputs: Vec<(String, Arc<Image>)>,
    pub params: HashMap<String, V>,
    pub arrays: HashMap<String, Vec<HashMap<String, V>>>,
    pub strs: HashMap<String, String>,
    pub resources: &'a HashMap<String, Arc<Image>>,
}

impl Cx<'_> {
    /// Input `slot`, resampled to the node's size.
    pub fn input(&self, slot: &str) -> Option<Cow<'_, Image>> {
        let img = self
            .inputs
            .iter()
            .find(|(s, _)| s == slot)
            .map(|(_, i)| i)?;
        Some(if (img.w, img.h) == (self.w, self.h) {
            Cow::Borrowed(img.as_ref())
        } else {
            Cow::Owned(img.resized(self.w, self.h))
        })
    }

    /// The first connected input, whatever its slot is called.
    pub fn main_input(&self) -> Option<Cow<'_, Image>> {
        let slot = self
            .inputs
            .iter()
            .find(|(s, _)| s == "input1" || s == "input")
            .or(self.inputs.first())?
            .0
            .clone();
        self.input(&slot)
    }

    pub fn f(&self, name: &str, default: f32) -> f32 {
        self.params.get(name).map_or(default, |v| v.float())
    }
    pub fn int(&self, name: &str, default: i32) -> i32 {
        self.params.get(name).map_or(default, |v| v.int())
    }
    pub fn bool(&self, name: &str, default: bool) -> bool {
        self.params.get(name).map_or(default, |v| v.bool())
    }
    /// Four components; a scalar is repeated.
    pub fn v4(&self, name: &str, default: [f32; 4]) -> [f32; 4] {
        match self.params.get(name) {
            Some(v) if v.ty.len() == 1 => [v.x[0]; 4],
            Some(v) => v.x,
            None => default,
        }
    }
    /// A color; RGB gets alpha 1.
    pub fn color(&self, name: &str, default: [f32; 4]) -> [f32; 4] {
        self.params.get(name).map_or(default, |v| v.rgba())
    }

    fn uv(&self, x: usize, y: usize) -> (f32, f32) {
        (
            (x as f32 + 0.5) / self.w as f32,
            (y as f32 + 0.5) / self.h as f32,
        )
    }

    /// Pixels per "256-pixel unit": Designer sizes many effects as if the image were 256 wide.
    fn scale(&self) -> f32 {
        self.w as f32 / 256.0
    }
}

pub fn run(name: &str, cx: &Cx) -> Result<Image, String> {
    if name.starts_with("reetched_") {
        return crate::own::run(name, cx);
    }
    let need = |slot: &str| {
        cx.input(slot)
            .ok_or_else(|| format!("{name}: input `{slot}` is not connected"))
    };
    let (w, h) = (cx.w, cx.h);
    Ok(match name {
        "uniform" => {
            let c = cx.color("outputcolor", [0.0, 0.0, 0.0, 1.0]);
            Image::uniform(w, h, c, cx.bool("colorswitch", true))
        }
        "passthrough" => cx
            .main_input()
            .ok_or("passthrough: nothing connected")?
            .into_owned(),
        "blend" => blend(cx),
        "levels" => {
            let img = cx.main_input().ok_or("levels: nothing connected")?;
            let lo = cx.v4("levelinlow", [0.0; 4]);
            let hi = cx.v4("levelinhigh", [1.0; 4]);
            let mid = cx.v4("levelinmid", [0.5; 4]);
            let olo = cx.v4("leveloutlow", [0.0; 4]);
            let ohi = cx.v4("levelouthigh", [1.0; 4]);
            let clamp = cx.bool("clamp", true);
            let level = |v: f32, i: usize| {
                let mut t = (v - lo[i]) / (hi[i] - lo[i]).max(1e-6);
                if clamp {
                    t = t.clamp(0.0, 1.0);
                }
                let m = mid[i].clamp(0.001, 0.999);
                let t = t.max(0.0).powf(0.5f32.ln() / m.ln());
                olo[i] + (ohi[i] - olo[i]) * t
            };
            map(&img, |c| std::array::from_fn(|i| level(c[i], i)))
        }
        "transformation" => {
            let img = cx.main_input().ok_or("transformation: nothing connected")?;
            let m = cx.v4("matrix22", [1.0, 0.0, 0.0, 1.0]);
            let off = cx
                .params
                .get("offset")
                .map_or([0.0; 2], |v| [v.x[0], v.x[1]]);
            let tiling = cx.int("tiling", 3);
            let nearest = cx.int("filteringmode", 0) == 1;
            Image::new(w, h, img.is_color(), |x, y| {
                let (u, v) = cx.uv(x, y);
                let (cu, cv) = (u - 0.5 - off[0], v - 0.5 - off[1]);
                let su = m[0] * cu + m[1] * cv + 0.5;
                let sv = m[2] * cu + m[3] * cv + 0.5;
                let outside_u = tiling & 1 == 0 && !(0.0..1.0).contains(&su);
                let outside_v = tiling & 2 == 0 && !(0.0..1.0).contains(&sv);
                if outside_u || outside_v {
                    return [0.0; 4];
                }
                if nearest {
                    img.sample_nearest(su, sv)
                } else {
                    img.sample(su, sv)
                }
            })
        }
        "blur" => {
            let img = cx.main_input().ok_or("blur: nothing connected")?;
            gaussian(&img, cx.f("intensity", 10.0) * cx.scale() * 0.5)
        }
        "dirmotionblur" => {
            let img = cx
                .main_input()
                .ok_or("directional blur: nothing connected")?;
            let len = cx.f("intensity", 10.0) * cx.scale() / w as f32;
            let a = cx.f("mblurangle", 0.0) * TAU;
            let (dx, dy) = (a.cos() * len, -a.sin() * len);
            let taps = ((len * w as f32) as usize).clamp(2, 64);
            Image::new(w, h, img.is_color(), |x, y| {
                let (u, v) = cx.uv(x, y);
                let mut acc = [0.0; 4];
                for k in 0..taps {
                    let t = k as f32 / (taps - 1) as f32 - 0.5;
                    let s = img.sample(u + dx * t, v + dy * t);
                    for i in 0..4 {
                        acc[i] += s[i];
                    }
                }
                acc.map(|a| a / taps as f32)
            })
        }
        "warp" => {
            let img = cx.main_input().ok_or("warp: nothing connected")?;
            let grad = need("inputgradient")?;
            let k = cx.f("intensity", 1.0) / 256.0;
            Image::new(w, h, img.is_color(), |x, y| {
                let (gx, gy) = gradient(&grad, x, y);
                let (u, v) = cx.uv(x, y);
                img.sample(u + gx * k, v + gy * k)
            })
        }
        "directionalwarp" => {
            let img = cx
                .main_input()
                .ok_or("directional warp: nothing connected")?;
            let amount = need("inputintensity")?;
            let a = cx.f("warpangle", 0.0) * TAU;
            let k = cx.f("intensity", 10.0) / 256.0;
            let (dx, dy) = (a.cos() * k, -a.sin() * k);
            Image::new(w, h, img.is_color(), |x, y| {
                let d = amount.get_gray(x, y);
                let (u, v) = cx.uv(x, y);
                img.sample(u + dx * d, v + dy * d)
            })
        }
        "gradient" => {
            let img = cx.main_input().ok_or("gradient map: nothing connected")?;
            let keys = gradient_keys(cx.arrays.get("gradientrgba"));
            Image::color(w, h, |x, y| eval_gradient(&keys, img.get_gray(x, y)))
        }
        "grayscaleconversion" => {
            let img = cx
                .main_input()
                .ok_or("grayscale conversion: nothing connected")?;
            let k = cx.v4("channelsweights", [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0]);
            Image::gray(w, h, |x, y| {
                let c = img.get(x, y);
                (0..4).map(|i| c[i] * k[i]).sum::<f32>().clamp(0.0, 1.0)
            })
        }
        "hsl" => {
            let img = cx.main_input().ok_or("hsl: nothing connected")?;
            let (dh, s, l) = (
                cx.f("hue", 0.5) - 0.5,
                cx.f("saturation", 0.5) * 2.0,
                cx.f("luminosity", 0.5) - 0.5,
            );
            map(&img, |c| {
                let [hh, ss, ll] = rgb_to_hsl([c[0], c[1], c[2]]);
                let rgb = hsl_to_rgb([
                    (hh + dh).rem_euclid(1.0),
                    (ss * s).clamp(0.0, 1.0),
                    (ll + l).clamp(0.0, 1.0),
                ]);
                [rgb[0], rgb[1], rgb[2], c[3]]
            })
        }
        "shuffle" => {
            let a = cx.input("input1");
            let b = cx.input("input2");
            let sel = [
                cx.int("channelred", 0),
                cx.int("channelgreen", 1),
                cx.int("channelblue", 2),
                cx.int("channelalpha", 3),
            ];
            Image::color(w, h, |x, y| {
                let ca = a.as_ref().map_or([0.0; 4], |i| i.get(x, y));
                let cb = b.as_ref().map_or([0.0; 4], |i| i.get(x, y));
                sel.map(|s| match s {
                    0..=3 => ca[s as usize],
                    4..=7 => cb[s as usize - 4],
                    _ => 0.0,
                })
            })
        }
        "normal" => {
            let img = cx.main_input().ok_or("normal: nothing connected")?;
            let k = cx.f("intensity", 1.0) * cx.scale();
            let opengl = cx.bool("inversedy", true);
            let alpha = cx.bool("input2alpha", false);
            Image::color(w, h, |x, y| {
                let (gx, gy) = gradient_px(&img, x, y);
                let n = normalize([-gx * k, if opengl { gy } else { -gy } * k, 1.0]);
                [
                    n[0] * 0.5 + 0.5,
                    n[1] * 0.5 + 0.5,
                    n[2] * 0.5 + 0.5,
                    if alpha { img.get_gray(x, y) } else { 1.0 },
                ]
            })
        }
        "emboss" => {
            let img = cx.main_input().ok_or("emboss: nothing connected")?;
            let height = need("inputgradient")?;
            let k = cx.f("intensity", 5.0) * cx.scale();
            let a = cx.f("lightangle", 0.0) * TAU;
            let hi = cx.color("highlightcolor", [1.0; 4]);
            let lo = cx.color("shadowcolor", [0.0, 0.0, 0.0, 1.0]);
            Image::new(w, h, img.is_color(), |x, y| {
                let (gx, gy) = gradient_px(&height, x, y);
                let shade = (gx * a.cos() - gy * a.sin()) * k;
                let c = img.get(x, y);
                std::array::from_fn(|i| {
                    if i == 3 {
                        c[3]
                    } else if shade > 0.0 {
                        (c[i] + (hi[i] - c[i]) * shade.min(1.0)).clamp(0.0, 1.0)
                    } else {
                        (c[i] + (lo[i] - c[i]) * (-shade).min(1.0)).clamp(0.0, 1.0)
                    }
                })
            })
        }
        "sharpen" => {
            let img = cx.main_input().ok_or("sharpen: nothing connected")?;
            let k = cx.f("intensity", 1.0);
            Image::new(w, h, img.is_color(), |x, y| {
                let (x, y) = (x as i64, y as i64);
                let c = img.fetch(x, y);
                let around =
                    [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dy)| img.fetch(x + dx, y + dy));
                std::array::from_fn(|i| {
                    let avg = around.iter().map(|p| p[i]).sum::<f32>() / 4.0;
                    (c[i] + (c[i] - avg) * k).clamp(0.0, 1.0)
                })
            })
        }
        "curve" => {
            let img = cx.main_input().ok_or("curve: nothing connected")?;
            let points = curve_points(cx.arrays.get("curveluminance"));
            map(&img, |c| {
                [
                    eval_curve(&points, c[0]),
                    eval_curve(&points, c[1]),
                    eval_curve(&points, c[2]),
                    c[3],
                ]
            })
        }
        "distance" => distance(cx)?,
        "bitmap" => {
            let path = cx
                .strs
                .get("bitmapresourcepath")
                .ok_or("bitmap: no image set")?;
            let img = cx
                .resources
                .get(crate::split_path(path).0)
                .ok_or_else(|| format!("bitmap: {path} is missing"))?;
            img.resized(w, h)
        }
        "fxmaps" | "svg" | "text" | "valueprocessor" | "dyngradient" => {
            return Err(format!("the {name} node is not supported yet"));
        }
        other => return Err(format!("unknown node `{other}`")),
    })
}

/// Same size and mode, per-pixel function of RGBA (gray uses `x`).
fn map(img: &Image, f: impl Fn([f32; 4]) -> [f32; 4] + Sync) -> Image {
    Image::new(img.w, img.h, img.is_color(), |x, y| f(img.get(x, y)))
}

fn blend(cx: &Cx) -> Image {
    let fg = cx.input("source");
    let bg = cx.input("destination");
    let mask = cx.input("opacity");
    let color =
        fg.as_ref().is_some_and(|i| i.is_color()) || bg.as_ref().is_some_and(|i| i.is_color());
    let mode = cx.int("blendingmode", 0);
    let opacity = cx.f("opacitymult", 1.0);
    let use_alpha = color && cx.int("alphablending", 0) == 0;
    Image::new(cx.w, cx.h, color, |x, y| {
        let b = bg.as_ref().map_or([0.0, 0.0, 0.0, 1.0], |i| i.get(x, y));
        let Some(fg) = &fg else { return b };
        let a = fg.get(x, y);
        let m = mask.as_ref().map_or(1.0, |i| i.get_gray(x, y));
        let t = opacity * m * if use_alpha { a[3] } else { 1.0 };
        let f = |a: f32, b: f32| -> f32 {
            match mode {
                1 => a + b,
                2 => b - a,
                3 => a * b,
                4 => b + 2.0 * a - 1.0,
                5 => a.max(b),
                6 => a.min(b),
                8 => {
                    if a <= 0.0 {
                        1.0
                    } else {
                        b / a
                    }
                }
                9 => {
                    if b < 0.5 {
                        2.0 * a * b
                    } else {
                        1.0 - 2.0 * (1.0 - a) * (1.0 - b)
                    }
                }
                10 => 1.0 - (1.0 - a) * (1.0 - b),
                11 => (1.0 - 2.0 * a) * b * b + 2.0 * a * b,
                _ => a, // 0 copy, 7 switch
            }
            .clamp(0.0, 1.0)
        };
        let mut out: [f32; 4] = std::array::from_fn(|i| b[i] + (f(a[i], b[i]) - b[i]) * t);
        out[3] = if use_alpha {
            t + b[3] * (1.0 - t)
        } else {
            b[3] + (a[3] - b[3]) * opacity * m
        };
        out
    })
}

/// Central-difference gradient in value per pixel.
fn gradient_px(img: &Image, x: usize, y: usize) -> (f32, f32) {
    let (x, y) = (x as i64, y as i64);
    (
        (img.fetch_gray(x + 1, y) - img.fetch_gray(x - 1, y)) * 0.5,
        (img.fetch_gray(x, y + 1) - img.fetch_gray(x, y - 1)) * 0.5,
    )
}

/// Gradient in value per 1/256 of the image (resolution independent).
fn gradient(img: &Image, x: usize, y: usize) -> (f32, f32) {
    let (gx, gy) = gradient_px(img, x, y);
    (gx * img.w as f32 / 256.0, gy * img.h as f32 / 256.0)
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    v.map(|c| c / l)
}

/// Approximate gaussian: three wrapping box blurs per axis.
pub fn gaussian(img: &Image, sigma_px: f32) -> Image {
    if sigma_px < 0.3 {
        return img.clone();
    }
    // Box width for 3 passes ≈ gaussian sigma.
    let r = (((12.0 * sigma_px * sigma_px / 3.0 + 1.0).sqrt() - 1.0) / 2.0)
        .round()
        .max(1.0) as usize;
    let (w, h) = (img.w, img.h);
    let mut px: Vec<[f32; 4]> = (0..w * h).map(|i| img.get(i % w, i / w)).collect();
    for _ in 0..3 {
        px = box_rows(&px, w, h, r);
        px = transpose(&px, w, h);
        px = box_rows(&px, h, w, r);
        px = transpose(&px, h, w);
    }
    Image::new(w, h, img.is_color(), |x, y| px[y * w + x])
}

fn box_rows(px: &[[f32; 4]], w: usize, h: usize, r: usize) -> Vec<[f32; 4]> {
    let mut out = vec![[0.0; 4]; w * h];
    let n = (2 * r + 1) as f32;
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &px[y * w..(y + 1) * w];
        let at = |i: i64| src[i.rem_euclid(w as i64) as usize];
        let mut acc = [0.0f32; 4];
        for i in -(r as i64)..=r as i64 {
            let p = at(i);
            for c in 0..4 {
                acc[c] += p[c];
            }
        }
        for (x, o) in row.iter_mut().enumerate() {
            *o = acc.map(|a| a / n);
            let add = at(x as i64 + r as i64 + 1);
            let sub = at(x as i64 - r as i64);
            for c in 0..4 {
                acc[c] += add[c] - sub[c];
            }
        }
    });
    out
}

fn transpose(px: &[[f32; 4]], w: usize, h: usize) -> Vec<[f32; 4]> {
    let mut out = vec![[0.0; 4]; w * h];
    out.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, o) in col.iter_mut().enumerate() {
            *o = px[y * w + x];
        }
    });
    out
}

fn gradient_keys(cells: Option<&Vec<HashMap<String, V>>>) -> Vec<(f32, [f32; 4])> {
    let mut keys: Vec<(f32, [f32; 4])> = cells
        .into_iter()
        .flatten()
        .filter_map(|c| Some((c.get("position")?.float(), c.get("value")?.rgba())))
        .collect();
    if keys.is_empty() {
        keys = vec![(0.0, [0.0, 0.0, 0.0, 1.0]), (1.0, [1.0; 4])];
    }
    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
    keys
}

pub fn eval_gradient(keys: &[(f32, [f32; 4])], t: f32) -> [f32; 4] {
    let i = keys.partition_point(|k| k.0 <= t);
    if i == 0 {
        return keys[0].1;
    }
    if i == keys.len() {
        return keys[i - 1].1;
    }
    let (a, b) = (keys[i - 1], keys[i]);
    let f = (t - a.0) / (b.0 - a.0).max(1e-6);
    std::array::from_fn(|c| a.1[c] + (b.1[c] - a.1[c]) * f)
}

fn curve_points(cells: Option<&Vec<HashMap<String, V>>>) -> Vec<(f32, f32)> {
    let mut pts: Vec<(f32, f32)> = cells
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let p = c.get("position")?;
            Some(if p.ty.len() >= 2 {
                (p.x[0], p.x[1])
            } else {
                (p.x[0], c.get("value")?.float())
            })
        })
        .collect();
    if pts.is_empty() {
        pts = vec![(0.0, 0.0), (1.0, 1.0)];
    }
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    pts
}

/// Smooth (Hermite with zero end slopes) interpolation between curve points.
/// ponytail: ignores Designer's per-point tangents.
fn eval_curve(pts: &[(f32, f32)], x: f32) -> f32 {
    let i = pts.partition_point(|p| p.0 <= x);
    if i == 0 {
        return pts[0].1;
    }
    if i == pts.len() {
        return pts[i - 1].1;
    }
    let (a, b) = (pts[i - 1], pts[i]);
    let t = ((x - a.0) / (b.0 - a.0).max(1e-6)).clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    a.1 + (b.1 - a.1) * s
}

fn rgb_to_hsl([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max - min < 1e-6 {
        return [0.0, 0.0, l];
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [h / 6.0, s, l]
}

fn hsl_to_rgb([h, s, l]: [f32; 3]) -> [f32; 3] {
    if s <= 0.0 {
        return [l; 3];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |t: f32| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

/// Distance node: falloff from the mask's white pixels, or the source's color
/// spread from them. Jump flooding on a wrapping grid.
fn distance(cx: &Cx) -> Result<Image, String> {
    let mask = cx
        .input("mask")
        .ok_or("distance: `mask` is not connected")?;
    let source = cx.input("source");
    let (w, h) = (cx.w, cx.h);
    let max_d = cx.f("distance", 10.0) * cx.scale();
    let combine = cx.bool("combinedistance", true);
    let wrap_d2 = |ax: i64, ay: i64, bx: i64, by: i64| {
        let dx = (ax - bx).rem_euclid(w as i64);
        let dy = (ay - by).rem_euclid(h as i64);
        let dx = dx.min(w as i64 - dx);
        let dy = dy.min(h as i64 - dy);
        dx * dx + dy * dy
    };
    let mut seed: Vec<Option<(i64, i64)>> = (0..w * h)
        .map(|i| (mask.get_gray(i % w, i / w) > 0.5).then_some(((i % w) as i64, (i / w) as i64)))
        .collect();
    let mut step = w.max(h) / 2;
    while step >= 1 {
        let prev = seed.clone();
        seed.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, s) in row.iter_mut().enumerate() {
                let (x, y) = (x as i64, y as i64);
                let mut best = *s;
                let mut best_d = best.map_or(i64::MAX, |(sx, sy)| wrap_d2(x, y, sx, sy));
                for dy in [-1, 0, 1] {
                    for dx in [-1, 0, 1] {
                        let nx = (x + dx * step as i64).rem_euclid(w as i64);
                        let ny = (y + dy * step as i64).rem_euclid(h as i64);
                        if let Some((sx, sy)) = prev[(ny as usize) * w + nx as usize] {
                            let d = wrap_d2(x, y, sx, sy);
                            if d < best_d {
                                best_d = d;
                                best = Some((sx, sy));
                            }
                        }
                    }
                }
                *s = best;
            }
        });
        step /= 2;
    }
    let falloff = |x: usize, y: usize| {
        seed[y * w + x].map_or(0.0, |(sx, sy)| {
            let d = (wrap_d2(x as i64, y as i64, sx, sy) as f32).sqrt();
            (1.0 - d / max_d.max(1e-3)).clamp(0.0, 1.0)
        })
    };
    Ok(match source {
        Some(src) => Image::new(w, h, src.is_color(), |x, y| {
            let Some((sx, sy)) = seed[y * w + x] else {
                return [0.0, 0.0, 0.0, 1.0];
            };
            let c = src.get(sx as usize, sy as usize);
            if combine {
                let f = falloff(x, y);
                [c[0] * f, c[1] * f, c[2] * f, c[3]]
            } else {
                c
            }
        }),
        None => Image::gray(w, h, falloff),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx<'a>(
        res: &'a HashMap<String, Arc<Image>>,
        inputs: Vec<(&str, Image)>,
        params: Vec<(&str, V)>,
    ) -> Cx<'a> {
        Cx {
            w: 8,
            h: 8,
            seed: 0,
            inputs: inputs
                .into_iter()
                .map(|(s, i)| (s.to_string(), Arc::new(i)))
                .collect(),
            params: params
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            arrays: HashMap::new(),
            strs: HashMap::new(),
            resources: res,
        }
    }

    fn flat(v: f32) -> Image {
        Image::uniform(8, 8, [v, v, v, 1.0], false)
    }

    #[test]
    fn levels_midpoint_and_output_range() {
        let res = HashMap::new();
        let img = run(
            "levels",
            &cx(
                &res,
                vec![("input1", flat(0.25))],
                vec![("levelinmid", V::f(0.25))],
            ),
        )
        .unwrap();
        assert!((img.get_gray(0, 0) - 0.5).abs() < 1e-5);
        let img = run(
            "levels",
            &cx(
                &res,
                vec![("input1", flat(0.5))],
                vec![("leveloutlow", V::f(0.2)), ("levelouthigh", V::f(0.4))],
            ),
        )
        .unwrap();
        assert!((img.get_gray(0, 0) - 0.3).abs() < 1e-5);
    }

    #[test]
    fn blend_modes() {
        let res = HashMap::new();
        let mode = |m: i32, a: f32, b: f32| {
            run(
                "blend",
                &cx(
                    &res,
                    vec![("source", flat(a)), ("destination", flat(b))],
                    vec![("blendingmode", V::i(m))],
                ),
            )
            .unwrap()
            .get_gray(0, 0)
        };
        assert_eq!(mode(0, 0.3, 0.8), 0.3);
        assert!((mode(2, 0.3, 0.8) - 0.5).abs() < 1e-6); // background minus foreground
        assert!((mode(3, 0.5, 0.8) - 0.4).abs() < 1e-6);
        assert_eq!(mode(5, 0.3, 0.8), 0.8);
        assert!((mode(10, 0.5, 0.5) - 0.75).abs() < 1e-6);
        assert_eq!(mode(1, 0.7, 0.8), 1.0); // clamped
    }

    #[test]
    fn transformation_offsets_wrap() {
        let res = HashMap::new();
        let ramp = Image::gray(8, 8, |x, _| x as f32 / 8.0);
        let img = run(
            "transformation",
            &cx(
                &res,
                vec![("input1", ramp)],
                vec![("offset", V::f2(0.5, 0.0))],
            ),
        )
        .unwrap();
        assert!((img.get_gray(4, 0) - 0.0).abs() < 1e-5);
        assert!((img.get_gray(0, 0) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn normal_follows_opengl_convention() {
        let res = HashMap::new();
        // Height rising to the right → normal leans left (red < 0.5).
        let right = Image::gray(8, 8, |x, _| (x as f32 / 8.0 * TAU).sin());
        let n = run("normal", &cx(&res, vec![("input1", right)], vec![])).unwrap();
        assert!(n.get(0, 0)[0] < 0.5);
        // Height rising upward (towards row 0) → normal leans down (green < 0.5 in OpenGL).
        let up = Image::gray(8, 8, |_, y| -((y as f32 / 8.0 * TAU).sin()));
        let n = run("normal", &cx(&res, vec![("input1", up)], vec![])).unwrap();
        assert!(n.get(0, 0)[1] < 0.5, "{:?}", n.get(0, 0));
    }

    #[test]
    fn distance_falls_off_from_the_mask() {
        let res = HashMap::new();
        let mask = Image::gray(8, 8, |x, y| if (x, y) == (0, 0) { 1.0 } else { 0.0 });
        // distance 4 px at 256 → 0.125 px at 8: use a large value instead.
        let d = run(
            "distance",
            &cx(&res, vec![("mask", mask)], vec![("distance", V::f(128.0))]),
        )
        .unwrap();
        assert_eq!(d.get_gray(0, 0), 1.0);
        assert!(d.get_gray(1, 0) > d.get_gray(3, 0));
        // Wraps: column 7 is next to column 0.
        assert!((d.get_gray(7, 0) - d.get_gray(1, 0)).abs() < 1e-6);
    }

    #[test]
    fn blur_keeps_the_average() {
        let img = Image::gray(32, 32, |x, y| ((x / 4 + y / 4) % 2) as f32);
        let b = gaussian(&img, 3.0);
        let mean = |i: &Image| {
            (0..32 * 32)
                .map(|k| i.get_gray(k % 32, k / 32))
                .sum::<f32>()
                / 1024.0
        };
        assert!((mean(&img) - mean(&b)).abs() < 1e-4);
        assert!((b.get_gray(0, 0) - 0.5).abs() < 0.3);
    }
}
