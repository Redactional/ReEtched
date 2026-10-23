//! Built-in material presets and their sphere thumbnails (shaded on the CPU).

use crate::env::{EnvImage, eval_sh};
use glam::Vec3;
use reetched_doc::Fill;

pub struct Preset {
    pub name: &'static str,
    /// sRGB, like a color picker shows it.
    pub base_srgb: [u8; 3],
    pub roughness: f32,
    pub metallic: f32,
}

/// A flat PBR material: what a thumbnail shows.
pub struct Look {
    /// Linear RGB.
    pub base: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
}

impl Preset {
    pub fn look(&self) -> Look {
        let [r, g, b] = self.base_srgb;
        let c = egui::Rgba::from(egui::Color32::from_rgb(r, g, b));
        Look {
            base: [c.r(), c.g(), c.b()],
            roughness: self.roughness,
            metallic: self.metallic,
        }
    }

    /// A fill layer's content for this preset.
    pub fn fill(&self) -> Fill {
        let l = self.look();
        Fill::new(l.base, l.metallic, l.roughness)
    }
}

const fn p(name: &'static str, base_srgb: [u8; 3], roughness: f32, metallic: f32) -> Preset {
    Preset {
        name,
        base_srgb,
        roughness,
        metallic,
    }
}

/// Metals use measured F0 colors; dielectrics plausible albedos.
pub const PRESETS: &[Preset] = &[
    p("Aluminium", [245, 246, 246], 0.30, 1.0),
    p("Brass", [245, 228, 172], 0.28, 1.0),
    p("Ceramic", [235, 233, 226], 0.08, 0.0),
    p("Chrome", [196, 197, 197], 0.06, 1.0),
    p("Clay", [170, 120, 92], 0.92, 0.0),
    p("Concrete", [158, 156, 150], 0.90, 0.0),
    p("Copper", [250, 208, 192], 0.30, 1.0),
    p("Gold", [255, 226, 155], 0.22, 1.0),
    p("Iron", [198, 198, 200], 0.55, 1.0),
    p("Leather", [92, 56, 36], 0.60, 0.0),
    p("Plastic Red", [190, 30, 28], 0.35, 0.0),
    p("Plastic White", [230, 230, 228], 0.40, 0.0),
    p("Rubber", [38, 38, 38], 0.85, 0.0),
    p("Silver", [252, 250, 245], 0.18, 1.0),
    p("Skin", [214, 160, 136], 0.50, 0.0),
    p("Wood", [150, 100, 60], 0.70, 0.0),
];

/// Small copy of the environment for shading thumbnails.
pub struct PreviewEnv {
    mips: Vec<EnvImage>,
    sh: [[f32; 4]; 9],
}

impl PreviewEnv {
    pub fn new(env: &EnvImage) -> Self {
        let small = env.shrunk(256);
        Self {
            sh: small.diffuse_sh(),
            mips: small.mip_chain(),
        }
    }

    /// Lit sphere on a transparent background, viewed from +Z.
    pub fn render(&self, m: &Look, size: usize) -> egui::ColorImage {
        let base = Vec3::from(m.base);
        let rough = m.roughness.clamp(0.02, 1.0);
        let f0 = Vec3::splat(0.04).lerp(base, m.metallic);
        let mut rgba = Vec::with_capacity(size * size * 4);
        for py in 0..size {
            for px in 0..size {
                let x = (px as f32 + 0.5) / size as f32 * 2.0 - 1.0;
                let y = 1.0 - (py as f32 + 0.5) / size as f32 * 2.0;
                let r = (x * x + y * y).sqrt();
                let coverage = ((1.0 - r) * size as f32 * 0.5 + 0.5).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    rgba.extend([0, 0, 0, 0]);
                    continue;
                }
                let n = Vec3::new(x, y, (1.0 - (r * r).min(1.0)).sqrt()).normalize();
                let v = Vec3::Z;
                let nv = n.dot(v).max(1e-4);
                // Same split-sum approximation as fs_pbr.
                let rr = [
                    1.0 - rough,
                    rough * -0.0275 + 0.0425,
                    rough * -0.572 + 1.04,
                    rough * 0.022 - 0.04,
                ];
                let a004 = (rr[0] * rr[0]).min((-9.28 * nv).exp2()) * rr[0] + rr[1];
                let (a, b) = (-1.04 * a004 + rr[2], 1.04 * a004 + rr[3]);
                let reflected = 2.0 * n.dot(v) * n - v;
                // ponytail: box-filtered mips stand in for GGX prefiltering; fine at thumbnail size.
                let spec = self.sample_lod(reflected, rough * 5.0) * (f0 * a + Vec3::splat(b));
                let diffuse = eval_sh(&self.sh, n) * base * (1.0 - m.metallic);
                let c = aces(diffuse + spec);
                rgba.extend([
                    encode(c.x),
                    encode(c.y),
                    encode(c.z),
                    (coverage * 255.0) as u8,
                ]);
            }
        }
        egui::ColorImage::from_rgba_unmultiplied([size, size], &rgba)
    }

    fn sample_lod(&self, d: Vec3, lod: f32) -> Vec3 {
        let lod = lod.clamp(0.0, (self.mips.len() - 1) as f32);
        let (lo, t) = (lod.floor() as usize, lod.fract());
        let hi = (lo + 1).min(self.mips.len() - 1);
        self.mips[lo].sample(d).lerp(self.mips[hi].sample(d), t)
    }
}

fn aces(x: Vec3) -> Vec3 {
    ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(Vec3::ZERO, Vec3::ONE)
}

fn encode(linear: f32) -> u8 {
    let s = if linear <= 0.0031308 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_is_round_with_transparent_corners() {
        let env = PreviewEnv::new(&EnvImage::procedural_sky());
        let img = env.render(&PRESETS[0].look(), 32);
        assert_eq!(img.size, [32, 32]);
        assert_eq!(img.pixels[0].a(), 0, "corner should be transparent");
        assert_eq!(img.pixels[16 * 32 + 16].a(), 255, "center should be opaque");
    }
}
