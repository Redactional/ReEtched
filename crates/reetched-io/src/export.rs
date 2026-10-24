//! Mesh export: OBJ (+ MTL) and USD (`.usda` with UsdPreviewSurface
//! materials; `.usdc`/`.usdz` in `usd.rs`), optionally displaced by the height channel.

use crate::Mesh;
pub use crate::usd::{write_usdc, write_usdz};
use anyhow::{Context, Result};
use glam::Vec3;
use std::{collections::HashMap, fmt::Write as _, path::Path};

/// Texture files (relative to the mesh file) for one texture set.
#[derive(Clone, Debug, Default)]
pub struct MaterialFiles {
    pub name: String,
    pub base_color: Option<String>,
    pub metallic: Option<String>,
    pub roughness: Option<String>,
    /// OpenGL-convention tangent-space normal map.
    pub normal: Option<String>,
    pub emissive: Option<String>,
    pub opacity: Option<String>,
}

/// Height offset (world units) for part `part` at `uv`.
pub type Displace<'a> = &'a dyn Fn(usize, [f32; 2]) -> f32;

/// Vertex positions, displaced along averaged normals. Vertices that share a
/// position (split at UV seams or hard edges) move together, so no cracks open.
fn positions(mesh: &Mesh, displace: Option<Displace>) -> Vec<Vec3> {
    let pos: Vec<Vec3> = mesh
        .vertices
        .iter()
        .map(|v| Vec3::from(v.position))
        .collect();
    let Some(displace) = displace else {
        return pos;
    };
    let mut part_of = vec![0usize; mesh.vertices.len()];
    for (p, part) in mesh.parts.iter().enumerate() {
        for &i in &mesh.indices[part.indices.start as usize..part.indices.end as usize] {
            part_of[i as usize] = p;
        }
    }
    let key = |v: usize| mesh.vertices[v].position.map(f32::to_bits);
    let mut sum: HashMap<[u32; 3], (Vec3, f32, f32)> = HashMap::new();
    for (i, v) in mesh.vertices.iter().enumerate() {
        let e = sum.entry(key(i)).or_insert((Vec3::ZERO, 0.0, 0.0));
        e.0 += Vec3::from(v.normal);
        e.1 += displace(part_of[i], v.uv);
        e.2 += 1.0;
    }
    pos.iter()
        .enumerate()
        .map(|(i, p)| {
            let (n, d, k) = sum[&key(i)];
            *p + n.normalize_or_zero() * (d / k)
        })
        .collect()
}

pub fn write_obj(
    path: &Path,
    mesh: &Mesh,
    displace: Option<Displace>,
    materials: &[MaterialFiles],
) -> Result<()> {
    let stem = path
        .file_stem()
        .map_or("mesh".into(), |s| s.to_string_lossy().into_owned());
    let mtl_name = format!("{stem}.mtl");
    let mut obj = format!("# Exported by ReEtched\nmtllib {mtl_name}\n");
    for p in positions(mesh, displace) {
        writeln!(obj, "v {} {} {}", p.x, p.y, p.z)?;
    }
    for v in &mesh.vertices {
        writeln!(obj, "vt {} {}", v.uv[0], v.uv[1])?;
    }
    for v in &mesh.vertices {
        let [x, y, z] = v.normal;
        writeln!(obj, "vn {x} {y} {z}")?;
    }
    for part in &mesh.parts {
        writeln!(obj, "usemtl {}", obj_name(&part.material))?;
        for t in mesh.indices[part.indices.start as usize..part.indices.end as usize]
            .as_chunks::<3>()
            .0
        {
            let [a, b, c] = t.map(|i| i + 1);
            writeln!(obj, "f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}")?;
        }
    }
    let mut mtl = String::from("# Exported by ReEtched\n");
    for m in materials {
        writeln!(mtl, "newmtl {}\nKd 1 1 1", obj_name(&m.name))?;
        let maps = [
            ("map_Kd", &m.base_color),
            ("map_Pm", &m.metallic),
            ("map_Pr", &m.roughness),
            ("norm", &m.normal),
            ("map_Ke", &m.emissive),
            ("map_d", &m.opacity),
        ];
        for (key, file) in maps {
            if let Some(f) = file {
                writeln!(mtl, "{key} {f}")?;
            }
        }
        if m.emissive.is_some() {
            writeln!(mtl, "Ke 1 1 1")?;
        }
        mtl.push('\n');
    }
    std::fs::write(path, obj).with_context(|| format!("could not write {}", path.display()))?;
    let mtl_path = path.with_file_name(mtl_name);
    std::fs::write(&mtl_path, mtl)
        .with_context(|| format!("could not write {}", mtl_path.display()))
}

