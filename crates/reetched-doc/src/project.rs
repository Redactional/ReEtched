//! `.retx` project files: a zip with
//! - `project.json` — the [`Document`]
//! - `mesh.json`, `mesh_vertices.bin`, `mesh_indices.bin` — the imported mesh
//! - `resources/<hash>` — every file the document references (bitmaps, …)

use crate::{Document, FORMAT_VERSION, Resources};
use anyhow::{Context, Result, bail};
use reetched_io::{Mesh, MeshPart, Vertex};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
};

pub const EXTENSION: &str = "retx";

#[derive(Serialize, Deserialize)]
struct MeshInfo {
    /// (material name, first index, end index)
    parts: Vec<(String, u32, u32)>,
    has_uvs: bool,
}

pub fn save(path: &Path, doc: &Document, mesh: &Mesh, resources: &Resources) -> Result<()> {
    // Write next to the target and rename at the end, so a failed save never
    // destroys the previous version of the project.
    let tmp = path.with_extension("retx.tmp");
    write_zip(&tmp, doc, mesh, resources)
        .with_context(|| format!("could not write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("could not replace {}", path.display()))
}

fn write_zip(path: &Path, doc: &Document, mesh: &Mesh, resources: &Resources) -> Result<()> {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path)?);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, bytes: &[u8]| -> Result<()> {
        zip.start_file(name, opts)?;
        zip.write_all(bytes)?;
        Ok(())
    };
    put("project.json", &serde_json::to_vec_pretty(doc)?)?;
    let info = MeshInfo {
        parts: mesh
            .parts
            .iter()
            .map(|p| (p.material.clone(), p.indices.start, p.indices.end))
            .collect(),
        has_uvs: mesh.has_uvs,
    };
    put("mesh.json", &serde_json::to_vec_pretty(&info)?)?;
    put("mesh_vertices.bin", bytemuck::cast_slice(&mesh.vertices))?;
    put("mesh_indices.bin", bytemuck::cast_slice(&mesh.indices))?;
    for id in doc.resources.keys() {
        let bytes = resources
            .files
            .get(id)
            .with_context(|| format!("resource {id} is missing from memory"))?;
        put(&format!("resources/{id}"), bytes)?;
    }
    zip.finish()?;
    Ok(())
}

pub fn load(path: &Path) -> Result<(Document, Mesh, Resources)> {
    let file =
        std::fs::File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("not a ReEtched project (bad zip)")?;
    let mut read = |name: &str| -> Result<Vec<u8>> {
        let mut entry = zip
            .by_name(name)
            .with_context(|| format!("project is missing {name}"))?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        Ok(bytes)
    };

    let doc: Document =
        serde_json::from_slice(&read("project.json")?).context("project.json is invalid")?;
    if doc.format_version > FORMAT_VERSION {
        bail!(
            "this project was saved by a newer ReEtched (format {}, this version reads up to {FORMAT_VERSION})",
            doc.format_version
        );
    }

    let info: MeshInfo = serde_json::from_slice(&read("mesh.json")?)?;
    let vertices: Vec<Vertex> = read("mesh_vertices.bin")?
        .as_chunks::<{ size_of::<Vertex>() }>()
        .0
        .iter()
        .map(|c| bytemuck::pod_read_unaligned(c))
        .collect();
    let indices: Vec<u32> = read("mesh_indices.bin")?
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    if indices.iter().any(|&i| i as usize >= vertices.len()) {
        bail!("mesh data is corrupt (index out of range)");
    }
    let mesh = Mesh {
        vertices,
        parts: info
            .parts
            .into_iter()
            .map(|(material, start, end)| MeshPart {
                material,
                indices: start..end.min(indices.len() as u32),
            })
            .collect(),
        indices,
        has_uvs: info.has_uvs,
    };

    let mut resources = Resources::default();
    for id in doc.resources.keys() {
        let bytes = read(&format!("resources/{id}"))?;
        resources.files.insert(id.clone(), bytes.into());
    }
    Ok((doc, mesh, resources))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Fill, ResourceInfo};

    #[test]
    fn save_and_load_round_trip() {
        let mesh = Mesh::uv_sphere(1.0, 8, 4);
        let mut doc = Document::new(Some("sphere.fbx".into()), &["sphere".to_string()]);
        let mut resources = Resources::default();
        let id = resources.add(vec![1, 2, 3]);
        doc.resources.insert(
            id.clone(),
            ResourceInfo {
                name: "dots.png".into(),
            },
        );
        let mut fill = Fill::new([1.0, 0.0, 0.0], 1.0, 0.2);
        fill.channels
            .get_mut(&crate::Channel::BaseColor)
            .unwrap()
            .bitmap = Some(id.clone());
        let node = doc.new_fill("red".into(), fill);
        doc.texture_sets[0].stack.insert(0, node);

        let dir = std::env::temp_dir().join("reetched-doc-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.retx");
        save(&path, &doc, &mesh, &resources).unwrap();
        let (doc2, mesh2, res2) = load(&path).unwrap();
        assert_eq!(doc2, doc);
        assert_eq!(mesh2.vertices, mesh.vertices);
        assert_eq!(mesh2.indices, mesh.indices);
        assert_eq!(mesh2.parts.len(), 1);
        assert_eq!(res2.files[&id].as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn rejects_newer_format() {
        let mut doc = Document::new(None, &[]);
        doc.format_version = FORMAT_VERSION + 1;
        let dir = std::env::temp_dir().join("reetched-doc-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("future.retx");
        save(
            &path,
            &doc,
            &Mesh::uv_sphere(1.0, 4, 2),
            &Resources::default(),
        )
        .unwrap();
        let err = load(&path).err().expect("newer format must be rejected");
        assert!(err.to_string().contains("newer"));
    }
}
