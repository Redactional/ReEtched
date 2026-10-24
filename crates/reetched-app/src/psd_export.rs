//! Layered PSD export: one file per texture set and channel, with the layer
//! stack's structure, blend modes, opacities and masks.

use crate::app::State;
use reetched_doc::{
    Channel, Fill, Mask, Node, NodeId, NodeKind, TextureSet,
    export::{ChannelData, linear_to_srgb},
    psd::{PsdNode, write_psd},
};
use std::{collections::HashMap, path::PathBuf};

/// A channel value (linear color, unit normal, signed height, scalar) as 8-bit pixel values.
fn encode(c: Channel, v: [f32; 3]) -> [u8; 3] {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    match c {
        Channel::BaseColor | Channel::Emissive => v.map(|x| b(linear_to_srgb(x))),
        Channel::Normal => v.map(|x| b(x * 0.5 + 0.5)),
        Channel::Height => [b(v[0] + 0.5); 3],
        _ => [b(v[0]); 3],
    }
}

fn value(d: &ChannelData, c: Channel, i: usize) -> [f32; 3] {
    match c {
        Channel::BaseColor => d.base_color[i],
        Channel::Metallic => [d.metallic[i]; 3],
        Channel::Roughness => [d.roughness[i]; 3],
        Channel::Normal => d.normal[i],
        Channel::Height => [d.height_map[i]; 3],
        Channel::Emissive => d.emissive[i],
        Channel::Opacity => [d.opacity[i]; 3],
    }
}

/// Does any layer in `nodes` write channel `c`?
fn uses(nodes: &[Node], c: Channel) -> bool {
    nodes.iter().any(|n| match &n.kind {
        NodeKind::Fill(f) => f.channels.get(&c).is_some_and(|s| s.enabled),
        NodeKind::Paint(p) => p.channels.get(&c).is_some_and(|m| !m.tiles.is_empty()),
        NodeKind::Folder { children } => uses(children, c),
    })
}

/// Composites rendered per node, shared by every channel of a set.
#[derive(Default)]
struct Cache {
    fills: HashMap<NodeId, ChannelData>,
    masks: HashMap<NodeId, Vec<u8>>,
}

