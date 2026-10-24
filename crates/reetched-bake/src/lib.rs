//! Mesh map baking on the CPU: projects a high-poly mesh (or the low-poly mesh
//! itself) into the low-poly mesh's UV space.
//!
//! Every map is an RGBA float image, row 0 = top of the texture (v = 1).
//! Encodings (what a 0..1 image file stores):
//! - normal: tangent-space, OpenGL (`n * 0.5 + 0.5`)
//! - world normal: `n * 0.5 + 0.5`
//! - position: normalised to the low mesh's bounding box
//! - height: `0.5 + distance / (2 * max distance)`
//! - AO, thickness, opacity, curvature: grey (curvature 0.5 = flat, brighter = convex)

use glam::{Vec2, Vec3, Vec3A};
use obvhs::{
    BvhBuildParams,
    cwbvh::{CwBvh, builder::build_cwbvh_from_tris},
    ray::{Ray, RayHit},
    triangle::Triangle,
};
use rayon::prelude::*;
use reetched_io::Mesh;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum MapKind {
    Normal,
    WorldNormal,
    Id,
    AmbientOcclusion,
    Curvature,
    Position,
    Thickness,
    Height,
    Opacity,
}

impl MapKind {
    pub const ALL: [MapKind; 9] = [
        MapKind::Normal,
        MapKind::WorldNormal,
        MapKind::Id,
        MapKind::AmbientOcclusion,
        MapKind::Curvature,
        MapKind::Position,
        MapKind::Thickness,
        MapKind::Height,
        MapKind::Opacity,
    ];
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub resolution: u32,
    /// Samples per texel side (1 = no antialiasing, 2 = 2×2, …).
    pub antialiasing: u32,
    /// Cage: how far outside / inside the low mesh to look for the high mesh,
    /// as a fraction of the low mesh's bounding-box diagonal.
    pub max_frontal: f32,
    pub max_rear: f32,
    pub ao_rays: u32,
    /// AO ray length, as a fraction of the bounding-box diagonal.
    pub ao_distance: f32,
    pub thickness_rays: u32,
    pub maps: Vec<MapKind>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            resolution: 2048,
            antialiasing: 1,
            max_frontal: 0.01,
            max_rear: 0.01,
            ao_rays: 32,
            ao_distance: 0.25,
            thickness_rays: 16,
            maps: MapKind::ALL.to_vec(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

/// Progress and cancellation shared with the caller's thread.
#[derive(Default)]
pub struct Progress {
    /// Rows finished, out of `total`.
    pub done: AtomicU32,
    pub total: AtomicU32,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed).max(1);
        self.done.load(Ordering::Relaxed) as f32 / total as f32
    }
}

/// Ray-traceable copy of a mesh.
struct Scene<'a> {
    mesh: &'a Mesh,
    tris: Vec<Triangle>,
    bvh: CwBvh,
}

impl<'a> Scene<'a> {
    fn new(mesh: &'a Mesh) -> Self {
        let tris: Vec<Triangle> = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let [v0, v1, v2] = t.map(|i| Vec3A::from(mesh.vertices[i as usize].position));
                Triangle { v0, v1, v2 }
            })
            .collect();
        let bvh = build_cwbvh_from_tris(
            &tris,
            BvhBuildParams::medium_build(),
            &mut Default::default(),
        );
        Self { mesh, tris, bvh }
    }

    /// Closest hit: (triangle index, distance).
    fn hit(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<(usize, f32)> {
        let ray = Ray::new(origin.into(), nudge(dir).into(), 0.0, max);
        let mut hit = RayHit::none();
        self.bvh
            .ray_traverse(ray, &mut hit, |r, id| {
                self.tris[self.bvh.primitive_indices[id] as usize].intersect(r)
            })
            .then(|| {
                (
                    self.bvh.primitive_indices[hit.primitive_id as usize] as usize,
                    hit.t,
                )
            })
    }

    fn occluded(&self, origin: Vec3, dir: Vec3, max: f32) -> bool {
        let ray = Ray::new(origin.into(), nudge(dir).into(), 0.0, max);
        !self.bvh.ray_traverse_miss(ray, |r, id| {
            self.tris[self.bvh.primitive_indices[id] as usize].intersect(r)
        })
    }

    /// Smooth normal at `point` on triangle `tri`.
    fn normal(&self, tri: usize, point: Vec3) -> Vec3 {
        let idx = &self.mesh.indices[tri * 3..tri * 3 + 3];
        let v = |k: usize| &self.mesh.vertices[idx[k] as usize];
        let [a, b, c] = [0, 1, 2].map(|k| Vec3::from(v(k).position));
        let w = barycentric(point, a, b, c);
        (Vec3::from(v(0).normal) * w.x
            + Vec3::from(v(1).normal) * w.y
            + Vec3::from(v(2).normal) * w.z)
            .normalize_or(Vec3::Z)
    }

    /// Which mesh part (material) a triangle belongs to.
    fn part(&self, tri: usize) -> usize {
        let first = tri as u32 * 3;
        self.mesh
            .parts
            .iter()
            .position(|p| p.indices.contains(&first))
            .unwrap_or(0)
    }
}

