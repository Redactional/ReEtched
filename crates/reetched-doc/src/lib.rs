//! The ReEtched document: texture sets and their layer stacks, modelled on
//! Substance Painter's so Painter projects can map onto it one-to-one.
//! Pure data — rendering lives in the app.

pub mod export;
pub mod history;
pub mod project;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Current `.retx` format version. Bump it and add a migration when the layout changes.
/// v2: paint layers and painted masks.
pub const FORMAT_VERSION: u32 = 2;

/// Texture channels of the metal/roughness workflow.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum Channel {
    BaseColor,
    Metallic,
    Roughness,
    Normal,
    Height,
}

impl Channel {
    pub const ALL: [Channel; 5] = [
        Channel::BaseColor,
        Channel::Metallic,
        Channel::Roughness,
        Channel::Normal,
        Channel::Height,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Channel::BaseColor => "Base color",
            Channel::Metallic => "Metallic",
            Channel::Roughness => "Roughness",
            Channel::Normal => "Normal",
            Channel::Height => "Height",
        }
    }

    /// Three-component channels; the rest use only `x`.
    pub fn is_vector(self) -> bool {
        matches!(self, Channel::BaseColor | Channel::Normal)
    }

    /// Value of an empty layer stack (base color is linear RGB, normal is tangent space).
    pub fn empty_value(self) -> [f32; 3] {
        match self {
            Channel::BaseColor => [0.214, 0.214, 0.214], // sRGB 128 grey, like Painter
            Channel::Metallic => [0.0; 3],
            Channel::Roughness => [0.5; 3],
            Channel::Normal => [0.0, 0.0, 1.0],
            Channel::Height => [0.0; 3],
        }
    }

    /// Painter's default blend mode for a new layer in this channel.
    pub fn default_blend(self) -> BlendMode {
        match self {
            Channel::Normal => BlendMode::NormalMapDetail,
            Channel::Height => BlendMode::LinearDodge,
            _ => BlendMode::Normal,
        }
    }
}

/// Painter's blend modes. The math lives in `compositor.wgsl` (indices must match).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum BlendMode {
    Normal,
    Passthrough,
    Disable,
    Replace,
    Multiply,
    Divide,
    InverseDivide,
    Darken,
    Lighten,
    LinearDodge,
    Subtract,
    InverseSubtract,
    Difference,
    Exclusion,
    SignedAddition,
    Overlay,
    Screen,
    LinearBurn,
    ColorBurn,
    ColorDodge,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    Tint,
    Saturation,
    Color,
    Value,
    NormalMapCombine,
    NormalMapDetail,
    NormalMapInverseDetail,
}

