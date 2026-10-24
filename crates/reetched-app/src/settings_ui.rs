//! The Settings window and the crash recovery prompt.

use crate::{app::State, settings::LibraryRoot, theme};
use egui::{RichText, vec2};
use reetched_doc::project;

impl State {
    pub fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut open = true;
        let mut add_root = false;
        let mut remove_root = None;
        let mut pick_python = false;
        egui::Window::new(theme::header("SETTINGS"))
            .open(&mut open)
            .collapsible(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                let s = &mut self.settings;
                ui.label(theme::header("GENERAL"));
                egui::Grid::new("settings")
                    .num_columns(2)
                    .spacing(vec2(12.0, 6.0))
                    .show(ui, |ui| {
                        ui.label("Autosave every");
                        ui.add(
                            egui::Slider::new(&mut s.autosave_minutes, 0.0..=60.0)
                                .suffix(" min")
                                .integer(),
                        )
                        .on_hover_text("0 turns autosave off. Autosaves are offered back after a crash.");
                        ui.end_row();
                        ui.label("GPU cache budget");
                        ui.add(
                            egui::DragValue::new(&mut s.vram_budget_mb)
                                .range(0..=65536)
                                .suffix(" MB"),
                        )
                        .on_hover_text("Paint and image textures beyond this are dropped and reloaded when needed (0 = automatic)");
                        ui.end_row();
                        ui.label("Python");
                        ui.horizontal(|ui| {
                            let text = s
                                .python
                                .as_ref()
                                .map_or("found automatically".to_owned(), |p| p.display().to_string());
                            ui.label(RichText::new(text).color(theme::WEAK));
                            if ui.small_button("Choose…").clicked() {
                                pick_python = true;
                            }
                            if s.python.is_some() && ui.small_button("Automatic").clicked() {
                                s.python = None;
                            }
                        });
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.label(theme::header("ASSET LIBRARIES"));
                for (i, root) in s.library_roots.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut root.name).desired_width(120.0));
                        ui.label(RichText::new(root.path.display().to_string()).color(theme::WEAK));
                        if crate::app::icon_button(
                            ui,
                            egui_material_icons::icons::ICON_CLOSE,
                            false,
                            "Remove this library (its files stay)",
                        )
                        .clicked()
                        {
                            remove_root = Some(i);
                        }
                    });
                }
                if ui.button("Add library folder…").clicked() {
                    add_root = true;
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Open the settings folder").clicked() {
                        let dir = crate::settings::config_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        open_folder(&dir);
                    }
                });
            });
        if let Some(i) = remove_root {
            self.settings.library_roots.remove(i);
            self.libraries_changed();
        }
        if add_root && let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let name = crate::app::file_name(&dir);
            self.settings
                .library_roots
                .push(LibraryRoot { name, path: dir });
            self.libraries_changed();
        }
        if pick_python && let Some(p) = rfd::FileDialog::new().pick_file() {
            self.settings.python = Some(p);
        }
        if !open {
            self.settings_open = false;
            if let Err(e) = self.settings.save() {
                self.error(format!("Could not save settings: {e:#}"));
            }
        }
    }

    /// Offers back the autosave of a session that crashed.
    pub fn recovery_window(&mut self, ctx: &egui::Context) {
        let Some(r) = &self.session.recovery else {
            return;
        };
        let when = r
            .saved_at
            .elapsed()
            .map(|d| {
                let m = d.as_secs() / 60;
                if m < 60 {
                    format!("{m} min ago")
                } else {
                    format!("{} h ago", m / 60)
                }
            })
            .unwrap_or_default();
        let what = r
            .project
            .as_ref()
            .map_or("an unsaved project".to_owned(), |p| {
                crate::app::file_name(p)
            });
        let mut choice = None;
        egui::Window::new(theme::header("RECOVER WORK"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(format!(
                    "ReEtched closed unexpectedly. An autosave of {what} from {when} is available."
                ));
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Recover").strong()).clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Discard").clicked() {
                        choice = Some(false);
                    }
                });
            });
        let Some(recover) = choice else { return };
        let Some(r) = self.session.recovery.take() else {
            return;
        };
        if recover {
            match project::load(&r.autosave) {
                Ok((doc, mesh, resources)) => {
                    self.set_mesh(mesh, what.clone(), Some(doc));
                    self.resources = resources;
                    // Saving goes back to the original file (if there was one).
                    self.project_path = r.project.clone();
                    self.info(format!("Recovered {what}"));
                }
                Err(e) => {
                    self.error(format!("Could not read the autosave: {e:#}"));
                    return;
                }
            }
        }
        r.forget();
    }
}

/// Shows a folder in the OS file manager.
pub fn open_folder(dir: &std::path::Path) {
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}
