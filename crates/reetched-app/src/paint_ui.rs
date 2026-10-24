//! Painting interaction: turning pointer input in the 3D/2D views into strokes,
//! and the brush properties panel.

use crate::{
    app::{Selected, State, Tool, icon_button},
    brush::{Axis, StrokePath, Symmetry, ViewInfo, raycast, uv_dab, view_ray, world_dabs},
    compositor::MASK_SLOT,
    paint_gpu::{BrushShape, Camera, Dab, StrokeMode, StrokeTarget},
    theme,
};
use egui::{Color32, ComboBox, Rect, RichText, Slider, Stroke, Ui, vec2};
use egui_material_icons::icons as ic;
use glam::{Mat4, Vec2};
use reetched_doc::{Channel, NodeId, NodeKind, TileMap, find_node, find_node_mut};

/// A stroke in progress.
pub struct ActiveStroke {
    node: NodeId,
    set: usize,
    /// Painting in the 2D (UV) view rather than the 3D view.
    uv: bool,
    pub path: StrokePath,
    shape: BrushShape,
    /// Smudge: UV under the brush at the end of the last frame.
    last_uv: Option<Vec2>,
}

impl State {
    /// Handles painting input over a view. `uv`: the 2D view.
    pub fn paint_input(&mut self, ui: &Ui, response: &egui::Response, rect: Rect, uv: bool) {
        if !matches!(
            self.tool,
            Tool::Paint | Tool::Eraser | Tool::Smudge | Tool::Clone
        ) {
            return;
        }
        let (alt, shift, ctrl, pressed, down, pos, touch) = ui.input(|i| {
            let touch = i.events.iter().rev().find_map(|e| match e {
                egui::Event::Touch { force, .. } => *force,
                _ => None,
            });
            (
                i.modifiers.alt,
                i.modifiers.shift,
                i.modifiers.ctrl,
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.interact_pos(),
                touch,
            )
        });
        if let Some(force) = touch {
            self.pressure = force.clamp(0.0, 1.0);
        }

        // Brush cursor.
        if let Some(p) = response.hover_pos()
            && !alt
        {
            let r = self.brush.size
                * if self.brush.pressure_size && self.stroke.is_some() {
                    self.pressure
                } else {
                    1.0
                };
            let painter = ui.painter_at(rect);
            painter.circle_stroke(p, r, Stroke::new(2.5, Color32::from_black_alpha(120)));
            painter.circle_stroke(p, r, Stroke::new(1.0, Color32::from_white_alpha(200)));
        }

        // Clone: Ctrl+click picks the source; the first stroke after sets the offset.
        if self.tool == Tool::Clone
            && let Some((src, _)) = self.clone_source
        {
            let painter = ui.painter_at(rect);
            if let Some(p) = self.uv_to_screen(src, rect, uv) {
                painter.circle_stroke(p, 6.0, Stroke::new(2.0, Color32::from_rgb(255, 170, 60)));
            }
        }
        if pressed && response.hovered() && !alt && ctrl && self.tool == Tool::Clone {
            if let Some((src, part)) = pos.and_then(|p| self.uv_under(p, rect, uv)) {
                self.clone_source = Some((src, part));
                self.clone_offset = None;
                self.info("Clone source set — paint to copy from it".into());
            }
            return;
        }

        if pressed && response.hovered() && !alt && self.stroke.is_none() {
            if touch.is_none() {
                self.pressure = 1.0;
            }
            let mode = match self.tool {
                Tool::Eraser => Some(StrokeMode::Erase),
                Tool::Smudge => Some(StrokeMode::Smudge),
                Tool::Clone => self.clone_mode(pos, rect, uv),
                _ => Some(StrokeMode::Paint),
            };
            if let Some(mode) = mode
                && self.begin_stroke(uv, mode)
            {
                // Shift+click: straight line from where the last stroke ended.
                if shift
                    && let (Some((was_uv, from)), Some(stroke)) =
                        (self.last_stroke_end, &mut self.stroke)
                    && was_uv == uv
                {
                    stroke.path.advance(from, f32::MAX, 0.0);
                }
            }
        }
        if let (Some(stroke), Some(p)) = (&self.stroke, pos)
            && stroke.uv == uv
        {
            if down {
                let step = self.brush.step(self.pressure);
                let lazy = self.brush.lazy;
                let points =
                    self.stroke
                        .as_mut()
                        .unwrap()
                        .path
                        .advance(Vec2::new(p.x, p.y), step, lazy);
                self.stamp(&points, rect);
            } else {
                self.end_stroke();
            }
        }
    }

