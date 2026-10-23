//! Mesh import. A file becomes one indexed triangle mesh, split into parts by
//! material — each part is what Painter calls a texture set.

use anyhow::{Context, Result, anyhow, bail};
use glam::{Mat4, Vec3};
use std::{collections::HashMap, f32::consts::PI, ops::Range, path::Path};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// OpenGL convention (v = 0 at the bottom), same as FBX/OBJ and Painter.
    pub uv: [f32; 2],
    /// MikkTSpace tangent; w is the bitangent sign.
    pub tangent: [f32; 4],
}

pub struct MeshPart {
    pub material: String,
    pub indices: Range<u32>,
}

pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub parts: Vec<MeshPart>,
    pub has_uvs: bool,
}

impl Mesh {
    pub fn bounds(&self) -> (Vec3, Vec3) {
        self.vertices
            .iter()
            .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), v| {
                let p = Vec3::from(v.position);
                (lo.min(p), hi.max(p))
            })
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Built-in preview mesh, so the viewport isn't empty on startup.
    pub fn uv_sphere(radius: f32, segments: u32, rings: u32) -> Mesh {
        let corner = |i: u32, j: u32| {
            let (u, v) = (i as f32 / segments as f32, j as f32 / rings as f32);
            let (phi, theta) = (u * 2.0 * PI, v * PI);
            let n = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                -theta.sin() * phi.sin(),
            );
            Vertex {
                position: (n * radius).into(),
                normal: n.into(),
                uv: [u, 1.0 - v],
                tangent: [0.0; 4],
            }
        };
        let mut corners = Vec::new();
        for j in 0..rings {
            for i in 0..segments {
                let (a, b) = (corner(i, j), corner(i + 1, j));
                let (c, d) = (corner(i + 1, j + 1), corner(i, j + 1));
                corners.extend([a, d, c, a, c, b]);
            }
        }
        build(Soup {
            groups: vec![("sphere".into(), corners)],
            has_uvs: true,
        })
    }
}

pub fn load_mesh(path: &Path) -> Result<Mesh> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let soup = match ext.as_str() {
        "fbx" | "obj" => load_ufbx(path)?,
        "gltf" | "glb" => load_gltf(path)?,
        _ => bail!("unsupported mesh format '.{ext}' (supported: fbx, obj, gltf, glb)"),
    };
    if soup.groups.iter().all(|(_, c)| c.is_empty()) {
        bail!("{} contains no triangles", path.display());
    }
    Ok(build(soup))
}

/// Triangle corners (3 per triangle) grouped by material, in first-seen order.
#[derive(Default)]
struct Soup {
    groups: Vec<(String, Vec<Vertex>)>,
    has_uvs: bool,
}

impl Soup {
    fn group(&mut self, name: &str) -> &mut Vec<Vertex> {
        let i = match self.groups.iter().position(|(n, _)| n == name) {
            Some(i) => i,
            None => {
                self.groups.push((name.to_owned(), Vec::new()));
                self.groups.len() - 1
            }
        };
        &mut self.groups[i].1
    }
}

fn load_ufbx(path: &Path) -> Result<Soup> {
    let opts = ufbx::LoadOpts {
        target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
        space_conversion: ufbx::SpaceConversion::ModifyGeometry,
        generate_missing_normals: true,
        ..Default::default()
    };
    let path_str = path.to_str().context("mesh path is not valid UTF-8")?;
    let scene = ufbx::load_file(path_str, opts)
        .map_err(|e| anyhow!("failed to load {}: {}", path.display(), e.description))?;

    let mut soup = Soup::default();
    let mut tri = Vec::new();
    for node in scene.nodes.iter() {
        let Some(mesh) = node.mesh.as_ref() else {
            continue;
        };
        let to_world = &node.geometry_to_world;
        let normal_m = ufbx::matrix_for_normals(to_world);
        soup.has_uvs |= mesh.vertex_uv.exists;
        tri.resize(mesh.max_face_triangles * 3, 0);
        for (fi, face) in mesh.faces.iter().enumerate() {
            let name = mesh
                .face_material
                .get(fi)
                .and_then(|&m| mesh.materials.get(m as usize))
                .map(|m| m.element.name.to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| node.element.name.to_string());
            let n = mesh.triangulate_face(&mut tri, *face) as usize;
            let out = soup.group(&name);
            for &ix in &tri[..n * 3] {
                let ix = ix as usize;
                let p = ufbx::transform_position(to_world, mesh.vertex_position[ix]);
                let nn = ufbx::transform_direction(&normal_m, mesh.vertex_normal[ix]);
                let uv = if mesh.vertex_uv.exists {
                    let t = mesh.vertex_uv[ix];
                    [t.x as f32, t.y as f32]
                } else {
                    [0.0; 2]
                };
                out.push(Vertex {
                    position: [p.x as f32, p.y as f32, p.z as f32],
                    normal: Vec3::new(nn.x as f32, nn.y as f32, nn.z as f32)
                        .normalize_or_zero()
                        .into(),
                    uv,
                    tangent: [0.0; 4],
                });
            }
        }
    }
    Ok(soup)
}

