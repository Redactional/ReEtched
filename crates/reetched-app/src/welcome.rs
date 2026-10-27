//! The welcome screen shown at startup: start or open a project, or pick a recent one.

use crate::app::{State, file_name};
use crate::theme;
use egui::{RichText, vec2};
use egui_material_icons::icons as ic;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Default)]
pub struct Welcome {
    pub open: bool,
    icon: Option<egui::TextureHandle>,
}

enum Choice {
    New,
    Open,
    ImportSpp,
    Recent(PathBuf),
    Forget(PathBuf),
    Close,
}

impl State {
    pub fn welcome_window(&mut self, ctx: &egui::Context) {
        // Other startup questions come first; New project replaces this screen until it closes.
        if !self.welcome.open || self.session.recovery.is_some() || self.new_project.is_some() {
            return;
        }
        let icon = self
            .welcome
            .icon
            .get_or_insert_with(|| {
                let png = include_bytes!("../../../assets/icons/linux/ReEtched_256x256.png");
                let img = image::load_from_memory(png)
                    .map(|i| i.into_rgba8())
                    .unwrap_or_default();
                let size = [img.width() as usize, img.height() as usize];
                ctx.load_texture(
                    "welcome_icon",
                    egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
                    Default::default(),
                )
            })
            .clone();
        let mut choice = None;
        let keys = |a| self.keymap.text(a);
        let (k_new, k_open) = (
            keys(crate::keymap::Action::NewProject),
            keys(crate::keymap::Action::OpenProject),
        );
        let recent = self.settings.recent_projects.clone();
        let mut show = self.settings.show_welcome;
        // A modal: the app behind is dimmed and can't be clicked until this closes.
        let modal = egui::Modal::new(egui::Id::unique("welcome"))
            .backdrop_color(egui::Color32::from_black_alpha(160))
            .frame(
                egui::Frame::window(&ctx.global_style())
                    .fill(theme::PANEL)
                    .inner_margin(20.0),
            )
            .show(ctx, |ui| {
                ui.set_width(640.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(&icon).fit_to_exact_size(vec2(56.0, 56.0)));
                    ui.vertical(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new("ReEtched").size(26.0).strong());
                        ui.label(
                            RichText::new(format!(
                                "Open-source 3D texture painter  ·  version {}",
                                env!("CARGO_PKG_VERSION")
                            ))
                            .color(theme::WEAK),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if crate::app::icon_button(ui, ic::ICON_CLOSE, false, "Close (Esc)")
                            .clicked()
                        {
                            choice = Some(Choice::Close);
                        }
                    });
                });
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);
                ui.columns(2, |cols| {
                    let ui = &mut cols[0];
                    ui.label(theme::header("START"));
                    ui.add_space(4.0);
                    let action = |ui: &mut egui::Ui, icon: &str, text: &str, keys: &str| {
                        ui.add(
                            egui::Button::new(format!("{icon}  {text}"))
                                .shortcut_text(keys)
                                .min_size(vec2(ui.available_width(), 30.0)),
                        )
                        .clicked()
                    };
                    if action(ui, ic::ICON_ADD.codepoint, "New project…", &k_new) {
                        choice = Some(Choice::New);
                    }
                    if action(ui, ic::ICON_FOLDER_OPEN.codepoint, "Open project…", &k_open) {
                        choice = Some(Choice::Open);
                    }
                    if action(ui, ic::ICON_INPUT.codepoint, "Import Painter project…", "") {
                        choice = Some(Choice::ImportSpp);
                    }
                    if action(ui, ic::ICON_MENU_BOOK.codepoint, "Documentation", "") {
                        let repo = env!("CARGO_PKG_REPOSITORY");
                        ui.ctx()
                            .open_url(egui::OpenUrl::new_tab(format!("{repo}#readme")));
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Or drop a mesh, project or .spp file on the window.")
                            .small()
                            .color(theme::WEAK),
                    );

                    let ui = &mut cols[1];
                    ui.label(theme::header("RECENT PROJECTS"));
                    ui.add_space(4.0);
                    if recent.is_empty() {
                        ui.label(RichText::new("No recent projects yet.").color(theme::WEAK));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for p in &recent {
                                if let Some(c) = recent_row(ui, p) {
                                    choice = Some(c);
                                }
                            }
                        });
                });
                ui.add_space(10.0);
                ui.separator();
                ui.checkbox(&mut show, "Show this screen at startup");
            });
        // Esc or a click on the dimmed backdrop closes it, like the ✕.
        if choice.is_none() && modal.should_close() {
            choice = Some(Choice::Close);
        }
        self.settings.show_welcome = show;
        let Some(choice) = choice else { return };
        match choice {
            Choice::New => self.open_new_project(),
            Choice::Open => self.pick_project(),
            Choice::ImportSpp => self.import_spp_dialog(),
            Choice::Recent(p) => self.open_project(&p),
            Choice::Forget(p) => self.settings.recent_projects.retain(|r| *r != p),
            Choice::Close => self.welcome.open = false,
        }
        if let Err(e) = self.settings.save() {
            self.error(format!("Could not save settings: {e:#}"));
        }
    }
}

/// One recent project: name, folder and age; missing files are dimmed and can be forgotten.
fn recent_row(ui: &mut egui::Ui, path: &PathBuf) -> Option<Choice> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let folder = path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let detail = match modified {
        Some(t) => format!("{}  ·  {folder}", ago(t)),
        None => format!("not found  ·  {folder}"),
    };
    let color = if modified.is_some() {
        theme::TEXT
    } else {
        theme::WEAK
    };
    let mut choice = None;
    ui.horizontal(|ui| {
        let width = ui.available_width() - 32.0;
        let sense = if modified.is_some() {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, row) = ui.allocate_exact_size(vec2(width, 40.0), sense);
        let row = row.on_hover_text(path.display().to_string());
        let painter = ui.painter_at(rect);
        if row.hovered() && modified.is_some() {
            painter.rect_filled(rect, 3.0, theme::ROW_SELECTED);
        }
        painter.text(
            rect.left_top() + vec2(6.0, 5.0),
            egui::Align2::LEFT_TOP,
            file_name(path),
            egui::FontId::proportional(14.0),
            color,
        );
        painter.text(
            rect.left_bottom() + vec2(6.0, -5.0),
            egui::Align2::LEFT_BOTTOM,
            detail,
            egui::FontId::proportional(11.0),
            theme::WEAK,
        );
        if row.clicked() {
            choice = Some(Choice::Recent(path.clone()));
        }
        if crate::app::icon_button(ui, ic::ICON_CLOSE, false, "Remove from the list").clicked() {
            choice = Some(Choice::Forget(path.clone()));
        }
    });
    choice
}

/// "5 min ago", "3 h ago", "2 days ago".
fn ago(t: SystemTime) -> String {
    let s = t.elapsed().map_or(0, |d| d.as_secs());
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        86400..172800 => "yesterday".into(),
        _ => format!("{} days ago", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    #[test]
    fn ages_read_naturally() {
        let back = |s| SystemTime::now() - Duration::from_secs(s);
        assert_eq!(super::ago(back(5)), "just now");
        assert_eq!(super::ago(back(300)), "5 min ago");
        assert_eq!(super::ago(back(7200)), "2 h ago");
        assert_eq!(super::ago(back(90000)), "yesterday");
        assert_eq!(super::ago(back(3 * 86400)), "3 days ago");
    }
}
