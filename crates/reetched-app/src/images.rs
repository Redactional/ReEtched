//! Image resources: raster files through `image`, SVG through resvg, and text
//! resources (stored as SVG) built from a string and a font.

use anyhow::{Context, Result, bail};
use resvg::{tiny_skia, usvg};
use std::sync::{Arc, OnceLock};

/// Longest side SVG resources are rendered at.
pub const SVG_SIZE: u32 = 2048;

/// File extensions the image pickers accept.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "tga", "tif", "tiff", "exr", "hdr", "svg",
];

/// Decodes a raster image, or renders an SVG at `SVG_SIZE`.
pub fn decode(bytes: &[u8]) -> Result<image::DynamicImage> {
    if is_svg(bytes) {
        return render_svg(bytes, SVG_SIZE).map(image::DynamicImage::ImageRgba8);
    }
    Ok(image::load_from_memory(bytes)?)
}

pub fn is_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    (head.starts_with('<') && head.contains("<svg")) || head.starts_with("<svg")
}

/// System fonts plus egui's built-in one as a fallback, loaded once.
fn fonts() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        db.load_font_data(
            egui::epaint::text::FontDefinitions::default()
                .font_data
                .get("Ubuntu-Light")
                .map(|f| (*f.font).as_ref().to_vec())
                .unwrap_or_default(),
        );
        Arc::new(db)
    })
    .clone()
}

/// Font family names available for text resources, sorted.
pub fn font_families() -> Vec<String> {
    let mut names: Vec<String> = fonts()
        .faces()
        .filter_map(|f| f.families.first().map(|(n, _)| n.clone()))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn options() -> usvg::Options<'static> {
    usvg::Options {
        fontdb: fonts(),
        font_family: "Ubuntu".into(),
        ..Default::default()
    }
}

/// Renders an SVG so its longest side is `size` pixels (straight-alpha RGBA).
pub fn render_svg(bytes: &[u8], size: u32) -> Result<image::RgbaImage> {
    let tree = usvg::Tree::from_data(bytes, &options()).context("not a readable SVG")?;
    let s = tree.size();
    let scale = size as f32 / s.width().max(s.height()).max(1e-3);
    let (w, h) = (
        ((s.width() * scale).round() as u32).max(1),
        ((s.height() * scale).round() as u32).max(1),
    );
    let mut pixmap = tiny_skia::Pixmap::new(w, h).context("SVG too large")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia stores premultiplied alpha.
    let px = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    image::RgbaImage::from_raw(w, h, px).context("SVG render size mismatch")
}

/// Settings of a text resource.
#[derive(Clone, PartialEq, Debug)]
pub struct TextStyle {
    pub text: String,
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// Line spacing as a multiple of the font size.
    pub line_height: f32,
    /// 0 left, 1 center, 2 right.
    pub align: u8,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            text: "Text".into(),
            family: "Arial".into(),
            bold: false,
            italic: false,
            line_height: 1.2,
            align: 1,
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// An SVG of white text on transparent, cropped to the text.
pub fn text_svg(style: &TextStyle) -> Result<String> {
    if style.text.trim().is_empty() {
        bail!("the text is empty");
    }
    let size = 100.0;
    let (anchor, x) = match style.align {
        0 => ("start", 0.0),
        2 => ("end", 0.0),
        _ => ("middle", 0.0),
    };
    let lines: String = style
        .text
        .lines()
        .enumerate()
        .map(|(i, line)| {
            format!(
                "<tspan x=\"{x}\" y=\"{}\">{}</tspan>",
                i as f32 * size * style.line_height,
                escape(if line.is_empty() { " " } else { line })
            )
        })
        .collect();
    let body = format!(
        "<text font-family=\"{}\" font-size=\"{size}\" font-weight=\"{}\" font-style=\"{}\" text-anchor=\"{anchor}\" fill=\"white\">{lines}</text>",
        escape(&style.family),
        if style.bold { "bold" } else { "normal" },
        if style.italic { "italic" } else { "normal" },
    );
    // Lay it out once on a huge canvas to find its extent, then crop to it.
    let probe = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100000\" height=\"100000\" viewBox=\"-50000 -50000 100000 100000\">{body}</svg>"
    );
    let tree = usvg::Tree::from_data(probe.as_bytes(), &options())?;
    let b = tree.root().abs_bounding_box();
    if b.width() <= 0.0 || b.height() <= 0.0 {
        bail!("no font could draw this text");
    }
    // The probe's viewBox put (-50000, -50000) at the canvas origin.
    let pad = size * 0.1;
    let (x0, y0) = (b.x() - 50000.0 - pad, b.y() - 50000.0 - pad);
    let (w, h) = (b.width() + 2.0 * pad, b.height() + 2.0 * pad);
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"{x0} {y0} {w} {h}\">{body}</svg>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_renders_at_the_requested_size() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect x="0" y="0" width="10" height="10" fill="#ff0000"/></svg>"##;
        assert!(is_svg(svg));
        let img = render_svg(svg, 64).unwrap();
        assert_eq!(img.dimensions(), (64, 32));
        assert_eq!(img.get_pixel(10, 16).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(50, 16).0[3], 0, "right half is empty");
        assert!(decode(svg).is_ok());
    }

    #[test]
    fn text_resources_are_cropped_white_text() {
        let style = TextStyle {
            text: "Hello".into(),
            family: "Ubuntu".into(),
            ..Default::default()
        };
        let svg = text_svg(&style).unwrap();
        let img = render_svg(svg.as_bytes(), 256).unwrap();
        let (w, h) = img.dimensions();
        assert!(w > h, "wider than tall: {w}×{h}");
        let lit = img.pixels().filter(|p| p.0[3] > 128).count();
        assert!(lit > 500, "the letters are drawn ({lit} px)");
        assert!(img.pixels().filter(|p| p.0[3] > 128).all(|p| p.0[0] > 200));
    }
}
