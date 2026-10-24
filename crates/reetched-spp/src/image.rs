//! Images stored in a project: embedded resources (a Qt header map, then
//! LZ4-compressed pixels) and texture cache entries.

use crate::qt::{QMap, QValue, read_map};
use anyhow::{Context, Result, bail};

/// Decoded pixels, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub enum Pixels {
    U8(Vec<u8>),
    U16(Vec<u16>),
    F32(Vec<f32>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// 1 (grey), 3 or 4.
    pub channels: u32,
    pub pixels: Pixels,
    /// Painter's color space flag (1 for baked and data maps).
    pub color_space: i64,
}

fn int(m: &QMap, k: &str) -> Result<i64> {
    m.get(k)
        .and_then(QValue::as_i64)
        .with_context(|| format!("image header has no {k}"))
}

fn lz4(data: &[u8], m: &QMap, raw_key: &str) -> Result<Vec<u8>> {
    let raw = int(m, raw_key)? as usize;
    match m.get("compression").and_then(QValue::as_str) {
        Some("lz4") => lz4_flex::block::decompress(data, raw).context("corrupt LZ4 image data"),
        None | Some("") | Some("none") => Ok(data[..raw.min(data.len())].to_vec()),
        // ponytail: JPEG 2000 tiles (seen only in an autosave) aren't decoded;
        // add a 16-bit JPEG 2000 decoder if real projects need them.
        Some(c) => bail!("unsupported image compression {c}"),
    }
}

/// Copies `rows` of `row_bytes` out of `raw` laid out with stride `pitch`.
fn unpitch(raw: &[u8], height: usize, row_bytes: usize, pitch: usize) -> Result<Vec<u8>> {
    if pitch == row_bytes {
        return Ok(raw[..(row_bytes * height).min(raw.len())].to_vec());
    }
    let mut out = Vec::with_capacity(row_bytes * height);
    for y in 0..height {
        let row = raw
            .get(y * pitch..y * pitch + row_bytes)
            .context("image data is shorter than its size")?;
        out.extend_from_slice(row);
    }
    Ok(out)
}

/// An embedded image resource (`resources/<sha1>`).
pub fn read_resource(b: &[u8]) -> Result<Image> {
    let (m, start) = read_map(b)?;
    let (w, h) = (int(&m, "width")? as u32, int(&m, "height")? as u32);
    let format = m.get("format").and_then(QValue::as_str).unwrap_or("");
    let (channels, bytes_per_channel, float) = match format {
        "RGBA8" | "BGRA8" => (4, 1, false),
        "RGB8" => (3, 1, false),
        "LUM8" | "L8" => (1, 1, false),
        "RGBA16" => (4, 2, false),
        "RGB16" => (3, 2, false),
        "LUM16" | "L16" => (1, 2, false),
        "RGBA16F" => (4, 2, true),
        "RGB16F" => (3, 2, true),
        "LUM16F" | "L16F" => (1, 2, true),
        "RGBA32F" => (4, 4, true),
        "RGB32F" => (3, 4, true),
        "LUM32F" | "L32F" => (1, 4, true),
        f => bail!("unsupported image format {f:?}"),
    };
    let raw = lz4(&b[start..], &m, "dataSizeRaw")?;
    let row = (w * channels * bytes_per_channel) as usize;
    let pitch = m
        .get("pitch")
        .and_then(QValue::as_i64)
        .map_or(row, |p| p as usize);
    let data = unpitch(&raw, h as usize, row, pitch)?;
    let pixels = match (bytes_per_channel, float) {
        (1, _) if format == "BGRA8" => Pixels::U8(
            data.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0], p[3]])
                .collect(),
        ),
        (1, _) => Pixels::U8(data),
        (2, false) => Pixels::U16(
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect(),
        ),
        (2, true) => Pixels::F32(
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|c| half_to_f32(u16::from_le_bytes([c[0], c[1]])))
                .collect(),
        ),
        _ => Pixels::F32(
            data.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        ),
    };
    Ok(Image {
        width: w,
        height: h,
        channels,
        pixels,
        color_space: m.get("colorSpace").and_then(QValue::as_i64).unwrap_or(0),
    })
}

