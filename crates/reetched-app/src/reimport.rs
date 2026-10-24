//! Reimporting a changed mesh into an open project, keeping its paint: every
//! paint layer and painted mask is re-projected from the old surface onto the
//! new UVs.

use crate::app::State;
use reetched_doc::{Node, NodeKind, Resources, TextureSet, TileMap};
use reetched_io::Mesh;
use std::path::Path;

/// How far (× mesh size) to look for the old surface from the new one.
const REACH: f32 = 0.02;

impl State {
    pub fn reimport_mesh_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Meshes", crate::app::MESH_EXTENSIONS)
            .pick_file()
        {
            self.reimport_mesh(&path);
        }
    }

    pub fn reimport_mesh(&mut self, path: &Path) {
        let name = crate::app::file_name(path);
        let mesh = match reetched_io::load_mesh(path) {
            Ok(m) if !m.has_uvs => match reetched_io::unwrap(&m) {
                Ok(m) => m,
                Err(e) => {
                    self.error(format!("{name} has no UVs and unwrapping failed: {e:#}"));
                    return;
                }
            },
            Ok(m) => m,
            Err(e) => {
                self.error(format!("Could not open mesh: {e:#}"));
                return;
            }
        };
        self.reimport_with(mesh, name);
        self.doc.mesh_source = Some(path.display().to_string());
    }

    /// Swaps in `new`, keeping texture sets that match by material name.
    /// ponytail: runs on the UI thread; move to a worker if big sets stall.
    pub fn reimport_with(&mut self, new: Mesh, name: String) {
        let old_mesh = self.mesh.clone();
        let mut doc = self.doc.clone();
        let mut sets = Vec::new();
        let mut stale_maps = false;
        for (i, part) in new.parts.iter().enumerate() {
            let Some(oi) = doc
                .texture_sets
                .iter()
                .position(|s| s.name == part.material)
            else {
                let layer = doc.new_paint("Layer 1".into());
                sets.push(TextureSet {
                    name: part.material.clone(),
                    resolution: 2048,
                    stack: vec![layer],
                    mesh_maps: Default::default(),
                });
                self.info(format!("New texture set {}", part.material));
                continue;
            };
            let mut set = doc.texture_sets[oi].clone();
            let res = set.resolution;
            let map = reetched_bake::transfer(&new, i, &old_mesh, oi, res, REACH);
            remap(&mut set.stack, &map, res, &mut self.resources);
            stale_maps |= !set.mesh_maps.is_empty();
            set.mesh_maps.clear();
            sets.push(set);
        }
        for s in &doc.texture_sets {
            if !new.parts.iter().any(|p| p.material == s.name) {
                self.warn(format!(
                    "Texture set {} is gone from the new mesh; its layers were dropped",
                    s.name
                ));
            }
        }
        doc.texture_sets = sets;
        let resources = std::mem::take(&mut self.resources);
        let path = self.project_path.clone();
        self.set_mesh(new, name.clone(), Some(doc));
        self.resources = resources;
        self.project_path = path;
        self.info(format!(
            "Reimported {name}; paint was re-projected onto the new UVs"
        ));
        if stale_maps {
            self.warn("Baked mesh maps no longer match the mesh; bake them again".into());
        }
    }
}

fn remap(nodes: &mut [Node], map: &[Option<[f32; 2]>], res: u32, resources: &mut Resources) {
    for n in nodes {
        if let Some(p) = n.mask.as_mut().and_then(|m| m.paint.as_mut()) {
            *p = remap_tiles(p, map, res, resources);
        }
        match &mut n.kind {
            NodeKind::Paint(paint) => {
                for t in paint.channels.values_mut() {
                    *t = remap_tiles(t, map, res, resources);
                }
            }
            NodeKind::Folder { children } => remap(children, map, res, resources),
            NodeKind::Fill(_) => {}
        }
    }
}

/// New tiles at `res`², each texel copied from where `map` says it was.
fn remap_tiles(
    old: &TileMap,
    map: &[Option<[f32; 2]>],
    res: u32,
    resources: &mut Resources,
) -> TileMap {
    if old.tiles.is_empty() {
        return TileMap::new(res);
    }
    let r = old.resolution.max(1);
    let px: Vec<[f32; 4]> = map
        .iter()
        .map(|uv| {
            uv.map_or([0.0; 4], |[u, v]| {
                let x = (u.rem_euclid(1.0) * r as f32) as u32;
                let y = ((1.0 - v).rem_euclid(1.0) * r as f32) as u32;
                old.texel(resources, x.min(r - 1), y.min(r - 1))
            })
        })
        .collect();
    TileMap::from_fn(res, resources, |x, y| px[(y * res + x) as usize])
}
