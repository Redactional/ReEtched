//! Brush settings, and turning pointer movement into dabs: ray casting onto the
//! mesh, spacing, lazy-mouse smoothing and symmetry.

use crate::paint_gpu::{DAB_DEPTH_TEST, DAB_UV_SPACE, Dab};
use glam::{Mat4, Quat, Vec2, Vec3};
use reetched_doc::{Channel, ResourceId};
use reetched_io::Mesh;
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn vector(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Symmetry {
    Off,
    /// Mirror across the plane through the mesh center, perpendicular to the axis.
    Mirror(Axis),
    /// `count` copies around the axis through the mesh center.
    Radial(Axis, u32),
}

#[derive(Clone, PartialEq, Debug)]
pub struct BrushChannel {
    pub enabled: bool,
    pub value: [f32; 3],
}

#[derive(Clone, PartialEq, Debug)]
pub struct Brush {
    /// Radius in screen points.
    pub size: f32,
    /// Coverage per dab, 0..=1.
    pub flow: f32,
    /// Most a single stroke can cover, 0..=1.
    pub opacity: f32,
    /// Dab distance as a fraction of the brush diameter.
    pub spacing: f32,
    pub hardness: f32,
    pub angle_deg: f32,
    /// Skip surfaces facing away more than `backface_deg` from the view.
    pub backface_culling: bool,
    pub backface_deg: f32,
    /// Lazy mouse distance in points (0 = off).
    pub lazy: f32,
    pub pressure_size: bool,
    pub pressure_flow: bool,
    pub symmetry: Symmetry,
    pub channels: BTreeMap<Channel, BrushChannel>,
    /// Grey level painted into masks.
    pub mask_value: f32,
    pub alpha: Option<ResourceId>,
}

impl Default for Brush {
    fn default() -> Self {
        let ch = |enabled, value| BrushChannel { enabled, value };
        Self {
            size: 30.0,
            flow: 1.0,
            opacity: 1.0,
            spacing: 0.08,
            hardness: 0.6,
            angle_deg: 0.0,
            backface_culling: true,
            backface_deg: 85.0,
            lazy: 0.0,
            pressure_size: true,
            pressure_flow: false,
            symmetry: Symmetry::Off,
            channels: BTreeMap::from([
                (Channel::BaseColor, ch(true, [0.8, 0.8, 0.8])),
                (Channel::Metallic, ch(true, [0.0; 3])),
                (Channel::Roughness, ch(true, [0.4; 3])),
                (Channel::Normal, ch(false, [0.0, 0.0, 1.0])),
                (Channel::Height, ch(false, [0.25; 3])),
            ]),
            mask_value: 1.0,
            alpha: None,
        }
    }
}

impl Brush {
    pub fn backface_cos(&self) -> Option<f32> {
        self.backface_culling
            .then(|| self.backface_deg.to_radians().cos())
    }

    fn size_factor(&self, pressure: f32) -> f32 {
        if self.pressure_size {
            pressure.max(0.05)
        } else {
            1.0
        }
    }

    fn flow_factor(&self, pressure: f32) -> f32 {
        self.flow * if self.pressure_flow { pressure } else { 1.0 }
    }

    /// Distance between dabs, in points.
    pub fn step(&self, pressure: f32) -> f32 {
        (self.spacing * 2.0 * self.size * self.size_factor(pressure)).max(1.0)
    }
}

pub struct Hit {
    pub pos: Vec3,
    /// UV coordinates at the hit point.
    pub uv: Vec2,
    /// Texture set (mesh part) that was hit.
    pub part: usize,
}

/// Nearest triangle hit along a ray (Möller–Trumbore).
/// ponytail: brute force over every triangle; add a BVH (e.g. obvhs) when meshes get large.
pub fn raycast(mesh: &Mesh, origin: Vec3, dir: Vec3) -> Option<Hit> {
    let mut best: Option<(f32, usize, f32, f32)> = None;
    for (t_index, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let [a, b, c] = tri.map(|i| Vec3::from(mesh.vertices[i as usize].position));
        let (e1, e2) = (b - a, c - a);
        let p = dir.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-12 {
            continue;
        }
        let inv = 1.0 / det;
        let s = origin - a;
        let u = s.dot(p) * inv;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = s.cross(e1);
        let v = dir.dot(q) * inv;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = e2.dot(q) * inv;
        if t > 1e-6 && best.is_none_or(|(bt, ..)| t < bt) {
            best = Some((t, t_index, u, v));
        }
    }
    let (t, t_index, u, v) = best?;
    let first = t_index as u32 * 3;
    let part = mesh.parts.iter().position(|p| p.indices.contains(&first))?;
    let uv = |k: u32| Vec2::from(mesh.vertices[mesh.indices[(first + k) as usize] as usize].uv);
    Some(Hit {
        pos: origin + dir * t,
        uv: uv(0) * (1.0 - u - v) + uv(1) * u + uv(2) * v,
        part,
    })
}

/// Ray through a point of the view (`ndc` in -1..1, y up).
pub fn view_ray(view_proj: Mat4, ndc: Vec2) -> (Vec3, Vec3) {
    let inv = view_proj.inverse();
    let near = inv.project_point3(ndc.extend(0.0));
    let far = inv.project_point3(ndc.extend(1.0));
    (near, (far - near).normalize())
}

/// Turns pointer samples into evenly spaced stamp positions, with lazy-mouse smoothing.
#[derive(Default)]
pub struct StrokePath {
    last: Option<Vec2>,
    /// Distance travelled since the last stamp.
    carry: f32,
    lazy: Option<Vec2>,
}

impl StrokePath {
    /// Feeds the pointer position; returns where to stamp. The first call stamps immediately.
    pub fn advance(&mut self, pointer: Vec2, step: f32, lazy: f32) -> Vec<Vec2> {
        let target = if lazy > 0.0 {
            let s = self.lazy.unwrap_or(pointer);
            let d = pointer - s;
            let s = if d.length() > lazy {
                s + d * (1.0 - lazy / d.length())
            } else {
                s
            };
            self.lazy = Some(s);
            s
        } else {
            pointer
        };
        let Some(last) = self.last else {
            self.last = Some(target);
            self.carry = 0.0;
            return vec![target];
        };
        let seg = target - last;
        let len = seg.length();
        let mut out = Vec::new();
        let mut d = step - self.carry;
        while d <= len {
            out.push(last + seg * (d / len));
            d += step;
        }
        self.carry = len - (d - step);
        self.last = Some(target);
        out
    }

    pub fn last(&self) -> Option<Vec2> {
        self.last
    }
}

/// How the 3D view maps screen points to the world, for sizing dabs.
pub struct ViewInfo {
    pub view_proj: Mat4,
    pub eye: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub orthographic: bool,
    /// World units per screen point at distance 1 (perspective) or absolute (orthographic).
    pub world_per_point: f32,
    /// Symmetry center.
    pub center: Vec3,
}

/// Dabs for one stamp in the 3D view (plus its symmetric copies).
pub fn world_dabs(brush: &Brush, view: &ViewInfo, hit: &Hit, pressure: f32) -> Vec<Dab> {
    let distance = if view.orthographic {
        1.0
    } else {
        (hit.pos - view.eye).dot(view.forward).max(1e-4)
    };
    let radius = brush.size * brush.size_factor(pressure) * view.world_per_point * distance;
    let view_dir = if view.orthographic {
        view.forward
    } else {
        (hit.pos - view.eye).normalize()
    };
    let dab = |c: Vec3, right: Vec3, up: Vec3, dir: Vec3, flags: u32| Dab {
        center_radius: c.extend(radius).into(),
        right_flow: right.extend(brush.flow_factor(pressure)).into(),
        up_angle: up.extend(brush.angle_deg.to_radians()).into(),
        view_flags: dir.extend(flags as f32).into(),
        uv: [0.0; 4],
    };
    let mut out = vec![dab(hit.pos, view.right, view.up, view_dir, DAB_DEPTH_TEST)];
    // Symmetric copies paint even where the camera can't see, like Painter.
    match brush.symmetry {
        Symmetry::Off => {}
        Symmetry::Mirror(axis) => {
            let a = axis.vector();
            let reflect = |v: Vec3| v - 2.0 * v.dot(a) * a;
            let p = view.center + reflect(hit.pos - view.center);
            out.push(dab(
                p,
                reflect(view.right),
                reflect(view.up),
                reflect(view_dir),
                0,
            ));
        }
        Symmetry::Radial(axis, count) => {
            for k in 1..count.max(1) {
                let q = Quat::from_axis_angle(
                    axis.vector(),
                    k as f32 * std::f32::consts::TAU / count as f32,
                );
                let p = view.center + q * (hit.pos - view.center);
                out.push(dab(p, q * view.right, q * view.up, q * view_dir, 0));
            }
        }
    }
    out
}

/// A dab placed directly in UV space (2D view).
pub fn uv_dab(brush: &Brush, uv: Vec2, radius_uv: f32, pressure: f32) -> Dab {
    Dab {
        right_flow: [0.0, 0.0, 0.0, brush.flow_factor(pressure)],
        up_angle: [0.0, 0.0, 0.0, brush.angle_deg.to_radians()],
        view_flags: [0.0, 0.0, 0.0, DAB_UV_SPACE as f32],
        uv: [uv.x, uv.y, radius_uv * brush.size_factor(pressure), 0.0],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_spacing_carries_over_between_samples() {
        let mut p = StrokePath::default();
        assert_eq!(p.advance(Vec2::ZERO, 10.0, 0.0), [Vec2::ZERO]);
        let a = p.advance(Vec2::new(25.0, 0.0), 10.0, 0.0);
        assert_eq!(a, [Vec2::new(10.0, 0.0), Vec2::new(20.0, 0.0)]);
        // 5 carried over: the next stamp lands at 30.
        let b = p.advance(Vec2::new(32.0, 0.0), 10.0, 0.0);
        assert_eq!(b, [Vec2::new(30.0, 0.0)]);
    }

    #[test]
    fn lazy_mouse_trails_the_pointer() {
        let mut p = StrokePath::default();
        p.advance(Vec2::ZERO, 1.0, 20.0);
        let stamps = p.advance(Vec2::new(30.0, 0.0), 1.0, 20.0);
        // The brush follows 20 points behind: it moved only to x = 10.
        assert!((p.last().unwrap().x - 10.0).abs() < 1e-4);
        assert!(stamps.iter().all(|s| s.x <= 10.0 + 1e-4), "{stamps:?}");
    }

    #[test]
    fn raycast_hits_the_near_side_of_a_sphere() {
        let mesh = Mesh::uv_sphere(1.0, 32, 16);
        let hit = raycast(&mesh, Vec3::new(0.0, 0.0, 5.0), -Vec3::Z).unwrap();
        assert!((hit.pos.z - 1.0).abs() < 0.02, "{}", hit.pos);
        assert_eq!(hit.part, 0);
        assert!(raycast(&mesh, Vec3::new(0.0, 3.0, 5.0), -Vec3::Z).is_none());
    }

    #[test]
    fn mirror_symmetry_reflects_position_and_direction() {
        let brush = Brush {
            symmetry: Symmetry::Mirror(Axis::X),
            ..Default::default()
        };
        let view = ViewInfo {
            view_proj: Mat4::IDENTITY,
            eye: Vec3::new(0.0, 0.0, 5.0),
            forward: -Vec3::Z,
            right: Vec3::X,
            up: Vec3::Y,
            orthographic: false,
            world_per_point: 0.01,
            center: Vec3::ZERO,
        };
        let hit = Hit {
            pos: Vec3::new(0.5, 0.0, 0.8),
            uv: Vec2::ZERO,
            part: 0,
        };
        let dabs = world_dabs(&brush, &view, &hit, 1.0);
        assert_eq!(dabs.len(), 2);
        assert_eq!(dabs[1].center_radius[0], -0.5);
        assert_eq!(dabs[1].right_flow[0], -1.0);
        assert_eq!(dabs[0].view_flags[3] as u32, DAB_DEPTH_TEST);
        assert_eq!(
            dabs[1].view_flags[3] as u32, 0,
            "copies skip the occlusion test"
        );
    }
}
