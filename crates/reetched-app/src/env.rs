//! CPU side of the HDR environment: loading, mip chain, diffuse spherical harmonics.
//! Equirect convention (shared with the shaders): u = 0.5 + atan2(z, x) / 2π, v = acos(y) / π, row 0 = up.

use anyhow::Result;
use glam::Vec3;
use std::{f32::consts::PI, path::Path};

/// Larger HDRIs are box-downsampled to this width; the viewport never needs more.
const MAX_WIDTH: u32 = 2048;

#[derive(Clone)]
pub struct EnvImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

pub fn dir_from_uv(u: f32, v: f32) -> Vec3 {
    let (phi, theta) = ((u - 0.5) * 2.0 * PI, v * PI);
    Vec3::new(
        theta.sin() * phi.cos(),
        theta.cos(),
        theta.sin() * phi.sin(),
    )
}

impl EnvImage {
    pub fn load(path: &Path) -> Result<Self> {
        let img = image::open(path)?.into_rgba32f();
        let mut env = EnvImage {
            width: img.width(),
            height: img.height(),
            pixels: bytemuck::cast_vec(img.into_raw()),
        };
        while env.width > MAX_WIDTH {
            env = env.half();
        }
        Ok(env)
    }

    /// Soft studio-ish sky, used until the user loads an HDRI.
    pub fn procedural_sky() -> Self {
        let (width, height) = (1024, 512);
        let sun = Vec3::new(0.5, 0.6, 0.35).normalize();
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let d = dir_from_uv(
                    (x as f32 + 0.5) / width as f32,
                    (y as f32 + 0.5) / height as f32,
                );
                let up = d.y.max(0.0).sqrt();
                let sky = Vec3::new(0.85, 0.83, 0.8).lerp(Vec3::new(0.45, 0.48, 0.52), up);
                let ground = Vec3::new(0.08, 0.075, 0.07);
                let horizon = ((d.y + 0.03) / 0.06).clamp(0.0, 1.0);
                let c = ground.lerp(sky, horizon)
                    + Vec3::new(60.0, 57.0, 52.0) * ((d.dot(sun) - 1.0) * 250.0).exp();
                pixels.push([c.x, c.y, c.z, 1.0]);
            }
        }
        EnvImage {
            width,
            height,
            pixels,
        }
    }

    /// 2×2 box downsample.
    pub fn half(&self) -> Self {
        let (w, h) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let at = |x: u32, y: u32| {
            Vec3::from_slice(
                &self.pixels
                    [(y.min(self.height - 1) * self.width + x.min(self.width - 1)) as usize],
            )
        };
        let pixels = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let c = (at(2 * x, 2 * y)
                    + at(2 * x + 1, 2 * y)
                    + at(2 * x, 2 * y + 1)
                    + at(2 * x + 1, 2 * y + 1))
                    * 0.25;
                [c.x, c.y, c.z, 1.0]
            })
            .collect();
        EnvImage {
            width: w,
            height: h,
            pixels,
        }
    }

    /// A copy no wider than `max_width` (box-downsampled).
    pub fn shrunk(&self, max_width: u32) -> EnvImage {
        let mut env = if self.width > max_width {
            self.half()
        } else {
            self.clone()
        };
        while env.width > max_width {
            env = env.half();
        }
        env
    }

    /// Bilinear lookup in the direction `d` (equirect convention above).
    pub fn sample(&self, d: Vec3) -> Vec3 {
        let u = d.z.atan2(d.x) / (2.0 * PI) + 0.5;
        let v = d.y.clamp(-1.0, 1.0).acos() / PI;
        let (x, y) = (u * self.width as f32 - 0.5, v * self.height as f32 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |x: f32, y: f32| {
            let xi = (x as i32).rem_euclid(self.width as i32) as u32;
            let yi = (y as i32).clamp(0, self.height as i32 - 1) as u32;
            Vec3::from_slice(&self.pixels[(yi * self.width + xi) as usize])
        };
        let top = at(x0, y0).lerp(at(x0 + 1.0, y0), fx);
        let bottom = at(x0, y0 + 1.0).lerp(at(x0 + 1.0, y0 + 1.0), fx);
        top.lerp(bottom, fy)
    }

    pub fn mip_chain(self) -> Vec<EnvImage> {
        let mut chain = vec![self];
        while let Some(last) = chain.last().filter(|m| m.width > 1 || m.height > 1) {
            chain.push(last.half());
        }
        chain
    }

    /// 9 SH coefficients of the cosine-convolved environment, divided by π,
    /// so `albedo * eval_sh(n)` is the diffuse radiance leaving the surface.
    pub fn diffuse_sh(&self) -> [[f32; 4]; 9] {
        const BAND: [f32; 9] = [
            PI,
            2.0 * PI / 3.0,
            2.0 * PI / 3.0,
            2.0 * PI / 3.0,
            PI / 4.0,
            PI / 4.0,
            PI / 4.0,
            PI / 4.0,
            PI / 4.0,
        ];
        let mut src = self;
        let small;
        if self.width > 256 {
            let mut s = self.half();
            while s.width > 256 {
                s = s.half();
            }
            small = s;
            src = &small;
        }
        let (w, h) = (src.width as f32, src.height as f32);
        let mut acc = [Vec3::ZERO; 9];
        for y in 0..src.height {
            let v = (y as f32 + 0.5) / h;
            let d_omega = (2.0 * PI / w) * (PI / h) * (v * PI).sin();
            for x in 0..src.width {
                let d = dir_from_uv((x as f32 + 0.5) / w, v);
                let l = Vec3::from_slice(&src.pixels[(y * src.width + x) as usize]);
                for (a, b) in acc.iter_mut().zip(sh_basis(d)) {
                    *a += l * b * d_omega;
                }
            }
        }
        std::array::from_fn(|i| {
            let c = acc[i] * BAND[i] / PI;
            [c.x, c.y, c.z, 0.0]
        })
    }
}

