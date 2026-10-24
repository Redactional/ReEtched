//! Imports a ReEtched Bridge bundle: a folder written by the Painter plugin
//! (`tools/painter-bridge`), holding `bridge.json`, the project's mesh and
//! image captures.
//!
//! What Painter's API exposes exactly (stack structure, names, visibility,
//! per-channel blend modes and opacities, uniform fills) is rebuilt exactly.
//! Everything else (paint, bitmap and Substance fills, effects, masks) arrives
//! as pixel captures: each one exported twice over different backgrounds, so
//! both the value and the coverage of every texel can be solved for.

use anyhow::{Context, Result, bail};
use reetched_doc::{
    BlendMode, Channel, ChannelBlend, Document, Fill, Mask, MaskBackground, MeshMap, Node,
    NodeKind, Paint, ResourceInfo, Resources, TileMap,
};
use reetched_io::Mesh;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub const FORMAT: &str = "reetched-bridge";
pub const VERSION: u32 = 1;
pub const MANIFEST: &str = "bridge.json";

#[derive(Deserialize)]
struct Bundle {
    format: String,
    version: u32,
    #[serde(default)]
    painter: Option<String>,
    mesh: Option<String>,
    /// Per channel: background values of the (black, white) capture passes.
    #[serde(default)]
    backgrounds: BTreeMap<String, [[f32; 3]; 2]>,
    texture_sets: Vec<BSet>,
    #[serde(default)]
    log: Vec<String>,
}

#[derive(Deserialize)]
struct BSet {
    name: String,
    resolution: [u32; 2],
    #[serde(default)]
    mesh_maps: BTreeMap<String, String>,
    /// Painter's own export of the whole stack, per channel (for verification).
    #[serde(default)]
    reference: BTreeMap<String, String>,
    layers: Vec<BNode>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct BNode {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default = "yes")]
    visible: bool,
    #[serde(default)]
    blend: BTreeMap<String, BBlend>,
    /// Exact uniform fill: channel → value (base color linear, normal encoded 0..1).
    #[serde(default)]
    fill: Option<BTreeMap<String, [f32; 3]>>,
    #[serde(default)]
    content: Option<Capture>,
    #[serde(default)]
    mask: Option<BMask>,
    #[serde(default)]
    children: Vec<BNode>,
    #[serde(default)]
    notes: Vec<String>,
}

#[derive(Deserialize)]
struct BBlend {
    mode: String,
    opacity: f32,
}

/// Channel → image file, for the black- and white-background passes.
#[derive(Deserialize)]
struct Capture {
    black: BTreeMap<String, String>,
    white: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct BMask {
    background: String,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    capture: Option<Capture>,
}

/// Painter's final export of one channel, to compare the import against.
pub struct Reference {
    pub set: usize,
    pub channel: Channel,
    pub width: u32,
    pub height: u32,
    /// Values as exported (base color sRGB-encoded, normal encoded 0..1).
    pub pixels: Vec<[f32; 3]>,
}

pub struct Imported {
    pub doc: Document,
    pub mesh: Mesh,
    pub mesh_path: PathBuf,
    pub resources: Resources,
    /// What the plugin and the importer could not carry over.
    pub log: Vec<String>,
    pub references: Vec<Reference>,
}

pub fn channel(name: &str) -> Option<Channel> {
    match name {
        "ao" => Some(Channel::AmbientOcclusion),
        _ => Channel::from_id(name),
    }
}

fn mesh_map(name: &str) -> Option<MeshMap> {
    Some(match name {
        "ambient_occlusion" => MeshMap::AmbientOcclusion,
        "curvature" => MeshMap::Curvature,
        "normal" => MeshMap::Normal,
        "world_normal" => MeshMap::WorldNormal,
        "id" => MeshMap::Id,
        "position" => MeshMap::Position,
        "thickness" => MeshMap::Thickness,
        "height" => MeshMap::Height,
        "opacity" => MeshMap::Opacity,
        _ => return None,
    })
}

/// Painter's `BlendingMode` names (e.g. `LinearDodge`, `Passthrough`) → ours.
pub fn blend_mode(name: &str) -> Option<BlendMode> {
    let key = |s: &str| s.to_ascii_lowercase().replace(['_', ' '], "");
    let want = key(name);
    BlendMode::ALL
        .into_iter()
        .find(|m| key(&format!("{m:?}")) == want)
}

/// sRGB-encoded → linear.
pub fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

struct Ctx<'a> {
    dir: &'a Path,
    resolution: u32,
    backgrounds: &'a BTreeMap<String, [[f32; 3]; 2]>,
    resources: &'a mut Resources,
    log: &'a mut Vec<String>,
    set: &'a str,
}