impl BlendMode {
    pub const ALL: [BlendMode; 32] = [
        BlendMode::Normal,
        BlendMode::Passthrough,
        BlendMode::Disable,
        BlendMode::Replace,
        BlendMode::Multiply,
        BlendMode::Divide,
        BlendMode::InverseDivide,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::LinearDodge,
        BlendMode::Subtract,
        BlendMode::InverseSubtract,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::SignedAddition,
        BlendMode::Overlay,
        BlendMode::Screen,
        BlendMode::LinearBurn,
        BlendMode::ColorBurn,
        BlendMode::ColorDodge,
        BlendMode::SoftLight,
        BlendMode::HardLight,
        BlendMode::VividLight,
        BlendMode::LinearLight,
        BlendMode::PinLight,
        BlendMode::Tint,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Value,
        BlendMode::NormalMapCombine,
        BlendMode::NormalMapDetail,
        BlendMode::NormalMapInverseDetail,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Passthrough => "Passthrough",
            BlendMode::Disable => "Disable",
            BlendMode::Replace => "Replace",
            BlendMode::Multiply => "Multiply",
            BlendMode::Divide => "Divide",
            BlendMode::InverseDivide => "Inverse Divide",
            BlendMode::Darken => "Darken (Min)",
            BlendMode::Lighten => "Lighten (Max)",
            BlendMode::LinearDodge => "Linear Dodge (Add)",
            BlendMode::Subtract => "Subtract",
            BlendMode::InverseSubtract => "Inverse Subtract",
            BlendMode::Difference => "Difference",
            BlendMode::Exclusion => "Exclusion",
            BlendMode::SignedAddition => "Signed Addition (AddSub)",
            BlendMode::Overlay => "Overlay",
            BlendMode::Screen => "Screen",
            BlendMode::LinearBurn => "Linear Burn",
            BlendMode::ColorBurn => "Color Burn",
            BlendMode::ColorDodge => "Color Dodge",
            BlendMode::SoftLight => "Soft Light",
            BlendMode::HardLight => "Hard Light",
            BlendMode::VividLight => "Vivid Light",
            BlendMode::LinearLight => "Linear Light",
            BlendMode::PinLight => "Pin Light",
            BlendMode::Tint => "Tint",
            BlendMode::Saturation => "Saturation",
            BlendMode::Color => "Color",
            BlendMode::Value => "Value",
            BlendMode::NormalMapCombine => "Normal Map Combine",
            BlendMode::NormalMapDetail => "Normal Map Detail",
            BlendMode::NormalMapInverseDetail => "Normal Map Inverse Detail",
        }
    }

    /// Short label for the layer row, like Painter's "Norm".
    pub fn short(self) -> &'static str {
        match self {
            BlendMode::Normal => "Norm",
            BlendMode::Passthrough => "Pass",
            BlendMode::Disable => "Dis",
            BlendMode::Replace => "Repl",
            BlendMode::Multiply => "Mul",
            BlendMode::LinearDodge => "Add",
            BlendMode::Subtract => "Sub",
            BlendMode::Overlay => "Over",
            BlendMode::Screen => "Scrn",
            BlendMode::NormalMapCombine => "NCmb",
            BlendMode::NormalMapDetail => "NDtl",
            BlendMode::NormalMapInverseDetail => "NInv",
            other => &other.label()[..4],
        }
    }

    /// Passthrough only makes sense for folders.
    pub fn allowed_on_layers(self) -> bool {
        self != BlendMode::Passthrough
    }

    pub fn index(self) -> u32 {
        BlendMode::ALL.iter().position(|&m| m == self).unwrap() as u32
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct ChannelBlend {
    pub mode: BlendMode,
    /// 0..=1
    pub opacity: f32,
}

/// Content hash (BLAKE3, hex) of an imported file; the bytes live in [`Resources`].
pub type ResourceId = String;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ResourceInfo {
    /// Original file name, for display and export.
    pub name: String,
}

/// File bytes and paint tiles referenced by the document, keyed by content hash.
/// Immutable, so documents (and undo snapshots) share them cheaply.
#[derive(Clone, Default)]
pub struct Resources {
    pub files: BTreeMap<ResourceId, std::sync::Arc<Vec<u8>>>,
    pub tiles: BTreeMap<TileId, std::sync::Arc<Vec<u8>>>,
}

impl Resources {
    pub fn add(&mut self, bytes: Vec<u8>) -> ResourceId {
        let id = blake3::hash(&bytes).to_hex().to_string();
        self.files.entry(id.clone()).or_insert_with(|| bytes.into());
        id
    }

    /// Stores one tile (`TILE_BYTES` of RGBA f16) and returns its id.
    pub fn add_tile(&mut self, bytes: Vec<u8>) -> TileId {
        debug_assert_eq!(bytes.len(), TILE_BYTES);
        let id = blake3::hash(&bytes).to_hex().to_string();
        self.tiles.entry(id.clone()).or_insert_with(|| bytes.into());
        id
    }
}

/// Paint is stored in square tiles of this many texels.
pub const TILE_SIZE: u32 = 256;
/// Bytes per tile: RGBA, 16-bit float each.
pub const TILE_BYTES: usize = (TILE_SIZE * TILE_SIZE * 8) as usize;

/// Content hash of a tile's bytes (see [`Resources::tiles`]).
pub type TileId = String;

/// Painted texels of one channel: RGBA f16 tiles (color + coverage alpha).
/// Tiles never painted are absent (fully transparent).
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TileMap {
    /// Texture size the tiles were painted at.
    pub resolution: u32,
    /// Keyed by "x,y" (tile column, row; row 0 = top of the texture).
    pub tiles: BTreeMap<String, TileId>,
}

impl TileMap {
    pub fn new(resolution: u32) -> Self {
        Self {
            resolution,
            tiles: BTreeMap::new(),
        }
    }

    pub fn key(x: u32, y: u32) -> String {
        format!("{x},{y}")
    }

    pub fn parse_key(key: &str) -> Option<(u32, u32)> {
        let (x, y) = key.split_once(',')?;
        Some((x.parse().ok()?, y.parse().ok()?))
    }
}

/// A paint layer: hand-painted texels per channel.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct Paint {
    pub channels: BTreeMap<Channel, TileMap>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ProjectionMode {
    Uv,
    Triplanar,
    Planar,
}

impl ProjectionMode {
    pub const ALL: [ProjectionMode; 3] = [
        ProjectionMode::Uv,
        ProjectionMode::Triplanar,
        ProjectionMode::Planar,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ProjectionMode::Uv => "UV projection",
            ProjectionMode::Triplanar => "Tri-planar projection",
            ProjectionMode::Planar => "Planar projection",
        }
    }
}

