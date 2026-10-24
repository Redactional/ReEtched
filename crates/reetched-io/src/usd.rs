//! USD import (`.usd`, `.usda`, `.usdc`, `.usdz`) and binary export
//! (`.usdc`, `.usdz`), both through the pure-Rust `openusd` crate.
//!
//! Import reads the composed stage, so references, payloads, sublayers and
//! variants are handled by the library. The binary writers reuse the text that
//! `write_usda` produces: parse it into layer data, then write that as a crate
//! file, so all three formats always carry the same scene.

use crate::{
    Mesh, Soup, Vertex,
    export::{Displace, MaterialFiles, usda_text},
};
use anyhow::{Context, Result, anyhow, bail};
use glam::{DMat4, DQuat, DVec3, Mat4, Vec3};
use openusd::{
    sdf::Value,
    usd::{Prim, PrimPredicate, Stage, TimeCode},
    usdc::CrateWriter,
    usdz::ArchiveWriter,
};
use std::{collections::HashSet, f64::consts::FRAC_PI_2, io::Cursor, path::Path};

pub(crate) fn load(path: &Path) -> Result<Soup> {
    let path_str = path.to_str().context("mesh path is not valid UTF-8")?;
    let stage =
        Stage::open(path_str).map_err(|e| anyhow!("failed to load {}: {e}", path.display()))?;
    // Unresolvable references and the like: import what did compose.
    for e in stage.composition_errors() {
        log::warn!("{}: {e}", path.display());
    }
    let z_up =
        matches!(stage.stage_metadata("upAxis")?, Some(Value::Token(t)) if t.as_str() == "Z");
    // Z-up to Y-up, the same rotation ufbx applies to Z-up FBX files.
    let axis = if z_up {
        DMat4::from_rotation_x(-FRAC_PI_2)
    } else {
        DMat4::IDENTITY
    };

    let mut paths = Vec::new();
    stage.traverse(PrimPredicate::DEFAULT_PROXIES, |p| paths.push(p.clone()))?;
    let mut soup = Soup::default();
    for p in paths {
        let prim = stage.prim(p.clone())?;
        if prim.type_name()?.as_deref() != Some("Mesh") {
            continue;
        }
        let world = axis * world_transform(&prim)?;
        read_mesh(&prim, world, &mut soup).with_context(|| format!("USD mesh {p}"))?;
    }
    Ok(soup)
}

