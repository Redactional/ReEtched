use rayon::prelude::*;

use crate::ops::{Op, trilinear};

/// A compiled color conversion. Cheap to share across threads.
#[derive(Clone)]
pub struct Processor {
    ops: Vec<Op>,
}

impl Processor {
    pub(crate) fn new(ops: Vec<Op>) -> Processor {
        Processor { ops }
    }

    /// True when the conversion does nothing (e.g. same color space or data bypass).
    pub fn is_noop(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn apply_rgb(&self, rgb: &mut [f32; 3]) {
        for op in &self.ops {
            op.apply(rgb);
        }
    }

    pub fn apply_rgb_slice(&self, pixels: &mut [[f32; 3]]) {
        if self.is_noop() {
            return;
        }
        pixels
            .par_chunks_mut(4096)
            .for_each(|c| c.iter_mut().for_each(|p| self.apply_rgb(p)));
    }

    /// Converts interleaved pixels with `channels` (3 or 4) floats each; alpha is left untouched.
    pub fn apply_interleaved(&self, data: &mut [f32], channels: usize) {
        assert!(channels >= 3, "need at least RGB");
        if self.is_noop() {
            return;
        }
        data.par_chunks_mut(channels * 4096).for_each(|chunk| {
            for px in chunk.chunks_exact_mut(channels) {
                let mut rgb = [px[0], px[1], px[2]];
                self.apply_rgb(&mut rgb);
                px[..3].copy_from_slice(&rgb);
            }
        });
    }

    /// Bakes the conversion into a `size`³ LUT addressed through `shaper`.
    pub fn bake_lut3d(&self, size: u32, shaper: Shaper) -> Lut3d {
        assert!(size >= 2, "LUT size must be at least 2");
        let n = size as usize;
        let coord: Vec<f32> = (0..n)
            .map(|i| shaper.decode(i as f32 / (n - 1) as f32))
            .collect();
        let mut data = vec![[0.0f32; 3]; n * n * n];
        data.par_chunks_mut(n * n)
            .enumerate()
            .for_each(|(b, slice)| {
                for (i, px) in slice.iter_mut().enumerate() {
                    *px = [coord[i % n], coord[i / n], coord[b]];
                    self.apply_rgb(px);
                }
            });
        Lut3d { size, shaper, data }
    }
}

/// Per-channel log2 encoding used to address a baked 3D LUT, so scene-linear HDR input
/// (0.18 middle grey, many stops above and below) spreads evenly over the grid.
///
/// Shader equivalent: `t = clamp((log2(max(x, 1e-10) / 0.18) - min_stop) / (max_stop - min_stop), 0, 1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shaper {
    /// Stops below 0.18 mapped to LUT coordinate 0.
    pub min_stop: f32,
    /// Stops above 0.18 mapped to LUT coordinate 1.
    pub max_stop: f32,
}

impl Default for Shaper {
    fn default() -> Self {
        Shaper {
            min_stop: -12.0,
            max_stop: 12.0,
        }
    }
}

impl Shaper {
    /// Linear value to LUT coordinate in [0, 1].
    pub fn encode(&self, x: f32) -> f32 {
        (((x.max(1e-10) / 0.18).log2() - self.min_stop) / (self.max_stop - self.min_stop))
            .clamp(0.0, 1.0)
    }

    /// LUT coordinate in [0, 1] to linear value.
    pub fn decode(&self, t: f32) -> f32 {
        0.18 * (self.min_stop + t * (self.max_stop - self.min_stop)).exp2()
    }
}

/// A baked 3D LUT: `data[r + size * (g + size * b)]` (red fastest), addressed by `shaper`.
#[derive(Clone, Debug)]
pub struct Lut3d {
    pub size: u32,
    pub shaper: Shaper,
    pub data: Vec<[f32; 3]>,
}

impl Lut3d {
    /// CPU reference of the shader lookup: shaper, then trilinear interpolation.
    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        trilinear(
            self.size as usize,
            &self.data,
            rgb.map(|v| self.shaper.encode(v)),
        )
    }
}
