//! `.spp` → ReEtched document: texture sets, channels, the layer stack
//! (folders, fill and paint layers, blending, opacity, masks), bitmaps and
//! baked mesh maps, plus Painter's own computed channels to check against.
//!
//! Painter's numeric codes (channel bits, blend modes, projections) were
//! matched up from the user's own projects; see docs/spp-format.md.

use crate::{container::Container, document, image, strokes::PaintJob};
use anyhow::{Context, Result};
use reetched_doc::{
    BlendMode, Channel, ChannelBlend, Document, Fill, Mask, MaskBackground, MeshMap, Node,
    NodeKind, Projection, ProjectionMode, ResourceId, ResourceInfo, Resources, TextureSet,
};
use reetched_substance::bridge::{Imported, Reference};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Painter's channel bit → ours (the metal/rough channels seen in projects).
fn channel_bit(bit: u64) -> Option<Channel> {
    Some(match bit {
        1 => Channel::BaseColor,
        2 => Channel::Height,
        128 => Channel::Roughness,
        8192 => Channel::Metallic,
        4_194_304 => Channel::Normal,
        _ => return None,
    })
}

/// Channels in a channel bit mask; all bits set = every channel.
fn channels(mask: u64, all: &[Channel]) -> Vec<Channel> {
    if mask == u64::MAX {
        return all.to_vec();
    }
    (0..64)
        .map(|b| 1u64 << b)
        .filter(|b| mask & b != 0)
        .filter_map(channel_bit)
        .collect()
}

/// Painter's stored blend mode number → ours, and whether the match is confirmed.
fn blend_mode(n: i64) -> Option<(BlendMode, bool)> {
    Some(match n {
        0 => (BlendMode::Disable, true),
        1 => (BlendMode::Replace, true),
        2 => (BlendMode::Normal, true),
        // A baked AO bitmap at 20 % over a whole shirt: darkening.
        4 => (BlendMode::Multiply, true),
        // ponytail: 9, 10 and 11 are matched from layer names (highlights,
        // edge highlight / height noise, shadows) only; confirm in Painter.
        9 => (BlendMode::Screen, false),
        10 => (BlendMode::LinearDodge, false),
        11 => (BlendMode::Multiply, false),
        25 => (BlendMode::Passthrough, true),
        _ => return None,
    })
}

fn int(v: &Value, k: &str) -> i64 {
    v[k].as_i64()
        .or_else(|| v[k].as_u64().map(|x| x as i64))
        .unwrap_or(0)
}

fn mask_bits(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or(0)
}

fn float(v: &Value, k: &str, default: f32) -> f32 {
    v[k].as_f64().map_or(default, |x| x as f32)
}

fn floats<const N: usize>(v: &Value, k: &str) -> Option<[f32; N]> {
    let a = v[k].as_array()?;
    let mut out = [0.0; N];
    for (o, x) in out.iter_mut().zip(a) {
        *o = x.as_f64()? as f32;
    }
    Some(out)
}

fn items(v: &Value) -> &[Value] {
    v["items"].as_array().map_or(&[], |a| a.as_slice())
}

fn ty(v: &Value) -> &str {
    v["@type"].as_str().unwrap_or("")
}

/// The resource hash in a Painter resource URL ("/Name?version=<sha1>").
fn url_hash(url: &str) -> Option<&str> {
    url.split("version=")
        .nth(1)
        .map(|s| s.trim_end_matches('&'))
}

/// The display name in a resource URL.
fn url_name(url: &str) -> &str {
    url.trim_start_matches('/').split('?').next().unwrap_or(url)
}

struct Ctx<'a> {
    c: &'a Container,
    doc: Document,
    resources: Resources,
    log: Vec<String>,
    /// Resource hash → converted image.
    images: HashMap<String, Option<ResourceId>>,
    /// Dataset names in `resources/`.
    resource_names: Vec<String>,
    /// Paint actions to replay, in order.
    jobs: Vec<PaintJob>,
    /// Set being converted (index and name), and its channels.
    set_index: usize,
    set: String,
    set_channels: Vec<Channel>,
    /// Counts of things not carried over, reported once per set.
    skipped: HashMap<&'static str, usize>,
}

