//! Replays the brush strokes of a converted Painter project with ReEtched's
//! brush engine (Painter stores strokes, not painted pixels).

use crate::app::State;
use crate::brush::{raycast, view_ray};
use crate::compositor::MASK_SLOT;
use crate::paint_gpu::{BrushShape, Camera, DAB_UV_SPACE, Dab, StrokeMode, StrokeTarget};
use glam::{Mat4, Vec2, Vec3};
use reetched_doc::{Channel, NodeId, NodeKind, TileMap, find_node, find_node_mut};
use reetched_io::topology::Selection;
use reetched_spp::convert::Job;
use reetched_spp::strokes::{DecalHit, DecalJob, PaintJob, Stroke};
use std::collections::BTreeMap;

/// Painter's brush size (size space 1) → radius in the normalized scene.
// ponytail: calibrated by comparing replays with Painter's output; revisit
// with projects that use other size spaces.
pub const SIZE_TO_RADIUS: f32 = 0.5;

/// `SIZE_TO_RADIUS`, overridable in tests for calibration.
fn size_to_radius() -> f32 {
    if cfg!(test)
        && let Some(v) = std::env::var("REETCHED_BRUSH_SCALE")
            .ok()
            .and_then(|v| v.parse().ok())
    {
        return v;
    }
    SIZE_TO_RADIUS
}

/// What a replayed action paints into.
struct Target<'a> {
    set: usize,
    node: NodeId,
    mask: bool,
    erase: bool,
    /// Layer paint: value per channel; mask paint: the grey value in `mask_value`.
    values: &'a [(Channel, [f32; 3])],
    mask_value: f32,
    opacity: f32,
}

impl State {
    /// Replays paint strokes and polygon fills onto the current document, in order.
    pub fn replay_strokes(&mut self, jobs: &[Job]) {
        let (mut missed, mut strokes, mut fills) = (0usize, 0usize, 0usize);
        for job in jobs {
            match job {
                Job::Paint(p) => {
                    strokes += p.strokes.len();
                    for s in &p.strokes {
                        missed += self.replay_stroke(p, s);
                    }
                }
                Job::Decal(d) => {
                    for hit in &d.hits {
                        fills += self.replay_fill(d, hit) as usize;
                    }
                }
            }
        }
        if strokes + fills > 0 {
            self.info(format!(
                "Replayed {strokes} Painter strokes and {fills} polygon fills"
            ));
        }
        if missed > 0 {
            self.warn(format!(
                "{missed} stroke points missed the model and were skipped"
            ));
        }
    }

    /// Starts a stroke on `t`, runs `paint`, and stores the result in the
    /// document. False when there is nothing to paint into.
    fn paint_into(&mut self, t: &Target, paint: impl FnOnce(&mut Self)) -> bool {
        let Some(set) = self.doc.texture_sets.get(t.set).cloned() else {
            return false;
        };
        let Some(node) = find_node(&set.stack, t.node) else {
            return false;
        };
        let tiles = self.viewport.tiles(t.set);
        let mut targets = Vec::new();
        if t.mask {
            let Some(mask) = &node.mask else { return false };
            targets.push(StrokeTarget {
                key: (node.id, MASK_SLOT),
                value: [t.mask_value; 3],
                is_normal: false,
                projected: false,
                old: tiles
                    .iter()
                    .filter_map(|&k| Some((k, mask.paint_for(k)?.clone())))
                    .collect(),
            });
        } else {
            let NodeKind::Paint(layer) = &node.kind else {
                return false;
            };
            let wanted: Vec<(Channel, [f32; 3])> = if t.erase {
                set.channels()
                    .into_iter()
                    .map(|c| (c, c.empty_value()))
                    .collect()
            } else {
                t.values
                    .iter()
                    .copied()
                    .filter(|(c, _)| set.has_channel(*c))
                    .collect()
            };
            for (c, mut value) in wanted {
                let old: BTreeMap<u32, TileMap> = tiles
                    .iter()
                    .filter_map(|&k| Some((k, layer.map(k, c)?.clone())))
                    .collect();
                if t.erase && old.values().all(|m| m.tiles.is_empty()) {
                    continue;
                }
                if c == Channel::Normal {
                    value = Vec3::from(value).normalize_or(Vec3::Z).into();
                }
                targets.push(StrokeTarget {
                    key: (node.id, c.index() as u8),
                    value,
                    is_normal: c == Channel::Normal,
                    projected: false,
                    old,
                });
            }
        }
        if targets.is_empty() {
            return false;
        }
        let mode = if t.erase {
            StrokeMode::Erase
        } else {
            StrokeMode::Paint
        };
        self.viewport.paint_begin(targets, &set, mode, t.opacity);
        paint(self);
        let results = match self
            .viewport
            .paint_end(&self.device, &self.queue, &mut self.resources)
        {
            Ok(r) => r,
            Err(e) => {
                self.error(format!("Could not replay Painter paint: {e:#}"));
                return false;
            }
        };
        let Some(node) = self
            .doc
            .texture_sets
            .get_mut(t.set)
            .and_then(|s| find_node_mut(&mut s.stack, t.node))
        else {
            return false;
        };
        for ((_, slot, tile), map) in results {
            if slot == MASK_SLOT {
                if let Some(mask) = &mut node.mask {
                    mask.set_paint(tile, map);
                }
            } else if let NodeKind::Paint(paint) = &mut node.kind {
                paint.set_map(tile, Channel::ALL[slot as usize], map);
            }
        }
        true
    }