impl State {
    pub fn export_psd(&mut self) {
        let dir = PathBuf::from(self.export.dir.trim());
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.error(format!("Could not create {}: {e}", dir.display()));
            return;
        }
        for w in self.viewport.sync(
            &self.device,
            &self.queue,
            &self.doc.texture_sets,
            &self.resources,
            None,
        ) {
            self.warn(w);
        }
        let mesh = crate::stack_ui::mesh_name(&self.doc.mesh_source);
        let mut written = 0;
        for (i, set) in self.doc.texture_sets.clone().iter().enumerate() {
            if !self.export.include.get(i).copied().unwrap_or(false) {
                continue;
            }
            let merged = match self.viewport.read_back(&self.device, &self.queue, i) {
                Ok(d) => d,
                Err(e) => {
                    self.error(format!("PSD export of {} failed: {e:#}", set.name));
                    return;
                }
            };
            let mut cache = Cache::default();
            for c in Channel::ALL {
                if c != Channel::BaseColor && !uses(&set.stack, c) {
                    continue;
                }
                let nodes = self.psd_nodes(i, set, &set.stack, c, &mut cache);
                let pixels: Vec<[u8; 4]> = (0..merged.base_color.len())
                    .map(|k| {
                        let [r, g, b] = encode(c, value(&merged, c, k));
                        [r, g, b, if merged.coverage[k] { 255 } else { 0 }]
                    })
                    .collect();
                let name = format!("{mesh}_{}_{c:?}.psd", set.name);
                let path = dir.join(name);
                match write_psd(&path, merged.width, merged.height, &pixels, &nodes) {
                    Ok(notes) => {
                        written += 1;
                        for n in notes {
                            self.warn(format!("{}: {n}", path.display()));
                        }
                    }
                    Err(e) => {
                        self.error(format!("PSD export failed: {e:#}"));
                        return;
                    }
                }
            }
        }
        self.info(format!(
            "Exported {written} layered PSD files to {}",
            dir.display()
        ));
    }

    fn psd_nodes(
        &mut self,
        part: usize,
        set: &TextureSet,
        nodes: &[Node],
        c: Channel,
        cache: &mut Cache,
    ) -> Vec<PsdNode> {
        let res = set.resolution;
        let n_texels = (res * res) as usize;
        nodes
            .iter()
            .map(|n| {
                let blend = n.blend(c);
                let mask = n
                    .mask
                    .as_ref()
                    .map(|m| self.mask_image(part, set, n.id, m, cache));
                match &n.kind {
                    NodeKind::Folder { children } => PsdNode::Group {
                        name: n.name.clone(),
                        visible: n.visible,
                        blend: blend.mode,
                        opacity: blend.opacity,
                        mask,
                        children: self.psd_nodes(part, set, children, c, cache),
                    },
                    kind => {
                        let pixels: Vec<[u8; 4]> = match kind {
                            NodeKind::Paint(p) => match p.channels.get(&c) {
                                Some(map) => (0..n_texels as u32)
                                    .map(|k| {
                                        let t = map.texel(&self.resources, k % res, k / res);
                                        let [r, g, b] = encode(c, [t[0], t[1], t[2]]);
                                        [r, g, b, (t[3].clamp(0.0, 1.0) * 255.0).round() as u8]
                                    })
                                    .collect(),
                                None => vec![[0; 4]; n_texels],
                            },
                            NodeKind::Fill(f) if f.channels.get(&c).is_some_and(|s| s.enabled) => {
                                let data = self.fill_content(part, set, n, f, cache);
                                (0..n_texels)
                                    .map(|k| {
                                        let [r, g, b] = encode(c, value(data, c, k));
                                        [r, g, b, 255]
                                    })
                                    .collect()
                            }
                            _ => vec![[0; 4]; n_texels],
                        };
                        PsdNode::Layer {
                            name: n.name.clone(),
                            visible: n.visible,
                            blend: blend.mode,
                            opacity: blend.opacity,
                            pixels,
                            mask,
                        }
                    }
                }
            })
            .collect()
    }

    /// A fill layer's content on its own (no mask, full opacity).
    fn fill_content<'a>(
        &mut self,
        part: usize,
        set: &TextureSet,
        node: &Node,
        fill: &Fill,
        cache: &'a mut Cache,
    ) -> &'a ChannelData {
        cache.fills.entry(node.id).or_insert_with(|| {
            let mut alone = self.doc.new_fill(node.name.clone(), fill.clone());
            alone.id = node.id;
            self.composite_alone(part, set, vec![alone])
        })
    }

    /// A layer's mask as 8-bit grey: white masked by it, over black.
    fn mask_image(
        &mut self,
        part: usize,
        set: &TextureSet,
        id: NodeId,
        mask: &Mask,
        cache: &mut Cache,
    ) -> Vec<u8> {
        if let Some(m) = cache.masks.get(&id) {
            return m.clone();
        }
        let only_base = |v: f32| {
            let mut f = Fill::new([v; 3], 0.0, 0.5);
            for (c, s) in &mut f.channels {
                s.enabled = *c == Channel::BaseColor;
            }
            f
        };
        let mut white = self.doc.new_fill("mask".into(), only_base(1.0));
        white.id = id; // painted mask texels are cached by layer id
        let mut mask = mask.clone();
        mask.enabled = true;
        white.mask = Some(mask);
        let black = self.doc.new_fill("black".into(), only_base(0.0));
        let data = self.composite_alone(part, set, vec![white, black]);
        let img: Vec<u8> = data
            .base_color
            .iter()
            .map(|c| (c[0].clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect();
        cache.masks.insert(id, img.clone());
        img
    }

    fn composite_alone(&mut self, part: usize, set: &TextureSet, stack: Vec<Node>) -> ChannelData {
        let mut tmp = set.clone();
        tmp.stack = stack;
        match self
            .viewport
            .composite_temp(&self.device, &self.queue, part, &tmp, &self.resources)
        {
            Ok(d) => d,
            Err(e) => {
                self.warn(format!(
                    "PSD export: could not render a layer of {}: {e:#}",
                    set.name
                ));
                let n = (set.resolution * set.resolution) as usize;
                ChannelData {
                    width: set.resolution,
                    height: set.resolution,
                    base_color: vec![[0.0; 3]; n],
                    metallic: vec![0.0; n],
                    roughness: vec![0.0; n],
                    normal: vec![[0.0, 0.0, 1.0]; n],
                    height_map: vec![0.0; n],
                    emissive: vec![[0.0; 3]; n],
                    opacity: vec![1.0; n],
                    coverage: vec![false; n],
                    ambient_occlusion: Vec::new(),
                }
            }
        }
    }
}