/// obvhs misses flat geometry with rays exactly along an axis (zero direction
/// components); a negligible tilt avoids that.
fn nudge(d: Vec3) -> Vec3 {
    d.map(|c| if c.abs() < 1e-6 { 1e-6 } else { c })
}

fn barycentric(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (v0, v1, v2) = (b - a, c - a, p - a);
    let (d00, d01, d11, d20, d21) = (v0.dot(v0), v0.dot(v1), v1.dot(v1), v2.dot(v0), v2.dot(v1));
    let denom = d00 * d11 - d01 * d01;
    if denom.abs() < 1e-20 {
        return Vec3::new(1.0, 0.0, 0.0);
    }
    let v = (d11 * d20 - d01 * d21) / denom;
    let w = (d00 * d21 - d01 * d20) / denom;
    Vec3::new(1.0 - v - w, v, w)
}

/// Surface point of the low mesh under one texel sample.
#[derive(Clone, Copy)]
struct Sample {
    pos: Vec3,
    normal: Vec3,
    /// Projection direction: the normal averaged over vertices sharing a position,
    /// so rays fan out at hard edges instead of missing rounded high-poly corners.
    ray_dir: Vec3,
    tangent: Vec3,
    bitangent: Vec3,
}

/// Rasterises a mesh part in UV space: for every texel, its samples' surface points.
/// Per-vertex normals averaged across vertices at the same position (Painter's "average normals").
fn averaged_normals(mesh: &Mesh) -> Vec<Vec3> {
    let key = |p: [f32; 3]| p.map(|c| (c * 1e4).round() as i64);
    let mut sums: std::collections::HashMap<[i64; 3], Vec3> = std::collections::HashMap::new();
    for tri in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = tri.map(|i| Vec3::from(mesh.vertices[i as usize].position));
        let face = (b - a).cross(c - a); // area-weighted
        for &i in tri {
            *sums
                .entry(key(mesh.vertices[i as usize].position))
                .or_default() += face;
        }
    }
    mesh.vertices
        .iter()
        .map(|v| sums[&key(v.position)].normalize_or(Vec3::from(v.normal)))
        .collect()
}

