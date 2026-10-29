//! Polygon Fill and Path tools, and their options in Properties.

use crate::{
    app::{State, Tool},
    brush::{StrokePath, raycast, view_ray},
    paint_gpu::StrokeMode,
    theme,
};
use egui::{Color32, Rect, RichText, Stroke, Ui};
use glam::{Vec2, Vec3};
use reetched_io::topology::{Selection, Topology};
use std::sync::Arc;

/// What the Path tool does along its curve.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathMode {
    Paint,
    Erase,
    Smudge,
    /// The brush alpha drawn as one continuous band along the path.
    Ribbon,
    /// Closes the path and fills the faces inside it.
    Fill,
}

impl PathMode {
    pub const ALL: [PathMode; 5] = [
        PathMode::Paint,
        PathMode::Erase,
        PathMode::Smudge,
        PathMode::Ribbon,
        PathMode::Fill,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PathMode::Paint => "Paint",
            PathMode::Erase => "Erase",
            PathMode::Smudge => "Smudge",
            PathMode::Ribbon => "Ribbon",
            PathMode::Fill => "Filled",
        }
    }
}

/// The Projection tool's image, placed over the 3D view.
pub struct Stencil {
    pub image: Option<reetched_doc::ResourceId>,
    pub texture: Option<egui::TextureHandle>,
    /// Image width / height.
    pub aspect: f32,
    /// Center in NDC (-1..1, y up).
    pub center: Vec2,
    /// Half height in NDC.
    pub size: f32,
    pub rotation_deg: f32,
    /// How visible the overlay is.
    pub preview: f32,
}

impl Default for Stencil {
    fn default() -> Self {
        Self {
            image: None,
            texture: None,
            aspect: 1.0,
            center: Vec2::ZERO,
            size: 0.5,
            rotation_deg: 0.0,
            preview: 0.5,
        }
    }
}

impl Stencil {
    /// Half extents in NDC for a view of `rect`'s shape.
    fn half(&self, rect: Rect) -> Vec2 {
        Vec2::new(
            self.size * self.aspect * rect.height() / rect.width(),
            self.size,
        )
    }

    pub fn uniform(&self, view_proj: glam::Mat4, rect: Rect) -> crate::paint_gpu::ProjectUniform {
        let h = self.half(rect);
        let a = self.rotation_deg.to_radians();
        crate::paint_gpu::ProjectUniform {
            view_proj: view_proj.to_cols_array_2d(),
            place: [self.center.x, self.center.y, h.x, h.y],
            rot: [a.cos(), a.sin(), 0.0, 0.0],
        }
    }

    /// Screen corners: top-left, top-right, bottom-right, bottom-left of the image.
    fn corners(&self, rect: Rect) -> [egui::Pos2; 4] {
        let h = self.half(rect);
        let (c, s) = (
            self.rotation_deg.to_radians().cos(),
            self.rotation_deg.to_radians().sin(),
        );
        [(-1.0, 1.0), (1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)].map(|(x, y)| {
            let q = Vec2::new(x * h.x, y * h.y);
            let d = Vec2::new(c * q.x - s * q.y, s * q.x + c * q.y) + self.center;
            egui::pos2(
                rect.min.x + (d.x + 1.0) * 0.5 * rect.width(),
                rect.min.y + (1.0 - d.y) * 0.5 * rect.height(),
            )
        })
    }
}

impl State {
    /// Mesh adjacency, built the first time a tool needs it.
    pub fn topology(&mut self) -> Arc<Topology> {
        self.topology
            .get_or_insert_with(|| Arc::new(Topology::new(&self.mesh)))
            .clone()
    }

