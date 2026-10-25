//! The Settings window and the crash recovery prompt.

use crate::{app::State, settings::LibraryRoot, theme};
use egui::{RichText, vec2};
use reetched_doc::project;

impl State {
    /// The Frame View key, unless a text field or rebinding has the keyboard.
    pub fn frame_key(&self, ctx: &egui::Context) -> bool {
        self.rebinding.is_none()
            && ctx.memory(|m| m.focused().is_none())
            && self.keymap.down(ctx, crate::keymap::Action::FrameView)
    }

    /// Keyboard settings: preset, per-action bindings (click, then press a key).
    fn keymap_ui(&mut self, ui: &mut egui::Ui) {
        use crate::keymap::{Action, Preset, captured};
        ui.label(theme::header("KEYBOARD AND MOUSE"));
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Preset");
            let k = &mut self.settings.keymap;
            egui::ComboBox::from_id_salt("keymap_preset")
                .selected_text(k.preset.label())
                .show_ui(ui, |ui| {
                    for p in Preset::ALL {
                        changed |= ui.selectable_value(&mut k.preset, p, p.label()).changed();
                    }
                });
            if !k.overrides.is_empty() && ui.small_button("Reset changes").clicked() {
                k.overrides.clear();
                changed = true;
            }
        });
        ui.label(
            RichText::new(match self.settings.keymap.preset {
                Preset::Painter => {
                    "Alt + left drag orbits, middle drag pans, Alt + right drag zooms"
                }
                Preset::Blender => "Middle drag orbits, Shift + middle pans, Ctrl + middle zooms",
            })
            .small()
            .color(theme::WEAK),
        );
        // A key pressed while waiting becomes the binding (Escape cancels).
        if let Some(a) = self.rebinding {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.rebinding = None;
            } else if let Some(s) = captured(ui.ctx()) {
                self.settings.keymap.overrides.insert(a, vec![s.text()]);
                self.rebinding = None;
                changed = true;
            }
        }
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                egui::Grid::new("keymap")
                    .num_columns(2)
                    .spacing(vec2(12.0, 4.0))
                    .show(ui, |ui| {
                        for a in Action::ALL {
                            ui.label(a.label());
                            let text = if self.rebinding == Some(a) {
                                "press a key…".to_owned()
                            } else {
                                let t: Vec<String> =
                                    self.keymap.shortcuts(a).iter().map(|s| s.text()).collect();
                                if t.is_empty() {
                                    "—".to_owned()
                                } else {
                                    t.join(", ")
                                }
                            };
                            if ui
                                .button(text)
                                .on_hover_text("Click, then press the new key (Escape cancels)")
                                .clicked()
                            {
                                self.rebinding = Some(a);
                            }
                            ui.end_row();
                        }
                    });
            });
        if changed {
            self.keymap = crate::keymap::Keymap::new(&self.settings.keymap);
        }
    }

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
                        ui.label("Interface size");
                        ui.add(
                            egui::Slider::new(&mut s.ui_scale, 0.75..=2.0)
                                .custom_formatter(|v, _| format!("{:.0} %", v * 100.0)),
                        );
                        ui.end_row();
                        ui.label("Updates");
                        ui.checkbox(&mut s.check_updates, "Check for a new version at startup")
                            .on_hover_text("Only tells you; nothing is downloaded or installed.");
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
                ui.add_space(8.0);
                self.keymap_ui(ui);
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