/// How bitmaps are mapped onto the mesh (Painter's fill projection).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Projection {
    pub mode: ProjectionMode,
    /// UV tiling.
    pub scale: [f32; 2],
    pub rotation_deg: f32,
    pub offset: [f32; 2],
    /// Tri-planar blend sharpness, 0..=1.
    pub hardness: f32,
    /// Tri-planar / planar: projector placement in object space.
    pub position: [f32; 3],
    pub rotation3d_deg: [f32; 3],
    /// World units covered by one texture repeat.
    pub size: f32,
}

impl Default for Projection {
    fn default() -> Self {
        Self {
            mode: ProjectionMode::Uv,
            scale: [1.0, 1.0],
            rotation_deg: 0.0,
            offset: [0.0, 0.0],
            hardness: 0.5,
            position: [0.0; 3],
            rotation3d_deg: [0.0; 3],
            size: 1.0,
        }
    }
}

/// What a fill layer puts into one channel.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ChannelSource {
    pub enabled: bool,
    /// Uniform value (linear RGB for base color, tangent-space vector for normal, `x` for scalars).
    pub value: [f32; 3],
    /// A bitmap overrides the uniform value.
    pub bitmap: Option<ResourceId>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Fill {
    pub channels: BTreeMap<Channel, ChannelSource>,
    pub projection: Projection,
}