    /// Clone's stroke mode for a stroke starting at `pos`, or None (with a hint).
    fn clone_mode(&mut self, pos: Option<egui::Pos2>, rect: Rect, uv: bool) -> Option<StrokeMode> {
        let Some((src, _)) = self.clone_source else {
            self.warn("Ctrl+click to choose where to clone from".into());
            return None;
        };
        let (start, _) = self.uv_under(pos?, rect, uv)?;
        let offset = *self.clone_offset.get_or_insert(src - start);
        let res = self.doc.texture_sets.get(self.selected_part)?.resolution as f32;
        // Texel rows run downwards, UV v upwards.
        Some(StrokeMode::Clone {
            offset: [offset.x * res, -offset.y * res],
        })
    }

    /// The UV under a point of a view, and the texture set it's on.
    pub fn uv_under(&self, p: egui::Pos2, rect: Rect, uv: bool) -> Option<(Vec2, usize)> {
        if uv {
            let cam = &self.uv_camera;
            let at =
                cam.center + Vec2::new(p.x - rect.center().x, rect.center().y - p.y) / cam.zoom;
            return Some((at, self.selected_part));
        }
        let view = self.view_info(rect);
        let ndc = Vec2::new(
            (p.x - rect.min.x) / rect.width() * 2.0 - 1.0,
            1.0 - (p.y - rect.min.y) / rect.height() * 2.0,
        );
        let (origin, dir) = view_ray(view.view_proj, ndc);
        raycast(&self.mesh, origin, dir).map(|h| (h.uv, h.part))
    }

    /// Where a UV shows in a view (2D view only; the 3D view would need a search).
    fn uv_to_screen(&self, at: Vec2, rect: Rect, uv: bool) -> Option<egui::Pos2> {
        uv.then(|| self.uv_camera.to_screen(rect, at))
    }

    pub fn begin_stroke(&mut self, uv: bool, mode: StrokeMode) -> bool {
        // Erase, smudge and clone work on the channels the layer has; paint on the brush's.
        let erase = mode != StrokeMode::Paint;
        let Some(sel) = self.selected else {
            self.warn("Select a paint layer or a mask to paint on".into());
            return false;
        };
        let set_index = self.selected_part;
        let Some(set) = self.doc.texture_sets.get(set_index) else {
            return false;
        };
        let res = set.resolution;
        let Some(node) = find_node(&set.stack, sel.node) else {
            return false;
        };
        let mut targets = Vec::new();
        if sel.mask {
            let Some(mask) = &node.mask else { return false };
            targets.push(StrokeTarget {
                key: (node.id, MASK_SLOT),
                value: [self.brush.mask_value; 3],
                is_normal: false,
                old: mask.paint.clone().unwrap_or_else(|| TileMap::new(res)),
            });
        } else {
            let NodeKind::Paint(paint) = &node.kind else {
                self.warn(format!(
                    "“{}” isn't a paint layer — add a paint layer or a mask to paint",
                    node.name
                ));
                return false;
            };
            for (i, c) in Channel::ALL.into_iter().enumerate() {
                let existing = paint.channels.get(&c);
                let brush = &self.brush.channels[&c];
                // The eraser clears every channel the layer has; the brush paints its enabled ones.
                let wanted = if erase {
                    existing.is_some_and(|m| !m.tiles.is_empty())
                } else {
                    brush.enabled
                };
                if !wanted {
                    continue;
                }
                let mut value = brush.value;
                if c == Channel::Normal {
                    value = glam::Vec3::from(value).normalize_or(glam::Vec3::Z).into();
                }
                targets.push(StrokeTarget {
                    key: (node.id, i as u8),
                    value,
                    is_normal: c == Channel::Normal,
                    old: existing.cloned().unwrap_or_else(|| TileMap::new(res)),
                });
            }
        }
        if targets.is_empty() {
            if !erase {
                self.warn("The brush has no channels enabled".into());
            }
            return false;
        }
        let alpha = self.brush.alpha.clone().and_then(|id| {
            self.viewport
                .brush_alpha(&self.device, &self.queue, &self.resources, &id)
        });
        let shape = BrushShape {
            hardness: self.brush.hardness,
            backface_cos: if uv { None } else { self.brush.backface_cos() },
            alpha,
        };
        let node_id = node.id;
        self.viewport.paint_begin(
            &self.device,
            &self.queue,
            targets,
            res,
            &self.resources,
            mode,
            self.brush.opacity,
        );
        self.stroke = Some(ActiveStroke {
            node: node_id,
            set: set_index,
            uv,
            path: StrokePath::default(),
            shape,
            last_uv: None,
        });
        true
    }