impl Ctx<'_> {
    /// An exported image at the texture set's resolution, values 0..1 as stored.
    fn image(&mut self, rel: &str) -> Option<image::Rgb32FImage> {
        let path = self.dir.join(rel);
        match image::open(&path) {
            Ok(img) => {
                let img = img.to_rgb32f();
                Some(if img.dimensions() == (self.resolution, self.resolution) {
                    img
                } else {
                    image::imageops::resize(
                        &img,
                        self.resolution,
                        self.resolution,
                        image::imageops::FilterType::Triangle,
                    )
                })
            }
            Err(e) => {
                self.log
                    .push(format!("{}: could not read {rel}: {e}", self.set));
                None
            }
        }
    }

    /// Solves a black/white capture pair for value and coverage, per channel.
    fn content(&mut self, cap: &Capture, layer: &str) -> Paint {
        let mut paint = Paint::default();
        for (name, black) in &cap.black {
            let Some(c) = channel(name) else { continue };
            let Some(white) = cap.white.get(name) else {
                self.log.push(format!(
                    "{}/{layer}: {name} has no white-background capture",
                    self.set
                ));
                continue;
            };
            let (Some(eb), Some(ew)) = (self.image(black), self.image(white)) else {
                continue;
            };
            let [b0, b1] = self
                .backgrounds
                .get(name)
                .copied()
                .unwrap_or([[0.0; 3], [1.0; 3]]);
            let map = TileMap::from_fn(self.resolution, self.resources, |x, y| {
                let (pb, pw) = (eb.get_pixel(x, y).0, ew.get_pixel(x, y).0);
                solve(c, pb, pw, b0, b1)
            });
            if !map.tiles.is_empty() {
                paint.channels.insert(c, map);
            }
        }
        paint
    }

    /// A mask capture: the layer's content forced to white, then black.
    fn mask(&mut self, cap: &Capture) -> Option<TileMap> {
        let (b, w) = (cap.black.get("basecolor")?, cap.white.get("basecolor")?);
        let (eb, ew) = (self.image(b)?, self.image(w)?);
        Some(TileMap::from_fn(self.resolution, self.resources, |x, y| {
            let (pb, pw) = (eb.get_pixel(x, y).0, ew.get_pixel(x, y).0);
            let m = ((0..3).map(|i| pw[i] - pb[i]).sum::<f32>() / 3.0).clamp(0.0, 1.0);
            [m, m, m, 1.0]
        }))
    }
}