fn load_gltf(path: &Path) -> Result<Soup> {
    let (doc, buffers, _) = gltf::import(path)?;
    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .context("glTF has no scenes")?;
    let mut soup = Soup::default();
    let mut stack: Vec<_> = scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((node, parent)) = stack.pop() {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        stack.extend(node.children().map(|c| (c, world)));
        let Some(mesh) = node.mesh() else { continue };
        let normal_m = world.inverse().transpose();
        // Mirrored transforms flip the winding.
        let order: [usize; 3] = if world.determinant() < 0.0 {
            [0, 2, 1]
        } else {
            [0, 1, 2]
        };
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let r = prim.reader(|b| Some(&buffers[b.index()]));
            let Some(pos) = r.read_positions() else {
                continue;
            };
            let pos: Vec<[f32; 3]> = pos.collect();
            let nrm: Option<Vec<[f32; 3]>> = r.read_normals().map(|n| n.collect());
            let uvs: Option<Vec<[f32; 2]>> = r.read_tex_coords(0).map(|t| t.into_f32().collect());
            let idx: Vec<u32> = match r.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..pos.len() as u32).collect(),
            };
            soup.has_uvs |= uvs.is_some();
            let mat = prim.material();
            let name = mat
                .name()
                .map(str::to_owned)
                .unwrap_or_else(|| match mat.index() {
                    Some(i) => format!("material_{i}"),
                    None => "default".into(),
                });
            let out = soup.group(&name);
            for t in idx.as_chunks::<3>().0 {
                let p = t.map(|i| world.transform_point3(pos[i as usize].into()));
                let flat = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
                for k in order {
                    let i = t[k] as usize;
                    let n = nrm.as_ref().map_or(flat, |n| {
                        normal_m.transform_vector3(n[i].into()).normalize_or_zero()
                    });
                    out.push(Vertex {
                        position: p[k].into(),
                        normal: n.into(),
                        // glTF puts v = 0 at the top; convert to the OpenGL convention.
                        uv: uvs.as_ref().map_or([0.0; 2], |u| [u[i][0], 1.0 - u[i][1]]),
                        tangent: [0.0; 4],
                    });
                }
            }
        }
    }
    Ok(soup)
}

/// Generates tangents per material group, then welds identical corners into an indexed mesh.
fn build(soup: Soup) -> Mesh {
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
        parts: Vec::new(),
        has_uvs: soup.has_uvs,
    };
    let mut lookup: HashMap<[u32; 12], u32> = HashMap::new();
    for (material, mut corners) in soup.groups {
        if corners.is_empty() {
            continue;
        }
        let ok =
            soup.has_uvs && bevy_mikktspace::generate_tangents(&mut Corners(&mut corners)).is_ok();
        if !ok {
            corners
                .iter_mut()
                .for_each(|v| v.tangent = [1.0, 0.0, 0.0, 1.0]);
        }
        let start = mesh.indices.len() as u32;
        for v in corners {
            let i = *lookup.entry(bytemuck::cast(v)).or_insert_with(|| {
                mesh.vertices.push(v);
                mesh.vertices.len() as u32 - 1
            });
            mesh.indices.push(i);
        }
        mesh.parts.push(MeshPart {
            material,
            indices: start..mesh.indices.len() as u32,
        });
    }
    mesh
}

struct Corners<'a>(&'a mut [Vertex]);

impl bevy_mikktspace::Geometry for Corners<'_> {
    fn num_faces(&self) -> usize {
        self.0.len() / 3
    }
    fn num_vertices_of_face(&self, _face: usize) -> usize {
        3
    }
    fn position(&self, face: usize, vert: usize) -> [f32; 3] {
        self.0[face * 3 + vert].position
    }
    fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
        self.0[face * 3 + vert].normal
    }
    fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
        self.0[face * 3 + vert].uv
    }
    fn set_tangent(&mut self, ts: Option<bevy_mikktspace::TangentSpace>, face: usize, vert: usize) {
        self.0[face * 3 + vert].tangent = ts.map_or([1.0, 0.0, 0.0, 1.0], |t| t.tangent_encoded());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obj_quad_with_two_materials() {
        let dir = std::env::temp_dir().join("reetched-io-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("quad.obj");
        std::fs::write(
            &path,
            "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n\
             vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nvn 0 0 1\n\
             usemtl A\nf 1/1/1 2/2/1 3/3/1\nusemtl B\nf 1/1/1 3/3/1 4/4/1\n",
        )
        .unwrap();
        let mesh = load_mesh(&path).unwrap();
        let names: Vec<_> = mesh.parts.iter().map(|p| p.material.as_str()).collect();
        assert_eq!(names, ["A", "B"]);
        assert_eq!(mesh.indices.len(), 6);
        assert!(mesh.has_uvs);
        // u runs along +X, v along +Y, normal +Z → tangent +X, bitangent sign +1.
        for v in &mesh.vertices {
            let t = Vec3::from_slice(&v.tangent[..3]);
            assert!(t.abs_diff_eq(Vec3::X, 1e-4), "tangent {t}");
            assert_eq!(v.tangent[3], 1.0);
        }
    }

    #[test]
    fn sphere_is_welded_and_has_tangents() {
        let mesh = Mesh::uv_sphere(1.0, 16, 8);
        assert!(mesh.vertices.len() < mesh.indices.len());
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.tangent.iter().all(|c| c.is_finite()))
        );
        let (lo, hi) = mesh.bounds();
        assert!(lo.abs_diff_eq(-Vec3::ONE, 1e-3) && hi.abs_diff_eq(Vec3::ONE, 1e-3));
    }
}