    /// Turns stamp positions (screen points) into dabs and paints them.
    pub fn stamp(&mut self, points: &[Vec2], rect: Rect) {
        let Some(stroke) = &self.stroke else { return };
        if points.is_empty() {
            return;
        }
        let mut dabs: Vec<Dab> = Vec::new();
        let mut end_uv = None;
        if stroke.uv {
            let cam = &self.uv_camera;
            for p in points {
                let uv =
                    cam.center + Vec2::new(p.x - rect.center().x, rect.center().y - p.y) / cam.zoom;
                dabs.push(uv_dab(
                    &self.brush,
                    uv,
                    self.brush.size / cam.zoom,
                    self.pressure,
                ));
                end_uv = Some(uv);
            }
        } else {
            let view = self.view_info(rect);
            for p in points {
                let ndc = Vec2::new(
                    (p.x - rect.min.x) / rect.width() * 2.0 - 1.0,
                    1.0 - (p.y - rect.min.y) / rect.height() * 2.0,
                );
                let (origin, dir) = view_ray(view.view_proj, ndc);
                if let Some(hit) = raycast(&self.mesh, origin, dir).filter(|h| h.part == stroke.set)
                {
                    dabs.extend(world_dabs(&self.brush, &view, &hit, self.pressure));
                    end_uv = Some(hit.uv);
                }
            }
        }
        let camera = if stroke.uv {
            Camera {
                view_proj: Mat4::IDENTITY,
                eye: glam::Vec3::ZERO,
            }
        } else {
            Camera {
                view_proj: self.view_proj(rect.width() / rect.height()),
                eye: self.camera.eye(),
            }
        };
        let (set, uv) = (stroke.set, stroke.uv);
        let shape = BrushShape {
            hardness: stroke.shape.hardness,
            backface_cos: stroke.shape.backface_cos,
            alpha: stroke.shape.alpha.clone(),
        };
        // Smudge drags paint by how far the brush moved in texels this frame.
        let res = self
            .doc
            .texture_sets
            .get(set)
            .map_or(0.0, |s| s.resolution as f32);
        let stroke = self.stroke.as_mut().expect("checked above");
        let motion = match (stroke.last_uv, end_uv) {
            (Some(a), Some(b)) => [(b.x - a.x) * res, -(b.y - a.y) * res],
            _ => [0.0; 2],
        };
        if end_uv.is_some() {
            stroke.last_uv = end_uv;
        }
        self.viewport.paint_dabs(
            &self.device,
            &self.queue,
            set,
            &camera,
            !uv,
            &shape,
            &dabs,
            motion,
        );
    }