fn rasterize(mesh: &Mesh, part: usize, res: u32, aa: u32) -> Vec<Vec<Sample>> {
    let mut texels: Vec<Vec<Sample>> = vec![Vec::new(); (res * res) as usize];
    let averaged = averaged_normals(mesh);
    let range = mesh.parts[part].indices.clone();
    let offsets: Vec<Vec2> = (0..aa * aa)
        .map(|i| Vec2::new((i % aa) as f32 + 0.5, (i / aa) as f32 + 0.5) / aa as f32)
        .collect();
    for tri in mesh.indices[range.start as usize..range.end as usize]
        .as_chunks::<3>()
        .0
    {
        let v = tri.map(|i| &mesh.vertices[i as usize]);
        let avg = tri.map(|i| averaged[i as usize]);
        // Texel space: x right, y down (row 0 = v 1).
        let p = v.map(|v| Vec2::new(v.uv[0] * res as f32, (1.0 - v.uv[1]) * res as f32));
        let area = (p[1] - p[0]).perp_dot(p[2] - p[0]);
        if area.abs() < 1e-12 {
            continue;
        }
        let lo = p[0].min(p[1]).min(p[2]).floor().max(Vec2::ZERO);
        let hi = p[0].max(p[1]).max(p[2]).ceil().min(Vec2::splat(res as f32));
        for y in lo.y as u32..hi.y as u32 {
            for x in lo.x as u32..hi.x as u32 {
                let cell = &mut texels[(y * res + x) as usize];
                for o in &offsets {
                    let q = Vec2::new(x as f32, y as f32) + *o;
                    let w0 = (p[1] - q).perp_dot(p[2] - q) / area;
                    let w1 = (p[2] - q).perp_dot(p[0] - q) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 || cell.len() >= offsets.len() {
                        continue;
                    }
                    let lerp3 = |f: &dyn Fn(&reetched_io::Vertex) -> Vec3| {
                        f(v[0]) * w0 + f(v[1]) * w1 + f(v[2]) * w2
                    };
                    let normal = lerp3(&|v| Vec3::from(v.normal)).normalize_or(Vec3::Z);
                    let t = lerp3(&|v| Vec3::from_slice(&v.tangent[..3]));
                    let tangent =
                        (t - normal * normal.dot(t)).normalize_or(normal.any_orthonormal_vector());
                    let sign = if v[0].tangent[3] < 0.0 { -1.0 } else { 1.0 };
                    let ray_dir = (avg[0] * w0 + avg[1] * w1 + avg[2] * w2).normalize_or(normal);
                    cell.push(Sample {
                        pos: lerp3(&|v| Vec3::from(v.position)),
                        normal,
                        ray_dir,
                        tangent,
                        bitangent: normal.cross(tangent) * sign,
                    });
                }
            }
        }
    }
    texels
}

/// Cosine-weighted direction around `n` from two numbers in [0, 1).
fn cosine_dir(n: Vec3, u: f32, v: f32) -> Vec3 {
    let (r, phi) = (u.sqrt(), v * std::f32::consts::TAU);
    let t = n.any_orthonormal_vector();
    let b = n.cross(t);
    (t * (r * phi.cos()) + b * (r * phi.sin()) + n * (1.0 - u).max(0.0).sqrt()).normalize()
}

/// Low-discrepancy sample `i` of `n`, rotated per texel so noise doesn't band.
fn sample2d(i: u32, n: u32, texel: u32) -> (f32, f32) {
    let h = texel.wrapping_mul(0x9E37_79B9) ^ (texel >> 15);
    let rot = (h & 0xFFFF) as f32 / 65536.0;
    let u = ((i as f32 + 0.5) / n as f32 + rot).fract();
    let v = (i.reverse_bits() as f32 * 2.328_306_4e-10 + (h >> 16) as f32 / 65536.0).fract();
    (u, v)
}

/// For mesh reimport: where each texel of `new`'s part `part` (at `res`²)
/// lies on `old`, as an old UV, or None if the old surface isn't there.
/// Only hits on `old`'s part `old_part` count. Texels are row-major, row 0 at
/// the top (v = 1). Rays follow averaged normals both ways, up to `reach` × the
/// mesh's size, so moved or reshaped geometry still finds its old surface.
pub fn transfer(
    new: &Mesh,
    part: usize,
    old: &Mesh,
    old_part: usize,
    res: u32,
    reach: f32,
) -> Vec<Option<[f32; 2]>> {
    let scene = Scene::new(old);
    let texels = rasterize(new, part, res, 1);
    let (lo, hi) = new.bounds();
    let d = (hi - lo).length().max(1e-6) * reach;
    texels
        .par_iter()
        .map(|samples| {
            let s = samples.first()?;
            let (tri, t) = scene.hit(s.pos + s.ray_dir * d, -s.ray_dir, 2.0 * d)?;
            if scene.part(tri) != old_part {
                return None;
            }
            let p = s.pos + s.ray_dir * (d - t);
            let idx = &old.indices[tri * 3..tri * 3 + 3];
            let v = |k: usize| &old.vertices[idx[k] as usize];
            let [a, b, c] = [0, 1, 2].map(|k| Vec3::from(v(k).position));
            let w = barycentric(p, a, b, c);
            let uv =
                Vec2::from(v(0).uv) * w.x + Vec2::from(v(1).uv) * w.y + Vec2::from(v(2).uv) * w.z;
            Some(uv.into())
        })
        .collect()
}