/// Value and coverage from exports over backgrounds `b0` and `b1`:
/// `E0 = c·a + b0·(1−a)`, `E1 = c·a + b1·(1−a)`.
fn solve(c: Channel, e0: [f32; 3], e1: [f32; 3], b0: [f32; 3], b1: [f32; 3]) -> [f32; 4] {
    let comps: Vec<usize> = (0..3).filter(|&i| (b1[i] - b0[i]).abs() > 0.01).collect();
    let n = if c.is_vector() {
        comps.len()
    } else {
        1.min(comps.len())
    };
    if n == 0 {
        return [0.0; 4];
    }
    let a = (comps[..n]
        .iter()
        .map(|&i| 1.0 - (e1[i] - e0[i]) / (b1[i] - b0[i]))
        .sum::<f32>()
        / n as f32)
        .clamp(0.0, 1.0);
    // Below 1/512 is export noise, not paint.
    if a < 1.0 / 512.0 {
        return [0.0; 4];
    }
    let v: [f32; 3] = std::array::from_fn(|i| ((e0[i] - b0[i] * (1.0 - a)) / a).clamp(0.0, 1.0));
    match c {
        _ if c.is_color() => [to_linear(v[0]), to_linear(v[1]), to_linear(v[2]), a],
        Channel::Normal => {
            let n = [v[0] * 2.0 - 1.0, v[1] * 2.0 - 1.0, v[2] * 2.0 - 1.0];
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if l < 1e-4 {
                [0.0, 0.0, 1.0, a]
            } else {
                [n[0] / l, n[1] / l, n[2] / l, a]
            }
        }
        _ => [v[0], v[0], v[0], a],
    }
}

fn node(doc: &mut Document, cx: &mut Ctx, b: &BNode) -> Node {
    let path = format!("{}/{}", cx.set, b.name);
    let mut n = match b.kind.as_str() {
        "group" => {
            let mut f = doc.new_folder(b.name.clone());
            let kids: Vec<Node> = b.children.iter().map(|c| node(doc, cx, c)).collect();
            if let NodeKind::Folder { children } = &mut f.kind {
                *children = kids;
            }
            f
        }
        _ if b.fill.is_some() => {
            let values = b.fill.as_ref().expect("checked");
            let mut fill = Fill::new([0.5; 3], 0.0, 0.5);
            for (c, src) in &mut fill.channels {
                src.enabled = false;
                let name = c.id();
                if let Some(v) = values.get(name) {
                    src.enabled = true;
                    src.value = match c {
                        c if c.is_color() => *v,
                        Channel::Normal => {
                            let n = v.map(|x| x * 2.0 - 1.0);
                            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
                            n.map(|x| x / l)
                        }
                        _ => [v[0]; 3],
                    };
                }
            }
            doc.new_fill(b.name.clone(), fill)
        }
        _ => {
            let mut layer = doc.new_paint(b.name.clone());
            match &b.content {
                Some(cap) => {
                    if let NodeKind::Paint(p) = &mut layer.kind {
                        *p = cx.content(cap, &b.name);
                    }
                }
                None => cx.log.push(format!("{path}: no capture; imported empty")),
            }
            layer
        }
    };
    n.visible = b.visible;
    for (name, bl) in &b.blend {
        let Some(c) = channel(name) else { continue };
        let mode = blend_mode(&bl.mode).unwrap_or_else(|| {
            cx.log.push(format!(
                "{path}: blend mode {} is not supported; using Normal",
                bl.mode
            ));
            BlendMode::Normal
        });
        n.blend.insert(
            c,
            ChannelBlend {
                mode,
                opacity: bl.opacity.clamp(0.0, 1.0),
            },
        );
    }
    if let Some(m) = &b.mask {
        let background = if m.background.eq_ignore_ascii_case("black") {
            MaskBackground::Black
        } else {
            MaskBackground::White
        };
        let mut mask = Mask::new(background);
        mask.enabled = m.enabled;
        if let Some(cap) = &m.capture {
            mask.paint = cx.mask(cap);
        }
        n.mask = Some(mask);
    }
    cx.log
        .extend(b.notes.iter().map(|note| format!("{path}: {note}")));
    n
}