impl Fill {
    /// Base color, metallic and roughness enabled; normal and height off (Painter's default).
    pub fn new(base_color: [f32; 3], metallic: f32, roughness: f32) -> Self {
        let source = |enabled, value| ChannelSource {
            enabled,
            value,
            bitmap: None,
        };
        let channels = Channel::ALL
            .into_iter()
            .map(|c| {
                let s = match c {
                    Channel::BaseColor => source(true, base_color),
                    Channel::Metallic => source(true, [metallic; 3]),
                    Channel::Roughness => source(true, [roughness; 3]),
                    c => source(false, c.empty_value()),
                };
                (c, s)
            })
            .collect();
        Self {
            channels,
            projection: Projection::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum MaskBackground {
    White,
    Black,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Mask {
    pub enabled: bool,
    pub background: MaskBackground,
    /// Greyscale bitmap filling the mask (Painter's "mask with bitmap").
    pub bitmap: Option<ResourceId>,
    pub projection: Projection,
    pub invert: bool,
    /// Painted mask texels (value in R, coverage in A) on top of the background/bitmap.
    #[serde(default)]
    pub paint: Option<TileMap>,
}

impl Mask {
    pub fn new(background: MaskBackground) -> Self {
        Self {
            enabled: true,
            background,
            bitmap: None,
            projection: Projection::default(),
            invert: false,
            paint: None,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum NodeKind {
    Fill(Fill),
    Paint(Paint),
    Folder { children: Vec<Node> },
}

pub type NodeId = u64;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub visible: bool,
    /// Blend mode and opacity per channel, like Painter.
    pub blend: BTreeMap<Channel, ChannelBlend>,
    pub mask: Option<Mask>,
    pub kind: NodeKind,
}

impl Node {
    fn new(id: NodeId, name: String, kind: NodeKind) -> Self {
        let folder = matches!(kind, NodeKind::Folder { .. });
        let blend = Channel::ALL
            .into_iter()
            .map(|c| {
                let mode = if folder {
                    BlendMode::Passthrough
                } else {
                    c.default_blend()
                };
                (c, ChannelBlend { mode, opacity: 1.0 })
            })
            .collect();
        Self {
            id,
            name,
            visible: true,
            blend,
            mask: None,
            kind,
        }
    }

    pub fn blend(&self, c: Channel) -> ChannelBlend {
        self.blend.get(&c).copied().unwrap_or(ChannelBlend {
            mode: c.default_blend(),
            opacity: 1.0,
        })
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct TextureSet {
    pub name: String,
    /// Square texture size in pixels.
    pub resolution: u32,
    /// Layer stack, top layer first (Painter's order).
    pub stack: Vec<Node>,
}

pub const RESOLUTIONS: [u32; 6] = [256, 512, 1024, 2048, 4096, 8192];

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Document {
    pub format_version: u32,
    /// Where the mesh was imported from (the mesh itself is embedded in the project).
    pub mesh_source: Option<String>,
    pub texture_sets: Vec<TextureSet>,
    pub resources: BTreeMap<ResourceId, ResourceInfo>,
    next_id: NodeId,
}

impl Document {
    /// One texture set per mesh material, each starting with a grey fill layer.
    pub fn new(mesh_source: Option<String>, texture_set_names: &[String]) -> Self {
        let mut doc = Self {
            format_version: FORMAT_VERSION,
            mesh_source,
            texture_sets: Vec::new(),
            resources: BTreeMap::new(),
            next_id: 1,
        };
        for name in texture_set_names {
            // Like a new Painter project: one empty paint layer.
            let layer = doc.new_paint("Layer 1".into());
            doc.texture_sets.push(TextureSet {
                name: name.clone(),
                resolution: 2048,
                stack: vec![layer],
            });
        }
        doc
    }

    pub fn new_fill(&mut self, name: String, fill: Fill) -> Node {
        let id = self.alloc_id();
        Node::new(id, name, NodeKind::Fill(fill))
    }

    pub fn new_paint(&mut self, name: String) -> Node {
        let id = self.alloc_id();
        Node::new(id, name, NodeKind::Paint(Paint::default()))
    }

    /// Every paint tile the document references (for saving).
    pub fn tile_ids(&self) -> std::collections::BTreeSet<TileId> {
        fn walk(nodes: &[Node], out: &mut std::collections::BTreeSet<TileId>) {
            for n in nodes {
                if let Some(p) = n.mask.as_ref().and_then(|m| m.paint.as_ref()) {
                    out.extend(p.tiles.values().cloned());
                }
                match &n.kind {
                    NodeKind::Paint(p) => {
                        for map in p.channels.values() {
                            out.extend(map.tiles.values().cloned());
                        }
                    }
                    NodeKind::Folder { children } => walk(children, out),
                    NodeKind::Fill(_) => {}
                }
            }
        }
        let mut out = std::collections::BTreeSet::new();
        for set in &self.texture_sets {
            walk(&set.stack, &mut out);
        }
        out
    }

    pub fn new_folder(&mut self, name: String) -> Node {
        let id = self.alloc_id();
        Node::new(
            id,
            name,
            NodeKind::Folder {
                children: Vec::new(),
            },
        )
    }

    fn alloc_id(&mut self) -> NodeId {
        self.next_id += 1;
        self.next_id - 1
    }

    /// Fresh ids for a copied subtree.
    pub fn reassign_ids(&mut self, node: &mut Node) {
        node.id = self.alloc_id();
        if let NodeKind::Folder { children } = &mut node.kind {
            for child in children {
                self.reassign_ids(child);
            }
        }
    }
}

/// Finds a node anywhere in a stack.
pub fn find_node(stack: &[Node], id: NodeId) -> Option<&Node> {
    stack.iter().find_map(|n| {
        if n.id == id {
            Some(n)
        } else if let NodeKind::Folder { children } = &n.kind {
            find_node(children, id)
        } else {
            None
        }
    })
}

pub fn find_node_mut(stack: &mut [Node], id: NodeId) -> Option<&mut Node> {
    for n in stack {
        if n.id == id {
            return Some(n);
        }
        if let NodeKind::Folder { children } = &mut n.kind
            && let Some(found) = find_node_mut(children, id)
        {
            return Some(found);
        }
    }
    None
}

/// Removes a node from a stack (searching folders) and returns it.
pub fn take_node(stack: &mut Vec<Node>, id: NodeId) -> Option<Node> {
    if let Some(i) = stack.iter().position(|n| n.id == id) {
        return Some(stack.remove(i));
    }
    stack.iter_mut().find_map(|n| match &mut n.kind {
        NodeKind::Folder { children } => take_node(children, id),
        NodeKind::Fill(_) | NodeKind::Paint(_) => None,
    })
}

/// Where a moved node goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Placement {
    /// Directly above `target`, in the same parent.
    Above(NodeId),
    /// At the top of folder `target`.
    Into(NodeId),
}

/// Inserts `node` at `placement`. Returns the node back if the target doesn't exist.
#[allow(clippy::result_large_err)] // the node is handed back to the caller, not an error payload
pub fn insert_node(stack: &mut Vec<Node>, node: Node, placement: Placement) -> Result<(), Node> {
    match placement {
        Placement::Above(target) => {
            if let Some(i) = stack.iter().position(|n| n.id == target) {
                stack.insert(i, node);
                return Ok(());
            }
        }
        Placement::Into(target) => {
            if let Some(n) = stack.iter_mut().find(|n| n.id == target)
                && let NodeKind::Folder { children } = &mut n.kind
            {
                children.insert(0, node);
                return Ok(());
            }
        }
    }
    let mut node = node;
    for n in stack.iter_mut() {
        if let NodeKind::Folder { children } = &mut n.kind {
            match insert_node(children, node, placement) {
                Ok(()) => return Ok(()),
                Err(back) => node = back,
            }
        }
    }
    Err(node)
}

/// Moves node `id` to `placement`; refuses to move a folder into itself.
pub fn move_node(stack: &mut Vec<Node>, id: NodeId, placement: Placement) -> bool {
    let target = match placement {
        Placement::Above(t) | Placement::Into(t) => t,
    };
    if target == id || find_node(stack, id).is_some_and(|n| contains(n, target)) {
        return false;
    }
    let Some(node) = take_node(stack, id) else {
        return false;
    };
    match insert_node(stack, node, placement) {
        Ok(()) => true,
        Err(node) => {
            stack.insert(0, node);
            false
        }
    }
}

fn contains(node: &Node, id: NodeId) -> bool {
    match &node.kind {
        NodeKind::Folder { children } => children.iter().any(|c| c.id == id || contains(c, id)),
        NodeKind::Fill(_) | NodeKind::Paint(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack() -> (Document, Vec<Node>) {
        let mut doc = Document::new(None, &[]);
        let a = doc.new_fill("a".into(), Fill::new([1.0; 3], 0.0, 0.5));
        let b = doc.new_fill("b".into(), Fill::new([1.0; 3], 0.0, 0.5));
        let f = doc.new_folder("f".into());
        (doc, vec![a, f, b])
    }

    #[test]
    fn move_into_folder_and_back_out() {
        let (_, mut s) = stack();
        let (a, f, b) = (s[0].id, s[1].id, s[2].id);
        assert!(move_node(&mut s, a, Placement::Into(f)));
        assert_eq!(s.len(), 2);
        assert!(find_node(&s, a).is_some());
        assert!(move_node(&mut s, a, Placement::Above(b)));
        assert_eq!(s.iter().map(|n| n.id).collect::<Vec<_>>(), [f, a, b]);
    }

    #[test]
    fn folder_cannot_move_into_itself() {
        let (mut doc, mut s) = stack();
        let f = s[1].id;
        let inner = doc.new_folder("inner".into());
        let inner_id = inner.id;
        insert_node(&mut s, inner, Placement::Into(f)).unwrap();
        assert!(!move_node(&mut s, f, Placement::Into(inner_id)));
        assert!(!move_node(&mut s, f, Placement::Into(f)));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn new_layers_use_painter_default_blend_modes() {
        let (_, s) = stack();
        assert_eq!(s[0].blend(Channel::Normal).mode, BlendMode::NormalMapDetail);
        assert_eq!(s[0].blend(Channel::Height).mode, BlendMode::LinearDodge);
        assert_eq!(s[1].blend(Channel::BaseColor).mode, BlendMode::Passthrough);
    }
}