    pub fn end_stroke(&mut self) {
        let Some(stroke) = self.stroke.take() else {
            return;
        };
        self.last_stroke_end = stroke.path.last().map(|p| (stroke.uv, p));
        let results = match self
            .viewport
            .paint_end(&self.device, &self.queue, &mut self.resources)
        {
            Ok(r) => r,
            Err(e) => {
                self.error(format!("Could not finish the stroke: {e:#}"));
                return;
            }
        };
        let Some(node) = self
            .doc
            .texture_sets
            .get_mut(stroke.set)
            .and_then(|s| find_node_mut(&mut s.stack, stroke.node))
        else {
            return;
        };
        for ((_, slot), map) in results {
            if slot == MASK_SLOT {
                if let Some(mask) = &mut node.mask {
                    mask.paint = Some(map);
                }
            } else if let NodeKind::Paint(paint) = &mut node.kind {
                paint.channels.insert(Channel::ALL[slot as usize], map);
            }
        }
    }

    /// Samples the ID map under `pos` into the pending color-selection effect.
    pub fn pick_id_color(&mut self, pos: Option<egui::Pos2>, rect: Rect) {
        let Some((node_id, fx)) = self.id_pick.take() else {
            return;
        };
        let Some(p) = pos else { return };
        let view = self.view_info(rect);
        let ndc = Vec2::new(
            (p.x - rect.min.x) / rect.width() * 2.0 - 1.0,
            1.0 - (p.y - rect.min.y) / rect.height() * 2.0,
        );
        let (origin, dir) = view_ray(view.view_proj, ndc);
        let Some(hit) = raycast(&self.mesh, origin, dir) else {
            self.warn("Missed the model — pick again".into());
            return;
        };
        let set = &self.doc.texture_sets[hit.part];
        let Some(img) = set
            .mesh_maps
            .get(&reetched_doc::MeshMap::Id)
            .and_then(|id| self.resources.files.get(id))
            .and_then(|b| image::load_from_memory(b).ok())
            .map(|i| i.to_rgb32f())
        else {
            self.warn("This texture set has no ID map — bake one first".into());
            return;
        };
        let (w, h) = img.dimensions();
        let x = ((hit.uv.x.fract() + 1.0).fract() * w as f32) as u32;
        let y = ((1.0 - (hit.uv.y.fract() + 1.0).fract()) * h as f32) as u32;
        let color = img.get_pixel(x.min(w - 1), y.min(h - 1)).0;
        if let Some(mask) = self
            .doc
            .texture_sets
            .get_mut(hit.part)
            .and_then(|s| find_node_mut(&mut s.stack, node_id))
            .and_then(|n| n.mask.as_mut())
            && let Some(reetched_doc::MaskEffectKind::ColorSelection { colors, .. }) =
                mask.effects.get_mut(fx).map(|e| &mut e.kind)
        {
            colors.push(color);
            self.info("Picked ID color".into());
        }
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let radius = (self.bounds.1 - self.bounds.0).length() * 0.5;
        self.camera.view_proj(aspect, radius)
    }

    fn view_info(&self, rect: Rect) -> ViewInfo {
        let eye = self.camera.eye();
        let (right, up) = self.camera.right_up();
        let half_fov = (self.camera.fov_y_deg.to_radians() * 0.5).tan();
        let world_per_point = if self.camera.orthographic {
            2.0 * self.camera.distance * half_fov / rect.height()
        } else {
            2.0 * half_fov / rect.height()
        };
        ViewInfo {
            view_proj: self.view_proj(rect.width() / rect.height()),
            eye,
            forward: (self.camera.target - eye).normalize(),
            right,
            up,
            orthographic: self.camera.orthographic,
            world_per_point,
            center: (self.bounds.0 + self.bounds.1) * 0.5,
        }
    }

