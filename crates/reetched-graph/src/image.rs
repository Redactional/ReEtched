//! Images flowing between nodes: grayscale or RGBA, f32, row-major from the top.

use rayon::prelude::*;

#[derive(Clone, Debug, PartialEq)]
pub enum Px {
    Gray(Vec<f32>),
    Color(Vec<[f32; 4]>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub px: Px,
}

/// Luminance used when a color image feeds a grayscale input.
pub fn luma(c: [f32; 4]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

impl Image {
    pub fn gray(w: usize, h: usize, f: impl Fn(usize, usize) -> f32 + Sync) -> Image {
        let mut px = vec![0.0; w * h];
        px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, p) in row.iter_mut().enumerate() {
                *p = f(x, y);
            }
        });
        Image {
            w,
            h,
            px: Px::Gray(px),
        }
    }

    pub fn color(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 4] + Sync) -> Image {
        let mut px = vec![[0.0; 4]; w * h];
        px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, p) in row.iter_mut().enumerate() {
                *p = f(x, y);
            }
        });
        Image {
            w,
            h,
            px: Px::Color(px),
        }
    }

    /// Color or gray depending on `color`, from an RGBA function.
    pub fn new(
        w: usize,
        h: usize,
        color: bool,
        f: impl Fn(usize, usize) -> [f32; 4] + Sync,
    ) -> Image {
        if color {
            Image::color(w, h, f)
        } else {
            Image::gray(w, h, |x, y| f(x, y)[0])
        }
    }

    pub fn uniform(w: usize, h: usize, c: [f32; 4], color: bool) -> Image {
        if color {
            Image {
                w,
                h,
                px: Px::Color(vec![c; w * h]),
            }
        } else {
            Image {
                w,
                h,
                px: Px::Gray(vec![c[0]; w * h]),
            }
        }
    }

    pub fn is_color(&self) -> bool {
        matches!(self.px, Px::Color(_))
    }

    /// Pixel as RGBA (gray → grey, alpha 1).
    pub fn get(&self, x: usize, y: usize) -> [f32; 4] {
        match &self.px {
            Px::Gray(p) => {
                let g = p[y * self.w + x];
                [g, g, g, 1.0]
            }
            Px::Color(p) => p[y * self.w + x],
        }
    }

    /// Pixel as gray (color → luminance).
    pub fn get_gray(&self, x: usize, y: usize) -> f32 {
        match &self.px {
            Px::Gray(p) => p[y * self.w + x],
            Px::Color(p) => luma(p[y * self.w + x]),
        }
    }

    /// Wrapped pixel fetch.
    pub fn fetch(&self, x: i64, y: i64) -> [f32; 4] {
        self.get(
            x.rem_euclid(self.w as i64) as usize,
            y.rem_euclid(self.h as i64) as usize,
        )
    }

    pub fn fetch_gray(&self, x: i64, y: i64) -> f32 {
        self.get_gray(
            x.rem_euclid(self.w as i64) as usize,
            y.rem_euclid(self.h as i64) as usize,
        )
    }

    /// Bilinear, wrapping sample at normalized `(u, v)` (v down).
    pub fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let fx = u * self.w as f32 - 0.5;
        let fy = v * self.h as f32 - 0.5;
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = self.fetch(x0, y0);
        let b = self.fetch(x0 + 1, y0);
        let c = self.fetch(x0, y0 + 1);
        let d = self.fetch(x0 + 1, y0 + 1);
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bot = c[i] + (d[i] - c[i]) * tx;
            top + (bot - top) * ty
        })
    }

    pub fn sample_gray(&self, u: f32, v: f32) -> f32 {
        match &self.px {
            Px::Gray(_) => self.sample(u, v)[0],
            Px::Color(_) => luma(self.sample(u, v)),
        }
    }

    /// Nearest, wrapping sample.
    pub fn sample_nearest(&self, u: f32, v: f32) -> [f32; 4] {
        self.fetch(
            (u * self.w as f32).floor() as i64,
            (v * self.h as f32).floor() as i64,
        )
    }

    /// Resampled to `w × h` (bilinear) if the size differs.
    pub fn resized(&self, w: usize, h: usize) -> Image {
        if (w, h) == (self.w, self.h) {
            return self.clone();
        }
        let f = |x: usize, y: usize| {
            self.sample((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32)
        };
        Image::new(w, h, self.is_color(), f)
    }

    pub fn to_color(&self) -> Image {
        match &self.px {
            Px::Color(_) => self.clone(),
            Px::Gray(_) => Image::color(self.w, self.h, |x, y| self.get(x, y)),
        }
    }

    /// RGBA8, sRGB values passed through unchanged (images are already display-encoded).
    pub fn to_rgba8(&self) -> Vec<u8> {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        (0..self.w * self.h)
            .flat_map(|i| self.get(i % self.w, i / self.w).map(q))
            .collect()
    }
}
