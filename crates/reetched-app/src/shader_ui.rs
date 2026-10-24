//! Painter custom shaders (.glsl): loading one onto a texture set, and its
//! parameters in Texture Set Settings.

use crate::{app::State, app::icon_button, theme};
use egui::{ComboBox, DragValue, RichText, Slider, Ui};
use egui_material_icons::icons as ic;
use reetched_doc::{ResourceInfo, ShaderModel};
use reetched_shader::ParamType;

impl State {
    /// Picks a Painter shader file and makes it the selected texture set's shader.
    pub fn load_custom_shader(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Painter shader", &["glsl", "frag"])
            .pick_file()
        else {
            return;
        };
        self.load_custom_shader_file(&path);
    }

    pub fn load_custom_shader_file(&mut self, path: &std::path::Path) {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.error(format!("Could not read {}: {e}", path.display()));
                return;
            }
        };
        let id = self.resources.add(bytes);
        let name = crate::app::file_name(path);
        self.doc
            .resources
            .insert(id.clone(), ResourceInfo { name: name.clone() });
        let stem = path
            .file_stem()
            .map_or(name.clone(), |s| s.to_string_lossy().into_owned());
        if let Some(set) = self.doc.texture_sets.get_mut(self.selected_part) {
            set.shader = ShaderModel::Custom {
                shader: id.clone(),
                name: stem,
                params: Default::default(),
                textures: Default::default(),
            };
        }
        // Compile again even if the same file was loaded before.
        self.viewport.forget_custom_shader(&id);
        self.info(format!("Shader {name} loaded"));
    }

    /// The selected texture set's Painter shader: status, parameters, textures.
    pub fn custom_shader_ui(&mut self, ui: &mut Ui) {
        let Some(set) = self.doc.texture_sets.get(self.selected_part) else {
            return;
        };
        let ShaderModel::Custom { shader, .. } = &set.shader else {
            return;
        };
        let id = shader.clone();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(theme::header("SHADER PARAMETERS"));
            if icon_button(ui, ic::ICON_REFRESH, false, "Compile again").clicked() {
                self.viewport.forget_custom_shader(&id);
            }
        });
        let compiled = match self.viewport.custom_shader(&id) {
            None => {
                ui.label(RichText::new("Compiling…").color(theme::WEAK));
                return;
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(theme::ERROR));
                ui.label(
                    RichText::new("The 3D view uses the PBR shader until this compiles.")
                        .color(theme::WEAK),
                );
                return;
            }
            Some(Ok(c)) => &c.shader,
        };
        let params = compiled.params.clone();
        let textures = compiled.textures.clone();
        let warnings = compiled.warnings.clone();
        let mut pick_texture: Option<String> = None;
        let set = &mut self.doc.texture_sets[self.selected_part];
        let ShaderModel::Custom {
            params: values,
            textures: bound,
            ..
        } = &mut set.shader
        else {
            return;
        };
        let names = self.doc.resources.clone();
        egui::Grid::new("shader_params")
            .num_columns(2)
            .spacing(egui::vec2(12.0, 6.0))
            .show(ui, |ui| {
                for p in params.iter().filter(|p| p.visible) {
                    ui.label(&p.label)
                        .on_hover_text(p.description.as_deref().unwrap_or(&p.name));
                    let v = values
                        .entry(p.name.clone())
                        .or_insert_with(|| p.default.iter().map(|&d| d as f32).collect());
                    v.resize(p.ty.components(), 0.0);
                    param_ui(ui, p, v);
                    ui.end_row();
                }
                for t in &textures {
                    ui.label(&t.label);
                    ui.horizontal(|ui| match bound.get(&t.name) {
                        Some(r) => {
                            ui.label(names.get(r).map_or("(image)", |n| n.name.as_str()));
                            if icon_button(ui, ic::ICON_CLOSE, false, "Remove image").clicked() {
                                bound.remove(&t.name);
                            }
                        }
                        None => {
                            if icon_button(ui, ic::ICON_IMAGE, false, "Use an image…").clicked() {
                                pick_texture = Some(t.name.clone());
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        for w in warnings.iter().take(6) {
            ui.label(RichText::new(w.to_string()).color(theme::WEAK).small());
        }
        if let Some(name) = pick_texture
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", crate::images::IMAGE_EXTENSIONS)
                .pick_file()
            && let Some(rid) = self.import_resource(&path)
            && let Some(set) = self.doc.texture_sets.get_mut(self.selected_part)
            && let ShaderModel::Custom { textures, .. } = &mut set.shader
        {
            textures.insert(name, rid);
        }
    }
}

/// An editor for one shader parameter.
fn param_ui(ui: &mut Ui, p: &reetched_shader::Param, v: &mut [f32]) {
    let range = p.min.unwrap_or(0.0) as f32..=p.max.unwrap_or(1.0) as f32;
    let has_range = p.min.is_some() && p.max.is_some();
    match p.ty {
        ParamType::Bool => {
            let mut on = v[0] != 0.0;
            if ui.checkbox(&mut on, "").changed() {
                v[0] = if on { 1.0 } else { 0.0 };
            }
        }
        ParamType::Int if !p.values.is_empty() => {
            let current = p
                .values
                .iter()
                .find(|(_, x)| *x as f32 == v[0])
                .map_or(v[0].to_string(), |(n, _)| n.clone());
            ComboBox::from_id_salt(("shader_combo", &p.name))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (name, x) in &p.values {
                        ui.selectable_value(&mut v[0], *x as f32, name);
                    }
                });
        }
        ParamType::Vec3 | ParamType::Vec4 if p.widget.as_deref() == Some("color") => {
            let mut rgba = egui::Rgba::from_rgba_unmultiplied(
                v[0],
                v[1],
                v[2],
                v.get(3).copied().unwrap_or(1.0),
            );
            let alpha = if p.ty == ParamType::Vec4 {
                egui::color_picker::Alpha::OnlyBlend
            } else {
                egui::color_picker::Alpha::Opaque
            };
            if egui::color_picker::color_edit_button_rgba(ui, &mut rgba, alpha).changed() {
                v[0] = rgba.r();
                v[1] = rgba.g();
                v[2] = rgba.b();
                if let Some(a) = v.get_mut(3) {
                    *a = rgba.a();
                }
            }
        }
        ParamType::Float | ParamType::Int if has_range => {
            let mut s = Slider::new(&mut v[0], range);
            if p.ty == ParamType::Int {
                s = s.integer();
            }
            ui.add(s);
        }
        _ => {
            ui.horizontal(|ui| {
                for x in v.iter_mut() {
                    let mut d = DragValue::new(x).speed(0.01);
                    if has_range {
                        d = d.range(range.clone());
                    }
                    ui.add(d);
                }
            });
        }
    }
}
