//! The asset libraries in the Assets panel: folders of textures, alphas,
//! environments, smart materials and masks, brushes, shaders, templates and
//! meshes (`reetched-library`), with search, tags and favorites. Also brush
//! presets and smart masks, which live in those folders.

use crate::{app::State, app::icon_button, settings::config_dir, theme};
use egui::{Color32, RichText, Sense, Ui, vec2};
use egui_material_icons::icons as ic;
use reetched_library::{Asset, AssetId, AssetKind, Library, LibraryEvent, LibraryRoot};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// The user's own library folder (always present, writable).
pub const MY_LIBRARY: &str = "My library";

pub struct LibraryUi {
    pub lib: Library,
    textures: HashMap<AssetId, egui::TextureHandle>,
    pub favorites_only: bool,
    /// Asset whose tags are being edited, and the text.
    tag_edit: Option<(AssetId, String)>,
    /// Asset waiting for delete confirmation.
    confirm_delete: Option<AssetId>,
    /// Roots that fell back to polling.
    pub polling: Vec<String>,
}

fn my_library_dir() -> PathBuf {
    config_dir().join("library")
}

impl LibraryUi {
    pub fn new(roots: &[crate::settings::LibraryRoot]) -> Self {
        let decoder: reetched_library::Decoder = Arc::new(|path: &Path, bytes: &[u8]| {
            let svg = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
            if svg || crate::images::is_svg(bytes) {
                crate::images::render_svg(bytes, 256).ok()
            } else {
                None
            }
        });
        let lib = Library::new(config_dir().join("library-meta")).with_thumbnails(
            config_dir().join("cache").join("thumbs"),
            reetched_library::DEFAULT_THUMB_SIZE,
            Some(decoder),
        );
        let mut ui = Self {
            lib,
            textures: HashMap::new(),
            favorites_only: false,
            tag_edit: None,
            confirm_delete: None,
            polling: Vec::new(),
        };
        ui.sync_roots(roots);
        ui
    }

    /// Makes the library's roots match the settings (plus My library).
    pub fn sync_roots(&mut self, roots: &[crate::settings::LibraryRoot]) {
        let mut wanted: Vec<LibraryRoot> = vec![LibraryRoot {
            name: MY_LIBRARY.into(),
            path: my_library_dir(),
            writable: true,
        }];
        for r in roots {
            let name = r.name.replace('/', "-");
            if wanted.iter().any(|w| w.name == name) {
                continue;
            }
            let writable = std::fs::metadata(&r.path).is_ok_and(|m| !m.permissions().readonly());
            wanted.push(LibraryRoot {
                name,
                path: r.path.clone(),
                writable,
            });
        }
        let _ = std::fs::create_dir_all(my_library_dir());
        let current: Vec<LibraryRoot> = self.lib.roots().cloned().collect();
        for c in &current {
            if !wanted.iter().any(|w| w.name == c.name && w.path == c.path) {
                self.lib.remove_root(&c.name);
            }
        }
        for w in wanted {
            if !current.iter().any(|c| c.name == w.name && c.path == w.path)
                && let Err(e) = self.lib.add_root(w.clone())
            {
                log::warn!("library {}: {e:#}", w.name);
            }
        }
    }

    /// Applies disk changes and ready thumbnails (once a frame).
    pub fn poll(&mut self) {
        for e in self.lib.poll() {
            match e {
                LibraryEvent::ThumbnailReady(id) | LibraryEvent::Changed(id) => {
                    self.textures.remove(&id);
                }
                LibraryEvent::Removed(id) => {
                    self.textures.remove(&id);
                }
                LibraryEvent::WatchFailed { root, .. } => {
                    if !self.polling.contains(&root) {
                        self.polling.push(root);
                    }
                }
                LibraryEvent::Added(_) => {}
            }
        }
    }

    fn texture(&mut self, ctx: &egui::Context, a: &Asset) -> Option<egui::TextureId> {
        if let Some(t) = self.textures.get(&a.id) {
            return Some(t.id());
        }
        let img = self.lib.thumbnail(&a.id)?;
        let handle = ctx.load_texture(
            format!("asset {}", a.id),
            egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                &img,
            ),
            egui::TextureOptions::LINEAR,
        );
        let id = handle.id();
        self.textures.insert(a.id.clone(), handle);
        Some(id)
    }
}

