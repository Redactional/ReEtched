//! Project templates (`.rtpl`): texture set defaults a new project starts with —
//! resolution, extra channels, shader model and export preset — plus the New
//! Project window that picks one.

use crate::{app::State, settings::config_dir, theme};
use egui::{ComboBox, RichText};
use reetched_doc::{Channel, Document, ShaderModel};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const EXTENSION: &str = "rtpl";

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub resolution: u32,
    #[serde(default)]
    pub extra_channels: Vec<Channel>,
    #[serde(default)]
    pub shader: ShaderModel,
    /// Name of the export preset to select.
    #[serde(default)]
    pub export_preset: Option<String>,
}

impl Template {
    pub fn builtin() -> Vec<Template> {
        let t = |name: &str, resolution, shader: ShaderModel, preset: &str| Template {
            name: name.into(),
            resolution,
            extra_channels: shader.channels().to_vec(),
            shader,
            export_preset: Some(preset.into()),
        };
        vec![
            t(
                "PBR Metal/Rough",
                2048,
                ShaderModel::MetalRough,
                "PBR Metallic Roughness",
            ),
            t(
                "Adobe Standard Material",
                2048,
                ShaderModel::Asm,
                "PBR Metallic Roughness",
            ),
            t(
                "OpenPBR",
                2048,
                ShaderModel::OpenPbr,
                "PBR Metallic Roughness",
            ),
            t(
                "Unreal Engine",
                2048,
                ShaderModel::MetalRough,
                "Unreal Engine (Packed)",
            ),
            t(
                "Unity HDRP",
                2048,
                ShaderModel::MetalRough,
                "Unity HDRP (Metallic Standard)",
            ),
            t(
                "Mobile (1K)",
                1024,
                ShaderModel::MetalRough,
                "glTF PBR Metal Roughness",
            ),
        ]
    }

    /// Templates saved in the config folder's `templates/`.
    pub fn user() -> Vec<(PathBuf, Template)> {
        let dir = config_dir().join("templates");
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut out: Vec<(PathBuf, Template)> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(EXTENSION))
            .filter_map(|p| Some((p.clone(), Self::load(&p).ok()?)))
            .collect();
        out.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        out
    }

    pub fn load(path: &Path) -> anyhow::Result<Template> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// Sets every texture set of `doc` up like this template.
    pub fn apply(&self, doc: &mut Document) {
        for set in &mut doc.texture_sets {
            set.resolution = self.resolution;
            set.extra_channels = self.extra_channels.clone();
            set.shader = self.shader.clone();
        }
    }
}

/// The New Project window.
pub struct NewProject {
    pub mesh: String,
    /// Index into built-ins, then user templates.
    pub template: usize,
    pub resolution: u32,
    /// A template from a library folder, offered first.
    pub custom: Option<Template>,
}

impl State {
    pub fn new_project_window(&mut self, ctx: &egui::Context) {
        let Some(np) = &mut self.new_project else {
            return;
        };
        let templates = np
            .custom
            .clone()
            .into_iter()
            .chain(Template::builtin())
            .chain(Template::user().into_iter().map(|(_, t)| t))
            .collect::<Vec<_>>();
        let mut open = true;
        let mut create = false;
        egui::Window::new(theme::header("NEW PROJECT"))
            .open(&mut open)
            .collapsible(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                egui::Grid::new("new_project")
                    .num_columns(2)
                    .spacing(egui::vec2(12.0, 6.0))
                    .show(ui, |ui| {
                        ui.label("Template");
                        let current = templates.get(np.template).map_or("", |t| t.name.as_str());
                        ComboBox::from_id_salt("np_template")
                            .selected_text(current)
                            .width(240.0)
                            .show_ui(ui, |ui| {
                                for (i, t) in templates.iter().enumerate() {
                                    if ui.selectable_value(&mut np.template, i, &t.name).changed() {
                                        np.resolution = t.resolution;
                                    }
                                }
                            });
                        ui.end_row();
                        ui.label("Mesh");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut np.mesh).desired_width(240.0));
                            if ui.button("Browse…").clicked()
                                && let Some(p) = rfd::FileDialog::new()
                                    .add_filter("Meshes", crate::app::MESH_EXTENSIONS)
                                    .pick_file()
                            {
                                np.mesh = p.display().to_string();
                            }
                        });
                        ui.end_row();
                        ui.label("Document resolution");
                        ComboBox::from_id_salt("np_res")
                            .selected_text(format!("{0} × {0}", np.resolution))
                            .show_ui(ui, |ui| {
                                for r in reetched_doc::RESOLUTIONS {
                                    ui.selectable_value(
                                        &mut np.resolution,
                                        r,
                                        format!("{r} × {r}"),
                                    );
                                }
                            });
                        ui.end_row();
                    });
                ui.label(
                    RichText::new("UVs beyond 0..1 become UDIM tiles automatically.")
                        .color(theme::WEAK),
                );
                create = ui
                    .add_enabled(!np.mesh.trim().is_empty(), egui::Button::new("  OK  "))
                    .clicked();
            });
        if !open {
            self.new_project = None;
            return;
        }
        if !create {
            return;
        }
        let Some(np) = self.new_project.take() else {
            return;
        };
        let mut template = templates
            .get(np.template)
            .cloned()
            .unwrap_or_else(|| Template::builtin().remove(0));
        template.resolution = np.resolution;
        self.create_project(Path::new(np.mesh.trim()), &template);
    }

    /// Loads `mesh` as a new project set up by `template`.
    pub fn create_project(&mut self, mesh: &Path, template: &Template) {
        self.load_mesh(mesh);
        template.apply(&mut self.doc);
        if let Some(preset) = &template.export_preset
            && let Some(i) = self.export.presets.iter().position(|p| &p.name == preset)
        {
            self.export.preset = i;
        }
        self.history = reetched_doc::history::History::new(&self.doc);
        self.notify_script("project_created", serde_json::json!({ "mesh": mesh }));
    }

    /// Saves the selected texture set's setup as a user template.
    pub fn save_template(&mut self) {
        let Some(set) = self.doc.texture_sets.get(self.selected_part) else {
            return;
        };
        let name = format!("{} template", self.project_name());
        let template = Template {
            name: name.clone(),
            resolution: set.resolution,
            extra_channels: set.extra_channels.clone(),
            shader: match &set.shader {
                ShaderModel::Custom { .. } => ShaderModel::MetalRough,
                s => s.clone(),
            },
            export_preset: self
                .export
                .presets
                .get(self.export.preset)
                .map(|p| p.name.clone()),
        };
        let Some(path) = rfd::FileDialog::new()
            .set_directory(config_dir().join("templates"))
            .add_filter("ReEtched template", &[EXTENSION])
            .set_file_name(format!("{name}.{EXTENSION}"))
            .save_file()
        else {
            return;
        };
        match template.save(&path) {
            Ok(()) => self.info(format!("Saved template {}", path.display())),
            Err(e) => self.error(format!("Could not save the template: {e:#}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_set_up_texture_sets_and_round_trip() {
        let asm = Template::builtin()
            .into_iter()
            .find(|t| t.shader == ShaderModel::Asm)
            .unwrap();
        let mut doc = Document::new(None, &["a".into(), "b".into()]);
        asm.apply(&mut doc);
        for set in &doc.texture_sets {
            assert_eq!(set.shader, ShaderModel::Asm);
            assert!(set.has_channel(Channel::Coat));
        }
        let path = std::env::temp_dir().join("reetched-template-test.rtpl");
        asm.save(&path).unwrap();
        assert_eq!(Template::load(&path).unwrap(), asm);
    }
}
