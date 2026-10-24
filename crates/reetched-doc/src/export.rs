//! Texture export: output templates (channel packing), padding, file writing,
//! and Painter export presets (`.spexp`).

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

/// Data an output channel can be built from (Painter's input / converted maps).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum MapSource {
    BaseColor,
    Metallic,
    Roughness,
    /// 1 − roughness.
    Glossiness,
    NormalOpenGl,
    NormalDirectX,
    Height,
    /// Baked ambient occlusion (white if none).
    AmbientOcclusion,
    Emissive,
    Opacity,
    Black,
    White,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Component {
    R,
    G,
    B,
    A,
    /// Grey / luminance.
    L,
}

impl Component {
    fn index(self) -> usize {
        match self {
            Component::R | Component::L => 0,
            Component::G => 1,
            Component::B => 2,
            Component::A => 3,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Component::R => "R",
            Component::G => "G",
            Component::B => "B",
            Component::A => "A",
            Component::L => "L",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "R" => Component::R,
            "G" => Component::G,
            "B" => Component::B,
            "A" => Component::A,
            "L" => Component::L,
            _ => return None,
        })
    }
}

/// One output channel: `dest` takes `component` of `source`.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Link {
    pub dest: Component,
    pub source: MapSource,
    pub component: Component,
}

/// One exported file.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct OutputMap {
    /// Supports `$project`, `$mesh`, `$textureSet`, `$udim`.
    pub file_name: String,
    pub links: Vec<Link>,
}

impl OutputMap {
    fn rgb(file_name: &str, source: MapSource) -> Self {
        let link = |c| Link {
            dest: c,
            source,
            component: c,
        };
        Self {
            file_name: file_name.into(),
            links: vec![link(Component::R), link(Component::G), link(Component::B)],
        }
    }

    fn gray(file_name: &str, source: MapSource) -> Self {
        Self {
            file_name: file_name.into(),
            links: vec![Link {
                dest: Component::L,
                source,
                component: Component::L,
            }],
        }
    }

    /// Packs one scalar source into each listed channel.
    fn packed(file_name: &str, channels: &[(Component, MapSource)]) -> Self {
        Self {
            file_name: file_name.into(),
            links: channels
                .iter()
                .map(|&(dest, source)| Link {
                    dest,
                    source,
                    component: Component::L,
                })
                .collect(),
        }
    }

