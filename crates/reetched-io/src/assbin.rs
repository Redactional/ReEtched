//! Reader for Assimp's binary dump format (`.assbin`, files starting with
//! "ASSIMP.binary-dump."). Painter `.spp` projects keep their mesh as one of
//! these (`paint/scene.assbin`).
//!
//! The format is Assimp's (BSD-3-Clause, Copyright (c) 2006-2025 assimp team).
//! The stock layout follows `code/AssetLib/Assbin/AssbinLoader.cpp`,
//! `code/AssetLib/Assbin/AssbinFileWriter.cpp` and `code/Common/assbin_chunks.h`.
//!
//! Painter writes a variant of it (header version 1.6 / 1.7 where stock Assimp
//! writes 1.0), worked out from sample files:
//! - chunks have no magic, just a u64 size that counts its own 8 bytes;
//! - every array is prefixed with its u32 length, and the scene, node and mesh
//!   headers drop the counts those prefixes replace;
//! - meshes carry a name; faces are (u32 flag, u32 count, u32 indices...);
//! - material properties store the type before the data length;
//! - node metadata (e.g. `ALG_PRE_PTV_TRANSFORM`) follows the children.
//!
//! Read: node hierarchy and transforms, mesh positions, normals, the first two UV
//! channels, faces, and material names. Tangents are rebuilt with MikkTSpace like
//! every other loader; bones, animations, textures, lights, cameras, vertex
//! colours and node metadata are skipped (each chunk carries its size).

use crate::{Mesh, Soup, Vertex, build};
use anyhow::{Context, Result, bail};
use glam::{Mat4, Vec3};
use std::io::Read;

const MAGIC: &[u8] = b"ASSIMP.binary-dump.";
const HEADER_LEN: usize = 512;
const CHUNK_MESH: u32 = 0x1237;
const CHUNK_SCENE: u32 = 0x1239;
const CHUNK_NODE: u32 = 0x123c;
const CHUNK_MATERIAL: u32 = 0x123d;
const CHUNK_MATERIAL_PROPERTY: u32 = 0x123e;
const HAS_POSITIONS: u32 = 0x1;
const HAS_NORMALS: u32 = 0x2;
const HAS_TANGENTS: u32 = 0x4;
const HAS_TEXCOORD: u32 = 0x100;
const HAS_COLOR: u32 = 0x10000;
/// aiGetCompileFlags(): ai_real is double.
const CFLAGS_DOUBLE: u32 = 0x20;

pub fn read_assbin(bytes: &[u8]) -> Result<Mesh> {
    if bytes.len() < HEADER_LEN || !bytes.starts_with(MAGIC) {
        bail!("not an Assimp binary dump");
    }
    // 44 bytes of "ASSIMP.binary-dump.<asctime>", then the version block;
    // the file name, options and padding fill the rest of the 512 bytes.
    let mut h = Reader::new(&bytes[44..64], false);
    let (major, _minor) = (h.u32()?, h.u32()?);
    let (_revision, flags) = (h.u32()?, h.u32()?);
    let (shortened, compressed) = (h.u16()? != 0, h.u16()? != 0);
    if major != 1 {
        bail!("unsupported assbin version {major}");
    }
    if shortened {
        bail!("shortened assbin dumps hold no geometry");
    }
    if flags & CFLAGS_DOUBLE != 0 {
        bail!("double-precision assbin dumps are not supported");
    }
    let body = &bytes[HEADER_LEN..];
    let inflated;
    let body = if compressed {
        let mut r = Reader::new(body, false);
        let size = r.u32()? as usize;
        // The size only sizes the buffer, capped so a bad header can't OOM.
        let mut out = Vec::with_capacity(size.min(1 << 30));
        flate2::read::ZlibDecoder::new(r.b)
            .read_to_end(&mut out)
            .context("assbin: zlib decompression failed")?;
        inflated = out;
        &inflated[..]
    } else {
        body
    };
    // Stock dumps open with the scene chunk's magic; Painter's with a size.
    let painter = !body.starts_with(&CHUNK_SCENE.to_le_bytes());
    let soup = read_scene(&mut Reader::new(body, painter))?;
    if soup.groups.iter().all(|(_, c)| c.is_empty()) {
        bail!("assbin contains no triangles");
    }
    Ok(build(soup))
}

struct AiMesh {
    positions: Vec<Vec3>,
    normals: Option<Vec<Vec3>>,
    uvs: Vec<Vec<Vec3>>,
    faces: Vec<Vec<u32>>,
    material: u32,
}