    /// Projection tool overlay and Ctrl+drag to move the image.
    pub fn stencil_input(&mut self, ui: &Ui, response: &egui::Response, rect: Rect) {
        if ui.input(|i| i.modifiers.ctrl) && response.dragged() {
            let d = response.drag_delta();
            self.stencil.center += Vec2::new(2.0 * d.x / rect.width(), -2.0 * d.y / rect.height());
        }
        let Some(tex) = &self.stencil.texture else {
            return;
        };
        let [a, b, c, d] = self.stencil.corners(rect);
        let tint = Color32::from_white_alpha((self.stencil.preview * 255.0) as u8);
        let mut mesh = egui::Mesh::with_texture(tex.id());
        for (p, uv) in [
            (a, (0.0, 0.0)),
            (b, (1.0, 0.0)),
            (c, (1.0, 1.0)),
            (d, (0.0, 1.0)),
        ] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: p,
                uv: egui::pos2(uv.0, uv.1),
                color: tint,
            });
        }
        mesh.indices = vec![0, 1, 2, 0, 2, 3];
        let painter = ui.painter_at(rect);
        painter.add(egui::Shape::mesh(mesh));
        painter.add(egui::Shape::closed_line(
            vec![a, b, c, d],
            Stroke::new(1.0, Color32::from_white_alpha(120)),
        ));
    }

    /// Projection tool: makes the stencil the current stroke's color source.
    pub fn project_stencil(&mut self) -> bool {
        let Some(id) = self.stencil.image.clone() else {
            self.warn("Load an image for the Projection tool first".into());
            return false;
        };
        let Some(view) = self
            .viewport
            .color_image(&self.device, &self.queue, &self.resources, &id)
        else {
            self.warn("The projection image could not be decoded".into());
            return false;
        };
        let rect = self.view3d_rect;
        let u = self
            .stencil
            .uniform(self.view_proj(rect.width() / rect.height()), rect);
        self.viewport.paint_project(view, u);
        true
    }

    fn load_stencil(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", crate::images::IMAGE_EXTENSIONS)
            .pick_file()
        else {
            return;
        };
        match std::fs::read(&path) {
            Ok(bytes) => self.set_stencil(ctx, &bytes, &path.display().to_string()),
            Err(e) => self.error(format!("Could not read {}: {e}", path.display())),
        }
    }

    /// Makes image file `bytes` the Projection tool's image.
    pub fn set_stencil(&mut self, ctx: &egui::Context, bytes: &[u8], name: &str) {
        let img = match crate::images::decode(bytes) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                self.error(format!("Could not decode {name}: {e:#}"));
                return;
            }
        };
        let (w, h) = img.dimensions();
        self.stencil.aspect = w as f32 / h.max(1) as f32;
        self.stencil.texture = Some(ctx.load_texture(
            "stencil",
            egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &img),
            egui::TextureOptions::LINEAR,
        ));
        // Held in resources (not the document) until painted with.
        self.stencil.image = Some(self.resources.add(bytes.to_vec()));
    }

    /// Polygon Fill: click fills the selection with the brush, Ctrl+click erases it;
    /// dragging (3D view) fills every front-facing triangle inside the rectangle.
    pub fn polygon_input(&mut self, ui: &Ui, response: &egui::Response, rect: Rect, uv_view: bool) {
        if self.tool != Tool::PolygonFill || ui.input(|i| i.modifiers.alt) {
            return;
        }
        if !uv_view {
            self.marquee_input(ui, response, rect);
        }
        if !response.clicked() {
            return;
        }
        let Some(p) = response.interact_pointer_pos() else {
            return;
        };
        let erase = ui.input(|i| i.modifiers.ctrl);
        let topo = self.topology();
        let hit = if uv_view {
            self.uv_under(p, rect, true).and_then(|(uv, part)| {
                let tri = topo.triangle_at_uv(&self.mesh, part, uv)?;
                Some((tri, part, Topology::point_at_uv(&self.mesh, tri, uv)))
            })
        } else {
            let view_proj = self.view_proj(rect.width() / rect.height());
            let ndc = Vec2::new(
                (p.x - rect.min.x) / rect.width() * 2.0 - 1.0,
                1.0 - (p.y - rect.min.y) / rect.height() * 2.0,
            );
            let (origin, dir) = view_ray(view_proj, ndc);
            raycast(&self.mesh, origin, dir).map(|h| (h.tri, h.part, h.pos))
        };
        let Some((tri, part, at)) = hit else { return };
        if part != self.selected_part {
            self.warn("That's another texture set — select it first".into());
            return;
        }
        self.polygon_fill(&topo.select_at(tri, self.polygon_mode, at), erase);
    }

    fn marquee_input(&mut self, ui: &Ui, response: &egui::Response, rect: Rect) {
        if response.drag_started() {
            self.marquee = response.interact_pointer_pos();
        }
        let (Some(start), Some(now)) = (self.marquee, ui.input(|i| i.pointer.interact_pos()))
        else {
            return;
        };
        let sel = Rect::from_two_pos(start, now);
        ui.painter_at(rect).rect_stroke(
            sel,
            0.0,
            Stroke::new(1.0, Color32::from_rgb(90, 170, 255)),
            egui::StrokeKind::Inside,
        );
        if !response.drag_stopped() {
            return;
        }
        self.marquee = None;
        let view_proj = self.view_proj(rect.width() / rect.height());
        let eye = self.camera.eye();
        let part = &self.mesh.parts[self.selected_part.min(self.mesh.parts.len() - 1)];
        let tris: Vec<u32> = (part.indices.start / 3..part.indices.end / 3)
            .filter(|&t| {
                let idx = &self.mesh.indices[t as usize * 3..t as usize * 3 + 3];
                let [a, b, c] =
                    [0, 1, 2].map(|k| Vec3::from(self.mesh.vertices[idx[k] as usize].position));
                let centroid = (a + b + c) / 3.0;
                let facing = (b - a).cross(c - a).dot(eye - centroid) > 0.0;
                facing && to_screen(view_proj, rect, centroid).is_some_and(|p| sel.contains(p))
            })
            .collect();
        if tris.is_empty() {
            return;
        }
        let erase = ui.input(|i| i.modifiers.ctrl);
        self.polygon_fill(&tris, erase);
    }

    /// Fills (or erases) triangles on the selected layer or mask, as one undo step.
    pub fn polygon_fill(&mut self, tris: &[u32], erase: bool) {
        let mode = if erase {
            StrokeMode::Erase
        } else {
            StrokeMode::Paint
        };
        if !self.begin_stroke(true, mode) {
            return;
        }
        // One fill per UDIM tile the triangles are in.
        let mut by_tile: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
        for &t in tris {
            by_tile
                .entry(self.mesh.triangle_udim(t as usize))
                .or_default()
                .extend_from_slice(&self.mesh.indices[t as usize * 3..t as usize * 3 + 3]);
        }
        for (tile, indices) in by_tile {
            self.viewport.paint_fill(
                &self.device,
                &self.queue,
                self.selected_part,
                tile,
                &indices,
                &self.resources,
            );
        }
        self.end_stroke();
    }

    /// Path tool (3D view): click to add points; Enter paints along them,
    /// Backspace removes the last, Esc clears.
    pub fn path_input(&mut self, ui: &Ui, response: &egui::Response, rect: Rect) {
        if self.tool != Tool::Path {
            return;
        }
        let view_proj = self.view_proj(rect.width() / rect.height());
        if response.clicked()
            && !ui.input(|i| i.modifiers.alt)
            && let Some(p) = response.interact_pointer_pos()
        {
            let ndc = Vec2::new(
                (p.x - rect.min.x) / rect.width() * 2.0 - 1.0,
                1.0 - (p.y - rect.min.y) / rect.height() * 2.0,
            );
            let (origin, dir) = view_ray(view_proj, ndc);
            match raycast(&self.mesh, origin, dir) {
                Some(hit) if hit.part == self.selected_part => self.path_points.push(hit.pos),
                Some(_) => self.warn("Path points must be on the selected texture set".into()),
                None => {}
            }
        }
        if ui.memory(|m| m.focused().is_none()) {
            let (enter, back, esc) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Backspace),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if back {
                self.path_points.pop();
            }
            if esc {
                self.path_points.clear();
            }
            if enter {
                self.commit_path(rect);
            }
        }
        // Draw the path.
        let screen: Vec<egui::Pos2> = self
            .path_points
            .iter()
            .filter_map(|p| to_screen(view_proj, rect, *p))
            .collect();
        let painter = ui.painter_at(rect);
        let line = Stroke::new(2.0, Color32::from_rgb(90, 170, 255));
        for w in screen.windows(2) {
            painter.line_segment([w[0], w[1]], line);
        }
        for p in &screen {
            painter.circle_filled(*p, 4.0, Color32::from_rgb(90, 170, 255));
        }
    }

    /// Paints (or erases) along the path as seen from the current view.
    pub fn commit_path(&mut self, rect: Rect) {
        if self.path_points.len() < 2 {
            self.warn("Click at least two points on the model".into());
            return;
        }
        let view_proj = self.view_proj(rect.width() / rect.height());
        let pts: Option<Vec<Vec2>> = self
            .path_points
            .iter()
            .map(|p| to_screen(view_proj, rect, *p).map(|s| Vec2::new(s.x, s.y)))
            .collect();
        let Some(pts) = pts else {
            self.warn("Part of the path is behind the camera".into());
            return;
        };
        if self.path_mode == PathMode::Fill {
            self.fill_lasso(&pts, rect, false);
            self.path_points.clear();
            return;
        }
        let mode = match self.path_mode {
            PathMode::Erase => StrokeMode::Erase,
            PathMode::Smudge => StrokeMode::Smudge,
            _ => StrokeMode::Paint,
        };
        self.pressure = 1.0;
        if !self.begin_stroke(false, mode) {
            return;
        }
        let curve = catmull_rom(&pts, 16);
        if self.path_mode == PathMode::Ribbon {
            self.stamp_ribbon(&curve, rect);
        } else {
            let step = self.brush.step(1.0);
            let mut path = StrokePath::default();
            for p in curve {
                let stamps = path.advance(p, step, 0.0);
                self.stamp(&stamps, rect);
            }
        }
        self.end_stroke();
        self.path_points.clear();
    }

    /// Ribbon: dense dabs along `curve` (screen points), each turned along the path,
    /// carrying the distance travelled so the brush alpha runs along it.
    fn stamp_ribbon(&mut self, curve: &[Vec2], rect: Rect) {
        let radius = self.brush.size.max(1.0);
        let step = (radius * 0.2).max(1.0);
        let mut path = StrokePath::default();
        let mut stamps = Vec::new();
        for &p in curve {
            stamps.extend(path.advance(p, step, 0.0));
        }
        let mut travelled = 0.0;
        for (i, &p) in stamps.iter().enumerate() {
            let next = stamps.get(i + 1).copied().unwrap_or(p);
            let prev = if i > 0 { stamps[i - 1] } else { p };
            let dir = (next - prev).normalize_or(Vec2::X);
            if i > 0 {
                travelled += p.distance(prev) / radius;
            }
            self.stamp_with(&[p], rect, Some((dir, travelled)));
        }
    }

    /// Fills (or erases) every front-facing triangle of the selected texture set
    /// whose center falls inside the screen polygon `pts`.
    pub fn fill_lasso(&mut self, pts: &[Vec2], rect: Rect, erase: bool) {
        let view_proj = self.view_proj(rect.width() / rect.height());
        let eye = self.camera.eye();
        let part = &self.mesh.parts[self.selected_part.min(self.mesh.parts.len() - 1)];
        let inside = |q: Vec2| {
            // Even-odd rule.
            let mut c = false;
            for k in 0..pts.len() {
                let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
                if (a.y > q.y) != (b.y > q.y) && q.x < a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x)
                {
                    c = !c;
                }
            }
            c
        };
        let tris: Vec<u32> = (part.indices.start / 3..part.indices.end / 3)
            .filter(|&t| {
                let idx = &self.mesh.indices[t as usize * 3..t as usize * 3 + 3];
                let [a, b, c] =
                    [0, 1, 2].map(|k| Vec3::from(self.mesh.vertices[idx[k] as usize].position));
                let centroid = (a + b + c) / 3.0;
                let facing = (b - a).cross(c - a).dot(eye - centroid) > 0.0;
                facing
                    && to_screen(view_proj, rect, centroid)
                        .is_some_and(|s| inside(Vec2::new(s.x, s.y)))
            })
            .collect();
        if tris.is_empty() {
            self.warn("The filled path covers no faces of this texture set".into());
            return;
        }
        self.polygon_fill(&tris, erase);
    }

    /// The selected layer's (or mask's) projection, when it's a warp projection.
    fn warp_projection(&mut self) -> Option<&mut reetched_doc::Projection> {
        let sel = self.selected?;
        let node = self
            .stack_mut()
            .and_then(|s| reetched_doc::find_node_mut(s, sel.node))?;
        let p = if sel.mask {
            &mut node.mask.as_mut()?.projection
        } else {
            match &mut node.kind {
                reetched_doc::NodeKind::Fill(f) => &mut f.projection,
                _ => return None,
            }
        };
        (p.mode == reetched_doc::ProjectionMode::Warp).then_some(p)
    }

    /// Warp projection lattice in the 2D view: drawn over the 0..1 tile, its
    /// points dragged with the mouse. Returns true while it has the pointer.
    pub fn warp_input(&mut self, ui: &Ui, response: &egui::Response, rect: Rect) -> bool {
        let cam_center = self.uv_camera.center;
        let zoom = self.uv_camera.zoom;
        let drag = self.warp_drag;
        let Some(p) = self.warp_projection() else {
            return false;
        };
        // Lattice space: t = R(rot) (uv * scale) + offset (see uv_xform in compositor.wgsl).
        let (s, c) = p.rotation_deg.to_radians().sin_cos();
        let scale = Vec2::from(p.scale).max(Vec2::splat(1e-4));
        let offset = Vec2::from(p.offset);
        let to_uv = |l: Vec2| {
            let d = l - offset;
            Vec2::new(c * d.x + s * d.y, -s * d.x + c * d.y) / scale
        };
        let to_lattice = |uv: Vec2| {
            let q = uv * scale;
            Vec2::new(c * q.x - s * q.y, s * q.x + c * q.y) + offset
        };
        let screen = |uv: Vec2| {
            let d = (uv - cam_center) * zoom;
            rect.center() + egui::vec2(d.x, -d.y)
        };
        let from_screen = |q: egui::Pos2| {
            cam_center + Vec2::new(q.x - rect.center().x, rect.center().y - q.y) / zoom
        };
        let points: Vec<egui::Pos2> = p
            .warp
            .iter()
            .map(|w| screen(to_uv(Vec2::from(*w))))
            .collect();
        let painter = ui.painter_at(rect);
        let line = Stroke::new(1.0, Color32::from_rgb(255, 170, 60));
        for j in 0..4 {
            for i in 0..4 {
                let k = j * 4 + i;
                if i < 3 {
                    painter.line_segment([points[k], points[k + 1]], line);
                }
                if j < 3 {
                    painter.line_segment([points[k], points[k + 4]], line);
                }
            }
        }
        for (k, q) in points.iter().enumerate() {
            let r = if drag == Some(k) { 6.0 } else { 4.5 };
            painter.circle_filled(*q, r, Color32::from_rgb(255, 170, 60));
        }
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let mut grabbed = drag;
        if response.drag_started_by(egui::PointerButton::Primary)
            && !ui.input(|i| i.modifiers.alt)
            && let Some(q) = pointer
        {
            grabbed = points
                .iter()
                .enumerate()
                .map(|(k, h)| (k, h.distance(q)))
                .filter(|(_, d)| *d < 10.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(k, _)| k);
        }
        if let (Some(k), Some(q)) = (grabbed, pointer)
            && response.dragged_by(egui::PointerButton::Primary)
        {
            p.warp[k] = to_lattice(from_screen(q)).into();
        }
        if response.drag_stopped() {
            grabbed = None;
        }
        self.warp_drag = grabbed;
        grabbed.is_some()
    }

    /// Tool-specific options at the top of the brush panel.
    pub fn tool_options_ui(&mut self, ui: &mut Ui) {
        let hint = |ui: &mut Ui, text: &str| {
            ui.label(RichText::new(text).color(theme::WEAK));
        };
        match self.tool {
            Tool::PolygonFill => {
                theme::grid(ui, "polygon_mode", |ui| {
                    ui.label("Select");
                    ui.horizontal_wrapped(|ui| {
                        for m in Selection::ALL {
                            ui.selectable_value(&mut self.polygon_mode, m, m.label());
                        }
                    });
                    ui.end_row();
                });
                hint(
                    ui,
                    "Click to fill with the brush's material, drag a rectangle to fill many, Ctrl to erase.",
                );
            }
            Tool::Smudge => hint(
                ui,
                "Drags paint along with the brush. Stroke opacity sets the strength.",
            ),
            Tool::Projection => {
                ui.horizontal(|ui| {
                    if ui.button("Load image…").clicked() {
                        let ctx = ui.ctx().clone();
                        self.load_stencil(&ctx);
                    }
                    if ui.button("Text…").clicked() {
                        self.text_ui = Some(crate::text_ui::TextUi::new(
                            crate::text_ui::TextTarget::Stencil,
                        ));
                    }
                    if ui.button("Center").clicked() {
                        self.stencil.center = Vec2::ZERO;
                        self.stencil.rotation_deg = 0.0;
                    }
                });
                if self.stencil.image.is_some() {
                    theme::grid(ui, "stencil", |ui| {
                        ui.label("Size");
                        ui.add(
                            egui::Slider::new(&mut self.stencil.size, 0.02..=4.0).logarithmic(true),
                        );
                        ui.end_row();
                        ui.label("Rotation");
                        ui.add(
                            egui::Slider::new(&mut self.stencil.rotation_deg, -180.0..=180.0)
                                .suffix("°"),
                        );
                        ui.end_row();
                        ui.label("Overlay");
                        ui.add(egui::Slider::new(&mut self.stencil.preview, 0.0..=1.0));
                        ui.end_row();
                    });
                }
                hint(
                    ui,
                    "Paint to project the image onto the model (into base color, or a mask). Ctrl+drag moves the image.",
                );
            }
            Tool::Clone => {
                hint(
                    ui,
                    "Ctrl+click to choose the source, then paint to copy from it.",
                );
                if self.clone_source.is_some() && ui.small_button("Forget source").clicked() {
                    self.clone_source = None;
                    self.clone_offset = None;
                }
            }
            Tool::Path => {
                ui.horizontal_wrapped(|ui| {
                    for m in PathMode::ALL {
                        ui.selectable_value(&mut self.path_mode, m, m.label());
                    }
                });
                hint(
                    ui,
                    &format!(
                        "Click points on the model ({} so far). Enter paints along them, Backspace removes the last, Esc clears.",
                        self.path_points.len()
                    ),
                );
                if ui.button("Paint path").clicked() {
                    let rect = self.view3d_rect;
                    self.commit_path(rect);
                }
            }
            _ => {}
        }
    }
}

fn to_screen(view_proj: glam::Mat4, rect: Rect, p: Vec3) -> Option<egui::Pos2> {
    let clip = view_proj * p.extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(egui::pos2(
        rect.min.x + (ndc.x + 1.0) * 0.5 * rect.width(),
        rect.min.y + (1.0 - ndc.y) * 0.5 * rect.height(),
    ))
}

/// A smooth curve through `pts`, `n` samples per segment.
pub fn catmull_rom(pts: &[Vec2], n: usize) -> Vec<Vec2> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let at = |i: isize| pts[i.clamp(0, pts.len() as isize - 1) as usize];
    let mut out = Vec::new();
    for i in 0..pts.len() as isize - 1 {
        let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        for k in 0..n {
            let t = k as f32 / n as f32;
            let (t2, t3) = (t * t, t * t * t);
            out.push(
                0.5 * (2.0 * p1
                    + (p2 - p0) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3),
            );
        }
    }
    out.push(*pts.last().unwrap());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catmull_rom_passes_through_its_points() {
        let pts = [Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)];
        let curve = catmull_rom(&pts, 4);
        assert_eq!(curve.len(), 9);
        for p in pts {
            assert!(curve.iter().any(|c| c.distance(p) < 1e-4), "{p}");
        }
    }
}
