//! Triangle adjacency, for selecting faces, UV islands and connected meshes
//! (Polygon Fill).

use crate::Mesh;
use glam::{Vec2, Vec3};
use std::collections::HashMap;

const NONE: u32 = u32::MAX;

/// What a Polygon Fill click selects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Selection {
    Triangle,
    /// The clicked triangle and its coplanar neighbours (a quad or flat n-gon).
    Polygon,
    /// Triangles connected through shared UV edges.
    UvIsland,
    /// Triangles connected in 3D.
    Mesh,
    /// A ring of quads, running along the quad edge nearest the click.
    Loop,
}

impl Selection {
    pub const ALL: [Selection; 5] = [
        Selection::Triangle,
        Selection::Polygon,
        Selection::Loop,
        Selection::UvIsland,
        Selection::Mesh,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Selection::Triangle => "Triangle",
            Selection::Polygon => "Polygon",
            Selection::UvIsland => "UV island",
            Selection::Mesh => "Mesh",
            Selection::Loop => "Loop",
        }
    }
}

pub struct Topology {
    /// Per triangle edge (ab, bc, ca): the triangle across it in 3D, or NONE.
    neighbors: Vec<[u32; 3]>,
    /// The same, but only across edges whose UVs match on both sides.
    uv_neighbors: Vec<[u32; 3]>,
    normals: Vec<Vec3>,
    /// Mesh part per triangle.
    part: Vec<u32>,
    /// Welded corner ids per triangle, and the welded positions.
    corners: Vec<[u32; 3]>,
    positions: Vec<Vec3>,
    /// The triangle each one forms a quad with (across its longest edge, when the
    /// pairing is mutual and flat), or NONE. Meshes arrive triangulated; this
    /// recovers their quads.
    mate: Vec<u32>,
}