    /// One stroke, with its own opacity cap (overlapping strokes build up as
    /// in Painter). Returns the points that didn't land on its texture set.
    fn replay_stroke(&mut self, job: &PaintJob, stroke: &Stroke) -> usize {
        let topo = self.topology();
        let (dabs, camera, missed) = self.stroke_dabs(job, stroke, &topo);
        if dabs.is_empty() {
            return missed;
        }
        let target = Target {
            set: job.set,
            node: job.node,
            mask: job.mask,
            erase: job.erase,
            values: &job.values,
            mask_value: job.mask_value,
            opacity: job.opacity,
        };
        let shape = BrushShape {
            hardness: job.hardness,
            backface_cos: None,
            alpha: None,
        };
        self.paint_into(&target, |s| {
            s.viewport.paint_dabs(
                &s.device,
                &s.queue,
                job.set,
                &camera,
                false,
                &shape,
                &dabs,
                [0.0; 2],
                &s.resources,
            );
        });
        missed
    }

    /// One Polygon Fill click or rectangle. True when it filled something.
    fn replay_fill(&mut self, job: &DecalJob, hit: &DecalHit) -> bool {
        let tris = self.decal_triangles(job.set, hit);
        if tris.is_empty() {
            return false;
        }
        let values: Vec<(Channel, [f32; 3])> = self
            .doc
            .texture_sets
            .get(job.set)
            .map(|s| s.channels().into_iter().map(|c| (c, hit.color)).collect())
            .unwrap_or_default();
        let target = Target {
            set: job.set,
            node: job.node,
            mask: job.mask,
            erase: false,
            values: &values,
            mask_value: hit.color[0],
            opacity: 1.0,
        };
        // One fill per UDIM tile the triangles are in.
        let mut by_tile: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for &t in &tris {
            by_tile
                .entry(self.mesh.triangle_udim(t as usize))
                .or_default()
                .extend_from_slice(&self.mesh.indices[t as usize * 3..t as usize * 3 + 3]);
        }
        self.paint_into(&target, |s| {
            for (tile, indices) in by_tile {
                s.viewport
                    .paint_fill(&s.device, &s.queue, job.set, tile, &indices, &s.resources);
            }
        })
    }