/// A stable, distinct color per mesh part (for ID maps).
pub fn id_color(part: usize) -> [f32; 3] {
    let h = (part as f32 * 0.618_034).fract();
    let k = |n: f32| ((n + h * 6.0) % 6.0 - 3.0).abs().clamp(1.0, 2.0) - 1.0;
    [k(5.0), k(3.0), k(1.0)].map(|c| 0.15 + 0.8 * (1.0 - c))
}

/// Bakes the requested maps for texture set `part` of `low`.
/// `high`: the detailed mesh to project; `None` bakes from `low` itself.
/// Returns `None` if cancelled.
pub fn bake(
    low: &Mesh,
    part: usize,
    high: Option<&Mesh>,
    s: &Settings,
    progress: &Progress,
) -> Option<Vec<(MapKind, Image)>> {
    let res = s.resolution;
    let (lo, hi) = low.bounds();
    let diag = (hi - lo).length().max(1e-6);
    let high_mesh = high.unwrap_or(low);
    let scene = Scene::new(high_mesh);
    let texels = rasterize(low, part, res, s.antialiasing.max(1));
    let wants = |k| s.maps.contains(&k);
    let (frontal, rear) = (s.max_frontal * diag, s.max_rear * diag);
    let ao_len = s.ao_distance * diag;
    let eps = diag * 1e-4;
    progress.total.store(res, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);

    /// Per-texel results before encoding.
    #[derive(Clone, Copy, Default)]
    struct Out {
        covered: bool,
        normal_ts: Vec3,
        world_normal: Vec3,
        pos: Vec3,
        height: f32,
        id: [f32; 3],
        ao: f32,
        thickness: f32,
        opacity: f32,
    }

    let rows: Vec<Vec<Out>> = (0..res)
        .into_par_iter()
        .map(|y| {
            if progress.cancel.load(Ordering::Relaxed) {
                return Vec::new();
            }
            let row = (0..res)
                .map(|x| {
                    let texel = y * res + x;
                    let samples = &texels[texel as usize];
                    if samples.is_empty() {
                        return Out::default();
                    }
                    let mut o = Out {
                        covered: true,
                        ..Default::default()
                    };
                    let k = samples.len() as f32;
                    for smp in samples {
                        // Project onto the high mesh from the cage, along the low normal.
                        let start = smp.pos + smp.ray_dir * frontal;
                        let (surface, n_high, opacity, height, id) =
                            match scene.hit(start, -smp.ray_dir, frontal + rear + eps) {
                                Some((tri, t)) => {
                                    let p = start - smp.ray_dir * t;
                                    let part = scene.part(tri);
                                    (p, scene.normal(tri, p), 1.0, frontal - t, id_color(part))
                                }
                                None => (smp.pos, smp.normal, 0.0, 0.0, [0.0; 3]),
                            };
                        let ts = Vec3::new(
                            n_high.dot(smp.tangent),
                            n_high.dot(smp.bitangent),
                            n_high.dot(smp.normal),
                        )
                        .normalize_or(Vec3::Z);
                        o.normal_ts += ts / k;
                        o.world_normal += n_high / k;
                        o.pos += smp.pos / k;
                        o.height += height / k;
                        o.opacity += opacity / k;
                        for (a, b) in o.id.iter_mut().zip(id) {
                            *a += b / k;
                        }
                        if wants(MapKind::AmbientOcclusion) {
                            let origin = surface + n_high * eps * 4.0;
                            let hits = (0..s.ao_rays)
                                .filter(|&i| {
                                    let (u, v) = sample2d(i, s.ao_rays, texel);
                                    scene.occluded(origin, cosine_dir(n_high, u, v), ao_len)
                                })
                                .count();
                            o.ao += (1.0 - hits as f32 / s.ao_rays.max(1) as f32) / k;
                        }
                        if wants(MapKind::Thickness) {
                            let origin = surface - n_high * eps * 4.0;
                            let total: f32 = (0..s.thickness_rays)
                                .map(|i| {
                                    let (u, v) = sample2d(i, s.thickness_rays, texel);
                                    scene
                                        .hit(origin, cosine_dir(-n_high, u, v), ao_len)
                                        .map_or(ao_len, |(_, t)| t)
                                })
                                .sum();
                            o.thickness += (total / s.thickness_rays.max(1) as f32 / ao_len) / k;
                        }
                    }
                    o.normal_ts = o.normal_ts.normalize_or(Vec3::Z);
                    o.world_normal = o.world_normal.normalize_or(Vec3::Z);
                    o
                })
                .collect();
            progress.done.fetch_add(1, Ordering::Relaxed);
            row
        })
        .collect();
    if progress.cancel.load(Ordering::Relaxed) {
        return None;
    }
    let out: Vec<Out> = rows.into_iter().flatten().collect();
    let covered: Vec<bool> = out.iter().map(|o| o.covered).collect();

    let image = |f: &dyn Fn(&Out) -> [f32; 4]| {
        let mut img = Image {
            width: res,
            height: res,
            pixels: out.iter().map(f).collect(),
        };
        dilate(&mut img, &covered);
        img
    };
    let gray = |v: f32| [v, v, v, 1.0];
    let enc = |v: Vec3| {
        let e = v * 0.5 + 0.5;
        [e.x, e.y, e.z, 1.0]
    };
    let mut maps = Vec::new();
    for &kind in &s.maps {
        let img = match kind {
            MapKind::Normal => image(&|o| enc(o.normal_ts)),
            MapKind::WorldNormal => image(&|o| enc(o.world_normal)),
            MapKind::Id => image(&|o| [o.id[0], o.id[1], o.id[2], 1.0]),
            MapKind::AmbientOcclusion => image(&|o| gray(o.ao)),
            MapKind::Position => image(&|o| {
                let p = (o.pos - lo) / (hi - lo).max(Vec3::splat(1e-6));
                [p.x, p.y, p.z, 1.0]
            }),
            MapKind::Thickness => image(&|o| gray(o.thickness.clamp(0.0, 1.0))),
            MapKind::Height => {
                image(&|o| gray(0.5 + o.height / (2.0 * frontal.max(rear).max(1e-6))))
            }
            MapKind::Opacity => image(&|o| gray(o.opacity)),
            MapKind::Curvature => curvature(
                &out.iter()
                    .map(|o| (o.covered, o.pos, o.world_normal))
                    .collect::<Vec<_>>(),
                res,
                diag,
                &covered,
            ),
        };
        maps.push((kind, img));
    }
    Some(maps)
}