fn read_scene(r: &mut Reader) -> Result<Soup> {
    let mut s = r.chunk(CHUNK_SCENE)?;
    let _flags = s.u32()?;
    let (mut num_meshes, mut num_materials) = (0, 0);
    if !s.painter {
        num_meshes = s.u32()?;
        num_materials = s.u32()?;
        // Animations, textures, lights and cameras follow the materials; never read.
        s.bytes(16)?;
    }
    let mut instances = Vec::new();
    read_node(&mut s, Mat4::IDENTITY, &mut instances)?;
    if s.painter {
        num_meshes = s.u32()?;
    }
    let meshes = (0..num_meshes)
        .map(|_| read_mesh(&mut s))
        .collect::<Result<Vec<_>>>()?;
    if s.painter {
        num_materials = s.u32()?;
    }
    let materials = (0..num_materials)
        .map(|_| read_material(&mut s))
        .collect::<Result<Vec<_>>>()?;

    let mut soup = Soup::default();
    for (world, mesh, node_name) in instances {
        let mesh = meshes
            .get(mesh as usize)
            .with_context(|| format!("node {node_name} references missing mesh {mesh}"))?;
        soup.has_uvs |= !mesh.uvs.is_empty();
        let name = materials
            .get(mesh.material as usize)
            .filter(|n| !n.is_empty())
            .unwrap_or(&node_name);
        let normal_m = world.inverse().transpose();
        // Mirrored transforms flip the winding.
        let flip = world.determinant() < 0.0;
        let p: Vec<Vec3> = mesh
            .positions
            .iter()
            .map(|&p| world.transform_point3(p))
            .collect();
        let uv = |set: usize, i: usize| mesh.uvs.get(set).map_or([0.0; 2], |u| [u[i].x, u[i].y]);
        let out = soup.group(name);
        for face in &mesh.faces {
            // ponytail: fan triangulation like usd.rs; Painter dumps are triangulated anyway.
            for k in 1..face.len().saturating_sub(1) {
                let mut tri = [face[0], face[k], face[k + 1]].map(|i| i as usize);
                if flip {
                    tri.swap(1, 2);
                }
                let flat = (p[tri[1]] - p[tri[0]])
                    .cross(p[tri[2]] - p[tri[0]])
                    .normalize_or_zero();
                for i in tri {
                    let n = mesh.normals.as_ref().map_or(flat, |n| {
                        normal_m.transform_vector3(n[i]).normalize_or_zero()
                    });
                    out.push(Vertex {
                        position: p[i].into(),
                        normal: n.into(),
                        // Assimp keeps v = 0 at the bottom unless asked to flip.
                        uv: uv(0, i),
                        tangent: [0.0; 4],
                        uv1: uv(1, i),
                    });
                }
            }
        }
    }
    Ok(soup)
}

/// Appends (world transform, mesh index, node name) for every mesh reference.
fn read_node(r: &mut Reader, parent: Mat4, out: &mut Vec<(Mat4, u32, String)>) -> Result<()> {
    let mut n = r.chunk(CHUNK_NODE)?;
    let name = n.string()?;
    // aiMatrix4x4 is row-major with the translation in the last column.
    let mut rows = [0.0; 16];
    for x in &mut rows {
        *x = n.f32()?;
    }
    let world = parent * Mat4::from_cols_array(&rows).transpose();
    let mut children = n.u32()?;
    let mut meshes = n.u32()?;
    // Stock: the metadata count (metadata follows the children; skipped with the
    // chunk). Painter: the mesh array's own length.
    let third = n.u32()?;
    if n.painter {
        meshes = third;
    }
    for _ in 0..meshes {
        out.push((world, n.u32()?, name.clone()));
    }
    if n.painter {
        children = n.u32()?;
    }
    for _ in 0..children {
        read_node(&mut n, world, out)?;
    }
    Ok(())
}