/// Reads the bundle in `dir`.
pub fn import(dir: &Path) -> Result<Imported> {
    let manifest = dir.join(MANIFEST);
    let text = std::fs::read_to_string(&manifest)
        .with_context(|| format!("could not read {}", manifest.display()))?;
    let bundle: Bundle = serde_json::from_str(&text).context("bridge.json is invalid")?;
    if bundle.format != FORMAT {
        bail!("not a ReEtched Bridge bundle");
    }
    if bundle.version > VERSION {
        bail!("this bundle was written by a newer ReEtched Bridge plugin");
    }
    let mut log = bundle.log.clone();
    if let Some(v) = &bundle.painter {
        log.insert(0, format!("Exported from Substance 3D Painter {v}"));
    }
    let mesh_rel = bundle
        .mesh
        .as_deref()
        .context("the bundle has no mesh; copy the project's mesh next to bridge.json")?;
    let mesh_path = dir.join(mesh_rel);
    let mesh = reetched_io::load_mesh(&mesh_path)
        .with_context(|| format!("could not load the mesh {}", mesh_path.display()))?;

    let names: Vec<String> = mesh.parts.iter().map(|p| p.material.clone()).collect();
    let mut doc = Document::new(Some(mesh_path.to_string_lossy().into_owned()), &names);
    let mut resources = Resources::default();
    let mut references = Vec::new();
    for b in &bundle.texture_sets {
        if !names.contains(&b.name) {
            log.push(format!(
                "texture set {} has no matching material in the mesh; skipped",
                b.name
            ));
        }
    }
    for (i, name) in names.iter().enumerate() {
        let Some(b) = bundle.texture_sets.iter().find(|s| &s.name == name) else {
            log.push(format!("{name}: not in the bundle; left empty"));
            continue;
        };
        let [w, h] = b.resolution;
        if w != h {
            log.push(format!("{name}: non-square {w}×{h} becomes {}²", w.max(h)));
        }
        let resolution = w.max(h).max(1);
        let mut cx = Ctx {
            dir,
            resolution,
            backgrounds: &bundle.backgrounds,
            resources: &mut resources,
            log: &mut log,
            set: name,
        };
        let stack: Vec<Node> = b
            .layers
            .iter()
            .map(|l| node(&mut doc, &mut cx, l))
            .collect();
        let mut mesh_maps = BTreeMap::new();
        for (k, rel) in &b.mesh_maps {
            let Some(m) = mesh_map(k) else { continue };
            match std::fs::read(dir.join(rel)) {
                Ok(bytes) => {
                    let id = resources.add(bytes);
                    doc.resources.insert(
                        id.clone(),
                        ResourceInfo {
                            name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
                        },
                    );
                    mesh_maps.insert(m, id);
                }
                Err(e) => log.push(format!("{name}: could not read mesh map {rel}: {e}")),
            }
        }
        for (k, rel) in &b.reference {
            let Some(channel) = channel(k) else { continue };
            match image::open(dir.join(rel)) {
                Ok(img) => {
                    let img = img.to_rgb32f();
                    references.push(Reference {
                        set: i,
                        channel,
                        width: img.width(),
                        height: img.height(),
                        pixels: img.pixels().map(|p| p.0).collect(),
                    });
                }
                Err(e) => log.push(format!("{name}: could not read reference {rel}: {e}")),
            }
        }
        let set = &mut doc.texture_sets[i];
        set.resolution = resolution;
        set.stack = stack;
        set.mesh_maps = mesh_maps;
    }
    Ok(Imported {
        doc,
        mesh,
        mesh_path,
        resources,
        log,
        references,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_value_and_coverage() {
        let (b0, b1) = ([0.0; 3], [1.0; 3]);
        // Half-covered 0.6 over black → 0.3, over white → 0.8.
        let r = solve(Channel::Roughness, [0.3; 3], [0.8; 3], b0, b1);
        assert!(
            (r[3] - 0.5).abs() < 1e-5 && (r[0] - 0.6).abs() < 1e-5,
            "{r:?}"
        );
        // Nothing painted: both exports show the background.
        assert_eq!(
            solve(Channel::Roughness, [0.0; 3], [1.0; 3], b0, b1)[3],
            0.0
        );
        // Normal with backgrounds differing only in red and green.
        let flat = [0.5, 0.5, 1.0];
        let r = solve(
            Channel::Normal,
            [0.75, 0.5, 1.0],
            [0.75, 0.5, 1.0],
            flat,
            [1.0, 1.0, 1.0],
        );
        assert_eq!(r[3], 1.0);
        assert!(r[0] > 0.4 && r[2] > 0.5, "{r:?}");
    }

    /// The bundle `tools/painter-bridge/tests/test_bridge.py` writes from its fake scene.
    #[test]
    fn imports_the_plugin_test_bundle() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/bridge");
        let imp = import(&dir).unwrap();
        let near = |a: f32, b: f32| (a - b).abs() < 2e-3;
        assert!(
            imp.log.iter().any(|l| l.contains("Specular")),
            "{:?}",
            imp.log
        );
        assert_eq!(imp.references.len(), 7);
        let set = &imp.doc.texture_sets[0];
        assert_eq!((set.name.as_str(), set.resolution), ("Body", 4));
        assert!(set.mesh_maps.contains_key(&MeshMap::AmbientOcclusion));
        assert_eq!(set.stack.len(), 3);
        let r = &imp.resources;

        // Paint layer: multiply on base color, half-covered roughness, painted mask.
        let scratches = &set.stack[0];
        assert_eq!(scratches.name, "Scratches");
        let bc = scratches.blend[&Channel::BaseColor];
        assert_eq!(bc.mode, BlendMode::Multiply);
        assert!(near(bc.opacity, 0.8));
        let NodeKind::Paint(p) = &scratches.kind else {
            panic!("paint layer")
        };
        let t = p.channels[&Channel::BaseColor].texel(r, 0, 0);
        assert!(
            near(t[0], to_linear(0.2)) && near(t[2], to_linear(0.6)) && near(t[3], 1.0),
            "{t:?}"
        );
        assert_eq!(p.channels[&Channel::BaseColor].texel(r, 3, 0)[3], 0.0);
        let t = p.channels[&Channel::Roughness].texel(r, 2, 2);
        assert!(near(t[0], 0.3) && near(t[3], 0.5), "{t:?}");
        assert!(
            !p.channels.contains_key(&Channel::Metallic),
            "nothing painted there"
        );
        let mask = scratches.mask.as_ref().unwrap();
        assert_eq!(mask.background, MaskBackground::Black);
        let m = mask.paint.as_ref().unwrap();
        assert!(near(m.texel(r, 1, 0)[0], 1.0) && near(m.texel(r, 1, 2)[0], 0.5));

        // Group with an exact uniform fill inside.
        let base = &set.stack[1];
        assert!(near(base.blend[&Channel::Roughness].opacity, 0.9));
        let NodeKind::Folder { children } = &base.kind else {
            panic!("folder")
        };
        let NodeKind::Fill(fill) = &children[0].kind else {
            panic!("fill")
        };
        assert_eq!(fill.channels[&Channel::BaseColor].value, [0.8; 3]);
        assert_eq!(fill.channels[&Channel::Metallic].value, [1.0; 3]);
        assert!(fill.channels[&Channel::Roughness].enabled);
        assert!(!fill.channels[&Channel::Normal].enabled);

        // A hidden Substance fill, captured as pixels.
        let rust = &set.stack[2];
        assert!(!rust.visible);
        let NodeKind::Paint(p) = &rust.kind else {
            panic!("captured fill")
        };
        let t = p.channels[&Channel::BaseColor].texel(r, 1, 1);
        assert!(
            near(t[0], to_linear(0.5)) && near(t[1], to_linear(0.25)) && near(t[3], 1.0),
            "{t:?}"
        );
    }

    #[test]
    fn maps_painter_blend_names() {
        assert_eq!(blend_mode("LinearDodge"), Some(BlendMode::LinearDodge));
        assert_eq!(blend_mode("PassThrough"), Some(BlendMode::Passthrough));
        assert_eq!(
            blend_mode("Normal_Map_Detail"),
            Some(BlendMode::NormalMapDetail)
        );
        assert_eq!(blend_mode("Frobnicate"), None);
    }
}