    fn layout(&self) -> Layout {
        if self.links.iter().any(|l| l.dest == Component::A) {
            Layout::Rgba
        } else if self.links.iter().all(|l| l.dest == Component::L) {
            Layout::Gray
        } else {
            Layout::Rgb
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Gray,
    Rgb,
    Rgba,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    pub maps: Vec<OutputMap>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum FileFormat {
    Png,
    Tga,
    Tiff,
    Exr,
}

impl FileFormat {
    pub const ALL: [FileFormat; 4] = [
        FileFormat::Png,
        FileFormat::Tga,
        FileFormat::Tiff,
        FileFormat::Exr,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            FileFormat::Png => "png",
            FileFormat::Tga => "tga",
            FileFormat::Tiff => "tif",
            FileFormat::Exr => "exr",
        }
    }

    pub fn depths(self) -> &'static [BitDepth] {
        match self {
            FileFormat::Png => &[BitDepth::Eight, BitDepth::Sixteen],
            FileFormat::Tga => &[BitDepth::Eight],
            FileFormat::Tiff => &[BitDepth::Eight, BitDepth::Sixteen, BitDepth::Float32],
            // ponytail: 16-bit float EXR needs writing via the exr crate directly; add when asked for.
            FileFormat::Exr => &[BitDepth::Float32],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum BitDepth {
    Eight,
    Sixteen,
    Float32,
}

impl BitDepth {
    pub fn label(self) -> &'static str {
        match self {
            BitDepth::Eight => "8 bits",
            BitDepth::Sixteen => "16 bits",
            BitDepth::Float32 => "32 bits float",
        }
    }
}

/// Painter's padding options (minus diffusion).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Padding {
    /// Leave texels outside UV islands at the stack's default value.
    None,
    /// Extend island edges to fill the whole texture.
    Infinite,
    /// Extend edges by `pixels`; beyond that, transparent (if the format has alpha) or default.
    Dilation { pixels: u32, transparent: bool },
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ExportSettings {
    pub format: FileFormat,
    pub depth: BitDepth,
    pub padding: Padding,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            format: FileFormat::Png,
            depth: BitDepth::Eight,
            padding: Padding::Infinite,
        }
    }
}

/// Composited channels of one texture set, row 0 = top of the texture (v = 1).
pub struct ChannelData {
    pub width: u32,
    pub height: u32,
    /// Linear RGB.
    pub base_color: Vec<[f32; 3]>,
    pub metallic: Vec<f32>,
    pub roughness: Vec<f32>,
    /// Tangent-space unit vectors (OpenGL convention).
    pub normal: Vec<[f32; 3]>,
    pub height_map: Vec<f32>,
    /// Linear RGB.
    pub emissive: Vec<[f32; 3]>,
    pub opacity: Vec<f32>,
    /// Texels inside a UV island.
    pub coverage: Vec<bool>,
    /// Baked ambient occlusion (empty = none, exported as white).
    pub ambient_occlusion: Vec<f32>,
}

impl ChannelData {
    /// Value of `source`'s `component` at texel `i`, ready to store.
    /// `encode_srgb`: color data goes to an 8/16-bit file.
    fn value(&self, source: MapSource, component: Component, i: usize, encode_srgb: bool) -> f32 {
        let c = component.index().min(2);
        match source {
            MapSource::BaseColor => {
                let v = if component == Component::L {
                    let [r, g, b] = self.base_color[i];
                    0.2126 * r + 0.7152 * g + 0.0722 * b
                } else {
                    self.base_color[i][c]
                };
                if encode_srgb { linear_to_srgb(v) } else { v }
            }
            MapSource::Metallic => self.metallic[i],
            MapSource::Roughness => self.roughness[i],
            MapSource::Glossiness => 1.0 - self.roughness[i],
            MapSource::NormalOpenGl | MapSource::NormalDirectX => {
                let n = self.normal[i];
                let mut v = n[if component == Component::L { 2 } else { c }];
                if source == MapSource::NormalDirectX && component == Component::G {
                    v = -v;
                }
                v * 0.5 + 0.5
            }
            // ponytail: height 0 is stored as mid grey; revisit with Painter golden exports.
            MapSource::Height => self.height_map[i] + 0.5,
            MapSource::AmbientOcclusion => self.ambient_occlusion.get(i).copied().unwrap_or(1.0),
            MapSource::Emissive => {
                let v = if component == Component::L {
                    let [r, g, b] = self.emissive[i];
                    0.2126 * r + 0.7152 * g + 0.0722 * b
                } else {
                    self.emissive[i][c]
                };
                if encode_srgb { linear_to_srgb(v) } else { v }
            }
            MapSource::Opacity => self.opacity[i],
            MapSource::White => 1.0,
            MapSource::Black => 0.0,
        }
    }
}

pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// For each texel, the texel to copy from (`None` = outside, beyond the padding).
fn padding_map(data: &ChannelData, padding: Padding) -> Vec<Option<u32>> {
    let (w, h) = (data.width as usize, data.height as usize);
    let limit = match padding {
        Padding::None => return (0..(w * h) as u32).map(Some).collect(),
        Padding::Infinite => u32::MAX,
        Padding::Dilation { pixels, .. } => pixels,
    };
    // Multi-source BFS from every covered texel: each texel copies its nearest island texel.
    let mut source: Vec<Option<u32>> = vec![None; w * h];
    let mut dist = vec![u32::MAX; w * h];
    let mut queue = VecDeque::new();
    for (i, &covered) in data.coverage.iter().enumerate() {
        if covered {
            source[i] = Some(i as u32);
            dist[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        if dist[i] >= limit {
            continue;
        }
        let (x, y) = (i % w, i / w);
        for (dx, dy) in [
            (-1i32, 0i32),
            (1, 0),
            (0, -1),
            (0, 1),
            (-1, -1),
            (1, -1),
            (-1, 1),
            (1, 1),
        ] {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let n = ny as usize * w + nx as usize;
            if dist[n] == u32::MAX {
                dist[n] = dist[i] + 1;
                source[n] = source[i];
                queue.push_back(n);
            }
        }
    }
    if let Padding::Dilation {
        transparent: false, ..
    } = padding
    {
        // Beyond the dilation width, keep the texel's own (default) value.
        for (i, s) in source.iter_mut().enumerate() {
            s.get_or_insert(i as u32);
        }
    }
    source
}

/// Replaces `$tokens` in a file name template and strips characters Windows rejects.
pub fn expand_name(template: &str, project: &str, mesh: &str, texture_set: &str) -> String {
    let name = template
        .replace("$project", project)
        .replace("$mesh", mesh)
        .replace("$textureSet", texture_set)
        .replace("$textureset", texture_set)
        .replace("$udim", "1001")
        .replace("$colorSpace", "");
    name.chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// Writes every map of `preset` for one texture set. Returns the written files.
pub fn export_texture_set(
    preset: &ExportPreset,
    settings: &ExportSettings,
    data: &ChannelData,
    dir: &Path,
    names: (&str, &str, &str),
) -> Result<Vec<PathBuf>> {
    if !settings.format.depths().contains(&settings.depth) {
        bail!(
            "{:?} can't be written at {}",
            settings.format,
            settings.depth.label()
        );
    }
    let pad = padding_map(data, settings.padding);
    let (w, h) = (data.width, data.height);
    let encode_srgb = settings.depth != BitDepth::Float32;
    let mut written = Vec::new();
    for map in &preset.maps {
        let layout = map.layout();
        let comps = match layout {
            Layout::Gray => 1,
            Layout::Rgb => 3,
            Layout::Rgba => 4,
        };
        let mut px = vec![0.0f32; (w * h) as usize * comps];
        for (i, src) in pad.iter().enumerate() {
            let out = &mut px[i * comps..(i + 1) * comps];
            if comps == 4 {
                out[3] = 1.0;
            }
            let Some(src) = *src else {
                if comps == 4 {
                    out[3] = 0.0;
                }
                continue;
            };
            for link in &map.links {
                let slot = if layout == Layout::Gray {
                    0
                } else {
                    link.dest.index()
                };
                out[slot] = data.value(link.source, link.component, src as usize, encode_srgb);
            }
        }
        let file = dir.join(format!(
            "{}.{}",
            expand_name(&map.file_name, names.0, names.1, names.2),
            settings.format.extension()
        ));
        write_image(&file, settings, w, h, comps, &px)
            .with_context(|| format!("could not write {}", file.display()))?;
        written.push(file);
    }
    Ok(written)
}

fn write_image(
    path: &Path,
    s: &ExportSettings,
    w: u32,
    h: u32,
    comps: usize,
    px: &[f32],
) -> Result<()> {
    use image::DynamicImage as D;
    let q8 = || {
        px.iter()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
            .collect::<Vec<_>>()
    };
    let q16 = || {
        px.iter()
            .map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16)
            .collect::<Vec<_>>()
    };
    let bad = || anyhow::anyhow!("image buffer size mismatch");
    let img = match (s.depth, comps) {
        (BitDepth::Eight, 1) => {
            D::ImageLuma8(image::ImageBuffer::from_raw(w, h, q8()).ok_or_else(bad)?)
        }
        (BitDepth::Eight, 3) => {
            D::ImageRgb8(image::ImageBuffer::from_raw(w, h, q8()).ok_or_else(bad)?)
        }
        (BitDepth::Eight, _) => {
            D::ImageRgba8(image::ImageBuffer::from_raw(w, h, q8()).ok_or_else(bad)?)
        }
        (BitDepth::Sixteen, 1) => {
            D::ImageLuma16(image::ImageBuffer::from_raw(w, h, q16()).ok_or_else(bad)?)
        }
        (BitDepth::Sixteen, 3) => {
            D::ImageRgb16(image::ImageBuffer::from_raw(w, h, q16()).ok_or_else(bad)?)
        }
        (BitDepth::Sixteen, _) => {
            D::ImageRgba16(image::ImageBuffer::from_raw(w, h, q16()).ok_or_else(bad)?)
        }
        (BitDepth::Float32, 4) => {
            D::ImageRgba32F(image::ImageBuffer::from_raw(w, h, px.to_vec()).ok_or_else(bad)?)
        }
        (BitDepth::Float32, _) => {
            // No grey float image type: write grey as RGB.
            let rgb = px
                .chunks_exact(comps)
                .flat_map(|p| {
                    if comps == 1 {
                        [p[0]; 3]
                    } else {
                        [p[0], p[1], p[2]]
                    }
                })
                .collect();
            D::ImageRgb32F(image::ImageBuffer::from_raw(w, h, rgb).ok_or_else(bad)?)
        }
    };
    let format = match s.format {
        FileFormat::Png => image::ImageFormat::Png,
        FileFormat::Tga => image::ImageFormat::Tga,
        FileFormat::Tiff => image::ImageFormat::Tiff,
        FileFormat::Exr => image::ImageFormat::OpenExr,
    };
    img.save_with_format(path, format)?;
    Ok(())
}

/// Built-in presets, matching the engines' expected maps and conventions.
pub fn builtin_presets() -> Vec<ExportPreset> {
    use Component::*;
    use MapSource::*;
    let p = |name: &str, maps| ExportPreset {
        name: name.into(),
        maps,
    };
    let orm = |file| {
        OutputMap::packed(
            file,
            &[(R, AmbientOcclusion), (G, Roughness), (B, Metallic)],
        )
    };
    vec![
        p(
            "PBR Metallic Roughness",
            vec![
                OutputMap::rgb("$mesh_$textureSet_BaseColor", BaseColor),
                OutputMap::gray("$mesh_$textureSet_Metallic", Metallic),
                OutputMap::gray("$mesh_$textureSet_Roughness", Roughness),
                OutputMap::rgb("$mesh_$textureSet_Normal", NormalOpenGl),
                OutputMap::gray("$mesh_$textureSet_Height", Height),
                OutputMap::rgb("$mesh_$textureSet_Emissive", Emissive),
                OutputMap::gray("$mesh_$textureSet_Opacity", Opacity),
            ],
        ),
        p(
            "Unreal Engine (Packed)",
            vec![
                OutputMap::rgb("T_$textureSet_BaseColor", BaseColor),
                OutputMap::rgb("T_$textureSet_Normal", NormalDirectX),
                orm("T_$textureSet_OcclusionRoughnessMetallic"),
                OutputMap::rgb("T_$textureSet_Emissive", Emissive),
            ],
        ),
        p(
            "Unity HDRP (Metallic Standard)",
            vec![
                OutputMap::rgb("$textureSet_BaseMap", BaseColor),
                OutputMap::packed(
                    "$textureSet_MaskMap",
                    &[
                        (R, Metallic),
                        (G, AmbientOcclusion),
                        (B, Black),
                        (A, Glossiness),
                    ],
                ),
                OutputMap::rgb("$textureSet_Normal", NormalOpenGl),
            ],
        ),
        p(
            "Unity URP (Metallic Standard)",
            vec![
                OutputMap::rgb("$textureSet_BaseMap", BaseColor),
                OutputMap::packed(
                    "$textureSet_MetallicSmoothness",
                    &[(R, Metallic), (G, Metallic), (B, Metallic), (A, Glossiness)],
                ),
                OutputMap::rgb("$textureSet_Normal", NormalOpenGl),
            ],
        ),
        p(
            "glTF PBR Metal Roughness",
            vec![
                OutputMap {
                    file_name: "$mesh_$textureSet_baseColor".into(),
                    links: [R, G, B]
                        .map(|c| Link {
                            dest: c,
                            source: BaseColor,
                            component: c,
                        })
                        .into_iter()
                        .chain([Link {
                            dest: A,
                            source: Opacity,
                            component: L,
                        }])
                        .collect(),
                },
                orm("$mesh_$textureSet_occlusionRoughnessMetallic"),
                OutputMap::rgb("$mesh_$textureSet_normal", NormalOpenGl),
                OutputMap::rgb("$mesh_$textureSet_emissive", Emissive),
            ],
        ),
        p(
            "Blender (Principled BSDF)",
            vec![
                OutputMap::rgb("$mesh_$textureSet_Base_Color", BaseColor),
                OutputMap::gray("$mesh_$textureSet_Metallic", Metallic),
                OutputMap::gray("$mesh_$textureSet_Roughness", Roughness),
                OutputMap::rgb("$mesh_$textureSet_Normal", NormalOpenGl),
                OutputMap::gray("$mesh_$textureSet_Height", Height),
            ],
        ),
        p(
            "Godot (ORM)",
            vec![
                OutputMap::rgb("$textureSet_albedo", BaseColor),
                orm("$textureSet_orm"),
                OutputMap::rgb("$textureSet_normal", NormalOpenGl),
            ],
        ),
    ]
}

// ---------------------------------------------------------------- Painter .spexp
//
// Uses the export-preset JSON documented for Painter's Python export API
// (`exportPresets` → `maps` → `channels` with srcMapType/srcMapName).
// [unverified] that `.spexp` files on disk use exactly this layout; check against real files.

fn painter_source(s: MapSource) -> Option<(&'static str, &'static str)> {
    Some(match s {
        MapSource::BaseColor => ("documentMap", "basecolor"),
        MapSource::Metallic => ("documentMap", "metallic"),
        MapSource::Roughness => ("documentMap", "roughness"),
        MapSource::Height => ("documentMap", "height"),
        MapSource::Glossiness => ("virtualMap", "Glossiness"),
        MapSource::NormalOpenGl => ("virtualMap", "Normal_OpenGL"),
        MapSource::NormalDirectX => ("virtualMap", "Normal_DirectX"),
        MapSource::AmbientOcclusion => ("virtualMap", "Mixed_AO"),
        MapSource::Emissive => ("documentMap", "emissive"),
        MapSource::Opacity => ("documentMap", "opacity"),
        MapSource::Black | MapSource::White => return None,
    })
}

fn from_painter_source(kind: &str, name: &str) -> Option<MapSource> {
    Some(match (kind, name.to_ascii_lowercase().as_str()) {
        ("documentMap", "basecolor") => MapSource::BaseColor,
        ("documentMap", "metallic") => MapSource::Metallic,
        ("documentMap", "roughness") => MapSource::Roughness,
        ("documentMap", "height") => MapSource::Height,
        ("documentMap", "emissive") => MapSource::Emissive,
        ("documentMap", "opacity") => MapSource::Opacity,
        ("documentMap", "normal") | ("virtualMap", "normal_opengl") => MapSource::NormalOpenGl,
        ("virtualMap", "normal_directx") => MapSource::NormalDirectX,
        ("virtualMap", "glossiness") => MapSource::Glossiness,
        ("virtualMap", "mixed_ao")
        | ("meshMap", "ambient_occlusion")
        | ("documentMap", "ambientocclusion") => MapSource::AmbientOcclusion,
        _ => return None,
    })
}

pub fn presets_to_spexp(presets: &[ExportPreset]) -> Value {
    let presets: Vec<Value> = presets
        .iter()
        .map(|p| {
            let maps: Vec<Value> = p
                .maps
                .iter()
                .map(|m| {
                    let channels: Vec<Value> = m
                        .links
                        .iter()
                        .filter_map(|l| {
                            let (kind, name) = painter_source(l.source)?;
                            Some(json!({
                                "destChannel": l.dest.name(),
                                "srcChannel": l.component.name(),
                                "srcMapType": kind,
                                "srcMapName": name,
                            }))
                        })
                        .collect();
                    json!({ "fileName": m.file_name, "channels": channels })
                })
                .collect();
            json!({ "name": p.name, "maps": maps })
        })
        .collect();
    json!({ "exportPresets": presets })
}

/// Reads Painter export presets. Returns the presets and a list of things that couldn't be mapped.
pub fn presets_from_spexp(value: &Value) -> Result<(Vec<ExportPreset>, Vec<String>)> {
    let list = value
        .get("exportPresets")
        .and_then(Value::as_array)
        .context("no \"exportPresets\" list found")?;
    let mut warnings = Vec::new();
    let mut presets = Vec::new();
    for p in list {
        let name = p
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Imported preset")
            .to_string();
        let mut maps = Vec::new();
        for m in p
            .get("maps")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let file_name = m
                .get("fileName")
                .and_then(Value::as_str)
                .unwrap_or("$textureSet")
                .to_string();
            let mut links = Vec::new();
            for c in m
                .get("channels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let s = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("");
                let (Some(dest), Some(component)) = (
                    Component::parse(s("destChannel")),
                    Component::parse(s("srcChannel")),
                ) else {
                    warnings.push(format!("{name} / {file_name}: unreadable channel entry"));
                    continue;
                };
                let source =
                    from_painter_source(s("srcMapType"), s("srcMapName")).unwrap_or_else(|| {
                        warnings.push(format!(
                        "{name} / {file_name}: source {}/{} isn't supported yet, exported as white",
                        s("srcMapType"),
                        s("srcMapName")
                    ));
                        MapSource::White
                    });
                links.push(Link {
                    dest,
                    source,
                    component,
                });
            }
            if !links.is_empty() {
                maps.push(OutputMap { file_name, links });
            }
        }
        presets.push(ExportPreset { name, maps });
    }
    Ok((presets, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4×1 texture: texels 1 and 2 covered.
    fn data() -> ChannelData {
        ChannelData {
            width: 4,
            height: 1,
            base_color: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]],
            metallic: vec![0.0, 1.0, 0.0, 0.0],
            roughness: vec![0.5, 0.25, 0.75, 0.5],
            normal: vec![[0.0, 0.0, 1.0]; 4],
            height_map: vec![0.0; 4],
            emissive: vec![[0.0; 3]; 4],
            opacity: vec![1.0; 4],
            coverage: vec![false, true, true, false],
            ambient_occlusion: Vec::new(),
        }
    }

    #[test]
    fn infinite_padding_copies_nearest_island_texel() {
        let map = padding_map(&data(), Padding::Infinite);
        assert_eq!(map, [Some(1), Some(1), Some(2), Some(2)]);
    }

    #[test]
    fn transparent_dilation_stops_at_width() {
        let mut d = data();
        d.width = 6;
        d.base_color.resize(6, [0.0; 3]);
        d.coverage = vec![false, false, false, true, false, false];
        let map = padding_map(
            &d,
            Padding::Dilation {
                pixels: 1,
                transparent: true,
            },
        );
        assert_eq!(map, [None, None, Some(3), Some(3), Some(3), None]);
    }

    #[test]
    fn unreal_preset_packs_orm_and_flips_normal_green() {
        let dir = std::env::temp_dir().join("reetched-export-test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut d = data();
        d.normal[1] = [0.0, 0.6, 0.8];
        let preset = &builtin_presets()[1];
        let files = export_texture_set(
            preset,
            &ExportSettings::default(),
            &d,
            &dir,
            ("p", "m", "Set"),
        )
        .unwrap();
        assert_eq!(files.len(), 4, "base, normal, ORM, emissive");
        let orm = image::open(&files[2]).unwrap().to_rgb8();
        assert_eq!(orm.get_pixel(1, 0).0, [255, 64, 255]); // AO white, roughness 0.25, metal 1
        assert_eq!(
            orm.get_pixel(0, 0).0,
            [255, 64, 255],
            "padding copies the island texel"
        );
        let normal = image::open(&files[1]).unwrap().to_rgb8();
        assert_eq!(normal.get_pixel(1, 0).0[1], 51); // DirectX: G = (-0.6 * 0.5 + 0.5) * 255
        let base = image::open(&files[0]).unwrap().to_rgb8();
        assert_eq!(base.get_pixel(2, 0).0, [0, 0, 255]);
    }

    #[test]
    fn float_exr_keeps_linear_values() {
        let dir = std::env::temp_dir().join("reetched-export-test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut d = data();
        d.base_color[1] = [0.5, 0.25, 0.125];
        let preset = &builtin_presets()[0];
        let settings = ExportSettings {
            format: FileFormat::Exr,
            depth: BitDepth::Float32,
            padding: Padding::None,
        };
        let files = export_texture_set(preset, &settings, &d, &dir, ("p", "m", "exr")).unwrap();
        let img = image::open(&files[0]).unwrap().to_rgb32f();
        assert_eq!(img.get_pixel(1, 0).0, [0.5, 0.25, 0.125]);
    }

    #[test]
    fn spexp_round_trip() {
        let presets = builtin_presets();
        let (back, warnings) = presets_from_spexp(&presets_to_spexp(&presets)).unwrap();
        // HDRP's "Black" detail mask has no Painter equivalent, so it's dropped on export.
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(back[1], presets[1]);
        assert_eq!(back.len(), presets.len());
    }

    #[test]
    fn names_expand_and_sanitize() {
        assert_eq!(
            expand_name("$mesh_$textureSet_$udim", "p", "ball", "Mat:1"),
            "ball_Mat_1_1001"
        );
    }
}