fn read_mesh(r: &mut Reader) -> Result<AiMesh> {
    let mut m = r.chunk(CHUNK_MESH)?;
    let (mut num_vertices, mut num_faces) = (0, 0);
    let material = if m.painter {
        let _name = m.string()?;
        let _primitive_types = m.u32()?;
        m.u32()?
    } else {
        let _primitive_types = m.u32()?;
        num_vertices = m.u32()? as usize;
        num_faces = m.u32()? as usize;
        let _bones = m.u32()?; // after the faces; skipped with the chunk
        m.u32()?
    };
    let c = m.u32()?;
    if m.painter && c & HAS_POSITIONS != 0 {
        num_vertices = m.clone().u32()? as usize;
    }
    let mut positions = vec![Vec3::ZERO; num_vertices];
    if c & HAS_POSITIONS != 0 {
        positions = m.vec3s(num_vertices)?;
    }
    let normals = if c & HAS_NORMALS != 0 {
        Some(m.vec3s(num_vertices)?)
    } else {
        None
    };
    if c & HAS_TANGENTS != 0 {
        m.array(num_vertices, 12)?; // tangents
        m.array(num_vertices, 12)?; // bitangents
    }
    for n in 0..8 {
        if c & (HAS_COLOR << n) == 0 {
            break;
        }
        m.array(num_vertices, 16)?;
    }
    let mut uvs = Vec::new();
    for n in 0..8 {
        if c & (HAS_TEXCOORD << n) == 0 {
            break;
        }
        let _components = m.u32()?; // 2 or 3; always stored as 3 floats
        uvs.push(m.vec3s(num_vertices)?);
    }
    if m.painter {
        num_faces = m.u32()? as usize;
    }
    let wide = m.painter || num_vertices >= 1 << 16;
    let mut faces = Vec::with_capacity(num_faces.min(m.b.len() / 2));
    for _ in 0..num_faces {
        let count = if m.painter {
            // ponytail: unknown flag; looks like 1 = starts a polygon, 0 = continues it.
            let _flag = m.u32()?;
            m.u32()?
        } else {
            m.u16()? as u32
        };
        let face = (0..count)
            .map(|_| {
                let i = if wide { m.u32()? } else { m.u16()? as u32 };
                if i as usize >= num_vertices {
                    bail!("assbin: face index {i} out of range ({num_vertices} vertices)");
                }
                Ok(i)
            })
            .collect::<Result<Vec<_>>>()?;
        faces.push(face);
    }
    Ok(AiMesh {
        positions,
        normals,
        uvs,
        faces,
        material,
    })
}

/// The material's `?mat.name`, or empty.
fn read_material(r: &mut Reader) -> Result<String> {
    let mut m = r.chunk(CHUNK_MATERIAL)?;
    let mut name = String::new();
    for _ in 0..m.u32()? {
        let mut p = m.chunk(CHUNK_MATERIAL_PROPERTY)?;
        let key = p.string()?;
        let (_semantic, _index) = (p.u32()?, p.u32()?);
        let (a, b) = (p.u32()?, p.u32()?);
        let len = if p.painter { b } else { a }; // the other is the type
        let data = p.bytes(len as usize)?;
        if key == "?mat.name" {
            // An aiString property: u32 length, the bytes, a terminating zero.
            name = Reader::new(data, false).string()?;
        }
    }
    Ok(name)
}