impl Ctx<'_> {
    fn note(&mut self, what: &'static str) {
        *self.skipped.entry(what).or_default() += 1;
    }

    /// Converts an embedded image resource (by URL) into a project resource.
    fn image(&mut self, url: &str) -> Option<ResourceId> {
        let hash = url_hash(url)?.to_owned();
        if let Some(r) = self.images.get(&hash) {
            return r.clone();
        }
        let result = (|| -> Result<ResourceId> {
            let name = self
                .resource_names
                .iter()
                .find(|n| n.starts_with(&hash))
                .with_context(|| format!("{url} is not in the project"))?;
            let bytes = self.c.bytes(&format!("resources/{name}"))?;
            let img = image::read_resource(&bytes)?;
            let id = self.resources.add(img.encode()?);
            let ext = if matches!(img.pixels, image::Pixels::F32(_)) {
                "exr"
            } else {
                "png"
            };
            self.doc.resources.insert(
                id.clone(),
                ResourceInfo {
                    name: format!("{}.{ext}", url_name(url)),
                },
            );
            Ok(id)
        })();
        let id = match result {
            Ok(id) => Some(id),
            Err(e) => {
                self.log
                    .push(format!("{}: image {}: {e:#}", self.set, url_name(url)));
                None
            }
        };
        self.images.insert(hash, id.clone());
        id
    }

    /// Painter lists layers bottom first; ReEtched top first.
    fn stack(&mut self, stack: &Value) -> Vec<Node> {
        items(stack)
            .iter()
            .rev()
            .filter(|l| !l.is_null())
            .map(|l| self.layer(l))
            .collect()
    }

    fn layer(&mut self, l: &Value) -> Node {
        let name = l["label"].as_str().unwrap_or("Layer").to_owned();
        let mut node = if ty(l) == "DataLayerGroup" {
            let mut f = self.doc.new_folder(name.clone());
            let kids = if l["subStack"].is_object() {
                self.stack(&l["subStack"])
            } else {
                Vec::new()
            };
            if let NodeKind::Folder { children } = &mut f.kind {
                *children = kids;
            }
            f
        } else {
            self.content(l, &name)
        };
        node.visible = int(l, "enabled") != 0;
        for b in l["perChannelBlending"].as_array().into_iter().flatten() {
            let mode = int(b, "blendingMode");
            let (m, sure) = blend_mode(mode).unwrap_or_else(|| {
                self.log.push(format!(
                    "{}/{name}: blend mode #{mode} is unknown; using Normal",
                    self.set
                ));
                (BlendMode::Normal, true)
            });
            if !sure {
                self.note("layers with an unconfirmed blend mode");
            }
            let opacity = float(b, "opacity", 1.0).clamp(0.0, 1.0);
            // Bits for channels the set doesn't have are Painter's presets; skip them.
            let bits = mask_bits(b, "channelTypes");
            for c in channels(bits, &self.set_channels)
                .into_iter()
                .filter(|c| self.set_channels.contains(c))
            {
                node.blend.insert(c, ChannelBlend { mode: m, opacity });
            }
        }
        if !l["maskActions"].is_null() {
            let background = if int(l, "maskInitial") == 0 {
                MaskBackground::Black
            } else {
                MaskBackground::White
            };
            let mut mask = Mask::new(background);
            mask.enabled = int(l, "maskEnabled") != 0;
            self.mask_actions(&l["maskActions"], node.id);
            node.mask = Some(mask);
        }
        node
    }

    /// Paint actions become jobs; counts what isn't converted.
    fn mask_actions(&mut self, actions: &Value, node: reetched_doc::NodeId) {
        for a in items(actions).iter().filter(|a| !a.is_null()) {
            match ty(a) {
                "DataActionGroup" => self.mask_actions(&a["subStack"], node),
                "DataActionPaint" | "DataActionEraser" => self.job(a, node, true),
                "DataActionSmudge" => self.note("smudge strokes (not replayed)"),
                "DataActionFill" => self.note("fills inside masks"),
                "DataActionDecal" => self.note("mask decals"),
                t if t.starts_with("DataActionFilter") => self.note("mask filters and generators"),
                _ => self.note("other mask effects"),
            }
        }
    }

    fn job(&mut self, action: &Value, node: reetched_doc::NodeId, mask: bool) {
        let all = self.set_channels.clone();
        let job = crate::strokes::read(action, self.set_index, node, mask, &|m| channels(m, &all));
        if !job.strokes.is_empty() {
            self.jobs.push(job);
        }
    }

    /// A layer's content: the first fill makes a fill layer, otherwise a paint
    /// layer whose strokes are replayed.
    fn content(&mut self, l: &Value, name: &str) -> Node {
        let mut fill_action = None;
        let mut paint = Vec::new();
        self.scan_actions(&l["actions"], &mut fill_action, &mut paint);
        match fill_action {
            Some(f) => {
                if !paint.is_empty() {
                    self.note("paint on fill layers (not replayed)");
                }
                let fill = self.fill(&f);
                self.doc.new_fill(name.to_owned(), fill)
            }
            None => {
                let node = self.doc.new_paint(name.to_owned());
                for a in &paint {
                    self.job(a, node.id, false);
                }
                node
            }
        }
    }

    /// Finds the first fill and the paint actions; counts the effects that aren't converted.
    fn scan_actions(&mut self, actions: &Value, fill: &mut Option<Value>, paint: &mut Vec<Value>) {
        for a in items(actions).iter().filter(|a| !a.is_null()) {
            match ty(a) {
                "DataActionGroup" => self.scan_actions(&a["subStack"], fill, paint),
                "DataActionFill" if fill.is_none() => *fill = Some(a.clone()),
                "DataActionFill" => self.note("extra fill effects in a layer"),
                "DataActionPaint" | "DataActionEraser" => paint.push(a.clone()),
                "DataActionSmudge" => self.note("smudge strokes (not replayed)"),
                t if t.starts_with("DataActionFilter") => self.note("layer filters"),
                _ => self.note("other layer effects"),
            }
        }
    }

    fn fill(&mut self, f: &Value) -> Fill {
        let mut fill = Fill::new([0.5; 3], 0.0, 0.5);
        for s in fill.channels.values_mut() {
            s.enabled = false;
        }
        fill.projection = projection(f);
        if !matches!(int(f, "projection"), 0 | 2 | 6 | 7) {
            self.note("fill projections ReEtched doesn't map yet (used UV)");
        }
        for src in f["sources"].as_array().into_iter().flatten() {
            if src.is_null() {
                continue;
            }
            let chans = channels(mask_bits(src, "channelTypes"), &self.set_channels);
            match ty(src) {
                "DataSourceUniform" => {
                    let color = floats::<3>(src, "color").unwrap_or([0.5; 3]);
                    for c in chans {
                        let s = fill.channels.entry(c).or_insert_with(|| default_source(c));
                        s.enabled = true;
                        s.bitmap = None;
                        s.value = match c {
                            c if c.is_color() => color,
                            Channel::Normal => {
                                let n = color.map(|x| x * 2.0 - 1.0);
                                let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
                                n.map(|x| x / l)
                            }
                            _ => [color[0]; 3],
                        };
                    }
                }
                "DataSourceBitmap" => {
                    let url = src["bitmap"]["urlToBitmapRes"].as_str().unwrap_or("");
                    let id = self.image(url);
                    for c in chans {
                        let s = fill.channels.entry(c).or_insert_with(|| default_source(c));
                        s.enabled = id.is_some();
                        s.bitmap = id.clone();
                    }
                }
                "DataSourceProcedural" => self.note("procedural (.sbsar) fill channels"),
                "DataSourceText" => self.note("text fills"),
                _ => self.note("other fill sources"),
            }
        }
        fill
    }
}