fn kind_icon(k: AssetKind) -> egui_material_icons::MaterialIcon {
    match k {
        AssetKind::Texture => ic::ICON_TEXTURE,
        AssetKind::Alpha => ic::ICON_BLUR_ON,
        AssetKind::Environment => ic::ICON_LIGHT_MODE,
        AssetKind::SmartMaterial => ic::ICON_AUTO_FIX_HIGH,
        AssetKind::SmartMask => ic::ICON_MASKED_TRANSITIONS,
        AssetKind::Brush => ic::ICON_BRUSH,
        AssetKind::Procedural => ic::ICON_GRAIN,
        AssetKind::Shader => ic::ICON_CODE,
        AssetKind::OcioConfig => ic::ICON_PALETTE,
        AssetKind::ProjectTemplate => ic::ICON_NOTE_ADD,
        AssetKind::Mesh => ic::ICON_VIEW_IN_AR,
    }
}

impl State {
    /// The library assets view for `kind` (None = everything), drawn in the Assets panel.
    pub fn library_assets_ui(&mut self, ui: &mut Ui, kind: Option<AssetKind>) {
        ui.horizontal(|ui| {
            let star = if self.library.favorites_only {
                ic::ICON_STAR
            } else {
                ic::ICON_STAR_RATE
            };
            if icon_button(ui, star, self.library.favorites_only, "Favorites only").clicked() {
                self.library.favorites_only = !self.library.favorites_only;
            }
            if icon_button(ui, ic::ICON_ADD, false, "Add files to My library…").clicked()
                && let Some(files) = rfd::FileDialog::new().pick_files()
            {
                match self.library.lib.import_into(MY_LIBRARY, &files) {
                    Ok(ids) => self.info(format!("Added {} file(s) to My library", ids.len())),
                    Err(e) => self.error(format!("Could not add files: {e:#}")),
                }
            }
            if icon_button(ui, ic::ICON_FOLDER_OPEN, false, "Open My library's folder").clicked() {
                crate::settings_ui::open_folder(&my_library_dir());
            }
            if icon_button(ui, ic::ICON_SETTINGS, false, "Library folders…").clicked() {
                self.settings_open = true;
            }
            if kind == Some(AssetKind::Brush) && ui.small_button("Save brush…").clicked() {
                self.save_brush_preset();
            }
        });
        if !self.library.polling.is_empty() {
            ui.label(
                RichText::new(format!(
                    "Checking {} for changes every few seconds",
                    self.library.polling.join(", ")
                ))
                .small()
                .color(theme::WEAK),
            );
        }
        let assets: Vec<Asset> = self
            .library
            .lib
            .query(&self.asset_search, kind, self.library.favorites_only)
            .into_iter()
            .cloned()
            .collect();
        if assets.is_empty() {
            ui.label(
                RichText::new(
                    "Nothing here yet. Add files with + or add library folders in Settings.",
                )
                .color(theme::WEAK),
            );
        }
        let mut open: Option<Asset> = None;
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
                for a in &assets {
                    let (rect, resp) = ui.allocate_exact_size(vec2(70.0, 86.0), Sense::click());
                    if resp.hovered() {
                        ui.painter()
                            .rect_filled(rect, 2.0, Color32::from_gray(0x36));
                    }
                    let thumb = egui::Rect::from_center_size(
                        rect.center_top() + vec2(0.0, 34.0),
                        vec2(62.0, 62.0),
                    );
                    match self.library.texture(&ctx, a) {
                        Some(tex) => {
                            let uv = egui::Rect::from_min_max(
                                egui::pos2(0.0, 0.0),
                                egui::pos2(1.0, 1.0),
                            );
                            ui.painter().image(tex, thumb, uv, Color32::WHITE);
                        }
                        None => {
                            let icon = kind_icon(a.kind);
                            ui.painter().text(
                                thumb.center(),
                                egui::Align2::CENTER_CENTER,
                                icon.codepoint,
                                egui::FontId::new(28.0, icon.font_family()),
                                theme::WEAK,
                            );
                        }
                    }
                    if a.favorite {
                        ui.painter().text(
                            thumb.right_top() + vec2(-2.0, 2.0),
                            egui::Align2::RIGHT_TOP,
                            ic::ICON_STAR.codepoint,
                            egui::FontId::new(12.0, ic::ICON_STAR.font_family()),
                            theme::ACCENT,
                        );
                    }
                    let label: String = if a.name.chars().count() > 11 {
                        a.name.chars().take(10).chain(['…']).collect()
                    } else {
                        a.name.clone()
                    };
                    ui.painter().text(
                        rect.center_bottom() - vec2(0.0, 8.0),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(10.5),
                        theme::TEXT,
                    );
                    let tags = if a.tags.is_empty() {
                        String::new()
                    } else {
                        format!("\nTags: {}", a.tags.join(", "))
                    };
                    let resp = resp.on_hover_text(format!(
                        "{} ({}){tags}\n{}\nDouble-click to use.",
                        a.name,
                        a.kind.label(),
                        a.path.display()
                    ));
                    if resp.double_clicked() {
                        open = Some(a.clone());
                    }
                    resp.context_menu(|ui| {
                        let fav = if a.favorite {
                            "Remove from favorites"
                        } else {
                            "Add to favorites"
                        };
                        if ui.button(fav).clicked() {
                            let _ = self.library.lib.set_favorite(&a.id, !a.favorite);
                            ui.close();
                        }
                        if ui.button("Edit tags…").clicked() {
                            self.library.tag_edit = Some((a.id.clone(), a.tags.join(", ")));
                            ui.close();
                        }
                        if ui.button("Show in folder").clicked() {
                            if let Some(dir) = a.path.parent() {
                                crate::settings_ui::open_folder(dir);
                            }
                            ui.close();
                        }
                        if ui.button("Delete file…").clicked() {
                            self.library.confirm_delete = Some(a.id.clone());
                            ui.close();
                        }
                    });
                }
            });
        });
        self.library_dialogs(&ctx);
        if let Some(a) = open {
            self.use_asset(&a);
        }
    }

    fn library_dialogs(&mut self, ctx: &egui::Context) {
        if let Some((id, text)) = &mut self.library.tag_edit {
            let mut done = None;
            egui::Window::new(theme::header("TAGS"))
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Comma-separated tags");
                    let r = ui.text_edit_singleline(text);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked()
                            || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            done = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            done = Some(false);
                        }
                    });
                });
            if let Some(save) = done {
                if save {
                    let tags: Vec<String> = text
                        .split(',')
                        .map(|t| t.trim().to_owned())
                        .filter(|t| !t.is_empty())
                        .collect();
                    let id = id.clone();
                    if let Err(e) = self.library.lib.set_tags(&id, tags) {
                        self.error(format!("Could not save tags: {e:#}"));
                    }
                }
                self.library.tag_edit = None;
            }
        }
        if let Some(id) = self.library.confirm_delete.clone() {
            let name = self
                .library
                .lib
                .get(&id)
                .map_or(id.to_string(), |a| a.name.clone());
            let mut done = None;
            egui::Window::new(theme::header("DELETE FILE"))
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("Delete “{name}” from disk? This can't be undone."));
                    ui.horizontal(|ui| {
                        if ui
                            .button(RichText::new("Delete").color(theme::ERROR))
                            .clicked()
                        {
                            done = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            done = Some(false);
                        }
                    });
                });
            if let Some(delete) = done {
                if delete && let Err(e) = self.library.lib.remove(&id) {
                    self.error(format!("Could not delete: {e:#}"));
                }
                self.library.confirm_delete = None;
            }
        }
    }

    /// What double-clicking a library asset does.
    pub fn use_asset(&mut self, a: &Asset) {
        let path = a.path.clone();
        match a.kind {
            AssetKind::Texture => {
                if let Some(id) = self.import_resource(&path) {
                    let target = self.selected.map(|s| {
                        if s.mask {
                            crate::text_ui::TextTarget::Mask(s.node)
                        } else {
                            crate::text_ui::TextTarget::Fill(s.node, self.layer_channel)
                        }
                    });
                    match target {
                        Some(t) => {
                            let ctx = egui::Context::default();
                            self.use_resource(t, id, &ctx);
                            self.info(format!("{} → {}", a.name, self.layer_channel.label()));
                        }
                        None => self.warn("Select a fill layer or a mask first".into()),
                    }
                }
            }
            AssetKind::Alpha => {
                if let Some(id) = self.import_resource(&path) {
                    self.brush.alpha = Some(id);
                    self.info(format!("Brush alpha: {}", a.name));
                }
            }
            AssetKind::SmartMaterial => match reetched_doc::smart::load(&path) {
                Ok((sm, res)) => {
                    let node = sm.instantiate(&mut self.doc, &res, &mut self.resources);
                    self.add_node(node);
                }
                Err(e) => self.error(format!("Could not open {}: {e:#}", a.name)),
            },
            AssetKind::SmartMask => self.apply_smart_mask(&path),
            AssetKind::Brush => self.load_brush_preset(&path),
            AssetKind::Mesh => {
                self.new_project = Some(crate::templates::NewProject {
                    mesh: path.display().to_string(),
                    template: 0,
                    resolution: 2048,
                    custom: None,
                });
            }
            AssetKind::ProjectTemplate => match crate::templates::Template::load(&path) {
                Ok(t) => {
                    let resolution = t.resolution;
                    self.new_project = Some(crate::templates::NewProject {
                        mesh: String::new(),
                        template: 0,
                        resolution,
                        custom: Some(t),
                    });
                }
                Err(e) => self.error(format!("Could not read {}: {e:#}", a.name)),
            },
            AssetKind::Environment
            | AssetKind::Procedural
            | AssetKind::Shader
            | AssetKind::OcioConfig => self.open_any(&path),
        }
    }

    /// Saves the brush as a preset in My library.
    pub fn save_brush_preset(&mut self) {
        let preset = BrushPreset {
            brush: self.brush.clone(),
            alpha: self
                .brush
                .alpha
                .as_ref()
                .and_then(|id| self.resources.files.get(id))
                .map(|b| hex(b)),
        };
        let dir = my_library_dir().join("brushes");
        let Some(path) = rfd::FileDialog::new()
            .set_directory(&dir)
            .add_filter("Brush preset", &["rbrush"])
            .set_file_name("Brush.rbrush")
            .save_file()
        else {
            return;
        };
        let result = serde_json::to_vec_pretty(&preset)
            .map_err(anyhow::Error::from)
            .and_then(|j| Ok(std::fs::write(&path, j)?));
        match result {
            Ok(()) => self.info(format!("Saved brush {}", path.display())),
            Err(e) => self.error(format!("Could not save the brush: {e:#}")),
        }
    }

    pub fn load_brush_preset(&mut self, path: &Path) {
        let preset: anyhow::Result<BrushPreset> = std::fs::read(path)
            .map_err(anyhow::Error::from)
            .and_then(|b| Ok(serde_json::from_slice(&b)?));
        match preset {
            Ok(p) => {
                let mut brush = p.brush;
                brush.alpha = p.alpha.and_then(|h| unhex(&h)).map(|bytes| {
                    let id = self.resources.add(bytes);
                    self.doc.resources.insert(
                        id.clone(),
                        reetched_doc::ResourceInfo {
                            name: "brush alpha".into(),
                        },
                    );
                    id
                });
                self.brush = brush;
                self.info(format!("Brush {}", crate::app::file_name(path)));
            }
            Err(e) => self.error(format!("Could not read {}: {e:#}", path.display())),
        }
    }

    /// Saves the selected layer's mask as a smart mask in My library.
    pub fn save_smart_mask(&mut self) {
        let Some(sel) = self.selected else { return };
        let Some(node) = self
            .stack()
            .and_then(|s| reetched_doc::find_node(s, sel.node))
            .cloned()
        else {
            return;
        };
        let Some(mask) = node.mask.clone() else {
            self.warn("This layer has no mask".into());
            return;
        };
        // A smart mask is stored as a smart material whose folder carries the mask.
        let mut holder = self.doc.new_folder(format!("{} mask", node.name));
        holder.mask = Some(mask);
        let sm = reetched_doc::smart::SmartMaterial::capture(&holder, &self.doc);
        let Some(path) = rfd::FileDialog::new()
            .set_directory(my_library_dir().join("smart masks"))
            .add_filter("Smart mask", &["rsmk"])
            .set_file_name(format!("{}.rsmk", holder.name))
            .save_file()
        else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match reetched_doc::smart::save(&path, &sm, &self.resources) {
            Ok(()) => self.info(format!("Saved smart mask {}", path.display())),
            Err(e) => self.error(format!("Could not save the smart mask: {e:#}")),
        }
    }

    /// Puts the smart mask in `path` on the selected layer.
    pub fn apply_smart_mask(&mut self, path: &Path) {
        let Some(sel) = self.selected else {
            self.warn("Select a layer to put the mask on".into());
            return;
        };
        match reetched_doc::smart::load(path) {
            Ok((sm, res)) => {
                let holder = sm.instantiate(&mut self.doc, &res, &mut self.resources);
                if let Some(node) = self
                    .stack_mut()
                    .and_then(|s| reetched_doc::find_node_mut(s, sel.node))
                {
                    node.mask = holder.mask;
                    self.selected = Some(crate::app::Selected {
                        node: sel.node,
                        mask: true,
                    });
                }
            }
            Err(e) => self.error(format!("Could not open {}: {e:#}", path.display())),
        }
    }
}

/// A brush preset file (`.rbrush`): the settings, with the alpha image inline.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct BrushPreset {
    pub brush: crate::brush::Brush,
    /// The alpha image's bytes, hex-encoded.
    pub alpha: Option<String>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn hex_round_trips() {
        let b = [0u8, 15, 16, 255];
        assert_eq!(super::unhex(&super::hex(&b)).unwrap(), b);
        assert!(super::unhex("zz").is_none());
    }
}