    /// Brush settings, shown in Properties while the paint or eraser tool is active.
    pub fn brush_ui(&mut self, ui: &mut Ui) {
        let target = self.paint_target_label();
        let names = self.doc.resources.clone();
        let mut pick_alpha = false;
        ui.label(theme::header(match self.tool {
            Tool::Eraser => "ERASER",
            Tool::PolygonFill => "POLYGON FILL",
            Tool::Smudge => "SMUDGE",
            Tool::Clone => "CLONE",
            Tool::Path => "PATH",
            _ => "PAINT",
        }));
        ui.label(RichText::new(target).color(theme::WEAK));
        self.tool_options_ui(ui);
        ui.add_space(4.0);
        let b = &mut self.brush;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Grid::new("brush")
                    .num_columns(2)
                    .spacing(vec2(12.0, 6.0))
                    .show(ui, |ui| {
                        ui.label("Size");
                        ui.add(
                            Slider::new(&mut b.size, 1.0..=400.0)
                                .logarithmic(true)
                                .suffix(" pt"),
                        );
                        ui.end_row();
                        ui.label("Flow");
                        ui.add(Slider::new(&mut b.flow, 0.0..=1.0));
                        ui.end_row();
                        ui.label("Stroke opacity");
                        ui.add(Slider::new(&mut b.opacity, 0.0..=1.0));
                        ui.end_row();
                        ui.label("Spacing");
                        ui.add(Slider::new(&mut b.spacing, 0.01..=2.0).logarithmic(true));
                        ui.end_row();
                        ui.label("Hardness");
                        ui.add(Slider::new(&mut b.hardness, 0.0..=1.0));
                        ui.end_row();
                        ui.label("Angle");
                        ui.add(Slider::new(&mut b.angle_deg, -180.0..=180.0).suffix("°"));
                        ui.end_row();
                        ui.label("Lazy mouse");
                        ui.add(Slider::new(&mut b.lazy, 0.0..=200.0).suffix(" pt"));
                        ui.end_row();
                        ui.label("Pen pressure");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut b.pressure_size, "Size");
                            ui.checkbox(&mut b.pressure_flow, "Flow");
                        });
                        ui.end_row();
                        ui.label("Backface culling");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut b.backface_culling, "");
                            ui.add_enabled(
                                b.backface_culling,
                                Slider::new(&mut b.backface_deg, 1.0..=180.0).suffix("°"),
                            );
                        });
                        ui.end_row();
                        ui.label("Alpha");
                        ui.horizontal(|ui| match &b.alpha {
                            Some(id) => {
                                ui.label(
                                    names.get(id).map_or("(missing image)", |r| r.name.as_str()),
                                );
                                if icon_button(ui, ic::ICON_CLOSE, false, "Use a round brush")
                                    .clicked()
                                {
                                    b.alpha = None;
                                }
                            }
                            None => {
                                ui.label(RichText::new("Round").color(theme::WEAK));
                                if icon_button(
                                    ui,
                                    ic::ICON_IMAGE,
                                    false,
                                    "Use a greyscale image as the brush shape…",
                                )
                                .clicked()
                                {
                                    pick_alpha = true;
                                }
                            }
                        });
                        ui.end_row();
                        ui.label("Symmetry");
                        symmetry_ui(ui, &mut b.symmetry);
                        ui.end_row();
                    });

                if matches!(self.tool, Tool::Paint | Tool::PolygonFill | Tool::Path) {
                    ui.add_space(8.0);
                    ui.label(theme::header("MATERIAL"));
                    if self.selected.is_some_and(|s| s.mask) {
                        egui::Grid::new("mask_brush")
                            .num_columns(2)
                            .spacing(vec2(12.0, 6.0))
                            .show(ui, |ui| {
                                ui.label("Mask value");
                                ui.add(Slider::new(&mut b.mask_value, 0.0..=1.0))
                                    .on_hover_text("1 = white (show), 0 = black (hide)");
                                ui.end_row();
                            });
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            for c in Channel::ALL {
                                let ch = b.channels.get_mut(&c).unwrap();
                                ui.toggle_value(&mut ch.enabled, c.label());
                            }
                        });
                        egui::Grid::new("brush_channels")
                            .num_columns(2)
                            .spacing(vec2(12.0, 6.0))
                            .show(ui, |ui| {
                                for c in Channel::ALL {
                                    let ch = b.channels.get_mut(&c).unwrap();
                                    if !ch.enabled {
                                        continue;
                                    }
                                    ui.label(c.label());
                                    crate::stack_ui::channel_value_ui(ui, c, &mut ch.value);
                                    ui.end_row();
                                }
                            });
                    }
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new("[ and ] change the size. Shift+click draws a straight line.")
                        .color(theme::WEAK),
                );
            });
        if pick_alpha
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "tga", "tif", "tiff"])
                .pick_file()
            && let Some(id) = self.import_resource(&path)
        {
            self.brush.alpha = Some(id);
        }
    }

    fn paint_target_label(&self) -> String {
        let Some(sel) = self.selected else {
            return "Select a paint layer or a mask to paint on".into();
        };
        let Some(node) = self.stack().and_then(|s| find_node(s, sel.node)) else {
            return "Select a paint layer or a mask to paint on".into();
        };
        match (sel.mask, &node.kind) {
            (true, _) => format!("Painting on the mask of “{}”", node.name),
            (false, NodeKind::Paint(_)) => format!("Painting on “{}”", node.name),
            _ => format!(
                "“{}” isn't a paint layer — select a paint layer or a mask",
                node.name
            ),
        }
    }

    /// Tool and brush-size shortcuts (ignored while typing).
    pub fn paint_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        use egui::Key;
        let (smaller, bigger) = ctx.input(|i| {
            (
                i.key_pressed(Key::OpenBracket),
                i.key_pressed(Key::CloseBracket),
            )
        });
        for (key, tool) in [
            (Key::Num1, Tool::Paint),
            (Key::Num2, Tool::Eraser),
            (Key::Num4, Tool::PolygonFill),
            (Key::Num5, Tool::Smudge),
            (Key::Num6, Tool::Clone),
            (Key::Num7, Tool::Path),
        ] {
            if ctx.input(|i| i.key_pressed(key)) {
                self.tool = tool;
                self.properties_tool = true;
            }
        }
        if smaller {
            self.brush.size = (self.brush.size / 1.2).max(1.0);
        }
        if bigger {
            self.brush.size = (self.brush.size * 1.2).min(400.0);
        }
    }

    /// Selects the top layer of the current texture set (Painter keeps a layer selected).
    pub fn select_top_layer(&mut self) {
        self.selected = self.stack().and_then(|s| s.first()).map(|n| Selected {
            node: n.id,
            mask: false,
        });
    }
}

fn symmetry_ui(ui: &mut Ui, s: &mut Symmetry) {
    let label = |s: &Symmetry| match s {
        Symmetry::Off => "Off".to_string(),
        Symmetry::Mirror(a) => format!("Mirror {a:?}"),
        Symmetry::Radial(a, n) => format!("Radial {a:?} ×{n}"),
    };
    ui.horizontal(|ui| {
        ComboBox::from_id_salt("symmetry")
            .selected_text(label(s))
            .show_ui(ui, |ui| {
                ui.selectable_value(s, Symmetry::Off, "Off");
                for a in [Axis::X, Axis::Y, Axis::Z] {
                    ui.selectable_value(s, Symmetry::Mirror(a), format!("Mirror {a:?}"));
                }
                for a in [Axis::X, Axis::Y, Axis::Z] {
                    let n = if let Symmetry::Radial(_, n) = s {
                        *n
                    } else {
                        6
                    };
                    ui.selectable_value(s, Symmetry::Radial(a, n), format!("Radial {a:?}"));
                }
            });
        if let Symmetry::Radial(_, n) = s {
            ui.add(egui::DragValue::new(n).range(2..=64).prefix("×"));
        }
    });
}
