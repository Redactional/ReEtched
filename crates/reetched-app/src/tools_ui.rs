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

impl State {
    /// Mesh adjacency, built the first time a tool needs it.
    pub fn topology(&mut self) -> Arc<Topology> {
        self.topology
            .get_or_insert_with(|| Arc::new(Topology::new(&self.mesh)))
            .clone()
    }

    /// Polygon Fill: click fills the selection with the brush, Ctrl+click erases it.
    pub fn polygon_input(&mut self, ui: &Ui, response: &egui::Response, rect: Rect, uv_view: bool) {
        if self.tool != Tool::PolygonFill || !response.clicked() || ui.input(|i| i.modifiers.alt) {
            return;
        }
        let Some(p) = response.interact_pointer_pos() else {
            return;
        };
        let erase = ui.input(|i| i.modifiers.ctrl);
        let topo = self.topology();
        let hit = if uv_view {
            self.uv_under(p, rect, true)
                .and_then(|(uv, part)| Some((topo.triangle_at_uv(&self.mesh, part, uv)?, part)))
        } else {
            let view_proj = self.view_proj(rect.width() / rect.height());
            let ndc = Vec2::new(
                (p.x - rect.min.x) / rect.width() * 2.0 - 1.0,
                1.0 - (p.y - rect.min.y) / rect.height() * 2.0,
            );
            let (origin, dir) = view_ray(view_proj, ndc);
            raycast(&self.mesh, origin, dir).map(|h| (h.tri, h.part))
        };
        let Some((tri, part)) = hit else { return };
        if part != self.selected_part {
            self.warn("That's another texture set — select it first".into());
            return;
        }
        self.polygon_fill(&topo.select(tri, self.polygon_mode), erase);
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
        let indices: Vec<u32> = tris
            .iter()
            .flat_map(|&t| self.mesh.indices[t as usize * 3..t as usize * 3 + 3].to_vec())
            .collect();
        self.viewport
            .paint_fill(&self.device, &self.queue, self.selected_part, &indices);
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
        let mode = if self.path_erase {
            StrokeMode::Erase
        } else {
            StrokeMode::Paint
        };
        self.pressure = 1.0;
        if !self.begin_stroke(false, mode) {
            return;
        }
        let step = self.brush.step(1.0);
        let mut path = StrokePath::default();
        for p in catmull_rom(&pts, 16) {
            let stamps = path.advance(p, step, 0.0);
            self.stamp(&stamps, rect);
        }
        self.end_stroke();
        self.path_points.clear();
    }

    /// Tool-specific options at the top of the brush panel.
    pub fn tool_options_ui(&mut self, ui: &mut Ui) {
        let hint = |ui: &mut Ui, text: &str| {
            ui.label(RichText::new(text).color(theme::WEAK));
        };
        match self.tool {
            Tool::PolygonFill => {
                ui.horizontal(|ui| {
                    ui.label("Select");
                    for m in Selection::ALL {
                        ui.selectable_value(&mut self.polygon_mode, m, m.label());
                    }
                });
                hint(
                    ui,
                    "Click to fill with the brush's material, Ctrl+click to erase.",
                );
            }
            Tool::Smudge => hint(
                ui,
                "Drags paint along with the brush. Stroke opacity sets the strength.",
            ),
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
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.path_erase, false, "Paint");
                    ui.selectable_value(&mut self.path_erase, true, "Erase");
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