    /// The triangles a Polygon Fill hit selects, grown to its granularity.
    fn decal_triangles(&mut self, set: usize, hit: &DecalHit) -> Vec<u32> {
        let mode = match hit.granularity {
            1 => Selection::Polygon,
            2 => Selection::Mesh,
            3 => Selection::UvIsland,
            _ => Selection::Triangle,
        };
        let topo = self.topology();
        let Some(part) = self.mesh.parts.get(set) else {
            return Vec::new();
        };
        let range = part.indices.start / 3..part.indices.end / 3;
        let lo = Vec2::from(hit.corners[0]).min(Vec2::from(hit.corners[1]));
        let hi = Vec2::from(hit.corners[0]).max(Vec2::from(hit.corners[1]));
        let corner = |t: u32, k: usize| self.mesh.indices[t as usize * 3 + k] as usize;
        let mut seeds: Vec<(u32, Vec3)> = Vec::new();
        match hit.inv_view_proj {
            // 2D view: positions are UVs.
            None => {
                let uv_of = |p: Vec2| p;
                if hit.rect {
                    let (a, b) = (uv_of(lo), uv_of(hi));
                    let (ulo, uhi) = (a.min(b), a.max(b));
                    for t in range.clone() {
                        let c = (0..3)
                            .map(|k| Vec2::from(self.mesh.vertices[corner(t, k)].uv))
                            .sum::<Vec2>()
                            / 3.0;
                        if c.cmpge(ulo).all() && c.cmple(uhi).all() {
                            seeds.push((t, Vec3::ZERO));
                        }
                    }
                } else if let Some(t) = topo.triangle_at_uv(&self.mesh, set, uv_of(lo)) {
                    let at = reetched_io::topology::Topology::point_at_uv(&self.mesh, t, uv_of(lo));
                    seeds.push((t as u32, at));
                }
            }
            Some(inv) => {
                let inv = Mat4::from_cols_array(&inv);
                let view_proj = inv.inverse();
                let ndc = |p: Vec2| Vec2::new(p.x * 2.0 - 1.0, p.y * 2.0 - 1.0);
                if hit.rect {
                    let eye = inv.project_point3(Vec3::new(0.0, 0.0, 0.0));
                    let (a, b) = (ndc(lo), ndc(hi));
                    let (nlo, nhi) = (a.min(b), a.max(b));
                    for t in range.clone() {
                        let [p0, p1, p2] = [0, 1, 2]
                            .map(|k| Vec3::from(self.mesh.vertices[corner(t, k)].position));
                        let c = (p0 + p1 + p2) / 3.0;
                        let facing = (p1 - p0).cross(p2 - p0).dot(eye - c) > 0.0;
                        let q = view_proj.project_point3(c).truncate();
                        if facing && q.cmpge(nlo).all() && q.cmple(nhi).all() {
                            seeds.push((t, c));
                        }
                    }
                } else {
                    let (o, d) = view_ray(view_proj, ndc(lo));
                    if let Some(h) = raycast(&self.mesh, o, d).filter(|h| h.part == set) {
                        seeds.push((h.tri as u32, h.pos));
                    }
                }
            }
        }
        if mode == Selection::Triangle {
            return seeds.into_iter().map(|(t, _)| t).collect();
        }
        let mut out = std::collections::BTreeSet::new();
        for (t, at) in seeds {
            if out.contains(&t) {
                continue;
            }
            out.extend(topo.select_at(t as usize, mode, at));
        }
        out.into_iter().collect()
    }

