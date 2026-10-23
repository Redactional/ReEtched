//! Contents of each dockable panel.

use crate::{
    app::{PresetDrag, State, icon_button},
    preview::PRESETS,
    theme,
    viewport::ViewMode,
};
use egui::{
    Align, Color32, CornerRadius, Frame, Layout, PointerButton, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, pos2, vec2,
};
use egui_material_icons::icons as ic;
use glam::Vec2;

impl State {
    pub fn assets_ui(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("›  All libraries").color(theme::WEAK));
        ui.add(
            egui::TextEdit::singleline(&mut self.asset_search)
                .hint_text(format!("{}  Search", ic::ICON_SEARCH.codepoint))
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            icon_button(ui, ic::ICON_DEPLOYED_CODE, true, "Materials");
            ui.add_enabled_ui(false, |ui| {
                for (icon, name) in [
                    (ic::ICON_AUTO_FIX_HIGH, "Smart materials"),
                    (ic::ICON_MASKED_TRANSITIONS, "Smart masks"),
                    (ic::ICON_FILTER_ALT, "Filters"),
                    (ic::ICON_BRUSH, "Brushes"),
                    (ic::ICON_BLUR_ON, "Alphas"),
                    (ic::ICON_TEXTURE, "Textures"),
                    (ic::ICON_LIGHT_MODE, "Environments"),
                ] {
                    icon_button(ui, icon, false, name)
                        .on_disabled_hover_text(format!("{name} — arrive in a later phase"));
                }
            });
        });
        ui.separator();

        let search = self.asset_search.to_lowercase();
        let mut apply = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
                for (i, preset) in PRESETS.iter().enumerate() {
                    if !preset.name.to_lowercase().contains(&search) {
                        continue;
                    }
                    let (rect, resp) = ui.allocate_exact_size(vec2(70.0, 86.0), Sense::click_and_drag());
                    let selected = self.selected_preset == Some(i);
                    if selected || resp.hovered() {
                        let fill = if selected { theme::ROW_SELECTED } else { Color32::from_gray(0x36) };
                        ui.painter().rect_filled(rect, 2.0, fill);
                    }
                    if selected {
                        ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
                    }
                    let thumb = Rect::from_center_size(rect.center_top() + vec2(0.0, 34.0), vec2(62.0, 62.0));
                    if let Some(tex) = self.thumbs.get(i) {
                        ui.painter().image(tex.id(), thumb, unit_uv(), Color32::WHITE);
                    }
                    let name = truncate(preset.name, 11);
                    ui.painter().text(
                        rect.center_bottom() - vec2(0.0, 8.0),
                        egui::Align2::CENTER_CENTER,
                        name,
                        egui::FontId::proportional(10.5),
                        theme::TEXT,
                    );
                    let resp = resp.on_hover_text(format!(
                        "{}\nDouble-click or drag onto the model, a texture set or the layer stack to apply.",
                        preset.name
                    ));
                    if resp.clicked() {
                        self.selected_preset = Some(i);
                    }
                    if resp.double_clicked() {
                        apply = Some(i);
                    }
                    if resp.drag_started() {
                        resp.dnd_set_drag_payload(PresetDrag(i));
                    }
                    if resp.dragged()
                        && let (Some(tex), Some(pos)) = (self.thumbs.get(i), ui.ctx().pointer_interact_pos())
                    {
                        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("drag_preview"));
                        ui.ctx().layer_painter(layer).image(
                            tex.id(),
                            Rect::from_center_size(pos, vec2(48.0, 48.0)),
                            unit_uv(),
                            Color32::from_white_alpha(220),
                        );
                    }
                }
            });
        });
        if let Some(i) = apply {
            self.apply_preset(i);
        }
    }

    /// Applies a preset dropped on `response`, if any, to texture set `part`.
    fn accept_preset_drop(&mut self, response: &egui::Response, part: Option<usize>) {
        if let Some(drag) = response.dnd_release_payload::<PresetDrag>() {
            if let Some(part) = part {
                self.selected_part = part;
            }
            self.apply_preset(drag.0);
        }
    }

    pub fn texture_sets_ui(&mut self, ui: &mut Ui) {
        let mut select = None;
        let mut drops = Vec::new();
        for (i, part) in self.viewport.parts.iter_mut().enumerate() {
            let selected = self.selected_part == i;
            let row = row_frame(selected).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let eye = if part.visible {
                        ic::ICON_VISIBILITY
                    } else {
                        ic::ICON_VISIBILITY_OFF
                    };
                    if icon_button(ui, eye, false, "Show / hide in the 3D view").clicked() {
                        part.visible = !part.visible;
                    }
                    let name = ui.add(
                        egui::Label::new(RichText::new(&part.name).strong())
                            .sense(Sense::click())
                            .selectable(false),
                    );
                    if name.clicked() {
                        select = Some(i);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("Main shader").small().color(theme::WEAK));
                    });
                });
            });
            // The frame only senses hover, so the eye button keeps its clicks; drops still land.
            drops.push((row.response, i));
        }
        if let Some(i) = select
            && i != self.selected_part
        {
            self.selected_part = i;
            self.select_top_layer();
        }
        for (resp, i) in drops {
            self.accept_preset_drop(&resp, Some(i));
        }
        if self.viewport.parts.is_empty() {
            ui.label(RichText::new("No mesh loaded").color(theme::WEAK));
        }
    }

    pub fn display_settings_ui(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(theme::header("ENVIRONMENT"));
            if ui.button("Load environment map…").clicked() {
                self.pick_environment();
            }
            egui::Grid::new("environment")
                .num_columns(2)
                .spacing(vec2(12.0, 6.0))
                .show(ui, |ui| {
                    ui.label("Opacity");
                    ui.add(egui::Slider::new(&mut self.env_opacity, 0.0..=1.0));
                    ui.end_row();
                    ui.label("Exposure (EV)");
                    ui.add(egui::Slider::new(&mut self.exposure_ev, -6.0..=6.0));
                    ui.end_row();
                    ui.label("Rotation");
                    ui.add(
                        egui::Slider::new(&mut self.env_rotation_deg, -180.0..=180.0).suffix("°"),
                    );
                    ui.end_row();
                    ui.label("Blur");
                    ui.add(egui::Slider::new(&mut self.background_blur, 0.0..=1.0));
                    ui.end_row();
                });
            ui.add_space(8.0);
            ui.label(theme::header("CAMERA"));
            egui::Grid::new("camera")
                .num_columns(2)
                .spacing(vec2(12.0, 6.0))
                .show(ui, |ui| {
                    ui.label("Projection");
                    camera_combo(ui, &mut self.camera.orthographic, "cam_settings");
                    ui.end_row();
                    ui.label("Field of view");
                    ui.add(egui::Slider::new(&mut self.camera.fov_y_deg, 10.0..=90.0).suffix("°"));
                    ui.end_row();
                });
            ui.add_space(8.0);
            ui.label(theme::header("VIEWER"));
            ui.add_enabled(
                self.viewport.wireframe_supported,
                egui::Checkbox::new(&mut self.show_wireframe, "Wireframe (3D view)"),
            )
            .on_disabled_hover_text("This GPU doesn't support line rasterization");
            ui.add_enabled(
                self.viewport.wireframe_supported,
                egui::Checkbox::new(&mut self.show_uv_wireframe, "UV wireframe (2D view)"),
            );
        });
    }

    pub fn log_ui(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in &self.log {
                    let color = if line.error {
                        theme::ERROR
                    } else {
                        theme::TEXT
                    };
                    ui.label(
                        RichText::new(format!("{}  {}", line.time, line.text))
                            .monospace()
                            .color(color),
                    );
                }
            });
    }

    pub fn view3d_ui(&mut self, ui: &mut Ui) {
        // Only the visible part of the pane; max_rect can extend past it.
        let rect = ui.max_rect().intersect(ui.clip_rect());
        self.view3d_rect = rect;
        let ppp = ui.pixels_per_point();
        self.viewport.view3d.requested_size = [
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        ];
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        ui.painter().image(
            self.viewport.view3d.texture_id,
            rect,
            unit_uv(),
            Color32::WHITE,
        );
        self.accept_preset_drop(&response, None);
        drop_highlight(ui, &response);
        self.paint_input(ui, &response, rect, false);

        // Painter's navigation: Alt+LMB orbit, Alt+MMB pan, Alt+RMB zoom, wheel zoom, F to frame.
        let (alt, scroll, frame_key) = ui.input(|i| {
            (
                i.modifiers.alt,
                i.smooth_scroll_delta.y,
                i.key_pressed(egui::Key::F),
            )
        });
        let d = response.drag_delta();
        let cam = &mut self.camera;
        if alt && response.dragged_by(PointerButton::Primary) {
            cam.yaw -= d.x * 0.008;
            cam.pitch = (cam.pitch + d.y * 0.008).clamp(-1.55, 1.55);
        }
        if response.dragged_by(PointerButton::Middle) {
            let (right, up) = cam.right_up();
            let scale = cam.distance * cam.fov_y_deg.to_radians() / rect.height().max(1.0);
            cam.target += (-right * d.x + up * d.y) * scale;
        }
        if alt && response.dragged_by(PointerButton::Secondary) {
            cam.distance *= (d.y * 0.01).exp();
        }
        if response.hovered() && scroll != 0.0 {
            cam.distance *= (-scroll * 0.0015).exp();
        }
        if response.hovered() && frame_key {
            cam.frame(self.bounds);
        }

        overlay(ui, rect, |ui| {
            camera_combo(ui, &mut self.camera.orthographic, "cam_overlay");
            view_mode_combo(ui, &mut self.view_mode, "mode3d");
        });
    }

    pub fn view2d_ui(&mut self, ui: &mut Ui) {
        // Only the visible part of the pane; max_rect can extend past it.
        let rect = ui.max_rect().intersect(ui.clip_rect());
        let ppp = ui.pixels_per_point();
        self.viewport.view2d.requested_size = [
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        ];
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        self.uv_camera.update(rect.size());
        ui.painter().image(
            self.viewport.view2d.texture_id,
            rect,
            unit_uv(),
            Color32::WHITE,
        );
        self.accept_preset_drop(&response, None);
        drop_highlight(ui, &response);
        self.paint_input(ui, &response, rect, true);

        // UDIM 1001 outline.
        let painter = ui.painter_at(rect);
        let lo = self.uv_camera.to_screen(rect, Vec2::ZERO);
        let hi = self.uv_camera.to_screen(rect, Vec2::ONE);
        painter.rect_stroke(
            Rect::from_two_pos(lo, hi),
            0.0,
            Stroke::new(1.0, Color32::from_gray(0x60)),
            StrokeKind::Middle,
        );
        painter.text(
            Rect::from_two_pos(lo, hi).left_top() + vec2(4.0, 3.0),
            egui::Align2::LEFT_TOP,
            "1001",
            egui::FontId::proportional(10.5),
            theme::WEAK,
        );

        // Pan: MMB (or Alt+MMB); zoom: wheel or Alt+RMB; F frames the tile.
        let (alt, scroll, frame_key) = ui.input(|i| {
            (
                i.modifiers.alt,
                i.smooth_scroll_delta.y,
                i.key_pressed(egui::Key::F),
            )
        });
        let d = response.drag_delta();
        let cam = &mut self.uv_camera;
        if response.dragged_by(PointerButton::Middle)
            || (alt && response.dragged_by(PointerButton::Primary))
        {
            cam.center -= Vec2::new(d.x, -d.y) / cam.zoom;
        }
        if alt && response.dragged_by(PointerButton::Secondary) {
            cam.zoom *= (-d.y * 0.01).exp();
        }
        if response.hovered() && scroll != 0.0 {
            cam.zoom *= (scroll * 0.0015).exp();
        }
        if response.hovered() && frame_key {
            cam.zoom = 0.0;
        }

        overlay(ui, rect, |ui| {
            view_mode_combo(ui, &mut self.uv_mode, "mode2d")
        });
    }
}