fn obj_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect()
}

/// A valid USD prim name.
fn usd_name(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

pub fn write_usda(
    path: &Path,
    mesh: &Mesh,
    displace: Option<Displace>,
    materials: &[MaterialFiles],
) -> Result<()> {
    let u = usda_text(mesh, displace, materials)?;
    std::fs::write(path, u).with_context(|| format!("could not write {}", path.display()))
}

/// The `.usda` text; the usdc/usdz writers convert it to binary.
pub(crate) fn usda_text(
    mesh: &Mesh,
    displace: Option<Displace>,
    materials: &[MaterialFiles],
) -> Result<String> {
    let list = |items: Vec<String>| items.join(", ");
    let pos = positions(mesh, displace);
    let mut u = String::new();
    writeln!(
        u,
        "#usda 1.0\n(\n    defaultPrim = \"Root\"\n    doc = \"Exported by ReEtched\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n)\n"
    )?;
    writeln!(u, "def Xform \"Root\"\n{{")?;
    writeln!(
        u,
        "    def Mesh \"Mesh\" (\n        prepend apiSchemas = [\"MaterialBindingAPI\"]\n    )\n    {{"
    )?;
    let tris = mesh.indices.len() / 3;
    writeln!(
        u,
        "        int[] faceVertexCounts = [{}]",
        list(vec!["3".into(); tris])
    )?;
    writeln!(
        u,
        "        int[] faceVertexIndices = [{}]",
        list(mesh.indices.iter().map(u32::to_string).collect())
    )?;
    writeln!(
        u,
        "        point3f[] points = [{}]",
        list(
            pos.iter()
                .map(|p| format!("({}, {}, {})", p.x, p.y, p.z))
                .collect()
        )
    )?;
    writeln!(
        u,
        "        normal3f[] normals = [{}] (\n            interpolation = \"vertex\"\n        )",
        list(
            mesh.vertices
                .iter()
                .map(|v| format!("({}, {}, {})", v.normal[0], v.normal[1], v.normal[2]))
                .collect()
        )
    )?;
    writeln!(
        u,
        "        texCoord2f[] primvars:st = [{}] (\n            interpolation = \"vertex\"\n        )",
        list(
            mesh.vertices
                .iter()
                .map(|v| format!("({}, {})", v.uv[0], v.uv[1]))
                .collect()
        )
    )?;
    writeln!(u, "        uniform token subdivisionScheme = \"none\"")?;
    // Each face is in exactly one material subset.
    writeln!(
        u,
        "        uniform token subsetFamily:materialBind:familyType = \"partition\""
    )?;
    for part in &mesh.parts {
        let name = usd_name(&part.material);
        let faces: Vec<String> = (part.indices.start / 3..part.indices.end / 3)
            .map(|f| f.to_string())
            .collect();
        writeln!(
            u,
            "\n        def GeomSubset \"{name}\" (\n            prepend apiSchemas = [\"MaterialBindingAPI\"]\n        )\n        {{\n            uniform token elementType = \"face\"\n            uniform token familyName = \"materialBind\"\n            int[] indices = [{}]\n            rel material:binding = </Root/Materials/{name}>\n        }}",
            list(faces)
        )?;
    }
    writeln!(u, "    }}\n\n    def Scope \"Materials\"\n    {{")?;
    for m in materials {
        let name = usd_name(&m.name);
        let base = format!("/Root/Materials/{name}");
        writeln!(
            u,
            "        def Material \"{name}\"\n        {{\n            token outputs:surface.connect = <{base}/Surface.outputs:surface>\n"
        )?;
        let mut inputs = Vec::new();
        let mut readers = String::new();
        let mut tex = |id: &str,
                       file: &Option<String>,
                       input: &str,
                       ty: &str,
                       out: &str,
                       raw: bool,
                       extra: &str|
         -> Result<()> {
            let Some(f) = file else { return Ok(()) };
            inputs.push(format!(
                "                {ty} inputs:{input}.connect = <{base}/{id}.outputs:{out}>"
            ));
            writeln!(
                readers,
                "            def Shader \"{id}\"\n            {{\n                uniform token info:id = \"UsdUVTexture\"\n                asset inputs:file = @{f}@\n                float2 inputs:st.connect = <{base}/St.outputs:result>\n                token inputs:sourceColorSpace = \"{}\"\n{extra}                {} outputs:{out}\n            }}\n",
                if raw { "raw" } else { "sRGB" },
                if out == "rgb" { "float3" } else { "float" },
            )?;
            Ok(())
        };
        tex(
            "BaseColor",
            &m.base_color,
            "diffuseColor",
            "color3f",
            "rgb",
            false,
            "",
        )?;
        tex("Metallic", &m.metallic, "metallic", "float", "r", true, "")?;
        tex(
            "Roughness",
            &m.roughness,
            "roughness",
            "float",
            "r",
            true,
            "",
        )?;
        tex(
            "Normal",
            &m.normal,
            "normal",
            "normal3f",
            "rgb",
            true,
            "                float4 inputs:scale = (2, 2, 2, 1)\n                float4 inputs:bias = (-1, -1, -1, 0)\n",
        )?;
        tex(
            "Emissive",
            &m.emissive,
            "emissiveColor",
            "color3f",
            "rgb",
            false,
            "",
        )?;
        tex("Opacity", &m.opacity, "opacity", "float", "r", true, "")?;
        writeln!(
            u,
            "            def Shader \"Surface\"\n            {{\n                uniform token info:id = \"UsdPreviewSurface\"\n{}\n                token outputs:surface\n            }}\n",
            inputs.join("\n")
        )?;
        writeln!(
            u,
            "            def Shader \"St\"\n            {{\n                uniform token info:id = \"UsdPrimvarReader_float2\"\n                string inputs:varname = \"st\"\n                float2 outputs:result\n            }}\n"
        )?;
        u.push_str(&readers);
        writeln!(u, "        }}")?;
    }
    writeln!(u, "    }}\n}}")?;
    Ok(u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MeshPart, Vertex};

    fn quad() -> Mesh {
        let v = |x: f32, y: f32| Vertex {
            position: [x, y, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [x, y],
            tangent: [1.0, 0.0, 0.0, 1.0],
            ..Default::default()
        };
        Mesh {
            vertices: vec![v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)],
            indices: vec![0, 1, 2, 0, 2, 3],
            parts: vec![MeshPart {
                material: "Body Paint".into(),
                indices: 0..6,
            }],
            has_uvs: true,
        }
    }

    fn files() -> Vec<MaterialFiles> {
        vec![MaterialFiles {
            name: "Body Paint".into(),
            base_color: Some("body_BaseColor.png".into()),
            normal: Some("body_Normal.png".into()),
            ..Default::default()
        }]
    }

    #[test]
    fn obj_round_trips_and_displaces() {
        let dir = std::env::temp_dir().join("reetched-mesh-export");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("quad.obj");
        // Raise the right edge (u = 1) by 0.5.
        let lift = |_: usize, uv: [f32; 2]| uv[0] * 0.5;
        write_obj(&path, &quad(), Some(&lift), &files()).unwrap();
        let back = crate::load_mesh(&path).unwrap();
        assert_eq!(back.indices.len(), 6);
        assert_eq!(back.parts[0].material, "Body_Paint");
        let max_z = back
            .vertices
            .iter()
            .map(|v| v.position[2])
            .fold(0.0, f32::max);
        assert!((max_z - 0.5).abs() < 1e-5, "displaced along +z: {max_z}");
        let mtl = std::fs::read_to_string(dir.join("quad.mtl")).unwrap();
        assert!(mtl.contains("map_Kd body_BaseColor.png") && mtl.contains("norm body_Normal.png"));
    }

    #[test]
    fn usda_has_mesh_subsets_and_preview_surface() {
        let dir = std::env::temp_dir().join("reetched-mesh-export");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("quad.usda");
        write_usda(&path, &quad(), None, &files()).unwrap();
        let u = std::fs::read_to_string(&path).unwrap();
        assert!(u.starts_with("#usda 1.0"));
        for want in [
            "int[] faceVertexIndices = [0, 1, 2, 0, 2, 3]",
            "def GeomSubset \"Body_Paint\"",
            "rel material:binding = </Root/Materials/Body_Paint>",
            "uniform token info:id = \"UsdPreviewSurface\"",
            "color3f inputs:diffuseColor.connect = </Root/Materials/Body_Paint/BaseColor.outputs:rgb>",
            "asset inputs:file = @body_Normal.png@",
            "float4 inputs:bias = (-1, -1, -1, 0)",
        ] {
            assert!(u.contains(want), "missing {want}\n{u}");
        }
        // Balanced braces.
        assert_eq!(u.matches('{').count(), u.matches('}').count());
    }
}