impl Topology {
    pub fn new(mesh: &Mesh) -> Self {
        let tris = mesh.indices.as_chunks::<3>().0;
        // Weld positions (exact bits) so split vertices still connect.
        let mut welded: HashMap<[u32; 3], u32> = HashMap::new();
        let pos_id: Vec<u32> = mesh
            .vertices
            .iter()
            .map(|v| {
                let n = welded.len() as u32;
                *welded.entry(v.position.map(f32::to_bits)).or_insert(n)
            })
            .collect();
        let uv_key = |i: u32| {
            let v = &mesh.vertices[i as usize];
            (pos_id[i as usize], v.uv.map(f32::to_bits))
        };
        let mut positions = vec![Vec3::ZERO; welded.len()];
        for (i, v) in mesh.vertices.iter().enumerate() {
            positions[pos_id[i] as usize] = Vec3::from(v.position);
        }
        let corners: Vec<[u32; 3]> = tris.iter().map(|t| t.map(|i| pos_id[i as usize])).collect();
        let mut part = vec![0u32; tris.len()];
        for (p, mp) in mesh.parts.iter().enumerate() {
            for t in mp.indices.start / 3..mp.indices.end / 3 {
                part[t as usize] = p as u32;
            }
        }
        // Edge (welded ends, sorted) → triangles using it.
        let mut edges: HashMap<(u32, u32), Vec<(u32, usize)>> = HashMap::new();
        for (t, tri) in tris.iter().enumerate() {
            for e in 0..3 {
                let (a, b) = (pos_id[tri[e] as usize], pos_id[tri[(e + 1) % 3] as usize]);
                edges
                    .entry((a.min(b), a.max(b)))
                    .or_default()
                    .push((t as u32, e));
            }
        }
        let mut neighbors = vec![[NONE; 3]; tris.len()];
        let mut uv_neighbors = vec![[NONE; 3]; tris.len()];
        for users in edges.values() {
            for &(t, e) in users {
                for &(o, oe) in users {
                    if o == t || part[o as usize] != part[t as usize] {
                        continue;
                    }
                    neighbors[t as usize][e] = o;
                    let mine = [tris[t as usize][e], tris[t as usize][(e + 1) % 3]].map(uv_key);
                    let theirs = [tris[o as usize][oe], tris[o as usize][(oe + 1) % 3]].map(uv_key);
                    let same = (mine[0] == theirs[0] && mine[1] == theirs[1])
                        || (mine[0] == theirs[1] && mine[1] == theirs[0]);
                    if same {
                        uv_neighbors[t as usize][e] = o;
                    }
                }
            }
        }
        let normals: Vec<Vec3> = tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| Vec3::from(mesh.vertices[i as usize].position));
                (b - a).cross(c - a).normalize_or_zero()
            })
            .collect();
        // Quads: pair each triangle with the one across its longest edge.
        let longest = |t: usize| {
            let c = corners[t].map(|i| positions[i as usize]);
            (0..3)
                .max_by(|&a, &b| {
                    let la = c[a].distance_squared(c[(a + 1) % 3]);
                    let lb = c[b].distance_squared(c[(b + 1) % 3]);
                    la.total_cmp(&lb)
                })
                .unwrap_or(0)
        };
        let across: Vec<u32> = (0..tris.len()).map(|t| neighbors[t][longest(t)]).collect();
        let mate = (0..tris.len())
            .map(|t| {
                let m = across[t];
                let mutual = m != NONE && across[m as usize] == t as u32;
                if mutual && normals[t].dot(normals[m as usize]) > 0.9 {
                    m
                } else {
                    NONE
                }
            })
            .collect();
        Self {
            neighbors,
            uv_neighbors,
            normals,
            part,
            corners,
            positions,
            mate,
        }
    }

    /// The quad triangle `t` belongs to, as its four welded corners in order around it.
    fn quad(&self, t: u32) -> Option<[u32; 4]> {
        let m = self.mate[t as usize];
        if m == NONE {
            return None;
        }
        let [a, b, c] = self.corners[t as usize];
        let other = self.corners[m as usize];
        // The shared diagonal is two of t's corners; the mate adds the fourth.
        let d = *other.iter().find(|x| ![a, b, c].contains(x))?;
        for (p, q, r) in [(a, b, c), (b, c, a), (c, a, b)] {
            if other.contains(&q) && other.contains(&r) {
                return Some([p, q, d, r]);
            }
        }
        None
    }

    /// Face loop through triangle `tri`: the ring of quads that continues across the
    /// two quad edges next to the one nearest `hit`.
    fn face_loop(&self, tri: u32, hit: Vec3) -> Vec<u32> {
        let Some(start) = self.quad(tri) else {
            return vec![tri];
        };
        let pos = |i: u32| self.positions[i as usize];
        let dist = |a: Vec3, b: Vec3| {
            let ab = b - a;
            let t = ((hit - a).dot(ab) / ab.length_squared().max(1e-20)).clamp(0.0, 1.0);
            hit.distance(a + ab * t)
        };
        let nearest = (0..4)
            .min_by(|&i, &j| {
                let di = dist(pos(start[i]), pos(start[(i + 1) % 4]));
                let dj = dist(pos(start[j]), pos(start[(j + 1) % 4]));
                di.total_cmp(&dj)
            })
            .unwrap_or(0);
        let mut seen = std::collections::HashSet::from([tri, self.mate[tri as usize]]);
        let mut out = vec![tri, self.mate[tri as usize]];
        for dir in [1, 3] {
            let mut quad = start;
            let mut exit = (nearest + dir) % 4;
            let mut tris = [tri, self.mate[tri as usize]];
            for _ in 0..self.mate.len() {
                let (a, b) = (quad[exit], quad[(exit + 1) % 4]);
                // The triangle on the other side of that edge.
                let Some(next) = tris.iter().find_map(|&t| {
                    let e = (0..3).find(|&k| {
                        let c = self.corners[t as usize];
                        let (p, q) = (c[k], c[(k + 1) % 3]);
                        (p == a && q == b) || (p == b && q == a)
                    })?;
                    let n = self.neighbors[t as usize][e];
                    (n != NONE).then_some(n)
                }) else {
                    break;
                };
                if seen.contains(&next) {
                    break;
                }
                let Some(q) = self.quad(next) else { break };
                let entry = (0..4).find(|&k| {
                    let (p, r) = (q[k], q[(k + 1) % 4]);
                    (p == a && r == b) || (p == b && r == a)
                });
                let Some(entry) = entry else { break };
                let m = self.mate[next as usize];
                seen.extend([next, m]);
                out.extend([next, m]);
                tris = [next, m];
                quad = q;
                exit = (entry + 2) % 4;
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Triangles selected by clicking triangle `tri` at `hit` (object space; only
    /// `Loop` uses it).
    pub fn select_at(&self, tri: usize, mode: Selection, hit: Vec3) -> Vec<u32> {
        match mode {
            Selection::Loop => self.face_loop(tri as u32, hit),
            m => self.select(tri, m),
        }
    }

    /// Where `uv` lands on triangle `tri` in object space.
    pub fn point_at_uv(mesh: &Mesh, tri: usize, uv: Vec2) -> Vec3 {
        let idx = &mesh.indices[tri * 3..tri * 3 + 3];
        let v = |k: usize| &mesh.vertices[idx[k] as usize];
        let [a, b, c] = [0, 1, 2].map(|k| Vec2::from(v(k).uv));
        let d = (b - a).perp_dot(c - a);
        if d.abs() < 1e-12 {
            return Vec3::from(v(0).position);
        }
        let w1 = (uv - a).perp_dot(c - a) / d;
        let w2 = (b - a).perp_dot(uv - a) / d;
        let p = |k: usize| Vec3::from(v(k).position);
        p(0) * (1.0 - w1 - w2) + p(1) * w1 + p(2) * w2
    }

    /// Triangles selected by clicking triangle `tri`.
    pub fn select(&self, tri: usize, mode: Selection) -> Vec<u32> {
        let flat = |a: u32, b: u32| self.normals[a as usize].dot(self.normals[b as usize]) > 0.999;
        let start = tri as u32;
        match mode {
            Selection::Triangle => vec![start],
            Selection::Polygon => self.flood(start, &self.neighbors, &|from, to| {
                flat(from, to) && flat(start, to)
            }),
            Selection::UvIsland => self.flood(start, &self.uv_neighbors, &|_, _| true),
            Selection::Mesh => self.flood(start, &self.neighbors, &|_, _| true),
            // Without a click point, the loop along the quad's first edge.
            Selection::Loop => {
                let [a, b, _] = self.corners[tri].map(|i| self.positions[i as usize]);
                self.face_loop(start, (a + b) * 0.5)
            }
        }
    }

    fn flood(&self, start: u32, adj: &[[u32; 3]], ok: &dyn Fn(u32, u32) -> bool) -> Vec<u32> {
        let mut seen = vec![false; adj.len()];
        let mut stack = vec![start];
        seen[start as usize] = true;
        let mut out = Vec::new();
        while let Some(t) = stack.pop() {
            out.push(t);
            for &n in &adj[t as usize] {
                if n != NONE && !seen[n as usize] && ok(t, n) {
                    seen[n as usize] = true;
                    stack.push(n);
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// The triangle of `part` whose UVs contain `uv`.
    pub fn triangle_at_uv(&self, mesh: &Mesh, part: usize, uv: Vec2) -> Option<usize> {
        mesh.indices
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
            .filter(|(t, _)| self.part[*t] as usize == part)
            .find(|(_, tri)| {
                let [a, b, c] = tri.map(|i| Vec2::from(mesh.vertices[i as usize].uv));
                let d = (b - a).perp_dot(c - a);
                if d.abs() < 1e-12 {
                    return false;
                }
                let w1 = (uv - a).perp_dot(c - a) / d;
                let w2 = (b - a).perp_dot(uv - a) / d;
                w1 >= 0.0 && w2 >= 0.0 && w1 + w2 <= 1.0
            })
            .map(|(t, _)| t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MeshPart, Vertex};

    /// Two separate quads (each two triangles): the first flat, the second
    /// folded along its diagonal. The first quad's triangles have split
    /// vertices with different UVs along their shared edge (a UV seam).
    fn mesh() -> Mesh {
        let v = |p: [f32; 3], uv: [f32; 2]| Vertex {
            position: p,
            normal: [0.0, 0.0, 1.0],
            uv,
            tangent: [1.0, 0.0, 0.0, 1.0],
            ..Default::default()
        };
        let vertices = vec![
            // quad A, triangle 0
            v([0.0, 0.0, 0.0], [0.0, 0.0]),
            v([1.0, 0.0, 0.0], [0.4, 0.0]),
            v([1.0, 1.0, 0.0], [0.4, 0.4]),
            // quad A, triangle 1: shares the 0–2 diagonal, but with other UVs
            v([0.0, 0.0, 0.0], [0.5, 0.0]),
            v([1.0, 1.0, 0.0], [0.9, 0.4]),
            v([0.0, 1.0, 0.0], [0.5, 0.4]),
            // quad B (folded), shared vertices
            v([3.0, 0.0, 0.0], [0.0, 0.5]),
            v([4.0, 0.0, 0.0], [0.4, 0.5]),
            v([4.0, 1.0, 1.0], [0.4, 0.9]),
            v([3.0, 1.0, 0.0], [0.0, 0.9]),
        ];
        let indices = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 6, 8, 9];
        Mesh {
            vertices,
            indices,
            parts: vec![MeshPart {
                material: "m".into(),
                indices: 0..12,
            }],
            has_uvs: true,
        }
    }

    #[test]
    fn selection_modes() {
        let m = mesh();
        let t = Topology::new(&m);
        assert_eq!(t.select(0, Selection::Triangle), [0]);
        assert_eq!(t.select(0, Selection::Polygon), [0, 1], "flat quad");
        assert_eq!(
            t.select(2, Selection::Polygon),
            [2],
            "folded quad stops at the fold"
        );
        assert_eq!(
            t.select(0, Selection::UvIsland),
            [0],
            "UV seam on the diagonal"
        );
        assert_eq!(t.select(2, Selection::UvIsland), [2, 3]);
        assert_eq!(t.select(0, Selection::Mesh), [0, 1]);
        assert_eq!(t.select(3, Selection::Mesh), [2, 3]);
    }

    /// A 4×2 grid of quads (two triangles each) in the z = 0 plane.
    fn grid() -> Mesh {
        let v = |x: f32, y: f32| Vertex {
            position: [x, y, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [x / 4.0, y / 2.0],
            ..Default::default()
        };
        let mut vertices = Vec::new();
        for y in 0..3 {
            for x in 0..5 {
                vertices.push(v(x as f32, y as f32));
            }
        }
        let mut indices = Vec::new();
        for y in 0..2u32 {
            for x in 0..4u32 {
                let k = y * 5 + x;
                indices.extend([k, k + 1, k + 6, k, k + 6, k + 5]);
            }
        }
        let n = indices.len() as u32;
        Mesh {
            vertices,
            indices,
            parts: vec![MeshPart {
                material: "m".into(),
                indices: 0..n,
            }],
            has_uvs: true,
        }
    }

    #[test]
    fn face_loops_follow_the_clicked_edge() {
        let m = grid();
        let t = Topology::new(&m);
        // Quad (1, 0) is triangles 2 and 3. Clicking near its bottom edge picks the
        // bottom row of four quads; near its left edge, the column of two.
        let row = t.select_at(2, Selection::Loop, Vec3::new(1.5, 0.05, 0.0));
        assert_eq!(row, [0, 1, 2, 3, 4, 5, 6, 7]);
        let column = t.select_at(2, Selection::Loop, Vec3::new(1.05, 0.5, 0.0));
        assert_eq!(column, [2, 3, 10, 11]);
    }

    #[test]
    fn finds_triangles_by_uv() {
        let m = mesh();
        let t = Topology::new(&m);
        assert_eq!(t.triangle_at_uv(&m, 0, Vec2::new(0.35, 0.1)), Some(0));
        assert_eq!(t.triangle_at_uv(&m, 0, Vec2::new(0.6, 0.3)), Some(1));
        assert_eq!(t.triangle_at_uv(&m, 0, Vec2::new(0.95, 0.95)), None);
    }
}