fn read_mesh(prim: &Prim, world: DMat4, soup: &mut Soup) -> Result<()> {
    let Some(points) = floats(prim, "points")? else {
        return Ok(());
    };
    let points: Vec<Vec3> = points
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| world.transform_point3(DVec3::from_array(*p)).as_vec3())
        .collect();
    let counts = ints(prim, "faceVertexCounts")?.unwrap_or_default();
    let fvi = ints(prim, "faceVertexIndices")?.unwrap_or_default();
    if counts.iter().any(|&c| c < 0)
        || counts.iter().map(|&c| c as usize).sum::<usize>() != fvi.len()
    {
        bail!("faceVertexCounts do not add up to faceVertexIndices");
    }
    if fvi.iter().any(|&i| i < 0 || i as usize >= points.len()) {
        bail!("faceVertexIndices out of range");
    }
    let holes: HashSet<i32> = ints(prim, "holeIndices")?
        .unwrap_or_default()
        .into_iter()
        .collect();

    let sizes = Sizes {
        faces: counts.len(),
        points: points.len(),
        corners: fvi.len(),
    };
    let normals = match Primvar::read(prim, "primvars:normals", 3, sizes)? {
        Some(n) => Some(n),
        None => Primvar::read(prim, "normals", 3, sizes)?,
    };
    // Texture coordinates: `st` first, then any other texCoord2f primvar.
    let mut tex: Vec<String> = Vec::new();
    for attr in prim.attributes_in_namespace("primvars")? {
        let name = attr.name().to_owned();
        if !name.ends_with(":indices")
            && attr
                .type_name()?
                .is_some_and(|t| t.as_str() == "texCoord2f[]")
        {
            tex.push(name);
        }
    }
    let mut first = None;
    for name in std::iter::once("primvars:st").chain(tex.iter().map(String::as_str)) {
        if let Some(pv) = Primvar::read(prim, name, 2, sizes)? {
            first = Some((name.to_owned(), pv));
            break;
        }
    }
    let mut second = None;
    let candidates = ["primvars:st1", "primvars:st_1"]
        .into_iter()
        .chain(tex.iter().map(String::as_str));
    for name in candidates {
        if first.as_ref().is_some_and(|(f, _)| f == name) {
            continue;
        }
        if let Some(pv) = Primvar::read(prim, name, 2, sizes)? {
            second = Some(pv);
            break;
        }
    }
    let uv0 = first.map(|(_, pv)| pv);
    soup.has_uvs |= uv0.is_some();

    // Per-face material: materialBind subsets, then the mesh's own (or an
    // ancestor's) binding, then the mesh name.
    let fallback =
        inherited_binding(prim)?.unwrap_or_else(|| prim.path().name().unwrap_or("mesh").to_owned());
    let mut face_material = vec![None; counts.len()];
    for child in prim.children()? {
        if child.type_name()?.as_deref() != Some("GeomSubset")
            || token(&child, "familyName")?.as_deref() != Some("materialBind")
            || token(&child, "elementType")?.is_some_and(|t| t != "face")
        {
            continue;
        }
        let Some(material) = binding(&child)? else {
            continue;
        };
        for f in ints(&child, "indices")?.unwrap_or_default() {
            if let Some(slot) = face_material.get_mut(f as usize) {
                *slot = Some(material.clone());
            }
        }
    }

    // Mirroring transforms and leftHanded orientation each flip the winding.
    let left = token(prim, "orientation")?.as_deref() == Some("leftHanded");
    let flip = left != (world.determinant() < 0.0);
    let normal_m: Mat4 = world.inverse().transpose().as_mat4();
    let mut start = 0;
    for (f, &count) in counts.iter().enumerate() {
        let count = count as usize;
        let face = start..start + count;
        start += count;
        if count < 3 || holes.contains(&(f as i32)) {
            continue;
        }
        let out = soup.group(face_material[f].as_deref().unwrap_or(&fallback));
        // ponytail: fan triangulation, wrong for concave n-gons; ear clipping if that bites.
        for i in 1..count - 1 {
            let mut tri = [face.start, face.start + i, face.start + i + 1];
            if flip {
                tri.swap(1, 2);
            }
            let p = tri.map(|c| points[fvi[c] as usize]);
            let flat = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
            for (k, c) in tri.into_iter().enumerate() {
                let pt = fvi[c] as usize;
                let normal = normals.as_ref().map_or(flat, |n| {
                    let v = n.at(f, pt, c);
                    normal_m
                        .transform_vector3(Vec3::new(v[0], v[1], v[2]))
                        .normalize_or_zero()
                });
                let uv = |pv: &Option<Primvar>| {
                    pv.as_ref().map_or([0.0; 2], |pv| {
                        let v = pv.at(f, pt, c);
                        [v[0], v[1]]
                    })
                };
                out.push(Vertex {
                    position: p[k].into(),
                    normal: normal.into(),
                    uv: uv(&uv0),
                    tangent: [0.0; 4],
                    uv1: uv(&second),
                });
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Sizes {
    faces: usize,
    points: usize,
    corners: usize,
}

/// A primvar resolved to one value per element of its interpolation.
struct Primvar {
    values: Vec<f32>,
    width: usize,
    indices: Option<Vec<i32>>,
    interpolation: String,
}

impl Primvar {
    /// `None` when absent or malformed (logged): a bad primvar shouldn't sink the mesh.
    fn read(prim: &Prim, name: &str, width: usize, sizes: Sizes) -> Result<Option<Primvar>> {
        let Some(values) = floats(prim, name)? else {
            return Ok(None);
        };
        let values: Vec<f32> = values.into_iter().map(|v| v as f32).collect();
        let interpolation = match prim
            .attribute(name)
            .get_metadata::<Value>("interpolation")?
        {
            Some(Value::Token(t)) => t.as_str().to_owned(),
            _ => "constant".to_owned(),
        };
        let indices = ints(prim, &format!("{name}:indices"))?;
        let needed = match interpolation.as_str() {
            "constant" => 1,
            "uniform" => sizes.faces,
            "vertex" | "varying" => sizes.points,
            "faceVarying" => sizes.corners,
            other => {
                log::warn!("{}: unknown interpolation {other:?}", prim.path());
                return Ok(None);
            }
        };
        let count = values.len() / width;
        let ok = match &indices {
            Some(ix) => ix.len() >= needed && ix.iter().all(|&i| i >= 0 && (i as usize) < count),
            None => count >= needed,
        };
        if !ok {
            log::warn!("{}: {name} has too few values; ignored", prim.path());
            return Ok(None);
        }
        Ok(Some(Primvar {
            values,
            width,
            indices,
            interpolation,
        }))
    }

    /// The value for corner `corner` (of point `point`) on face `face`.
    fn at(&self, face: usize, point: usize, corner: usize) -> &[f32] {
        let i = match self.interpolation.as_str() {
            "constant" => 0,
            "uniform" => face,
            "vertex" | "varying" => point,
            _ => corner,
        };
        let i = self.indices.as_ref().map_or(i, |ix| ix[i] as usize);
        &self.values[i * self.width..(i + 1) * self.width]
    }
}

/// Bound material name of `prim` (direct `material:binding`).
fn binding(prim: &Prim) -> Result<Option<String>> {
    let targets = prim.relationship("material:binding").targets()?;
    Ok(targets.first().and_then(|t| t.name()).map(str::to_owned))
}

/// Bindings inherit down the hierarchy: the nearest bound ancestor wins.
fn inherited_binding(prim: &Prim) -> Result<Option<String>> {
    let mut cur = Some(prim.clone());
    while let Some(p) = cur {
        if let Some(m) = binding(&p)? {
            return Ok(Some(m));
        }
        cur = p.parent();
    }
    Ok(None)
}

/// Local-to-world from the `xformOp`s of `prim` and its ancestors.
fn world_transform(prim: &Prim) -> Result<DMat4> {
    let mut world = DMat4::IDENTITY;
    let mut cur = Some(prim.clone());
    while let Some(p) = cur {
        let (local, reset) = local_transform(&p)?;
        world = local * world;
        if reset {
            break;
        }
        cur = p.parent();
    }
    Ok(world)
}

/// The prim's own transform, and whether it ignores its parents (`!resetXformStack!`).
fn local_transform(prim: &Prim) -> Result<(DMat4, bool)> {
    let order = match value(prim, "xformOpOrder")? {
        Some(Value::TokenVec(t)) => t,
        _ => return Ok((DMat4::IDENTITY, false)),
    };
    let mut m = DMat4::IDENTITY;
    let mut reset = false;
    for op in &order {
        let op = op.as_str();
        if op == "!resetXformStack!" {
            m = DMat4::IDENTITY;
            reset = true;
            continue;
        }
        let (invert, attr) = match op.strip_prefix("!invert!") {
            Some(a) => (true, a),
            None => (false, op),
        };
        let kind = attr.split(':').nth(1).unwrap_or("");
        let Some(v) = value(prim, attr)? else {
            continue;
        };
        let f = flatten(&v).unwrap_or_default();
        let vec3 = || DVec3::new(f[0], f[1], f[2]);
        let rot = |axis: usize, deg: f64| match axis {
            0 => DMat4::from_rotation_x(deg.to_radians()),
            1 => DMat4::from_rotation_y(deg.to_radians()),
            _ => DMat4::from_rotation_z(deg.to_radians()),
        };
        let op_m = match (kind, &v) {
            ("transform", Value::Matrix4d(mat)) => {
                // USD matrices are row-major with row vectors, which is glam's column-major layout.
                DMat4::from_cols_array(&mat.0)
            }
            ("orient", Value::Quatf(q)) => DMat4::from_quat(DQuat::from_xyzw(
                q.x as f64, q.y as f64, q.z as f64, q.w as f64,
            )),
            ("orient", Value::Quatd(q)) => DMat4::from_quat(DQuat::from_xyzw(q.x, q.y, q.z, q.w)),
            ("translate", _) if f.len() == 3 => DMat4::from_translation(vec3()),
            ("scale", _) if f.len() == 3 => DMat4::from_scale(vec3()),
            ("rotateX", _) if f.len() == 1 => rot(0, f[0]),
            ("rotateY", _) if f.len() == 1 => rot(1, f[0]),
            ("rotateZ", _) if f.len() == 1 => rot(2, f[0]),
            (k, _) if k.len() == 9 && k.starts_with("rotate") && f.len() == 3 => {
                // rotateXYZ applies X first: the matrix is Rz * Ry * Rx.
                k[6..].bytes().fold(DMat4::IDENTITY, |acc, axis| {
                    let a = (axis - b'X') as usize;
                    rot(a, f[a]) * acc
                })
            }
            _ => {
                log::warn!("{}: unsupported xform op {op}; ignored", prim.path());
                continue;
            }
        };
        m *= if invert { op_m.inverse() } else { op_m };
    }
    Ok((m, reset))
}

/// The attribute's value: the default, or its earliest time sample.
fn value(prim: &Prim, name: &str) -> Result<Option<Value>> {
    Ok(prim.attribute(name).get_at::<Value>(TimeCode::EARLIEST)?)
}

fn token(prim: &Prim, name: &str) -> Result<Option<String>> {
    Ok(match value(prim, name)? {
        Some(Value::Token(t)) => Some(t.as_str().to_owned()),
        _ => None,
    })
}

fn floats(prim: &Prim, name: &str) -> Result<Option<Vec<f64>>> {
    Ok(value(prim, name)?.as_ref().and_then(flatten))
}

fn ints(prim: &Prim, name: &str) -> Result<Option<Vec<i32>>> {
    Ok(match value(prim, name)? {
        Some(Value::IntVec(v)) => Some(v),
        Some(Value::Int64Vec(v)) => Some(v.into_iter().map(|i| i as i32).collect()),
        Some(Value::UintVec(v)) => Some(v.into_iter().map(|i| i as i32).collect()),
        _ => None,
    })
}

/// Float/double scalars, vectors and arrays as a flat list of components.
// ponytail: half-precision values aren't read; add if a file needs them.
fn flatten(v: &Value) -> Option<Vec<f64>> {
    fn wide(s: &[f32]) -> Vec<f64> {
        s.iter().map(|&x| x as f64).collect()
    }
    use bytemuck::cast_slice;
    Some(match v {
        Value::Float(x) => vec![*x as f64],
        Value::Double(x) => vec![*x],
        Value::FloatVec(a) => wide(a),
        Value::DoubleVec(a) => a.clone(),
        Value::Vec2f(a) => wide(cast_slice(std::slice::from_ref(a))),
        Value::Vec3f(a) => wide(cast_slice(std::slice::from_ref(a))),
        Value::Vec2d(a) => cast_slice(std::slice::from_ref(a)).to_vec(),
        Value::Vec3d(a) => cast_slice(std::slice::from_ref(a)).to_vec(),
        Value::Vec2fVec(a) => wide(cast_slice(a)),
        Value::Vec3fVec(a) => wide(cast_slice(a)),
        Value::Vec2dVec(a) => cast_slice(a).to_vec(),
        Value::Vec3dVec(a) => cast_slice(a).to_vec(),
        _ => return None,
    })
}

fn usdc_bytes(
    mesh: &Mesh,
    displace: Option<Displace>,
    materials: &[MaterialFiles],
) -> Result<Vec<u8>> {
    let text = usda_text(mesh, displace, materials)?;
    let data = openusd::usda::parse(&text).map_err(|e| anyhow!("internal usda is invalid: {e}"))?;
    let mut out = Cursor::new(Vec::new());
    CrateWriter::write(&data, &mut out)?;
    Ok(out.into_inner())
}

/// Binary USD (crate file) with the same scene `write_usda` writes.
pub fn write_usdc(
    path: &Path,
    mesh: &Mesh,
    displace: Option<Displace>,
    materials: &[MaterialFiles],
) -> Result<()> {
    let bytes = usdc_bytes(mesh, displace, materials)?;
    std::fs::write(path, bytes).with_context(|| format!("could not write {}", path.display()))
}

/// A USDZ package: the stage as usdc (first entry, the package's root layer)
/// plus every texture `materials` names, read from `texture_dir`.
pub fn write_usdz(
    path: &Path,
    mesh: &Mesh,
    displace: Option<Displace>,
    materials: &[MaterialFiles],
    texture_dir: &Path,
) -> Result<()> {
    let stem = path
        .file_stem()
        .map_or("mesh".into(), |s| s.to_string_lossy().into_owned());
    let layer = usdc_bytes(mesh, displace, materials)?;
    let mut zip = ArchiveWriter::create(path)?;
    zip.add_layer(&format!("{stem}.usdc"), &layer)?;
    let mut seen = HashSet::new();
    for m in materials {
        let files = [
            &m.base_color,
            &m.metallic,
            &m.roughness,
            &m.normal,
            &m.emissive,
            &m.opacity,
        ];
        for f in files.into_iter().flatten() {
            if !seen.insert(f) {
                continue;
            }
            let file = texture_dir.join(f);
            let bytes = std::fs::read(&file)
                .with_context(|| format!("could not read texture {}", file.display()))?;
            zip.add_layer(f, &bytes)?;
        }
    }
    zip.finish()?;
    Ok(())
}
