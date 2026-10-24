//! Our own generator nodes (`reetched_*`). All tile seamlessly.

use crate::{Image, atomic::Cx, func::hash01};
use std::f32::consts::TAU;

pub fn run(name: &str, cx: &Cx) -> Result<Image, String> {
    let (w, h) = (cx.w, cx.h);
    let seed = cx.seed;
    Ok(match name {
        "reetched_noise" => {
            let scale = cx.int("scale", 8).max(1);
            let stretch = cx.f("stretch", 1.0).max(1.0 / 64.0);
            let periods = [scale, ((scale as f32 * stretch).round() as i32).max(1)];
            let octaves = cx.int("octaves", 5).clamp(1, 10);
            let roughness = cx.f("roughness", 0.5);
            let kind = cx.int("kind", 0);
            let img = Image::gray(w, h, |x, y| {
                let (u, v) = uv(x, y, w, h);
                let mut sum = 0.0;
                let mut amp = 1.0;
                let mut total = 0.0;
                for o in 0..octaves {
                    let f = 1 << o;
                    let n = perlin(
                        u,
                        v,
                        [periods[0] * f, periods[1] * f],
                        seed.wrapping_add(o * 101),
                    );
                    sum += amp
                        * match kind {
                            1 => 1.0 - n.abs() * 2.0, // ridged
                            2 => n.abs() * 2.0,       // turbulence
                            _ => n,
                        };
                    total += amp;
                    amp *= roughness;
                }
                sum / total
            });
            normalized(img)
        }
        "reetched_cells" => {
            let scale = cx.int("scale", 8).max(1);
            let jitter = cx.f("jitter", 1.0);
            let mode = cx.int("mode", 0);
            let img = Image::gray(w, h, |x, y| {
                let (u, v) = uv(x, y, w, h);
                let (f1, f2, id) = worley(u, v, scale, jitter, seed);
                match mode {
                    1 => f2 - f1,
                    2 => id,
                    _ => f1,
                }
            });
            normalized(img)
        }
        "reetched_tiles" => {
            let cols = cx.int("columns", 4).max(1);
            let rows = cx.int("rows", 8).max(1);
            let offset = cx.f("offset", 0.5);
            let gap = cx.f("gap", 0.004);
            let bevel = cx.f("bevel", 0.01).max(1e-4);
            let variation = cx.f("variation", 0.3);
            let mode = cx.int("mode", 0);
            Image::gray(w, h, |x, y| {
                let (u, v) = uv(x, y, w, h);
                let ry = v * rows as f32;
                let row = ry.floor() as i32;
                let rx = u * cols as f32 + offset * row as f32;
                let col = rx.floor() as i32;
                let id = hash01(
                    seed as u32 ^ 0x5bd1_e995,
                    (col.rem_euclid(cols) as u32) << 16 | row.rem_euclid(rows) as u32,
                );
                if mode == 1 {
                    return id;
                }
                // Distance to the tile's edge in image (UV) units; gap and bevel use them too.
                let (ex, ey) = (rx - rx.floor(), ry - ry.floor());
                let edge = (ex.min(1.0 - ex) / cols as f32).min(ey.min(1.0 - ey) / rows as f32);
                let shape = ((edge - gap * 0.5) / bevel).clamp(0.0, 1.0);
                shape * (1.0 - variation * id)
            })
        }
        "reetched_weave" => {
            let threads = cx.int("scale", 16).max(1);
            Image::gray(w, h, |x, y| {
                let (u, v) = uv(x, y, w, h);
                let (tu, tv) = (u * threads as f32, v * threads as f32);
                let (cu, cv) = (tu.floor() as i32, tv.floor() as i32);
                let (fu, fv) = (tu.fract(), tv.fract());
                // Horizontal thread on top when cell parity is even.
                let over_h = (cu + cv).rem_euclid(2) == 0;
                let across = |f: f32| (f * std::f32::consts::PI).sin();
                let (along_h, along_v) = (across(fv), across(fu));
                let lift_h = (fu * std::f32::consts::PI).sin();
                let lift_v = (fv * std::f32::consts::PI).sin();
                let h_thread = along_h
                    * if over_h {
                        0.6 + 0.4 * lift_h
                    } else {
                        0.5 - 0.3 * lift_h
                    };
                let v_thread = along_v
                    * if over_h {
                        0.5 - 0.3 * lift_v
                    } else {
                        0.6 + 0.4 * lift_v
                    };
                h_thread.max(v_thread)
            })
        }
        "reetched_wood" => {
            let rings = cx.f("rings", 12.0);
            let warp = cx.f("warp", 0.5);
            let scale = cx.int("scale", 4).max(1);
            Image::gray(w, h, |x, y| {
                let (u, v) = uv(x, y, w, h);
                // Rings run along u; warped by low-frequency noise for growth irregularity.
                let n =
                    perlin(u, v, [scale, 1], seed) + 0.5 * perlin(u, v, [scale * 2, 2], seed + 7);
                let t = (v + n * warp * 0.2) * rings;
                let ring = (t * TAU).sin() * 0.5 + 0.5;
                ring.powf(3.0)
            })
        }
        "reetched_gradient" => {
            let kind = cx.int("kind", 0);
            let angle = cx.f("angle", 0.0) * TAU;
            Image::gray(w, h, |x, y| {
                let (u, v) = uv(x, y, w, h);
                match kind {
                    1 => (1.0 - ((u - 0.5).hypot(v - 0.5) * 2.0)).clamp(0.0, 1.0),
                    _ => {
                        let (c, s) = (angle.cos(), angle.sin());
                        ((u - 0.5) * c - (v - 0.5) * s + 0.5).clamp(0.0, 1.0)
                    }
                }
            })
        }
        other => return Err(format!("unknown node `{other}`")),
    })
}

