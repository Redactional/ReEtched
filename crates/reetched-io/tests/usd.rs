//! USD import/export against fixtures written by Pixar's USD (see
//! tests/data/usd/make_fixtures.py for what the scene holds).

use glam::{Vec2, Vec3};
use reetched_io::{
    Mesh,
    export::{MaterialFiles, write_usdc, write_usdz},
    load_mesh,
};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/usd")
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("reetched-io-usd-test");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn part_tris(mesh: &Mesh) -> Vec<(&str, usize)> {
    mesh.parts
        .iter()
        .map(|p| (p.material.as_str(), p.indices.len() / 3))
        .collect()
}

fn triangles(mesh: &Mesh) -> impl Iterator<Item = [&reetched_io::Vertex; 3]> {
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| &mesh.vertices[i as usize]))
}

/// The scene is Z-up; after import everything lies in Y-up space:
/// Box (quad + pentagon) in the y = 0 plane, Tri in the y = 5 plane.
fn check_scene(mesh: &Mesh) {
    assert_eq!(mesh.triangle_count(), 6);
    assert_eq!(part_tris(mesh), [("MatA", 2), ("MatB", 4)]);
    assert!(mesh.has_uvs);
    let (lo, hi) = mesh.bounds();
    assert!(lo.abs_diff_eq(Vec3::new(9.0, 0.0, -4.0), 1e-5), "{lo}");
    assert!(hi.abs_diff_eq(Vec3::new(17.0, 5.0, 0.0), 1e-5), "{hi}");

    let mut tri_count = 0;
    for t in triangles(mesh) {
        let p = t.map(|v| Vec3::from(v.position));
        // Every face is front-facing towards +Y: leftHanded Tri was flipped.
        let face_n = (p[1] - p[0]).cross(p[2] - p[0]).normalize();
        assert!(face_n.abs_diff_eq(Vec3::Y, 1e-5), "winding {face_n}");
        for v in t {
            assert!(Vec3::from(v.normal).abs_diff_eq(Vec3::Y, 1e-5));
            let [x, y, z] = v.position;
            if y.abs() < 1e-5 {
                // Box: local point (px, py) = ((x - 10) / 2, -z / 2) under scale 2,
                // st = (px / 4, py / 2), st1 = (py / 2, px / 4).
                let (px, py) = ((x - 10.0) / 2.0, -z / 2.0);
                assert!(Vec2::from(v.uv).abs_diff_eq(Vec2::new(px / 4.0, py / 2.0), 1e-5));
                assert!(Vec2::from(v.uv1).abs_diff_eq(Vec2::new(py / 2.0, px / 4.0), 1e-5));
            } else {
                // Tri: no UVs.
                assert_eq!(v.uv, [0.0; 2]);
                tri_count += 1;
            }
        }
    }
    assert_eq!(tri_count, 3);
    // The pentagon's tip (local (2.5, 2)) and Tri's rotated corner (local (1, 0)).
    let has = |q: Vec3| {
        mesh.vertices
            .iter()
            .any(|v| Vec3::from(v.position).abs_diff_eq(q, 1e-5))
    };
    assert!(has(Vec3::new(15.0, 0.0, -4.0)));
    assert!(has(Vec3::new(10.0, 5.0, -1.0)));
}

#[test]
fn loads_usda() {
    check_scene(&load_mesh(&fixture("scene.usda")).unwrap());
}

#[test]
fn loads_usdc() {
    check_scene(&load_mesh(&fixture("scene.usdc")).unwrap());
}

#[test]
fn loads_usdz() {
    check_scene(&load_mesh(&fixture("scene.usdz")).unwrap());
}

fn materials() -> Vec<MaterialFiles> {
    vec![
        MaterialFiles {
            name: "MatA".into(),
            base_color: Some("MatA_BaseColor.png".into()),
            normal: Some("MatA_Normal.png".into()),
            ..Default::default()
        },
        MaterialFiles {
            name: "MatB".into(),
            ..Default::default()
        },
    ]
}

#[test]
fn usdc_round_trips() {
    let mesh = load_mesh(&fixture("scene.usda")).unwrap();
    let path = scratch("round.usdc");
    write_usdc(&path, &mesh, None, &materials()).unwrap();
    assert!(std::fs::read(&path).unwrap().starts_with(b"PXR-USDC"));
    let back = load_mesh(&path).unwrap();
    assert_eq!(part_tris(&back), part_tris(&mesh));
    assert_eq!(back.indices.len(), mesh.indices.len());
    let corners = |m: &Mesh| -> Vec<[f32; 5]> {
        m.indices
            .iter()
            .map(|&i| {
                let v = &m.vertices[i as usize];
                [
                    v.position[0],
                    v.position[1],
                    v.position[2],
                    v.uv[0],
                    v.uv[1],
                ]
            })
            .collect()
    };
    for (a, b) in corners(&mesh).iter().zip(corners(&back)) {
        for k in 0..5 {
            assert!((a[k] - b[k]).abs() < 1e-5, "{a:?} vs {b:?}");
        }
    }
}

/// (name, data offset, compression method) of each local file entry.
fn zip_entries(bytes: &[u8]) -> Vec<(String, usize, u16)> {
    let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let mut out = Vec::new();
    let mut o = 0;
    while u32_at(o) == 0x0403_4b50 {
        let method = u16_at(o + 8);
        let size = u32_at(o + 18) as usize;
        let (name_len, extra_len) = (u16_at(o + 26) as usize, u16_at(o + 28) as usize);
        let name = String::from_utf8(bytes[o + 30..o + 30 + name_len].to_vec()).unwrap();
        let data = o + 30 + name_len + extra_len;
        out.push((name, data, method));
        o = data + size;
    }
    out
}

#[test]
fn usdz_is_aligned_and_loads() {
    let mesh = load_mesh(&fixture("scene.usdc")).unwrap();
    let dir = scratch("");
    std::fs::write(dir.join("MatA_BaseColor.png"), b"not really a png").unwrap();
    std::fs::write(dir.join("MatA_Normal.png"), b"nor this").unwrap();
    let path = scratch("pkg.usdz");
    write_usdz(&path, &mesh, None, &materials(), &dir).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let entries = zip_entries(&bytes);
    let names: Vec<_> = entries.iter().map(|e| e.0.as_str()).collect();
    assert_eq!(names, ["pkg.usdc", "MatA_BaseColor.png", "MatA_Normal.png"]);
    for (name, data, method) in &entries {
        assert_eq!(*method, 0, "{name} is compressed");
        assert_eq!(data % 64, 0, "{name} is not 64-byte aligned");
    }
    let back = load_mesh(&path).unwrap();
    assert_eq!(part_tris(&back), part_tris(&mesh));
}
