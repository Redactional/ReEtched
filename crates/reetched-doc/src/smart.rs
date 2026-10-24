//! Smart materials (`.rsm`): a layer or folder saved with the images and paint
//! tiles it uses, so it can be dropped into any project.
//! Zip layout: `material.json`, `resources/<hash>`, `tiles/<hash>`.

use crate::{
    Document, FORMAT_VERSION, Node, NodeKind, ResourceId, ResourceInfo, Resources, TILE_BYTES,
    TileId,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::Path,
};

pub const EXTENSION: &str = "rsm";

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SmartMaterial {
    pub format_version: u32,
    pub name: String,
    pub node: Node,
    /// Names of the image resources `node` uses.
    pub resources: BTreeMap<ResourceId, ResourceInfo>,
}

/// Images and paint tiles a subtree references.
fn refs(node: &Node, files: &mut BTreeSet<ResourceId>, tiles: &mut BTreeSet<TileId>) {
    if let Some(mask) = &node.mask {
        files.extend(mask.bitmap.iter().cloned());
        if let Some(p) = &mask.paint {
            tiles.extend(p.tiles.values().cloned());
        }
    }
    match &node.kind {
        NodeKind::Fill(fill) => {
            files.extend(fill.channels.values().filter_map(|c| c.bitmap.clone()))
        }
        NodeKind::Paint(paint) => {
            for map in paint.channels.values() {
                tiles.extend(map.tiles.values().cloned());
            }
        }
        NodeKind::Folder { children } => {
            for child in children {
                refs(child, files, tiles);
            }
        }
    }
}

impl SmartMaterial {
    /// Captures `node` (and the names of the images it uses) from `doc`.
    pub fn capture(node: &Node, doc: &Document) -> Self {
        let (mut files, mut tiles) = (BTreeSet::new(), BTreeSet::new());
        refs(node, &mut files, &mut tiles);
        Self {
            format_version: FORMAT_VERSION,
            name: node.name.clone(),
            node: node.clone(),
            resources: files
                .into_iter()
                .filter_map(|id| doc.resources.get(&id).map(|info| (id, info.clone())))
                .collect(),
        }
    }

    /// A copy of the node ready to insert into `doc`: fresh ids, and its images and
    /// tiles copied from `from` into `into`.
    pub fn instantiate(&self, doc: &mut Document, from: &Resources, into: &mut Resources) -> Node {
        let mut node = self.node.clone();
        doc.reassign_ids(&mut node);
        let (mut files, mut tiles) = (BTreeSet::new(), BTreeSet::new());
        refs(&node, &mut files, &mut tiles);
        for id in files {
            if let Some(bytes) = from.files.get(&id) {
                into.files.insert(id.clone(), bytes.clone());
            }
            if let Some(info) = self.resources.get(&id) {
                doc.resources.insert(id, info.clone());
            }
        }
        for id in tiles {
            if let Some(bytes) = from.tiles.get(&id) {
                into.tiles.insert(id, bytes.clone());
            }
        }
        node
    }
}

pub fn save(path: &Path, sm: &SmartMaterial, resources: &Resources) -> Result<()> {
    let tmp = path.with_extension("rsm.tmp");
    let write = || -> Result<()> {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&tmp)?);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut put = |name: &str, bytes: &[u8]| -> Result<()> {
            zip.start_file(name, opts)?;
            zip.write_all(bytes)?;
            Ok(())
        };
        put("material.json", &serde_json::to_vec_pretty(sm)?)?;
        let (mut files, mut tiles) = (BTreeSet::new(), BTreeSet::new());
        refs(&sm.node, &mut files, &mut tiles);
        for id in files {
            let bytes = resources
                .files
                .get(&id)
                .with_context(|| format!("image {id} is missing"))?;
            put(&format!("resources/{id}"), bytes)?;
        }
        for id in tiles {
            let bytes = resources
                .tiles
                .get(&id)
                .with_context(|| format!("paint tile {id} is missing"))?;
            put(&format!("tiles/{id}"), bytes)?;
        }
        zip.finish()?;
        Ok(())
    };
    write().with_context(|| format!("could not write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("could not replace {}", path.display()))
}

pub fn load(path: &Path) -> Result<(SmartMaterial, Resources)> {
    let file =
        std::fs::File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("not a smart material (bad zip)")?;
    let mut read = |name: &str| -> Result<Vec<u8>> {
        let mut entry = zip
            .by_name(name)
            .with_context(|| format!("smart material is missing {name}"))?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    let sm: SmartMaterial =
        serde_json::from_slice(&read("material.json")?).context("material.json is invalid")?;
    if sm.format_version > FORMAT_VERSION {
        bail!("this smart material was saved by a newer ReEtched");
    }
    let (mut files, mut tiles) = (BTreeSet::new(), BTreeSet::new());
    refs(&sm.node, &mut files, &mut tiles);
    let mut res = Resources::default();
    for id in files {
        res.files
            .insert(id.clone(), read(&format!("resources/{id}"))?.into());
    }
    for id in tiles {
        let bytes = read(&format!("tiles/{id}"))?;
        if bytes.len() != TILE_BYTES {
            bail!("paint tile {id} is corrupt");
        }
        res.tiles.insert(id, bytes.into());
    }
    Ok((sm, res))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Channel, Fill, Mask, MaskBackground, TileMap};

    #[test]
    fn save_load_and_instantiate() {
        let mut doc = Document::new(None, &["a".to_string()]);
        let mut res = Resources::default();
        let img = res.add(vec![9, 9, 9]);
        doc.resources.insert(
            img.clone(),
            ResourceInfo {
                name: "grunge.png".into(),
            },
        );
        let tile = res.add_tile(vec![1; TILE_BYTES]);

        let mut fill = Fill::new([1.0; 3], 0.0, 0.5);
        fill.channels.get_mut(&Channel::BaseColor).unwrap().bitmap = Some(img.clone());
        let mut layer = doc.new_fill("rust".into(), fill);
        let mut mask = Mask::new(MaskBackground::Black);
        let mut painted = TileMap::new(256);
        painted.tiles.insert(TileMap::key(0, 0), tile.clone());
        mask.paint = Some(painted);
        layer.mask = Some(mask);
        let mut folder = doc.new_folder("Rusty".into());
        if let NodeKind::Folder { children } = &mut folder.kind {
            children.push(layer);
        }

        let sm = SmartMaterial::capture(&folder, &doc);
        assert_eq!(sm.resources.len(), 1);
        let dir = std::env::temp_dir().join("reetched-smart-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rusty.rsm");
        save(&path, &sm, &res).unwrap();
        let (back, back_res) = load(&path).unwrap();
        assert_eq!(back, sm);
        assert_eq!(back_res.tiles[&tile].len(), TILE_BYTES);

        // Into a fresh project: new ids, images and tiles carried over.
        let mut other = Document::new(None, &["b".to_string()]);
        let mut other_res = Resources::default();
        let node = back.instantiate(&mut other, &back_res, &mut other_res);
        assert_ne!(node.id, folder.id);
        assert!(other.resources.contains_key(&img));
        assert!(other_res.files.contains_key(&img) && other_res.tiles.contains_key(&tile));
    }
}