fn uv(x: usize, y: usize, w: usize, h: usize) -> (f32, f32) {
    ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32)
}

/// Stretches a grayscale image to the full 0–1 range.
fn normalized(img: Image) -> Image {
    let crate::Px::Gray(px) = &img.px else {
        return img;
    };
    let (lo, hi) = px
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    let span = (hi - lo).max(1e-6);
    Image::gray(img.w, img.h, |x, y| (px[y * img.w + x] - lo) / span)
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Tileable gradient noise with `periods` cells across the image, roughly in [-0.5, 0.5].
pub fn perlin(u: f32, v: f32, periods: [i32; 2], seed: i32) -> f32 {
    let (x, y) = (u * periods[0] as f32, v * periods[1] as f32);
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - ix as f32, y - iy as f32);
    let grad = |cx: i32, cy: i32, dx: f32, dy: f32| {
        let a = hash01(
            seed as u32,
            (cx.rem_euclid(periods[0]) as u32).wrapping_mul(73_856_093)
                ^ (cy.rem_euclid(periods[1]) as u32).wrapping_mul(19_349_663),
        ) * TAU;
        a.cos() * dx + a.sin() * dy
    };
    let n00 = grad(ix, iy, fx, fy);
    let n10 = grad(ix + 1, iy, fx - 1.0, fy);
    let n01 = grad(ix, iy + 1, fx, fy - 1.0);
    let n11 = grad(ix + 1, iy + 1, fx - 1.0, fy - 1.0);
    let (sx, sy) = (fade(fx), fade(fy));
    let a = n00 + (n10 - n00) * sx;
    let b = n01 + (n11 - n01) * sx;
    a + (b - a) * sy
}

/// Tileable Worley noise: (nearest distance, second distance, nearest cell's random value),
/// distances in cell units.
pub fn worley(u: f32, v: f32, scale: i32, jitter: f32, seed: i32) -> (f32, f32, f32) {
    let (x, y) = (u * scale as f32, v * scale as f32);
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let (mut f1, mut f2, mut id) = (f32::MAX, f32::MAX, 0.0);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (ix + dx, iy + dy);
            let key = (cx.rem_euclid(scale) as u32) << 16 | cy.rem_euclid(scale) as u32;
            let px = cx as f32 + 0.5 + (hash01(seed as u32, key) - 0.5) * jitter;
            let py = cy as f32 + 0.5 + (hash01(seed as u32 ^ 0xabcd, key) - 0.5) * jitter;
            let d = (px - x).hypot(py - y);
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = hash01(seed as u32 ^ 0x1234, key);
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2, id)
}