/// A texture cache entry (Painter's computed channel texture): GL format.
pub fn read_cache_entry(b: &[u8]) -> Result<Image> {
    let (m, start) = read_map(b)?;
    let (w, h) = (int(&m, "width")? as u32, int(&m, "height")? as u32);
    // The pixels are a QByteArray: big-endian length, then the LZ4 block.
    let len = b
        .get(start..start + 4)
        .map(|l| u32::from_be_bytes(l.try_into().unwrap()) as usize)
        .context("texture cache entry has no data")?;
    let data = b
        .get(start + 4..start + 4 + len)
        .context("texture cache entry is shorter than its data")?;
    let raw = lz4(data, &m, "bufferSize")?;
    let n = (w * h) as usize;
    // GL internal formats seen in projects.
    const SRGB8_ALPHA8: i64 = 0x8C43;
    const RGBA8: i64 = 0x8058;
    const R8: i64 = 0x8229;
    const RG8: i64 = 0x822B;
    const RG16F: i64 = 0x822F;
    const RGBA16: i64 = 0x805B;
    const R16: i64 = 0x822A;
    const RGBA16F: i64 = 0x881A;
    const R16F: i64 = 0x822D;
    let take = |bytes: usize| -> Result<&[u8]> {
        raw.get(..n * bytes)
            .context("texture cache entry is too short")
    };
    let (channels, pixels) = match int(&m, "internalFormat")? {
        SRGB8_ALPHA8 | RGBA8 => (4, Pixels::U8(take(4)?.to_vec())),
        R8 => (1, Pixels::U8(take(1)?.to_vec())),
        // Two-channel data becomes RGB with blue 0.
        RG8 => (
            3,
            Pixels::U8(
                take(2)?
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], 0])
                    .collect(),
            ),
        ),
        RG16F => (
            3,
            Pixels::F32(
                u16s(take(4)?)
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .flat_map(|p| [half_to_f32(p[0]), half_to_f32(p[1]), 0.0])
                    .collect(),
            ),
        ),
        RGBA16 => (4, Pixels::U16(u16s(take(8)?))),
        R16 => (1, Pixels::U16(u16s(take(2)?))),
        RGBA16F => (
            4,
            Pixels::F32(u16s(take(8)?).into_iter().map(half_to_f32).collect()),
        ),
        R16F => (
            1,
            Pixels::F32(u16s(take(2)?).into_iter().map(half_to_f32).collect()),
        ),
        f => bail!("unsupported texture cache format 0x{f:X}"),
    };
    Ok(Image {
        width: w,
        height: h,
        channels,
        pixels,
        color_space: 0,
    })
}

fn u16s(b: &[u8]) -> Vec<u16> {
    b.as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

pub fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1F) as i32;
    let frac = (h & 0x3FF) as f32;
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 if frac == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        e => (1.0 + frac / 1024.0) * 2f32.powi(e - 15),
    }
}

impl Image {
    /// Encodes as a file ReEtched stores: 8/16-bit PNG, or EXR for float data.
    pub fn encode(&self) -> Result<Vec<u8>> {
        use image::{DynamicImage, ImageBuffer};
        let (w, h) = (self.width, self.height);
        let bad = || anyhow::anyhow!("image size doesn't match its data");
        let img = match (&self.pixels, self.channels) {
            (Pixels::U8(p), 1) => {
                DynamicImage::ImageLuma8(ImageBuffer::from_raw(w, h, p.clone()).ok_or_else(bad)?)
            }
            (Pixels::U8(p), 3) => {
                DynamicImage::ImageRgb8(ImageBuffer::from_raw(w, h, p.clone()).ok_or_else(bad)?)
            }
            (Pixels::U8(p), _) => {
                DynamicImage::ImageRgba8(ImageBuffer::from_raw(w, h, p.clone()).ok_or_else(bad)?)
            }
            (Pixels::U16(p), 1) => {
                DynamicImage::ImageLuma16(ImageBuffer::from_raw(w, h, p.clone()).ok_or_else(bad)?)
            }
            (Pixels::U16(p), 3) => {
                DynamicImage::ImageRgb16(ImageBuffer::from_raw(w, h, p.clone()).ok_or_else(bad)?)
            }
            (Pixels::U16(p), _) => {
                DynamicImage::ImageRgba16(ImageBuffer::from_raw(w, h, p.clone()).ok_or_else(bad)?)
            }
            (Pixels::F32(p), c) => {
                // EXR wants RGB(A) floats.
                let rgba: Vec<f32> = p
                    .chunks_exact(c as usize)
                    .flat_map(|px| match c {
                        1 => [px[0], px[0], px[0], 1.0],
                        3 => [px[0], px[1], px[2], 1.0],
                        _ => [px[0], px[1], px[2], px[3]],
                    })
                    .collect();
                DynamicImage::ImageRgba32F(ImageBuffer::from_raw(w, h, rgba).ok_or_else(bad)?)
            }
        };
        let format = if matches!(self.pixels, Pixels::F32(_)) {
            image::ImageFormat::OpenExr
        } else {
            image::ImageFormat::Png
        };
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, format)?;
        Ok(out.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_decode() {
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0), 0.0);
    }
}