fn default_source(c: Channel) -> reetched_doc::ChannelSource {
    reetched_doc::ChannelSource {
        enabled: false,
        value: c.empty_value(),
        bitmap: None,
        color_space: Default::default(),
        ocio: None,
    }
}

/// A fill's projection: UV (with its tiling) or the 3D modes.
fn projection(f: &Value) -> Projection {
    let mut p = Projection::default();
    let uv = &f["uvTransformation"];
    if uv.is_object() {
        // Painter scales and rotates the image about its center, which the
        // translation moves from (0.5, 0.5): tex = R(s (uv - c)) + 0.5 with
        // c = 0.5 + t. Ours is tex = R(s uv) + offset.
        let s = floats::<2>(uv, "scaleFactors").unwrap_or([1.0, 1.0]);
        let t = floats::<2>(uv, "translation").unwrap_or([0.0, 0.0]);
        // ponytail: rotation units (degrees) not yet seen non-zero in a project.
        p.rotation_deg = float(uv, "rotation", 0.0);
        let (sn, cs) = p.rotation_deg.to_radians().sin_cos();
        let c = [s[0] * (0.5 + t[0]), s[1] * (0.5 + t[1])];
        p.scale = s;
        p.offset = [0.5 - (cs * c[0] - sn * c[1]), 0.5 - (sn * c[0] + cs * c[1])];
    }
    // Wrap 0 = the image appears once.
    p.repeat = int(f, "uvSamplingWrap") != 0;
    p.mode = match int(f, "projection") {
        2 => ProjectionMode::Triplanar,
        7 => ProjectionMode::Planar,
        _ => ProjectionMode::Uv,
    };
    if p.mode != ProjectionMode::Uv {
        p.position = floats::<3>(f, "dddPosition").unwrap_or([0.0; 3]);
        p.rotation3d_deg = floats::<3>(f, "dddEulerAngles")
            .unwrap_or([0.0; 3])
            .map(f32::to_degrees);
        p.size = floats::<3>(f, "dddSize").map_or(1.0, |s| s[0]);
        p.hardness = float(f, "sideHardness", 0.5);
    }
    p
}