#[derive(Clone)]
struct Reader<'a> {
    b: &'a [u8],
    /// Painter's variant of the layout (see the module docs).
    painter: bool,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8], painter: bool) -> Self {
        Self { b, painter }
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.b.len() {
            bail!("assbin: unexpected end of data");
        }
        let (head, tail) = self.b.split_at(n);
        self.b = tail;
        Ok(head)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into()?))
    }
    fn string(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        Ok(String::from_utf8_lossy(self.bytes(len)?).into_owned())
    }
    /// A per-vertex array of `n` elements of `stride` bytes (length-prefixed in Painter's layout).
    fn array(&mut self, n: usize, stride: usize) -> Result<&'a [u8]> {
        if self.painter {
            let len = self.u32()? as usize;
            if len != n {
                bail!("assbin: array of {len} elements, expected {n}");
            }
        }
        self.bytes(n.checked_mul(stride).context("assbin: array too large")?)
    }
    fn vec3s(&mut self, n: usize) -> Result<Vec<Vec3>> {
        Ok(self
            .array(n, 12)?
            .as_chunks::<12>()
            .0
            .iter()
            .map(|c| {
                let f = |k: usize| f32::from_le_bytes(c[k..k + 4].try_into().unwrap());
                Vec3::new(f(0), f(4), f(8))
            })
            .collect())
    }
    /// The body of the next chunk, which must carry `magic` (stock layout).
    fn chunk(&mut self, magic: u32) -> Result<Reader<'a>> {
        let size = if self.painter {
            let size = u64::from_le_bytes(self.bytes(8)?.try_into()?);
            usize::try_from(size)?
                .checked_sub(8)
                .context("assbin: bad chunk size")?
        } else {
            let found = self.u32()?;
            if found != magic {
                bail!("assbin: expected chunk {magic:#x}, found {found:#x}");
            }
            self.u32()? as usize
        };
        Ok(Reader::new(self.bytes(size)?, self.painter))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Writes either layout, so one scene can be built as both.
    struct W {
        b: Vec<u8>,
        painter: bool,
    }

    impl W {
        fn new(painter: bool) -> Self {
            Self {
                b: Vec::new(),
                painter,
            }
        }
        fn u32(&mut self, v: u32) -> &mut Self {
            self.b.extend(v.to_le_bytes());
            self
        }
        fn f32s(&mut self, v: &[f32]) -> &mut Self {
            v.iter().for_each(|f| self.b.extend(f.to_le_bytes()));
            self
        }
        fn str(&mut self, s: &str) -> &mut Self {
            self.u32(s.len() as u32);
            self.b.extend(s.as_bytes());
            self
        }
        /// Array length prefix (Painter only).
        fn len(&mut self, n: u32) -> &mut Self {
            if self.painter {
                self.u32(n);
            }
            self
        }
        fn chunk(&mut self, magic: u32, body: W) -> &mut Self {
            if self.painter {
                self.b.extend((body.b.len() as u64 + 8).to_le_bytes());
            } else {
                self.u32(magic).u32(body.b.len() as u32);
            }
            self.b.extend(body.b);
            self
        }
    }

    /// One triangle in a child node moved +2 on X, material "Body", two UV sets.
    fn scene(painter: bool) -> Vec<u8> {
        let node = |name: &str, tx: f32, child: Option<W>, mesh: Option<u32>| {
            let mut w = W::new(painter);
            w.str(name);
            w.f32s(&[
                1., 0., 0., tx, 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ]);
            let (kids, meshes) = (child.is_some() as u32, mesh.is_some() as u32);
            w.u32(kids).u32(meshes);
            if !painter {
                w.u32(0); // metadata
            }
            w.len(meshes);
            if let Some(m) = mesh {
                w.u32(m);
            }
            w.len(kids);
            if let Some(c) = child {
                w.chunk(CHUNK_NODE, c);
            }
            w
        };
        let root = node("root", 0.0, Some(node("tri", 2.0, None, Some(0))), None);

        let mut m = W::new(painter);
        let flags = HAS_POSITIONS | HAS_NORMALS | HAS_TEXCOORD | (HAS_TEXCOORD << 1);
        if painter {
            m.str("mesh").u32(4).u32(0);
        } else {
            m.u32(4).u32(3).u32(1).u32(0).u32(0);
        }
        m.u32(flags);
        m.len(3).f32s(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]);
        m.len(3).f32s(&[0., 0., 1., 0., 0., 1., 0., 0., 1.]);
        for _ in 0..2 {
            m.u32(2).len(3).f32s(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]);
        }
        if painter {
            m.u32(1).u32(1).u32(3).u32(0).u32(1).u32(2);
        } else {
            m.b.extend([3u16, 0, 1, 2].iter().flat_map(|i| i.to_le_bytes()));
        }

        let mut data = W::new(painter);
        data.str("Body").b.push(0);
        let mut prop = W::new(painter);
        prop.str("?mat.name").u32(0).u32(0);
        let (len, ty) = (data.b.len() as u32, 3);
        if painter {
            prop.u32(ty).u32(len);
        } else {
            prop.u32(len).u32(ty);
        }
        prop.b.extend(data.b);
        let mut mat = W::new(painter);
        mat.u32(1).chunk(CHUNK_MATERIAL_PROPERTY, prop);

        let mut s = W::new(painter);
        s.u32(0);
        if !painter {
            s.u32(1).u32(1).u32(0).u32(0).u32(0).u32(0);
        }
        s.chunk(CHUNK_NODE, root);
        s.len(1).chunk(CHUNK_MESH, m);
        s.len(1).chunk(CHUNK_MATERIAL, mat);
        let mut out = W::new(painter);
        out.chunk(CHUNK_SCENE, s);
        out.b
    }

    fn dump(painter: bool, compressed: bool) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_LEN];
        out[..44].copy_from_slice(b"ASSIMP.binary-dump.Thu Jan  1 00:00:00 1970\n");
        out[44..48].copy_from_slice(&1u32.to_le_bytes());
        out[62] = compressed as u8;
        let scene = scene(painter);
        if compressed {
            out.extend((scene.len() as u32).to_le_bytes());
            let mut z = flate2::write::ZlibEncoder::new(out, flate2::Compression::best());
            z.write_all(&scene).unwrap();
            z.finish().unwrap()
        } else {
            out.extend(scene);
            out
        }
    }

    #[test]
    fn hand_built_triangle() {
        for (painter, compressed) in [(false, false), (false, true), (true, false), (true, true)] {
            let mesh = read_assbin(&dump(painter, compressed)).unwrap();
            assert_eq!(mesh.parts.len(), 1);
            assert_eq!(mesh.parts[0].material, "Body");
            assert_eq!(mesh.indices.len(), 3);
            assert!(mesh.has_uvs);
            let (lo, hi) = mesh.bounds();
            assert_eq!((lo, hi), (Vec3::new(2., 0., 0.), Vec3::new(3., 1., 0.)));
            for v in &mesh.vertices {
                assert_eq!(v.normal, [0., 0., 1.]);
                assert_eq!(v.uv1, v.uv);
                assert_eq!(v.tangent, [1., 0., 0., 1.]);
            }
        }
    }

    /// Reads every `*.assbin` in `REETCHED_ASSBIN_DIR` (private samples, not in the repo).
    #[test]
    fn sample_dumps() {
        let Ok(dir) = std::env::var("REETCHED_ASSBIN_DIR") else {
            eprintln!("REETCHED_ASSBIN_DIR not set; skipping assbin samples");
            return;
        };
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "assbin") {
                continue;
            }
            seen += 1;
            let mesh = read_assbin(&std::fs::read(&path).unwrap())
                .unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
            let name = path.file_name().unwrap().to_string_lossy();
            assert!(mesh.has_uvs, "{name}: no UVs");
            assert!(mesh.triangle_count() > 0 && !mesh.vertices.is_empty());
            assert!(mesh.vertices.len() <= mesh.indices.len());
            assert!(
                mesh.indices
                    .iter()
                    .all(|&i| (i as usize) < mesh.vertices.len())
            );
            let bad_normals = mesh
                .vertices
                .iter()
                .filter(|v| (Vec3::from(v.normal).length() - 1.0).abs() > 1e-3)
                .count();
            assert_eq!(bad_normals, 0, "{name}: non-unit normals");
            let (lo, hi) = mesh.bounds();
            let (mut uv_lo, mut uv_hi) = (glam::Vec2::MAX, glam::Vec2::MIN);
            for v in &mesh.vertices {
                uv_lo = uv_lo.min(v.uv.into());
                uv_hi = uv_hi.max(v.uv.into());
            }
            println!(
                "{name}: {} vertices, {} triangles, bounds {lo:.3} .. {hi:.3}, uv {uv_lo:.3} .. {uv_hi:.3}",
                mesh.vertices.len(),
                mesh.triangle_count()
            );
            for (i, p) in mesh.parts.iter().enumerate() {
                assert!(!p.material.is_empty(), "{name}: unnamed part");
                let tiles: Vec<u32> = mesh.udim_tiles(i).iter().map(|t| t.0).collect();
                let covered = uv_coverage(&mesh, i);
                println!(
                    "  part {:?}: {} triangles, tiles {tiles:?}, covers {:.1}% of 0..1",
                    p.material,
                    p.indices.len() / 3,
                    covered * 100.0
                );
                // Every part should cover a real share of its UV square (a
                // misread face list once left only slivers).
                assert!(
                    covered > 0.05,
                    "{name} {:?}: covers only {covered}",
                    p.material
                );
            }
        }
        assert!(seen > 0, "no .assbin files in {dir}");
    }

    /// Fraction of a 512² grid over the 0..1 UV square whose texel centres a
    /// triangle of part `part` covers.
    fn uv_coverage(mesh: &Mesh, part: usize) -> f32 {
        const S: usize = 512;
        let mut grid = vec![false; S * S];
        let range = mesh.parts[part].indices.clone();
        for t in mesh.indices[range.start as usize..range.end as usize]
            .as_chunks::<3>()
            .0
        {
            let [a, b, c] = t.map(|i| glam::Vec2::from(mesh.vertices[i as usize].uv) * S as f32);
            let lo = a.min(b).min(c).floor().max(glam::Vec2::ZERO);
            let hi = a.max(b).max(c).ceil().min(glam::Vec2::splat(S as f32));
            let edge = |p: glam::Vec2, q: glam::Vec2, x: glam::Vec2| (q - p).perp_dot(x - p);
            for y in lo.y as usize..hi.y as usize {
                for x in lo.x as usize..hi.x as usize {
                    let pt = glam::Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let w = [edge(a, b, pt), edge(b, c, pt), edge(c, a, pt)];
                    if w.iter().all(|&w| w >= 0.0) || w.iter().all(|&w| w <= 0.0) {
                        grid[y * S + x] = true;
                    }
                }
            }
        }
        grid.iter().filter(|&&g| g).count() as f32 / (S * S) as f32
    }
}
