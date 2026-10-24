//! GPU-friendly binary BVH. obvhs builds it (PLOC + reinsertion, the same library the
//! baker uses); we flatten it into nodes that carry both children's boxes, so the
//! shader tests two boxes per node fetch and only pushes the farther child.

use bytemuck::{Pod, Zeroable};
use glam::{Vec3, Vec3A};
use obvhs::{BvhBuildParams, bvh2::builder::build_bvh2_from_tris, triangle::Triangle};

/// Both children of an inner node. `count` 0: `idx` is a node; otherwise the child is
/// a leaf of `count` triangle records starting at `idx`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuNode {
    pub lmin: [f32; 3],
    pub lidx: u32,
    pub lmax: [f32; 3],
    pub lcount: u32,
    pub rmin: [f32; 3],
    pub ridx: u32,
    pub rmax: [f32; 3],
    pub rcount: u32,
}

/// A triangle in leaf order, ready for Möller–Trumbore: `id` is its index in the
/// mesh (for attributes), `layer` its material layer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuTri {
    pub v0: [f32; 3],
    pub id: u32,
    pub e1: [f32; 3],
    pub layer: u32,
    pub e2: [f32; 3],
    pub pad: u32,
}

pub struct Flat {
    pub nodes: Vec<GpuNode>,
    pub tris: Vec<GpuTri>,
}

// An inverted box. With a tiny ray direction component the slab test can still
// overflow to ±inf and let a ray "enter" it, so empty children are leaves of one
// harmless triangle record rather than inner nodes (which would loop back to node 0).
const EMPTY_MIN: [f32; 3] = [1e30; 3];
const EMPTY_MAX: [f32; 3] = [-1e30; 3];

/// Builds the flattened BVH over triangles `tri_positions[t]`.
pub fn build(tri_positions: &[[Vec3; 3]], layer_of_triangle: &[u32]) -> Flat {
    let empty = GpuNode {
        lmin: EMPTY_MIN,
        lidx: 0,
        lmax: EMPTY_MAX,
        lcount: 1,
        rmin: EMPTY_MIN,
        ridx: 0,
        rmax: EMPTY_MAX,
        rcount: 1,
    };
    if tri_positions.is_empty() {
        // One node with two empty children: every ray misses.
        return Flat {
            nodes: vec![empty],
            tris: vec![GpuTri::zeroed()],
        };
    }
    let tris: Vec<Triangle> = tri_positions
        .iter()
        .map(|[a, b, c]| Triangle {
            v0: Vec3A::from(*a),
            v1: Vec3A::from(*b),
            v2: Vec3A::from(*c),
        })
        .collect();
    // No spatial splits, so every triangle sits in exactly one leaf. Small leaves
    // suit a GPU (divergent triangle loops are the expensive part).
    let params = BvhBuildParams {
        max_prims_per_leaf: 4,
        ..BvhBuildParams::fast_build()
    };
    let bvh = build_bvh2_from_tris(&tris, params, &mut Default::default());

    let records = bvh
        .primitive_indices
        .iter()
        .map(|&t| {
            let [a, b, c] = tri_positions[t as usize];
            GpuTri {
                v0: a.into(),
                id: t,
                e1: (b - a).into(),
                layer: layer_of_triangle.get(t as usize).copied().unwrap_or(0),
                e2: (c - a).into(),
                pad: 0,
            }
        })
        .collect();

    let child = |i: usize, nodes: &mut Vec<GpuNode>, stack: &mut Vec<(usize, usize)>| {
        let n = &bvh.nodes[i];
        let (idx, count) = if n.prim_count > 0 {
            (n.first_index, n.prim_count)
        } else {
            nodes.push(empty);
            stack.push((i, nodes.len() - 1));
            (nodes.len() as u32 - 1, 0)
        };
        (
            Vec3::from(n.aabb.min).into(),
            Vec3::from(n.aabb.max).into(),
            idx,
            count,
        )
    };

    let mut nodes = vec![empty];
    let mut stack = Vec::new();
    if bvh.nodes[0].prim_count > 0 {
        // The whole mesh is one leaf: hang it under a root with an empty sibling.
        let (lmin, lmax, lidx, lcount) = child(0, &mut nodes, &mut stack);
        nodes[0] = GpuNode {
            lmin,
            lidx,
            lmax,
            lcount,
            ..empty
        };
    } else {
        stack.push((0, 0));
    }
    // Depth first, so a parent's first child tends to sit right after it in memory.
    while let Some((b, slot)) = stack.pop() {
        let first = bvh.nodes[b].first_index as usize;
        let (lmin, lmax, lidx, lcount) = child(first, &mut nodes, &mut stack);
        let (rmin, rmax, ridx, rcount) = child(first + 1, &mut nodes, &mut stack);
        nodes[slot] = GpuNode {
            lmin,
            lidx,
            lmax,
            lcount,
            rmin,
            ridx,
            rmax,
            rcount,
        };
    }
    Flat {
        nodes,
        tris: records,
    }
}

/// Möller–Trumbore; the distance, barycentrics u and v. Mirrors `hit_tri` in trace.wgsl.
#[cfg(test)]
pub fn hit_tri(t: &GpuTri, o: Vec3, d: Vec3) -> Option<(f32, f32, f32)> {
    let (e1, e2) = (Vec3::from(t.e1), Vec3::from(t.e2));
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det == 0.0 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - Vec3::from(t.v0);
    let u = s.dot(p) * inv;
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    let dist = e2.dot(q) * inv;
    (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && dist > 0.0).then_some((dist, u, v))
}