/// Selectable row background with Painter's blue outline when selected.
fn row_frame(selected: bool) -> Frame {
    Frame::new()
        .inner_margin(vec2(4.0, 2.0))
        .corner_radius(CornerRadius::same(2))
        .fill(if selected {
            theme::ROW_SELECTED
        } else {
            Color32::TRANSPARENT
        })
        .stroke(if selected {
            Stroke::new(1.0, theme::ACCENT)
        } else {
            Stroke::NONE
        })
}

/// Toolbar floating over the top-left of a view.
fn overlay(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui)) {
    let bar = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(rect.width() - 16.0, 24.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bar)
            .layout(Layout::left_to_right(Align::Center)),
    );
    add(&mut child);
}

fn camera_combo(ui: &mut Ui, orthographic: &mut bool, id: &str) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(if *orthographic {
            "Orthographic"
        } else {
            "Perspective"
        })
        .width(120.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(orthographic, false, "Perspective");
            ui.selectable_value(orthographic, true, "Orthographic");
        });
}

fn view_mode_combo(ui: &mut Ui, mode: &mut ViewMode, id: &str) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(mode.label())
        .width(130.0)
        .show_ui(ui, |ui| {
            for m in ViewMode::ALL {
                ui.selectable_value(mode, m, m.label());
            }
        });
}

/// Outlines a view while a preset is dragged over it.
fn drop_highlight(ui: &Ui, response: &egui::Response) {
    if response.dnd_hover_payload::<PresetDrag>().is_some() {
        ui.painter().rect_stroke(
            response.rect.shrink(1.0),
            0.0,
            Stroke::new(2.0, theme::ACCENT),
            StrokeKind::Inside,
        );
    }
}

fn unit_uv() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        s.chars().take(max - 1).chain(['…']).collect()
    }
}