/// Baked mesh map by its resource name ("Ambient Occlusion Map from Mesh Rings").
fn mesh_map(name: &str) -> Option<MeshMap> {
    let n = name.to_ascii_lowercase();
    Some(if n.contains("world space normal") {
        MeshMap::WorldNormal
    } else if n.contains("normal") {
        MeshMap::Normal
    } else if n.contains("ambient occlusion") || n.ends_with("_ao") {
        MeshMap::AmbientOcclusion
    } else if n.contains("curvature") || n.ends_with("_curve") || n.ends_with("_curvature") {
        MeshMap::Curvature
    } else if n.contains("position") {
        MeshMap::Position
    } else if n.contains("thickness") {
        MeshMap::Thickness
    } else if n.contains("color map") || n.contains(" id") {
        MeshMap::Id
    } else if n.contains("height") {
        MeshMap::Height
    } else if n.contains("opacity") {
        MeshMap::Opacity
    } else {
        return None;
    })
}

/// The mesh part for texture set `name`: same name, else a name containing
/// the other ("Body" / "BodyMaterial"), else the only part.
fn match_part(parts: &[String], name: &str, sets: usize) -> Option<usize> {
    let lower = |s: &str| s.to_ascii_lowercase();
    if let Some(i) = parts.iter().position(|p| p == name) {
        return Some(i);
    }
    let (n, mut hits) = (lower(name), Vec::new());
    for (i, p) in parts.iter().enumerate() {
        let p = lower(p);
        if !n.is_empty() && (p.contains(&n) || n.contains(&p)) {
            hits.push(i);
        }
    }
    match hits[..] {
        [i] => Some(i),
        _ if parts.len() == 1 && sets == 1 => Some(0),
        _ => None,
    }
}

/// A converted project, and the paint strokes still to replay onto it.
pub struct Converted {
    pub imported: Imported,
    pub jobs: Vec<PaintJob>,
}