    /// The dabs of one stroke, the camera to paint them with, and the points that missed.
    fn stroke_dabs(
        &self,
        job: &PaintJob,
        s: &Stroke,
        topo: &reetched_io::topology::Topology,
    ) -> (Vec<Dab>, Camera, usize) {
        let view = Mat4::from_cols_array(&s.view);
        let proj = Mat4::from_cols_array(&s.proj);
        let view_proj = proj * view;
        // Normalized view coordinates, y down.
        let ndc = |p: [f32; 2]| Vec2::new(p[0] * 2.0 - 1.0, p[1] * 2.0 - 1.0);
        let pressure = if s.pressure.is_empty() {
            1.0
        } else {
            s.pressure.iter().map(|k| k[1]).sum::<f32>() / s.pressure.len() as f32
        };
        let radius = job.size
            * size_to_radius()
            * if job.pressure_size {
                pressure.max(0.05)
            } else {
                1.0
            };
        let flow = job.flow * if job.pressure_flow { pressure } else { 1.0 };
        let step = (job.spacing.max(0.02) * 2.0 * radius).max(1e-5);
        let inv_view = view.inverse();
        let eye = inv_view.w_axis.truncate();
        let right = inv_view.x_axis.truncate().normalize_or(Vec3::X);
        let up = inv_view.y_axis.truncate().normalize_or(Vec3::Y);
        let camera = Camera { view_proj, eye };
        if s.uv_view {
            // The 2D view: no camera is stored; the points are UVs.
            let uvs: Vec<Vec2> = s.points.iter().map(|&p| Vec2::from(p)).collect();
            let mirrors: Vec<Mat4> = s.mirrors.iter().map(Mat4::from_cols_array).collect();
            let mut dabs = Vec::new();
            for uv in spaced(&uvs, step, |a, b| a.distance(b), |a, b, t| a.lerp(b, t)) {
                dabs.push(Dab {
                    right_flow: [0.0, 0.0, 0.0, flow],
                    view_flags: [0.0, 0.0, 0.0, DAB_UV_SPACE as f32],
                    uv: [uv.x, uv.y, radius, 0.0],
                    ..Default::default()
                });
                // Symmetry is a 3D mirror even when painting in the 2D view:
                // stamp the copy in 3D, sized by the surface's UV scale there.
                if mirrors.is_empty() {
                    continue;
                }
                let Some(tri) = topo.triangle_at_uv(&self.mesh, job.set, uv) else {
                    continue;
                };
                let idx = &self.mesh.indices[tri * 3..tri * 3 + 3];
                let v = |k: usize| &self.mesh.vertices[idx[k] as usize];
                let [p0, p1, p2] = [0, 1, 2].map(|k| Vec3::from(v(k).position));
                let [t0, t1, t2] = [0, 1, 2].map(|k| Vec2::from(v(k).uv));
                let world_area = (p1 - p0).cross(p2 - p0).length();
                let uv_area = (t1 - t0).perp_dot(t2 - t0).abs().max(1e-12);
                let scale = (world_area / uv_area).sqrt();
                let at = reetched_io::topology::Topology::point_at_uv(&self.mesh, tri, uv);
                let normal = (p1 - p0).cross(p2 - p0).normalize_or(Vec3::Z);
                for m in &mirrors {
                    let n = m.transform_vector3(normal).normalize_or(Vec3::Z);
                    let right = n.any_orthonormal_vector();
                    let up = n.cross(right);
                    dabs.push(Dab {
                        center_radius: m.transform_point3(at).extend(radius * scale).into(),
                        right_flow: right.extend(flow).into(),
                        up_angle: up.extend(0.0).into(),
                        view_flags: (-n).extend(0.0).into(),
                        uv: [0.0; 4],
                    });
                }
            }
            return (
                dabs,
                Camera {
                    view_proj: Mat4::IDENTITY,
                    eye: Vec3::ZERO,
                },
                0,
            );
        }
        use rayon::prelude::*;
        let mesh = &self.mesh;
        let hits: Vec<Option<Vec3>> = s
            .points
            .par_iter()
            .map(|&p| {
                let (o, d) = view_ray(view_proj, ndc(p));
                raycast(mesh, o, d)
                    .filter(|h| h.part == job.set)
                    .map(|h| h.pos)
            })
            .collect();
        let missed = hits.iter().filter(|h| h.is_none()).count();
        // Runs of consecutive hits become dab paths (a miss breaks the path).
        let mut dabs = Vec::new();
        let mirrors: Vec<Mat4> = s.mirrors.iter().map(Mat4::from_cols_array).collect();
        for run in hits.split(Option::is_none) {
            let run: Vec<Vec3> = run.iter().flatten().copied().collect();
            for c in spaced(&run, step, |a, b| a.distance(b), |a, b, t| a.lerp(b, t)) {
                let dir = (c - eye).normalize_or(-Vec3::Z);
                let dab = |c: Vec3, r: Vec3, u: Vec3, d: Vec3| Dab {
                    center_radius: c.extend(radius).into(),
                    right_flow: r.extend(flow).into(),
                    up_angle: u.extend(0.0).into(),
                    view_flags: d.extend(0.0).into(),
                    uv: [0.0; 4],
                };
                dabs.push(dab(c, right, up, dir));
                for m in &mirrors {
                    dabs.push(dab(
                        m.transform_point3(c),
                        m.transform_vector3(right),
                        m.transform_vector3(up),
                        m.transform_vector3(dir),
                    ));
                }
            }
        }
        (dabs, camera, missed)
    }
}

/// Points every `step` along a polyline (the first point always).
fn spaced<T: Copy>(
    pts: &[T],
    step: f32,
    dist: impl Fn(T, T) -> f32,
    lerp: impl Fn(T, T, f32) -> T,
) -> Vec<T> {
    let Some(&first) = pts.first() else {
        return Vec::new();
    };
    let mut out = vec![first];
    let mut carry = 0.0;
    for w in pts.windows(2) {
        let len = dist(w[0], w[1]);
        if len <= 0.0 {
            continue;
        }
        let mut d = step - carry;
        while d <= len {
            out.push(lerp(w[0], w[1], d / len));
            d += step;
        }
        carry = len - (d - step);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dabs_are_evenly_spaced_along_the_path() {
        let pts = [Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0)];
        let out = spaced(&pts, 0.25, |a, b| a.distance(b), |a, b, t| a.lerp(b, t));
        assert_eq!(out.len(), 9);
        assert!((out[4] - Vec2::new(1.0, 0.0)).length() < 1e-5);
        assert!((out[8] - Vec2::new(1.0, 1.0)).length() < 1e-5);
    }
}
