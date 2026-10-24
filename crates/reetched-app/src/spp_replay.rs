//! Replays the brush strokes of a converted Painter project with ReEtched's
//! brush engine (Painter stores strokes, not painted pixels).

use crate::app::State;
use crate::brush::{raycast, view_ray};
use crate::compositor::MASK_SLOT;
use crate::paint_gpu::{BrushShape, Camera, DAB_UV_SPACE, Dab, StrokeMode, StrokeTarget};
use glam::{Mat4, Vec2, Vec3};
use reetched_doc::{Channel, NodeKind, TileMap, find_node, find_node_mut};
use reetched_spp::strokes::{PaintJob, Stroke};
use std::collections::BTreeMap;

/// Painter's brush size (size space 1) → radius in the normalized scene.
// ponytail: calibrated by comparing replays with Painter's output; revisit
// with projects that use other size spaces.
pub const SIZE_TO_RADIUS: f32 = 0.5;

impl State {
    /// Replays every paint job onto the current document.
    pub fn replay_strokes(&mut self, jobs: &[PaintJob]) {
        let mut missed = 0usize;
        for job in jobs {
            missed += self.replay(job);
        }
        if !jobs.is_empty() {
            let strokes: usize = jobs.iter().map(|j| j.strokes.len()).sum();
            self.info(format!("Replayed {strokes} Painter strokes"));
        }
        if missed > 0 {
            self.warn(format!(
                "{missed} stroke points missed the model and were skipped"
            ));
        }
    }

    /// One job, stroke by stroke (each stroke has its own opacity cap, so
    /// overlapping strokes build up as in Painter). Returns the points that
    /// didn't land on its texture set.
    fn replay(&mut self, job: &PaintJob) -> usize {
        job.strokes.iter().map(|s| self.replay_stroke(job, s)).sum()
    }

    fn replay_stroke(&mut self, job: &PaintJob, stroke: &Stroke) -> usize {
        let Some(set) = self.doc.texture_sets.get(job.set).cloned() else {
            return 0;
        };
        let Some(node) = find_node(&set.stack, job.node) else {
            return 0;
        };
        let tiles = self.viewport.tiles(job.set);
        let mut targets = Vec::new();
        if job.mask {
            let Some(mask) = &node.mask else { return 0 };
            targets.push(StrokeTarget {
                key: (node.id, MASK_SLOT),
                value: [job.mask_value; 3],
                is_normal: false,
                projected: false,
                old: tiles
                    .iter()
                    .filter_map(|&t| Some((t, mask.paint_for(t)?.clone())))
                    .collect(),
            });
        } else {
            let NodeKind::Paint(paint) = &node.kind else {
                return 0;
            };
            let wanted: Vec<(Channel, [f32; 3])> = if job.erase {
                set.channels()
                    .into_iter()
                    .map(|c| (c, c.empty_value()))
                    .collect()
            } else {
                job.values
                    .iter()
                    .copied()
                    .filter(|(c, _)| set.has_channel(*c))
                    .collect()
            };
            for (c, mut value) in wanted {
                let old: BTreeMap<u32, TileMap> = tiles
                    .iter()
                    .filter_map(|&t| Some((t, paint.map(t, c)?.clone())))
                    .collect();
                if job.erase && old.values().all(|m| m.tiles.is_empty()) {
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
            return 0;
        }
        let mode = if job.erase {
            StrokeMode::Erase
        } else {
            StrokeMode::Paint
        };
        self.viewport.paint_begin(targets, &set, mode, job.opacity);
        let shape = BrushShape {
            hardness: job.hardness,
            backface_cos: None,
            alpha: None,
        };
        let (dabs, camera, missed) = self.stroke_dabs(job, stroke);
        if !dabs.is_empty() {
            self.viewport.paint_dabs(
                &self.device,
                &self.queue,
                job.set,
                &camera,
                false,
                &shape,
                &dabs,
                [0.0; 2],
                &self.resources,
            );
        }
        let results = match self
            .viewport
            .paint_end(&self.device, &self.queue, &mut self.resources)
        {
            Ok(r) => r,
            Err(e) => {
                self.error(format!("Could not replay a Painter stroke: {e:#}"));
                return missed;
            }
        };
        let Some(node) = self
            .doc
            .texture_sets
            .get_mut(job.set)
            .and_then(|s| find_node_mut(&mut s.stack, job.node))
        else {
            return missed;
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
        missed
    }

    /// The dabs of one stroke, the camera to paint them with, and the points that missed.
    fn stroke_dabs(&self, job: &PaintJob, s: &Stroke) -> (Vec<Dab>, Camera, usize) {
        let view = Mat4::from_cols_array(&s.view);
        let proj = Mat4::from_cols_array(&s.proj);
        let view_proj = proj * view;
        // Normalized view coordinates, y down.
        let ndc = |p: [f32; 2]| Vec2::new(p[0] * 2.0 - 1.0, 1.0 - p[1] * 2.0);
        let pressure = if s.pressure.is_empty() {
            1.0
        } else {
            s.pressure.iter().map(|k| k[1]).sum::<f32>() / s.pressure.len() as f32
        };
        let radius = job.size
            * SIZE_TO_RADIUS
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
            // The 2D view: no camera is stored with these strokes.
            // ponytail: points are taken as UVs (y down); they are really 2D view
            // positions, so placement is approximate until the view mapping is known.
            let uvs: Vec<Vec2> = s
                .points
                .iter()
                .map(|&p| Vec2::new(p[0], 1.0 - p[1]))
                .collect();
            let dabs = spaced(&uvs, step, |a, b| a.distance(b), |a, b, t| a.lerp(b, t))
                .into_iter()
                .map(|uv| Dab {
                    right_flow: [0.0, 0.0, 0.0, flow],
                    view_flags: [0.0, 0.0, 0.0, DAB_UV_SPACE as f32],
                    uv: [uv.x, uv.y, radius, 0.0],
                    ..Default::default()
                })
                .collect();
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