/// Converts the project at `path`.
pub fn convert(path: &Path) -> Result<Converted> {
    let c = Container::open(path)?;
    let (version, tree) = document::decode(&c.bytes("paint/document.bin")?)?;
    let mesh = reetched_io::assbin::read_assbin(&c.bytes("paint/scene.assbin")?)
        .context("reading the project's mesh")?;
    let part_names: Vec<String> = mesh.parts.iter().map(|p| p.material.clone()).collect();
    let mut cx = Ctx {
        c: &c,
        doc: Document::new(None, &part_names),
        resources: Resources::default(),
        log: Vec::new(),
        images: HashMap::new(),
        resource_names: c.datasets("resources").unwrap_or_default(),
        set: String::new(),
        set_channels: Vec::new(),
        skipped: HashMap::new(),
        jobs: Vec::new(),
        set_index: 0,
    };
    cx.doc.mesh_source = Some(path.display().to_string());
    let mut references = Vec::new();
    let materials = tree["materials"].as_array().cloned().unwrap_or_default();
    for m in &materials {
        let name = m["sceneMaterialName"].as_str().unwrap_or("").to_owned();
        let Some(index) = match_part(&part_names, &name, materials.len()) else {
            cx.log.push(format!(
                "texture set {name} has no matching mesh part; skipped"
            ));
            continue;
        };
        cx.set = name.clone();
        cx.set_index = index;
        cx.skipped.clear();
        let stack = &m["stacks"][0];
        let mut set_channels = Vec::new();
        for ch in stack["channels"].as_array().into_iter().flatten() {
            match channel_bit(int(ch, "type") as u64) {
                Some(c) => set_channels.push(c),
                None => cx.log.push(format!(
                    "{name}: channel type {} isn't mapped yet; left out",
                    int(ch, "type")
                )),
            }
        }
        cx.set_channels = set_channels.clone();
        let nodes = cx.stack(&stack["stack"]);
        let res =
            floats::<2>(m, "resolutionLog2").map_or(2048, |r| 1u32 << (r[0] as u32).clamp(8, 13));
        let mut set = TextureSet::new(name.clone(), nodes);
        set.resolution = res;
        set.extra_channels = set_channels
            .iter()
            .copied()
            .filter(|c| !c.is_core())
            .collect();
        // Baked mesh maps.
        let mut maps: Vec<String> = m["additionalMaps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["urlToBitmapRes"].as_str().map(str::to_owned))
            .collect();
        maps.extend(
            m["normalMapBitmap"]["urlToBitmapRes"]
                .as_str()
                .map(str::to_owned),
        );
        for url in maps {
            let Some(kind) = mesh_map(url_name(&url)) else {
                cx.log.push(format!(
                    "{name}: mesh map {} not recognized",
                    url_name(&url)
                ));
                continue;
            };
            if let Some(id) = cx.image(&url) {
                set.mesh_maps.insert(kind, id);
            }
        }
        cx.doc.texture_sets[index] = set;
        let mut skipped: Vec<_> = cx.skipped.drain().collect();
        skipped.sort();
        for (what, n) in skipped {
            cx.log.push(format!("{name}: {n} {what}"));
        }
        // Painter's own computed channels for this set.
        let uid = m["sceneMaterialUID"].as_u64().unwrap_or(0).to_string();
        references.extend(references_for(&c, &uid, index));
    }
    cx.log.insert(
        0,
        format!("Painter document version {version}; layers, fills, blending and masks converted"),
    );
    let strokes: usize = cx.jobs.iter().map(|j| j.strokes.len()).sum();
    cx.log.insert(
        1,
        format!(
            "{} paint actions with {strokes} strokes to replay",
            cx.jobs.len()
        ),
    );
    let Ctx {
        doc,
        resources,
        log,
        jobs,
        ..
    } = cx;
    Ok(Converted {
        imported: Imported {
            doc,
            mesh,
            mesh_path: path.to_owned(),
            resources,
            log,
            references,
        },
        jobs,
    })
}

/// Painter's cached final channels of texture set `uid`, as references.
fn references_for(c: &Container, uid: &str, set: usize) -> Vec<Reference> {
    let dir = format!("texture-cache/{uid}");
    let Some(stack) = c.groups(&dir).ok().and_then(|g| g.into_iter().next()) else {
        return Vec::new();
    };
    let dir = format!("{dir}/{stack}");
    let mut out = Vec::new();
    for entry in c.datasets(&dir).unwrap_or_default() {
        let Some(channel) = entry.parse::<u64>().ok().and_then(channel_bit) else {
            continue;
        };
        let Ok(img) = c
            .bytes(&format!("{dir}/{entry}"))
            .and_then(|b| image::read_cache_entry(&b))
        else {
            continue;
        };
        let ch = img.channels as usize;
        let px = |i: usize| -> [f32; 3] {
            let get = |k: usize| match &img.pixels {
                image::Pixels::U8(p) => p[i * ch + k.min(ch - 1)] as f32 / 255.0,
                image::Pixels::U16(p) => p[i * ch + k.min(ch - 1)] as f32 / 65535.0,
                image::Pixels::F32(p) => p[i * ch + k.min(ch - 1)],
            };
            match channel {
                Channel::BaseColor | Channel::Normal => [get(0), get(1), get(2)],
                _ => [get(0); 3],
            }
        };
        let (w, h) = (img.width as usize, img.height as usize);
        // GL textures store the bottom row first.
        let pixels = (0..h)
            .rev()
            .flat_map(|y| (0..w).map(move |x| y * w + x))
            .map(px)
            .collect();
        out.push(Reference {
            set,
            channel,
            width: img.width,
            height: img.height,
            pixels,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_masks_and_urls() {
        let all = [Channel::BaseColor, Channel::Roughness];
        assert_eq!(channels(u64::MAX, &all), all.to_vec());
        assert_eq!(
            channels(1 | 8192 | 4_194_304, &all),
            vec![Channel::BaseColor, Channel::Metallic, Channel::Normal]
        );
        let url = "/Ambient Occlusion Map from Mesh Rings?version=3363d5bc";
        assert_eq!(url_hash(url), Some("3363d5bc"));
        assert_eq!(mesh_map(url_name(url)), Some(MeshMap::AmbientOcclusion));
        assert_eq!(
            mesh_map("World Space Normals Rings"),
            Some(MeshMap::WorldNormal)
        );
        assert_eq!(mesh_map("Color Map from Mesh Rings"), Some(MeshMap::Id));
        assert_eq!(mesh_map("SweatpantsBake_curve"), Some(MeshMap::Curvature));
        let parts = ["BodyMaterial".to_owned(), "Zipper".to_owned()];
        assert_eq!(match_part(&parts, "Body", 2), Some(0));
        assert_eq!(match_part(&parts, "Zipper", 2), Some(1));
        assert_eq!(match_part(&parts, "Hood", 2), None);
    }
}