/// Curvature from neighbouring texels' world normals and positions: how fast the
/// normal turns per unit of surface (positive = convex). Bevels on a high-poly
/// mesh show up as bright edges.
fn curvature(t: &[(bool, Vec3, Vec3)], res: u32, diag: f32, covered: &[bool]) -> Image {
    let at = |x: i64, y: i64| -> Option<(Vec3, Vec3)> {
        if x < 0 || y < 0 || x >= res as i64 || y >= res as i64 {
            return None;
        }
        let (c, p, n) = t[(y as u32 * res + x as u32) as usize];
        c.then_some((p, n))
    };
    let pixels = (0..res as i64)
        .flat_map(|y| (0..res as i64).map(move |x| (x, y)))
        .map(|(x, y)| {
            let Some(center) = at(x, y) else {
                return [0.5, 0.5, 0.5, 1.0];
            };
            let mut k = 0.0;
            let mut count = 0.0;
            for (a, b) in [((x - 1, y), (x + 1, y)), ((x, y - 1), (x, y + 1))] {
                let (pa, na) = at(a.0, a.1).unwrap_or(center);
                let (pb, nb) = at(b.0, b.1).unwrap_or(center);
                let dp = pb - pa;
                let len2 = dp.length_squared();
                if len2 > 1e-20 {
                    k += (nb - na).dot(dp) / len2;
                    count += 1.0;
                }
            }
            let k = if count > 0.0 { k / count } else { 0.0 };
            // Dimensionless (× scene size) and squashed into 0..1; ~50 (sharp bevels) saturates.
            let v = 0.5 + 0.5 * (k * diag * 0.05).tanh();
            [v, v, v, 1.0]
        })
        .collect();
    let mut img = Image {
        width: res,
        height: res,
        pixels,
    };
    dilate(&mut img, covered);
    img
}