/// Closest hit through the flattened layout: (mesh triangle id, distance). The same
/// traversal as `traverse` in trace.wgsl, so tests of this cover the shader's layout.
#[cfg(test)]
pub fn intersect(flat: &Flat, o: Vec3, d: Vec3) -> Option<(u32, f32)> {
    let dd = Vec3::select(
        d.abs().cmplt(Vec3::splat(1e-12)),
        Vec3::splat(1e-12).copysign(d),
        d,
    );
    let inv = dd.recip();
    let slab = |lo: [f32; 3], hi: [f32; 3], best: f32| {
        let t0 = (Vec3::from(lo) - o) * inv;
        let t1 = (Vec3::from(hi) - o) * inv;
        let near = t0.min(t1).max_element().max(0.0);
        // Slightly widened exit (Ize 2013) so boxes flush with a triangle don't miss.
        let far = t0.max(t1).min_element().min(best) * 1.000_000_4;
        if near <= far { near } else { f32::INFINITY }
    };
    let mut best = f32::INFINITY;
    let mut hit = None;
    let mut stack = Vec::new();
    let mut cur = 0usize;
    loop {
        let n = &flat.nodes[cur];
        let mut tl = slab(n.lmin, n.lmax, best);
        let mut tr = slab(n.rmin, n.rmax, best);
        for (t, idx, count) in [(&mut tl, n.lidx, n.lcount), (&mut tr, n.ridx, n.rcount)] {
            if count > 0 && t.is_finite() {
                for r in &flat.tris[idx as usize..(idx + count) as usize] {
                    if let Some((dist, _, _)) = hit_tri(r, o, d)
                        && dist < best
                    {
                        best = dist;
                        hit = Some((r.id, dist));
                    }
                }
                *t = f32::INFINITY;
            }
        }
        let (gl, gr) = (tl < best, tr < best);
        if gl && gr {
            let (near, far) = if tl <= tr {
                (n.lidx, n.ridx)
            } else {
                (n.ridx, n.lidx)
            };
            stack.push(far as usize);
            cur = near as usize;
        } else if gl {
            cur = n.lidx as usize;
        } else if gr {
            cur = n.ridx as usize;
        } else if let Some(next) = stack.pop() {
            cur = next;
        } else {
            return hit;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift, so the test needs no rand crate.
    struct Rng(u64);
    impl Rng {
        fn f(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
        fn v(&mut self) -> Vec3 {
            Vec3::new(self.f(), self.f(), self.f()) * 2.0 - 1.0
        }
    }

    fn positions(mesh: &reetched_io::Mesh) -> Vec<[Vec3; 3]> {
        mesh.indices
            .chunks(3)
            .map(|t| {
                t.iter()
                    .map(|&i| Vec3::from(mesh.vertices[i as usize].position))
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap()
            })
            .collect()
    }

    fn check(tris: &[[Vec3; 3]], rng: &mut Rng) {
        let flat = build(tris, &vec![0; tris.len()]);
        let (mut hits, n) = (0, 2000);
        for k in 0..n {
            let o = rng.v() * 3.0;
            // Half the rays aim at a random triangle, so most of them hit something.
            let d = if k % 2 == 0 {
                let [a, b, c] = tris[(rng.f() * tris.len() as f32) as usize % tris.len()];
                ((a + b + c) / 3.0 - o).normalize()
            } else {
                rng.v().normalize()
            };
            let brute = flat
                .tris
                .iter()
                .filter_map(|r| hit_tri(r, o, d).map(|(t, _, _)| (r.id, t)))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let bvh = intersect(&flat, o, d);
            match (brute, bvh) {
                (None, None) => {}
                (Some((_, a)), Some((_, b))) => {
                    hits += 1;
                    assert!((a - b).abs() <= 1e-5 * a.max(1.0), "ray {k}: {a} vs {b}");
                }
                _ => panic!("ray {k}: brute {brute:?}, bvh {bvh:?}"),
            }
        }
        assert!(hits > n / 3, "only {hits} hits");
    }

    #[test]
    fn bvh_agrees_with_brute_force() {
        let mut rng = Rng(0x9e3779b97f4a7c15);
        check(
            &positions(&reetched_io::Mesh::uv_sphere(1.0, 48, 24)),
            &mut rng,
        );
        // Random soup: overlapping, sliver and huge triangles.
        let soup: Vec<[Vec3; 3]> = (0..3000)
            .map(|_| {
                let c = rng.v() * 1.5;
                let s = 0.02 + rng.f() * rng.f() * 0.8;
                [c + rng.v() * s, c + rng.v() * s, c + rng.v() * s]
            })
            .collect();
        check(&soup, &mut rng);
        // A single triangle: the root itself is a leaf.
        check(&soup[..1], &mut rng);
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../reetched-substance/tests/data/bridge/mesh.obj"
        );
        if let Ok(mesh) = reetched_io::load_mesh(std::path::Path::new(path)) {
            let mut p = positions(&mesh);
            // Normalize into the [-1, 1] box the rays come from.
            let (lo, hi) = mesh.bounds();
            let (c, s) = ((lo + hi) / 2.0, 2.0 / (hi - lo).max_element());
            p.iter_mut().flatten().for_each(|v| *v = (*v - c) * s);
            check(&p, &mut rng);
        }
    }
}