/// Real SH basis, bands 0–2. The shader's `sh_irradiance` uses the same order and constants.
/// Diffuse radiance factor for normal `n` from `EnvImage::diffuse_sh` coefficients.
pub fn eval_sh(sh: &[[f32; 4]; 9], n: Vec3) -> Vec3 {
    sh.iter()
        .zip(sh_basis(n))
        .map(|(c, b)| Vec3::from_slice(c) * b)
        .sum()
}

fn sh_basis(d: Vec3) -> [f32; 9] {
    let Vec3 { x, y, z } = d;
    [
        0.282095,
        0.488603 * y,
        0.488603 * z,
        0.488603 * x,
        1.092548 * x * y,
        1.092548 * y * z,
        0.315392 * (3.0 * z * z - 1.0),
        1.092548 * x * z,
        0.546274 * (x * x - y * y),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_environment_gives_unit_diffuse() {
        let env = EnvImage {
            width: 128,
            height: 64,
            pixels: vec![[1.0; 4]; 128 * 64],
        };
        let sh = env.diffuse_sh();
        for n in [
            Vec3::X,
            Vec3::Y,
            -Vec3::Z,
            Vec3::new(1.0, -1.0, 1.0).normalize(),
        ] {
            assert!(
                eval_sh(&sh, n).abs_diff_eq(Vec3::ONE, 0.01),
                "{n}: {}",
                eval_sh(&sh, n)
            );
        }
    }

    #[test]
    fn sky_is_brighter_from_above() {
        let sh = EnvImage::procedural_sky().diffuse_sh();
        assert!(eval_sh(&sh, Vec3::Y).x > eval_sh(&sh, -Vec3::Y).x);
    }

    #[test]
    fn mip_chain_ends_at_one_pixel() {
        let chain = EnvImage {
            width: 16,
            height: 8,
            pixels: vec![[2.0; 4]; 128],
        }
        .mip_chain();
        let last = chain.last().unwrap();
        assert_eq!((chain.len(), last.width, last.height), (5, 1, 1));
        assert_eq!(last.pixels[0][0], 2.0);
    }
}
