//! Small layer and mask thumbnails for the layer stack, made on the CPU from
//! paint tiles and bitmaps, cached by their content.

use crate::app::State;
use reetched_doc::{Channel, FIRST_UDIM, Node, NodeId, NodeKind, ResourceId, Resources, TileMap};
use std::collections::HashMap;

pub const SIZE: u32 = 32;

/// Cached thumbnails by (layer, mask?, channel), with the content key they were made from.
#[derive(Default)]
pub struct Thumbs {
    cache: HashMap<(NodeId, bool, Channel), (String, egui::TextureHandle)>,
    /// Decoded bitmaps, shrunk to SIZE.
    images: HashMap<ResourceId, Option<image::RgbaImage>>,
}

/// What a thumbnail shows, identified by content (tile and resource ids).
enum Source<'a> {
    Paint(&'a TileMap),
    Bitmap(&'a ResourceId),
}

impl Source<'_> {
    fn key(&self) -> String {
        match self {
            Source::Paint(m) => {
                let mut h = blake3::Hasher::new();
                for (k, v) in &m.tiles {
                    h.update(k.as_bytes());
                    h.update(v.as_bytes());
                }
                format!("paint:{}", h.finalize().to_hex())
            }
            Source::Bitmap(id) => format!("bitmap:{id}"),
        }
    }
}

/// Paint texels over a checkerboard-free grey, value encoded for display.
fn paint_image(map: &TileMap, resources: &Resources, c: Channel) -> image::RgbaImage {
    let res = map.resolution.max(1);
    image::RgbaImage::from_fn(SIZE, SIZE, |x, y| {
        let tx = ((x as f32 + 0.5) / SIZE as f32 * res as f32) as u32;
        let ty = ((y as f32 + 0.5) / SIZE as f32 * res as f32) as u32;
        let t = map.texel(resources, tx.min(res - 1), ty.min(res - 1));
        let v = match c {
            Channel::Normal => [t[0] * 0.5 + 0.5, t[1] * 0.5 + 0.5, t[2] * 0.5 + 0.5],
            Channel::Height => [t[0] + 0.5; 3],
            c if c.is_color() => [t[0], t[1], t[2]].map(reetched_doc::export::linear_to_srgb),
            _ => [t[0]; 3],
        };
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        // Unpainted texels show the row background.
        image::Rgba([b(v[0]), b(v[1]), b(v[2]), b(t[3])])
    })
}

impl State {
    /// Thumbnails of the selected texture set's layers (and masks) for `channel`,
    /// for layers that have paint or a bitmap; others keep their swatch.
    pub fn layer_thumbnails(
        &mut self,
        ctx: &egui::Context,
        channel: Channel,
    ) -> HashMap<(NodeId, bool), egui::TextureId> {
        let mut wanted: Vec<(NodeId, bool, String, image::RgbaImage)> = Vec::new();
        let mut keys: HashMap<(NodeId, bool), String> = HashMap::new();
        fn walk<'a>(
            nodes: &'a [Node],
            channel: Channel,
            out: &mut Vec<(NodeId, bool, Source<'a>)>,
        ) {
            for n in nodes {
                if let Some(m) = &n.mask {
                    if let Some(p) = m.paint_for(FIRST_UDIM).filter(|p| !p.tiles.is_empty()) {
                        out.push((n.id, true, Source::Paint(p)));
                    } else if let Some(b) = &m.bitmap {
                        out.push((n.id, true, Source::Bitmap(b)));
                    }
                }
                match &n.kind {
                    NodeKind::Paint(p) => {
                        if let Some(m) = p.map(FIRST_UDIM, channel).filter(|m| !m.tiles.is_empty())
                        {
                            out.push((n.id, false, Source::Paint(m)));
                        }
                    }
                    NodeKind::Fill(f) => {
                        if let Some(b) = f
                            .channels
                            .get(&channel)
                            .filter(|s| s.enabled)
                            .and_then(|s| s.bitmap.as_ref())
                        {
                            out.push((n.id, false, Source::Bitmap(b)));
                        }
                    }
                    NodeKind::Folder { children } => walk(children, channel, out),
                }
            }
        }
        let Some(set) = self.doc.texture_sets.get(self.selected_part) else {
            return HashMap::new();
        };
        let mut sources = Vec::new();
        walk(&set.stack, channel, &mut sources);
        for (id, mask, src) in sources {
            let key = src.key();
            keys.insert((id, mask), key.clone());
            let c = if mask { Channel::Roughness } else { channel };
            let cached = self.layer_thumbs.cache.get(&(id, mask, c));
            if cached.is_some_and(|(k, _)| *k == key) {
                continue;
            }
            let img = match src {
                Source::Paint(m) => Some(paint_image(m, &self.resources, c)),
                Source::Bitmap(rid) => self
                    .layer_thumbs
                    .images
                    .entry(rid.clone())
                    .or_insert_with(|| {
                        let bytes = self.resources.files.get(rid)?;
                        let img = crate::images::decode(bytes).ok()?;
                        Some(img.thumbnail(SIZE, SIZE).to_rgba8())
                    })
                    .clone(),
            };
            if let Some(img) = img {
                wanted.push((id, mask, key, img));
            }
        }
        for (id, mask, key, img) in wanted {
            let c = if mask { Channel::Roughness } else { channel };
            let handle = ctx.load_texture(
                format!("thumb {id} {mask} {c:?}"),
                egui::ColorImage::from_rgba_unmultiplied(
                    [img.width() as usize, img.height() as usize],
                    &img,
                ),
                egui::TextureOptions::LINEAR,
            );
            self.layer_thumbs.cache.insert((id, mask, c), (key, handle));
        }
        keys.iter()
            .filter_map(|(&(id, mask), _)| {
                let c = if mask { Channel::Roughness } else { channel };
                self.layer_thumbs
                    .cache
                    .get(&(id, mask, c))
                    .map(|(_, h)| ((id, mask), h.id()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_thumbnails_show_painted_texels() {
        let mut resources = Resources::default();
        // Left half red.
        let map = TileMap::from_fn(64, &mut resources, |x, _| {
            if x < 32 {
                [1.0, 0.0, 0.0, 1.0]
            } else {
                [0.0; 4]
            }
        });
        let img = paint_image(&map, &resources, Channel::BaseColor);
        assert_eq!(img.get_pixel(4, 16).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(28, 16).0[3], 0);
        assert_ne!(
            Source::Paint(&map).key(),
            Source::Paint(&TileMap::new(64)).key()
        );
    }
}
