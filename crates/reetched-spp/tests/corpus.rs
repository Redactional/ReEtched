//! Runs the reader over real Painter projects in the folder named by
//! `REETCHED_SPP_DIR` (searched recursively). Skipped when it isn't set:
//! Painter projects are never part of the repository.

use reetched_spp::{container::Container, document, image};
use std::path::{Path, PathBuf};

fn projects() -> Vec<PathBuf> {
    let Some(dir) = std::env::var_os("REETCHED_SPP_DIR") else {
        eprintln!("REETCHED_SPP_DIR not set; skipping");
        return Vec::new();
    };
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "spp") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(Path::new(&dir), &mut out);
    out.sort();
    out
}

#[test]
fn containers_documents_and_images_read() {
    for p in projects() {
        let name = p.file_name().unwrap().to_string_lossy();
        let c = Container::open(&p).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let doc = c.bytes("paint/document.bin").unwrap();
        let decoded = document::decode(&doc);
        let status = match &decoded {
            Ok((v, tree)) => format!("v{v} ok, {}", tree["@type"]),
            Err(e) => format!("{e:#}"),
        };
        // Images: every embedded bitmap resource and texture cache entry.
        let mut images = 0;
        let mut skipped = 0;
        for r in c.datasets("resources").unwrap() {
            let b = c.bytes(&format!("resources/{r}")).unwrap();
            if b.starts_with(&[0, 0, 0]) && (!r.contains('.') || r.ends_with(".image")) {
                match image::read_resource(&b) {
                    Ok(img) => {
                        assert!(img.width > 0 && img.height > 0);
                        images += 1;
                    }
                    Err(e) if format!("{e}").contains("jp2") => skipped += 1,
                    Err(e) => panic!("{name} {r}: {e:#}"),
                }
            }
        }
        let mut cached = 0;
        for set in c.groups("texture-cache").unwrap_or_default() {
            for stack in c.groups(&format!("texture-cache/{set}")).unwrap() {
                let dir = format!("texture-cache/{set}/{stack}");
                for entry in c.datasets(&dir).unwrap() {
                    if entry == "list" {
                        continue;
                    }
                    let b = c.bytes(&format!("{dir}/{entry}")).unwrap();
                    image::read_cache_entry(&b)
                        .unwrap_or_else(|e| panic!("{name} {dir}/{entry}: {e:#}"));
                    cached += 1;
                }
            }
        }
        eprintln!(
            "{name}: document {status}; {images} images ({skipped} JPEG 2000 skipped), {cached} cached channels"
        );
        decoded.unwrap_or_else(|e| panic!("{name}: {e:#}"));
    }
}

#[test]
fn every_project_converts() {
    for p in projects() {
        let name = p.file_name().unwrap().to_string_lossy();
        let i = reetched_spp::convert::convert(&p)
            .unwrap_or_else(|e| panic!("{name}: {e:#}"))
            .imported;
        fn count(nodes: &[reetched_doc::Node]) -> usize {
            nodes
                .iter()
                .map(|n| match &n.kind {
                    reetched_doc::NodeKind::Folder { children } => 1 + count(children),
                    _ => 1,
                })
                .sum()
        }
        let sets: Vec<String> = i
            .doc
            .texture_sets
            .iter()
            .map(|s| {
                format!(
                    "{} {}px {} layers {} maps",
                    s.name,
                    s.resolution,
                    count(&s.stack),
                    s.mesh_maps.len()
                )
            })
            .collect();
        eprintln!(
            "{name}: {} triangles; {}; {} resources; {} Painter references",
            i.mesh.triangle_count(),
            sets.join(", "),
            i.resources.files.len(),
            i.references.len()
        );
        for l in &i.log {
            eprintln!("    {l}");
        }
        assert!(!i.doc.texture_sets.is_empty());
    }
}