/// Extends island edges outward (nearest covered texel) so filtering never sees background.
fn dilate(img: &mut Image, covered: &[bool]) {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut source: Vec<u32> = vec![u32::MAX; w * h];
    let mut queue = std::collections::VecDeque::new();
    for (i, &c) in covered.iter().enumerate() {
        if c {
            source[i] = i as u32;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % w, i / w);
        for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                continue;
            }
            let n = ny as usize * w + nx as usize;
            if source[n] == u32::MAX {
                source[n] = source[i];
                queue.push_back(n);
            }
        }
    }
    if covered.iter().any(|&c| c) {
        let copy = img.pixels.clone();
        for (i, s) in source.iter().enumerate() {
            img.pixels[i] = copy[*s as usize];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reetched_io::{MeshPart, Vertex};

    /// A unit quad in z = `z` facing +z, covering the whole UV tile.
    fn quad(z: f32, normal: [f32; 3]) -> Mesh {
        let v = |x: f32, y: f32| Vertex {
            position: [x, y, z],
            normal,
            uv: [x, y],
            tangent: [1.0, 0.0, 0.0, 1.0],
        };
        Mesh {
            vertices: vec![v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)],
            indices: vec![0, 1, 2, 0, 2, 3],
            parts: vec![MeshPart {
                material: "q".into(),
                indices: 0..6,
            }],
            has_uvs: true,
        }
    }

    fn settings(maps: Vec<MapKind>) -> Settings {
        Settings {
            resolution: 32,
            max_frontal: 0.1,
            max_rear: 0.1,
            ao_rays: 64,
            maps,
            ..Default::default()
        }
    }

    fn px(img: &Image, x: u32, y: u32) -> [f32; 4] {
        img.pixels[(y * img.width + x) as usize]
    }

    #[test]
    fn flat_high_on_flat_low_gives_flat_normal_and_mid_height() {
        let low = quad(0.0, [0.0, 0.0, 1.0]);
        let mut high = quad(0.05, [0.0, 0.0, 1.0]);
        // Tilt the high normals 45° towards +x.
        let tilt = Vec3::new(1.0, 0.0, 1.0).normalize();
        high.vertices
            .iter_mut()
            .for_each(|v| v.normal = tilt.into());
        let maps = bake(
            &low,
            0,
            Some(&high),
            &settings(vec![MapKind::Normal, MapKind::Height, MapKind::Opacity]),
            &Progress::default(),
        )
        .unwrap();
        let n = px(&maps[0].1, 16, 16);
        assert!(
            (n[0] - (0.707 * 0.5 + 0.5)).abs() < 0.02 && (n[1] - 0.5).abs() < 0.02,
            "{n:?}"
        );
        let h = px(&maps[1].1, 16, 16)[0];
        assert!(h > 0.5, "the high surface is above the low one: {h}");
        assert_eq!(px(&maps[2].1, 16, 16)[0], 1.0);
    }

    #[test]
    fn ao_darkens_under_a_roof() {
        let mut low = quad(0.0, [0.0, 0.0, 1.0]);
        // Add a roof over the left half, facing down (part of the occluding scene).
        let base = low.vertices.len() as u32;
        let roof = |x: f32, y: f32| Vertex {
            position: [x, y, 0.05],
            normal: [0.0, 0.0, -1.0],
            uv: [0.0, 0.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
        };
        low.vertices.extend([
            roof(-1.0, -1.0),
            roof(0.5, -1.0),
            roof(0.5, 2.0),
            roof(-1.0, 2.0),
        ]);
        low.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        low.parts.push(MeshPart {
            material: "roof".into(),
            indices: 6..12,
        });
        let maps = bake(
            &low,
            0,
            None,
            &settings(vec![MapKind::AmbientOcclusion]),
            &Progress::default(),
        )
        .unwrap();
        let (left, right) = (px(&maps[0].1, 4, 16)[0], px(&maps[0].1, 30, 16)[0]);
        assert!(left < 0.3, "covered side is dark: {left}");
        assert!(right > left, "open side is brighter: {right} vs {left}");
    }

    #[test]
    fn curvature_is_bright_on_a_sphere_ridge_and_neutral_on_a_plane() {
        let plane = quad(0.0, [0.0, 0.0, 1.0]);
        let maps = bake(
            &plane,
            0,
            None,
            &settings(vec![MapKind::Curvature]),
            &Progress::default(),
        )
        .unwrap();
        assert!((px(&maps[0].1, 16, 16)[0] - 0.5).abs() < 0.01);
        let sphere = Mesh::uv_sphere(1.0, 64, 32);
        let maps = bake(
            &sphere,
            0,
            None,
            &settings(vec![MapKind::Curvature]),
            &Progress::default(),
        )
        .unwrap();
        assert!(px(&maps[0].1, 16, 16)[0] > 0.5, "spheres are convex");
    }

    #[test]
    fn axis_aligned_rays_hit_flat_geometry() {
        let high = quad(0.05, [0.0, 0.0, 1.0]);
        let scene = Scene::new(&high);
        let (_, t) = scene
            .hit(Vec3::new(0.5, 0.5, 0.14), -Vec3::Z, 0.3)
            .expect("hits");
        assert!((t - 0.09).abs() < 1e-4);
        assert!(scene.occluded(Vec3::new(0.25, 0.5, 0.14), -Vec3::Z, 0.3));
    }

    #[test]
    fn cancel_stops_the_bake() {
        let progress = Progress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        assert!(
            bake(
                &quad(0.0, [0.0, 0.0, 1.0]),
                0,
                None,
                &Settings::default(),
                &progress
            )
            .is_none()
        );
    }

    #[test]
    fn transfer_follows_the_surface_to_its_old_uvs() {
        let old = quad(0.0, [0.0, 0.0, 1.0]);
        // Same surface, UVs mirrored left-right.
        let mut new = old.clone();
        new.vertices
            .iter_mut()
            .for_each(|v| v.uv[0] = 1.0 - v.uv[0]);
        let res = 16;
        let map = transfer(&new, 0, &old, 0, res, 0.05);
        // New texel (x, y) is at old u = 1 - (x + 0.5) / res, same v.
        for (x, y) in [(2, 3), (13, 8), (7, 14)] {
            let uv = map[(y * res + x) as usize].expect("on the surface");
            let want_u = 1.0 - (x as f32 + 0.5) / res as f32;
            let want_v = 1.0 - (y as f32 + 0.5) / res as f32;
            assert!(
                (uv[0] - want_u).abs() < 1e-3 && (uv[1] - want_v).abs() < 1e-3,
                "{uv:?}"
            );
        }
        // A surface far from the old one finds nothing.
        let far: Mesh = {
            let mut m = old.clone();
            m.vertices.iter_mut().for_each(|v| v.position[2] = 5.0);
            m
        };
        assert!(
            transfer(&far, 0, &old, 0, res, 0.05)
                .iter()
                .all(Option::is_none)
        );
    }
}
