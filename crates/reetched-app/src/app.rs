//! Editor shell: menus, toolbars and the dockable panel layout (modelled on Substance Painter).
//! The panels' contents live in `panels.rs`.

use crate::{
    brush::Brush,
    env::EnvImage,
    paint_ui::ActiveStroke,
    preview::{PRESETS, PreviewEnv},
    theme,
    viewport::{FrameParams, ViewMode, Viewport},
};
use egui::{Frame, RichText, vec2};
use egui_material_icons::icons as ic;
use egui_tiles::{Container, Tile, TileId, Tiles};
use glam::{Mat4, Vec2, Vec3};
use reetched_doc::{
    Channel, Document, Node, NodeId, Placement, Resources,
    export::{ExportPreset, ExportSettings, builtin_presets},
    find_node,
    history::History,
    insert_node, project, take_node,
};
use reetched_io::Mesh;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Instant,
};

pub const MESH_EXTENSIONS: &[&str] = &["fbx", "obj", "gltf", "glb", "usd", "usda", "usdc", "usdz"];
pub const ENV_EXTENSIONS: &[&str] = &["hdr", "exr"];
const THUMB_SIZE: usize = 96;

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Pane {
    Assets,
    View3d,
    View2d,
    TextureSets,
    Layers,
    TextureSetSettings,
    Properties,
    DisplaySettings,
    Log,
}

impl Pane {
    const ALL: [Pane; 9] = [
        Pane::Assets,
        Pane::View3d,
        Pane::View2d,
        Pane::TextureSets,
        Pane::Layers,
        Pane::TextureSetSettings,
        Pane::Properties,
        Pane::DisplaySettings,
        Pane::Log,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Assets => "ASSETS",
            Pane::View3d => "3D VIEW",
            Pane::View2d => "2D VIEW",
            Pane::TextureSets => "TEXTURE SET LIST",
            Pane::Layers => "LAYERS",
            Pane::TextureSetSettings => "TEXTURE SET SETTINGS",
            Pane::Properties => "PROPERTIES",
            Pane::DisplaySettings => "DISPLAY SETTINGS",
            Pane::Log => "LOG",
        }
    }
}

/// Painting tools.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Paint,
    Eraser,
    Projection,
    PolygonFill,
    Smudge,
    Clone,
    Path,
}

impl Tool {
    const ALL: [(Tool, egui_material_icons::MaterialIcon, &'static str); 7] = [
        (Tool::Paint, ic::ICON_BRUSH, "Paint (1)"),
        (Tool::Eraser, ic::ICON_INK_ERASER, "Eraser (2)"),
        (
            Tool::Projection,
            ic::ICON_CENTER_FOCUS_STRONG,
            "Projection (3)",
        ),
        (Tool::PolygonFill, ic::ICON_PENTAGON, "Polygon fill (4)"),
        (Tool::Smudge, ic::ICON_GESTURE, "Smudge (5)"),
        (Tool::Clone, ic::ICON_CONTENT_COPY, "Clone (6)"),
        (Tool::Path, ic::ICON_POLYLINE, "Path (7)"),
    ];
}

/// Payload when dragging an asset out of the Assets panel: a material preset,
/// or a smart material at `SMART_OFFSET + index`.
pub struct PresetDrag(pub usize);

/// Asset indices from here on are smart materials (built-ins, then imports).
pub const SMART_OFFSET: usize = 10_000;
/// Asset indices from here on are procedural materials (library, then imported graphs).
pub const PROC_OFFSET: usize = 20_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AssetCategory {
    Materials,
    SmartMaterials,
    Procedural,
    /// Files from the asset library folders (None = every kind).
    Library(Option<reetched_library::AssetKind>),
}

pub struct App {
    tree: egui_tiles::Tree<Pane>,
    state: State,
}

pub struct State {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub viewport: Viewport,
    pub camera: OrbitCamera,
    pub uv_camera: UvCamera,
    pub bounds: (Vec3, Vec3),
    pub mesh_label: String,
    /// The imported mesh, kept for saving projects.
    pub mesh: Mesh,
    pub doc: Document,
    pub resources: Resources,
    pub history: History,
    /// Where the project was last saved or opened from.
    pub project_path: Option<PathBuf>,
    /// Selected texture set.
    pub selected_part: usize,
    pub selected: Option<Selected>,
    /// Folders collapsed in the layer stack.
    pub collapsed: HashSet<NodeId>,
    pub export: ExportUi,
    pub bake: crate::bake_ui::BakeUi,
    /// Picking an ID-map color for (layer, mask effect index).
    pub id_pick: Option<(NodeId, usize)>,
    /// Set by the export window; handled after it draws (file dialogs block).
    pub pending_preset_import: bool,
    pub pending_preset_save: bool,
    pub exposure_ev: f32,
    pub render: crate::viewport::RenderSettings,
    pub env_rotation_deg: f32,
    pub background_blur: f32,
    pub env_opacity: f32,
    pub show_wireframe: bool,
    pub show_uv_wireframe: bool,
    pub view_mode: ViewMode,
    pub uv_mode: ViewMode,
    /// Which channel the layer stack's blend mode, opacity and thumbnails show.
    pub layer_channel: Channel,
    pub tool: Tool,
    pub brush: Brush,
    pub stroke: Option<ActiveStroke>,
    pub polygon_mode: reetched_io::topology::Selection,
    /// Mesh adjacency for Polygon Fill (built on first use).
    pub topology: Option<std::sync::Arc<reetched_io::topology::Topology>>,
    /// Clone source: UV and texture set.
    pub clone_source: Option<(glam::Vec2, usize)>,
    /// Source minus destination (UV), fixed by the first stroke after picking a source.
    pub clone_offset: Option<glam::Vec2>,
    /// Path tool points (object space) and whether committing erases.
    pub path_points: Vec<Vec3>,
    pub path_mode: crate::tools_ui::PathMode,
    /// The Projection tool's image.
    pub stencil: crate::tools_ui::Stencil,
    /// Polygon Fill rectangle selection: where the drag started.
    pub marquee: Option<egui::Pos2>,
    /// Warp projection point being dragged in the 2D view.
    pub warp_drag: Option<usize>,
    /// The text resource window, while open.
    pub text_ui: Option<crate::text_ui::TextUi>,
    pub color: crate::color_ui::ColorUi,
    pub layer_thumbs: crate::thumbs::Thumbs,
    pub settings: crate::settings::Settings,
    pub library: crate::library_ui::LibraryUi,
    pub settings_open: bool,
    pub keymap: crate::keymap::Keymap,
    /// Settings: the action waiting for a new key.
    pub rebinding: Option<crate::keymap::Action>,
    pub updates: crate::updates::Updates,
    pub welcome: crate::welcome::Welcome,
    pub script: crate::scripting::Scripting,
    /// The loaded environment map (None = the built-in sky).
    pub env_path: Option<PathBuf>,
    pub session: crate::session::Session,
    pub new_project: Option<crate::templates::NewProject>,
    /// Properties shows the tool (true) or the selected layer (false).
    pub properties_tool: bool,
    /// Latest pen pressure (1 with a mouse).
    pub pressure: f32,
    /// Where the last stroke ended (2D view?, screen point), for Shift+click lines.
    pub last_stroke_end: Option<(bool, glam::Vec2)>,
    /// Where the 3D view was drawn last frame.
    pub view3d_rect: egui::Rect,
    pub asset_search: String,
    pub asset_category: AssetCategory,
    /// Smart materials imported this session (with their images and tiles).
    pub smart_imports: Vec<(reetched_doc::smart::SmartMaterial, Resources)>,
    pub smart_thumbs: Vec<egui::TextureHandle>,
    pub procedural: crate::procedural::Procedurals,
    pub selected_preset: Option<usize>,
    pub preview_env: PreviewEnv,
    pub thumbs: Vec<egui::TextureHandle>,
    pub log: Vec<LogLine>,
    started: Instant,
    adapter: String,
    quit: bool,
    /// Waits for an answer to "Save changes?" before running.
    discard: Option<Discard>,
}

/// An action that throws away the open project.
pub enum Discard {
    Quit,
    NewProject(crate::templates::NewProject),
    /// A project, mesh or Painter import (see `State::open_document`).
    Open(PathBuf),
}

/// A selected layer (or its mask).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selected {
    pub node: NodeId,
    pub mask: bool,
}

/// State of the Export Textures window.
pub struct ExportUi {
    pub open: bool,
    pub dir: String,
    pub presets: Vec<ExportPreset>,
    pub preset: usize,
    pub settings: ExportSettings,
    /// Which texture sets to export.
    pub include: Vec<bool>,
    /// Mesh export file type, and displacement from the height channel (world units).
    pub mesh_format: crate::stack_ui::MeshFormat,
    pub displacement: f32,
}

pub struct LogLine {
    pub time: String,
    pub text: String,
    pub error: bool,
}

impl App {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
        adapter: &wgpu::AdapterInfo,
        wireframe: bool,
        startup_mesh: Option<PathBuf>,
    ) -> Self {
        let env = EnvImage::procedural_sky();
        let settings = crate::settings::Settings::load();
        let keymap = crate::keymap::Keymap::new(&settings.keymap);
        let mut state = State {
            device: device.clone(),
            queue: queue.clone(),
            preview_env: PreviewEnv::new(&env),
            viewport: Viewport::new(device, queue, egui, wireframe, env),
            camera: OrbitCamera::default(),
            uv_camera: UvCamera::default(),
            bounds: (Vec3::splat(-1.0), Vec3::ONE),
            mesh_label: String::new(),
            mesh: Mesh::uv_sphere(1.0, 4, 2),
            doc: Document::new(None, &[]),
            resources: Resources::default(),
            history: History::new(&Document::new(None, &[])),
            project_path: None,
            selected_part: 0,
            selected: None,
            collapsed: HashSet::new(),
            pending_preset_import: false,
            pending_preset_save: false,
            bake: Default::default(),
            id_pick: None,
            export: ExportUi {
                open: false,
                dir: String::new(),
                presets: builtin_presets(),
                preset: 0,
                settings: ExportSettings::default(),
                include: Vec::new(),
                mesh_format: crate::stack_ui::MeshFormat::Usda,
                displacement: 0.0,
            },
            exposure_ev: 0.0,
            render: Default::default(),
            env_rotation_deg: 0.0,
            background_blur: 0.6,
            env_opacity: 0.0,
            show_wireframe: false,
            show_uv_wireframe: true,
            view_mode: ViewMode::Material,
            uv_mode: ViewMode::Material,
            layer_channel: Channel::BaseColor,
            tool: Tool::Paint,
            brush: Brush::default(),
            stroke: None,
            polygon_mode: reetched_io::topology::Selection::Polygon,
            topology: None,
            clone_source: None,
            clone_offset: None,
            path_points: Vec::new(),
            path_mode: crate::tools_ui::PathMode::Paint,
            stencil: Default::default(),
            marquee: None,
            text_ui: None,
            warp_drag: None,
            color: Default::default(),
            layer_thumbs: Default::default(),
            library: crate::library_ui::LibraryUi::new(&settings.library_roots),
            settings,
            settings_open: false,
            keymap,
            rebinding: None,
            updates: Default::default(),
            welcome: Default::default(),
            script: Default::default(),
            env_path: None,
            session: crate::session::Session::start(),
            new_project: None,
            properties_tool: false,
            pressure: 1.0,
            last_stroke_end: None,
            view3d_rect: egui::Rect::NOTHING,
            asset_search: String::new(),
            asset_category: AssetCategory::Materials,
            smart_imports: Vec::new(),
            smart_thumbs: Vec::new(),
            procedural: Default::default(),
            selected_preset: None,
            thumbs: Vec::new(),
            log: Vec::new(),
            started: Instant::now(),
            adapter: format!("{} ({:?})", adapter.name, adapter.backend),
            quit: false,
            discard: None,
        };
        state.set_mesh(Mesh::uv_sphere(1.0, 64, 32), "Preview sphere".into(), None);
        // Tests start Python themselves (it costs a process launch).
        if !cfg!(test) {
            state.start_scripting();
        }
        // Tests drive the UI directly, without the welcome screen in the way.
        state.welcome.open = state.settings.show_welcome && !cfg!(test);
        if !cfg!(test) {
            crate::console::set_visible(state.settings.show_console);
            log::debug!("GPU: {}", state.adapter);
            log::debug!("settings: {}", crate::settings::Settings::path().display());
        }
        if !cfg!(test) {
            state.reload_imports();
        }
        if let Some(path) = startup_mesh {
            state.open_any(&path);
        }
        // The layout as the last session left it (unless it no longer parses).
        let saved = state.settings.layout.clone().filter(|_| !cfg!(test));
        Self {
            tree: saved
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_else(default_layout),
            state,
        }
    }

    pub fn wants_quit(&self) -> bool {
        self.state.quit
    }

    /// The window's close button: quits, after asking to save changes.
    pub fn request_quit(&mut self) {
        self.state.confirm_discard(Discard::Quit);
    }

    /// Clean exit: keep settings, drop this session's autosave.
    pub fn shutdown(&mut self) {
        let s = &mut self.state;
        s.notify_script("shutdown", serde_json::json!({}));
        s.flush_script_events();
        s.settings.layout = serde_json::to_value(&self.tree).ok();
        if let Err(e) = s.settings.save() {
            log::warn!("could not save settings: {e:#}");
        }
        s.session.end();
        crate::gpu_cache::save();
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let scale = self.state.settings.ui_scale.clamp(0.5, 3.0);
        if (ctx.zoom_factor() - scale).abs() > 1e-3 {
            ctx.set_zoom_factor(scale);
        }
        if self.state.thumbs.is_empty() {
            self.state.refresh_thumbnails(&ctx);
        }
        if self.state.procedural.thumbs.is_empty() {
            self.state.procedural.refresh_thumbs(&ctx);
        }
        self.state.update_procedurals();
        self.state.library.poll();
        if self.state.procedural.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if self.state.viewport.accumulating(&self.state.render) {
            ctx.request_repaint();
        }
        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_owned())
                .collect()
        });
        for path in dropped {
            if self.state.tool == Tool::Projection && crate::images::is_image_path(&path) {
                match std::fs::read(&path) {
                    Ok(bytes) => self
                        .state
                        .set_stencil(&ctx, &bytes, &path.display().to_string()),
                    Err(e) => self
                        .state
                        .error(format!("Could not read {}: {e}", path.display())),
                }
            } else {
                self.state.open_any(&path);
            }
        }
        self.state.shortcuts(&ctx);
        self.state.paint_shortcuts(&ctx);
        // Panes report their size each frame; hidden ones stay at zero and skip rendering.
        self.state.viewport.view3d.requested_size = [0, 0];
        self.state.viewport.view2d.requested_size = [0, 0];

        egui::Panel::top("menu")
            .frame(
                Frame::new()
                    .fill(theme::HEADER)
                    .inner_margin(vec2(6.0, 2.0)),
            )
            .show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| self.menu_ui(ui));
            });
        egui::Panel::top("toolbar")
            .frame(Frame::new().fill(theme::STRIP).inner_margin(vec2(6.0, 3.0)))
            .show(ui, |ui| self.toolbar_ui(ui));
        egui::Panel::bottom("status")
            .frame(
                Frame::new()
                    .fill(theme::HEADER)
                    .inner_margin(vec2(8.0, 3.0)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.state.status_line()).small());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "GPU cache {} MB  |  {}  |  Version: {}",
                                self.state.cache_mb(),
                                self.state.adapter,
                                env!("CARGO_PKG_VERSION")
                            ))
                            .small()
                            .color(theme::WEAK),
                        );
                    });
                });
            });
        egui::Panel::left("tools")
            .resizable(false)
            .default_size(38.0)
            .frame(Frame::new().fill(theme::STRIP).inner_margin(vec2(4.0, 6.0)))
            .show(ui, |ui| self.state.tool_strip_ui(ui));
        egui::Panel::right("side_strip")
            .resizable(false)
            .default_size(38.0)
            .frame(Frame::new().fill(theme::STRIP).inner_margin(vec2(4.0, 6.0)))
            .show(ui, |ui| self.side_strip_ui(ui));
        egui::CentralPanel::no_frame()
            .frame(Frame::new().fill(theme::GAP))
            .show(ui, |ui| self.tree.ui(&mut self.state, ui));
        self.state.export_window(&ctx);
        self.state.bake_window(&ctx);
        self.state.text_window(&ctx);
        self.state.new_project_window(&ctx);
        self.state.discard_window(&ctx);
        self.state.welcome_window(&ctx);
        self.state.settings_window(&ctx);
        let auto = self.state.settings.check_updates;
        self.state.updates.ui(&ctx, auto);
        self.state.recovery_window(&ctx);
        self.state.console_window(&ctx);
        self.state.plugins_window(&ctx);
        let s = &mut self.state;
        if let Some(e) = s.session.tick(
            s.settings.autosave_minutes,
            &s.doc,
            &s.mesh,
            &s.resources,
            s.project_path.as_deref(),
        ) {
            s.error(e);
        }

        // A whole drag or text edit becomes one undo step.
        let interacting =
            ctx.input(|i| i.pointer.any_down()) || ctx.memory(|m| m.focused().is_some());
        self.state.history.observe(&self.state.doc, interacting);
        self.state.script_frame();
        if self.state.script.wants_ticks() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn tile(&self, pane: Pane) -> Option<TileId> {
        self.tree.tiles.find_pane(&pane)
    }

    fn pane_visible(&self, pane: Pane) -> bool {
        self.tile(pane)
            .is_some_and(|id| self.tree.tiles.is_visible(id))
    }

    fn set_pane_visible(&mut self, pane: Pane, visible: bool) {
        let Some(id) = self.tile(pane) else { return };
        self.tree.tiles.set_visible(id, visible);
        if visible
            && let Some(parent) = self.tree.tiles.parent_of(id)
            && let Some(Tile::Container(Container::Tabs(tabs))) = self.tree.tiles.get_mut(parent)
        {
            tabs.set_active(id);
        }
    }

    fn menu_ui(&mut self, ui: &mut egui::Ui) {
        let shortcut = |ui: &mut egui::Ui, label: &str, keys: &str| {
            ui.add(egui::Button::new(label).shortcut_text(keys))
                .clicked()
        };
        ui.menu_button("File", |ui| {
            use crate::keymap::Action as A;
            let keys = |a| self.state.keymap.text(a);
            let (k_new, k_open, k_save, k_save_as, k_export) = (
                keys(A::NewProject),
                keys(A::OpenProject),
                keys(A::Save),
                keys(A::SaveAs),
                keys(A::ExportTextures),
            );
            if shortcut(ui, "New project…", &k_new) {
                self.state.open_new_project();
            }
            if shortcut(ui, "Open project…", &k_open) {
                self.state.pick_project();
            }
            let recent = self.state.settings.recent_projects.clone();
            ui.add_enabled_ui(!recent.is_empty(), |ui| {
                ui.menu_button("Open recent", |ui| {
                    for p in recent {
                        if ui
                            .button(file_name(&p))
                            .on_hover_text(p.display().to_string())
                            .clicked()
                        {
                            self.state.open_any(&p);
                        }
                    }
                });
            });
            if shortcut(ui, "Save", &k_save) {
                self.state.save(false);
            }
            if shortcut(ui, "Save as…", &k_save_as) {
                self.state.save(true);
            }
            if ui
                .button("Save and compact")
                .on_hover_text("Drops images and files no layer uses any more, then saves")
                .clicked()
            {
                self.state.save_compact();
            }
            if ui.button("Save texture set as template…").clicked() {
                self.state.save_template();
            }
            if ui
                .button("Reimport mesh…")
                .on_hover_text("Swap in a changed mesh and keep the paint")
                .clicked()
            {
                self.state.reimport_mesh_dialog();
            }
            ui.separator();
            if shortcut(ui, "Export textures…", &k_export) {
                self.state.export.open = true;
            }
            if ui.button("Bake mesh maps…").clicked() {
                self.state.bake.open = true;
            }
            if ui.button("Load environment…").clicked() {
                self.state.pick_environment();
            }
            if ui.button("Import Substance graph (.sbs)…").clicked() {
                self.state.import_sbs_dialog();
            }
            if ui
                .button("Import Painter project (.spp)…")
                .on_hover_text("Converts a Substance 3D Painter project; the .spp is only read")
                .clicked()
            {
                self.state.import_spp_dialog();
            }
            if ui.button("Import Painter bridge…").clicked() {
                self.state.import_bridge_dialog();
            }
            ui.separator();
            if ui.button("Settings…").clicked() {
                self.state.settings_open = true;
            }
            if ui.button("Quit").clicked() {
                self.state.confirm_discard(Discard::Quit);
            }
        });
        ui.menu_button("Edit", |ui| {
            let s = &mut self.state;
            let undo =
                egui::Button::new("Undo").shortcut_text(s.keymap.text(crate::keymap::Action::Undo));
            if ui.add_enabled(s.history.can_undo(), undo).clicked() {
                s.history.undo(&mut s.doc);
            }
            let redo =
                egui::Button::new("Redo").shortcut_text(s.keymap.text(crate::keymap::Action::Redo));
            if ui.add_enabled(s.history.can_redo(), redo).clicked() {
                s.history.redo(&mut s.doc);
            }
        });
        ui.menu_button("Python", |ui| self.state.python_menu(ui));
        ui.menu_button("Window", |ui| {
            for pane in Pane::ALL {
                let mut visible = self.pane_visible(pane);
                if ui.checkbox(&mut visible, pane_name(pane)).changed() {
                    self.set_pane_visible(pane, visible);
                }
            }
            ui.separator();
            if ui.button("Reset layout").clicked() {
                self.tree = default_layout();
            }
        });
        ui.menu_button("Help", |ui| {
            let repo = env!("CARGO_PKG_REPOSITORY");
            if ui.button("Documentation").clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(
                    "https://redactional.github.io/ReLauncher/docs.html?p=reetched",
                ));
            }
            if ui.button("Report a problem").clicked() {
                ui.ctx()
                    .open_url(egui::OpenUrl::new_tab(format!("{repo}/issues")));
            }
            ui.separator();
            if ui.button("Check for updates").clicked() {
                let ctx = ui.ctx().clone();
                self.state.updates.check(&ctx, true);
            }
            if ui.button("Welcome screen").clicked() {
                self.state.welcome.open = true;
            }
            if ui.button("About ReEtched").clicked() {
                self.state.updates.about_open = true;
            }
        });
    }

    fn toolbar_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (show3d, show2d) = (
                self.pane_visible(Pane::View3d),
                self.pane_visible(Pane::View2d),
            );
            let layouts = [
                (ic::ICON_SPLITSCREEN_LEFT, "3D / 2D view", true, true),
                (ic::ICON_VIEW_IN_AR, "3D view only", true, false),
                (ic::ICON_CROP_SQUARE, "2D view only", false, true),
            ];
            for (icon, tip, want3d, want2d) in layouts {
                let on = show3d == want3d && show2d == want2d;
                if icon_button(ui, icon, on, tip).clicked() {
                    self.set_pane_visible(Pane::View3d, want3d);
                    self.set_pane_visible(Pane::View2d, want2d);
                }
            }
            ui.separator();
            if icon_button(
                ui,
                ic::ICON_CENTER_FOCUS_STRONG,
                false,
                &format!(
                    "Fit view ({})",
                    self.state.keymap.text(crate::keymap::Action::FrameView)
                ),
            )
            .clicked()
            {
                self.state.camera.frame(self.state.bounds);
                self.state.uv_camera.zoom = 0.0;
            }
        });
    }

    fn side_strip_ui(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            let items = [
                (ic::ICON_TUNE, Pane::DisplaySettings, "Display settings"),
                (ic::ICON_ARTICLE, Pane::Log, "Log"),
            ];
            for (icon, pane, tip) in items {
                if icon_button(ui, icon, self.pane_visible(pane), tip).clicked() {
                    self.set_pane_visible(pane, true);
                }
            }
        });
    }

    pub fn render_viewport(
        &mut self,
        egui: &mut egui_wgpu::Renderer,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let (view, proj) = {
            let [w, h] = self.state.viewport.view3d.requested_size;
            let radius = (self.state.bounds.1 - self.state.bounds.0).length() * 0.5;
            self.state
                .camera
                .view_and_proj(w.max(1) as f32 / h.max(1) as f32, radius)
        };
        let s = &mut self.state;
        let warnings = s.viewport.sync(
            &s.device,
            &s.queue,
            &s.doc.texture_sets,
            &s.resources,
            s.preview_map(),
        );
        for w in warnings {
            s.warn(w);
        }
        if s.render.path_trace && s.viewport.trace_needs_mesh() {
            let (device, queue) = (s.device.clone(), s.queue.clone());
            if let Err(e) = s.viewport.set_trace_mesh(&device, &queue, &s.mesh) {
                s.render.path_trace = false;
                s.error(format!("The path tracer can't take this mesh: {e:#}"));
            }
        }
        let params = FrameParams {
            view,
            proj,
            target_distance: s.camera.distance,
            fov_y: if s.camera.orthographic {
                0.0
            } else {
                s.camera.fov_y_deg.to_radians()
            },
            eye: s.camera.eye(),
            exposure_ev: s.exposure_ev,
            env_rotation: s.env_rotation_deg.to_radians(),
            background_blur: s.background_blur,
            env_opacity: s.env_opacity,
            show_wireframe: s.show_wireframe,
            show_uv_wireframe: s.show_uv_wireframe,
            view_mode: s.view_mode,
            uv_mode: s.uv_mode,
            uv_xform: s.uv_camera.xform,
            uv_part: Some(s.selected_part),
            render: s.render,
        };
        s.viewport
            .render(&s.device, &s.queue, egui, encoder, &params);
        s.viewport.end_frame(s.settings.vram_budget());
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn pane_name(pane: Pane) -> String {
    let t = pane.title().trim_end_matches(" - FILL").to_lowercase();
    let mut c = t.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Painter's default arrangement: assets | 3D + 2D views | texture sets, layers, properties.
fn default_layout() -> egui_tiles::Tree<Pane> {
    let mut tiles = Tiles::default();
    let tabs = |tiles: &mut Tiles<Pane>, panes: &[Pane]| {
        let ids = panes.iter().map(|&p| tiles.insert_pane(p)).collect();
        tiles.insert_tab_tile(ids)
    };
    let assets = tabs(&mut tiles, &[Pane::Assets]);
    let view3d = tabs(&mut tiles, &[Pane::View3d]);
    let view2d = tabs(&mut tiles, &[Pane::View2d]);
    let sets = tabs(&mut tiles, &[Pane::TextureSets]);
    let layers = tabs(&mut tiles, &[Pane::Layers, Pane::TextureSetSettings]);
    let props = tabs(
        &mut tiles,
        &[Pane::Properties, Pane::DisplaySettings, Pane::Log],
    );
    let views = tiles.insert_horizontal_tile(vec![view3d, view2d]);
    let right = tiles.insert_vertical_tile(vec![sets, layers, props]);
    let root = tiles.insert_horizontal_tile(vec![assets, views, right]);
    let mut share = |container, shares: &[(TileId, f32)]| {
        if let Some(Tile::Container(Container::Linear(l))) = tiles.get_mut(container) {
            for &(id, s) in shares {
                l.shares.set_share(id, s);
            }
        }
    };
    share(right, &[(sets, 1.0), (layers, 2.0), (props, 2.6)]);
    share(root, &[(assets, 1.2), (views, 4.6), (right, 1.7)]);
    egui_tiles::Tree::new(egui::Id::unique("main_layout"), root, tiles)
}

/// Adds `path` to a list of remembered files, once.
pub(crate) fn remember(list: &mut Vec<PathBuf>, path: &Path) {
    if !list.iter().any(|p| p == path) {
        list.push(path.to_owned());
    }
}

/// Square, frameless icon button that lights up when `selected`.
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: egui_material_icons::MaterialIcon,
    selected: bool,
    tip: &str,
) -> egui::Response {
    let color = if selected { theme::ACCENT } else { theme::TEXT };
    ui.add(
        egui::Button::new(icon.rich_text().size(18.0).color(color))
            .frame(false)
            .min_size(vec2(26.0, 26.0)),
    )
    .on_hover_text(tip)
}

impl State {
    /// Opens a project, mesh, environment, image and so on, by extension.
    /// Anything that replaces the project asks to save changes first.
    pub fn open_any(&mut self, path: &Path) {
        log::debug!("opening {}", path.display());
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ENV_EXTENSIONS.contains(&ext.as_str()) {
            self.load_environment(path);
        } else if crate::images::is_image_path(path) {
            self.add_image_fill(path);
        } else if ext == "glsl" {
            self.load_custom_shader_file(path);
        } else if ext == "ocio" {
            match reetched_color::Config::from_file(path) {
                Ok(c) => {
                    self.color.ocio =
                        Some(crate::color_ui::Ocio::new(c, path.display().to_string()));
                    self.apply_ocio();
                }
                Err(e) => self.error(format!("Could not load {}: {e:#}", path.display())),
            }
        } else if ext == "sbs" {
            self.import_sbs(path);
        } else {
            self.confirm_discard(Discard::Open(path.to_owned()));
        }
    }

    /// Replaces the project with a project, Painter import or mesh.
    fn open_document(&mut self, path: &Path) {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case(project::EXTENSION) {
            self.open_project(path);
        } else if ext.eq_ignore_ascii_case("spp") {
            self.import_spp(path);
        } else if path.is_dir() || file_name(path) == reetched_substance::bridge::MANIFEST {
            self.import_bridge(path);
        } else {
            self.load_mesh(path);
        }
    }

    /// Adds a fill layer with the image file as its base color.
    fn add_image_fill(&mut self, path: &Path) {
        if self.stack().is_none() {
            self.warn("Open a mesh before adding an image".into());
            return;
        }
        let Some(id) = self.import_resource(path) else {
            return;
        };
        let mut fill = reetched_doc::Fill::new([1.0; 3], 0.0, 0.5);
        if let Some(c) = fill.channels.get_mut(&Channel::BaseColor) {
            c.bitmap = Some(id);
        }
        let name = path
            .file_stem()
            .map_or_else(|| "Image".into(), |s| s.to_string_lossy().into_owned());
        let node = self.doc.new_fill(name, fill);
        self.add_node(node);
    }

    /// Runs `action`, first asking to save the project if it has unsaved changes.
    pub fn confirm_discard(&mut self, action: Discard) {
        if self.session.is_dirty(&self.doc, self.history.can_undo()) {
            self.discard = Some(action);
        } else {
            self.run_discard(action);
        }
    }

    fn run_discard(&mut self, action: Discard) {
        match action {
            Discard::Quit => self.quit = true,
            Discard::NewProject(np) => self.new_project = Some(np),
            Discard::Open(path) => self.open_document(&path),
        }
    }

    /// The "Save changes?" dialog, while an action waits for it.
    pub fn discard_window(&mut self, ctx: &egui::Context) {
        if self.discard.is_none() {
            return;
        }
        let (mut save, mut discard, mut cancel) = (false, false, false);
        let name = self.project_name();
        let modal = egui::Modal::new(egui::Id::unique("save_changes")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(RichText::new(format!("Save changes to {name}?")).strong());
            ui.label("Your changes are lost if you don't save them.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                save = ui.button("Save").clicked();
                discard = ui.button("Don't save").clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if cancel || modal.should_close() {
            self.discard = None;
        } else if save {
            self.save(false);
            // Still dirty: the save failed or its file dialog was cancelled.
            if !self.session.is_dirty(&self.doc, self.history.can_undo())
                && let Some(action) = self.discard.take()
            {
                self.run_discard(action);
            }
        } else if discard && let Some(action) = self.discard.take() {
            self.run_discard(action);
        }
    }

    pub fn pick_project(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("ReEtched project", &[project::EXTENSION])
            .pick_file()
        {
            self.open_any(&path);
        }
    }

    pub fn open_project(&mut self, path: &Path) {
        let start = Instant::now();
        let loaded = project::load(path);
        log::debug!("loaded {} in {:.2?}", path.display(), start.elapsed());
        match loaded {
            Ok((doc, mesh, resources)) => {
                let name = file_name(path);
                if doc.texture_sets.len() != mesh.parts.len() {
                    self.error(format!("{name}: texture sets don't match the mesh"));
                    return;
                }
                self.set_mesh(mesh, name.clone(), Some(doc));
                self.resources = resources;
                self.project_path = Some(path.to_owned());
                self.settings.add_recent(path);
                self.session.mark_saved(&self.doc, Some(path));
                self.info(format!("Opened project {name}"));
                self.notify_script("project_opened", serde_json::json!({ "path": path }));
            }
            Err(e) => self.error(format!("Could not open project: {e:#}")),
        }
    }

    /// Saves to the current path, or asks for one (`ask`, or never saved).
    pub fn save(&mut self, ask: bool) {
        let path = match (&self.project_path, ask) {
            (Some(p), false) => p.clone(),
            _ => {
                let default = format!("{}.{}", self.project_name(), project::EXTENSION);
                let Some(p) = rfd::FileDialog::new()
                    .add_filter("ReEtched project", &[project::EXTENSION])
                    .set_file_name(default)
                    .save_file()
                else {
                    return;
                };
                p
            }
        };
        self.save_to(&path);
    }

    /// Saves to `path`; true on success.
    pub fn save_to(&mut self, path: &Path) -> bool {
        self.notify_script("project_about_to_save", serde_json::json!({ "path": path }));
        let start = Instant::now();
        let saved = project::save(path, &self.doc, &self.mesh, &self.resources);
        log::debug!("saved {} in {:.2?}", path.display(), start.elapsed());
        match saved {
            Ok(()) => {
                self.info(format!("Saved {}", path.display()));
                self.project_path = Some(path.to_owned());
                self.settings.add_recent(path);
                self.session.mark_saved(&self.doc, Some(path));
                self.notify_script("project_saved", serde_json::json!({ "path": path }));
                true
            }
            Err(e) => {
                self.error(format!("Could not save: {e:#}"));
                false
            }
        }
    }

    /// Drops resources nothing uses any more, then saves.
    pub fn save_compact(&mut self) {
        let keep: Vec<_> = self.brush.alpha.iter().cloned().collect();
        let dropped = self.doc.compact(&keep);
        self.info(format!("Compacted: {dropped} unused file(s) dropped"));
        self.save(false);
    }

    pub fn open_new_project(&mut self) {
        self.confirm_discard(Discard::NewProject(crate::templates::NewProject {
            mesh: String::new(),
            template: 0,
            resolution: 2048,
            custom: None,
        }));
    }

    pub fn project_name(&self) -> String {
        let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().into_owned());
        self.project_path
            .as_deref()
            .and_then(stem)
            .or_else(|| {
                self.doc
                    .mesh_source
                    .as_deref()
                    .map(Path::new)
                    .and_then(stem)
            })
            .unwrap_or_else(|| "Untitled".into())
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use crate::keymap::Action as A;
        // While Settings waits for a new key, that key is the binding.
        if self.rebinding.is_some() {
            return;
        }
        let typing = ctx.memory(|m| m.focused().is_some());
        let pressed = |a| self.keymap.pressed(ctx, a);
        let (redo, save_as, export, undo, save, open, new, delete) = (
            pressed(A::Redo),
            pressed(A::SaveAs),
            pressed(A::ExportTextures),
            pressed(A::Undo),
            pressed(A::Save),
            pressed(A::OpenProject),
            pressed(A::NewProject),
            !typing && pressed(A::Delete),
        );
        if redo && !typing {
            self.history.redo(&mut self.doc);
        }
        if undo && !typing {
            self.history.undo(&mut self.doc);
        }
        if save_as {
            self.save(true);
        } else if save {
            self.save(false);
        }
        if export {
            self.export.open = true;
        }
        if open {
            self.pick_project();
        }
        if new {
            self.open_new_project();
        }
        if delete {
            self.delete_selected();
        }
    }

    pub fn load_mesh(&mut self, path: &Path) {
        match reetched_io::load_mesh(path) {
            Ok(mut mesh) => {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                if !mesh.has_uvs {
                    match reetched_io::unwrap(&mesh) {
                        Ok(m) => {
                            mesh = m;
                            self.info(format!("{name} had no UVs; generated them"));
                        }
                        Err(e) => self.warn(format!(
                            "{name} has no UVs and unwrapping failed ({e:#}); painting needs UVs"
                        )),
                    }
                }
                self.set_mesh(mesh, name, None);
                self.doc.mesh_source = Some(path.display().to_string());
                self.history = History::new(&self.doc);
                self.project_path = None;
            }
            Err(e) => self.error(format!("Could not open mesh: {e:#}")),
        }
    }

    /// Shows `mesh` with `doc`, or a fresh document (one texture set per material).
    pub fn set_mesh(&mut self, mut mesh: Mesh, name: String, doc: Option<Document>) {
        // Anything opened (project, mesh, import) replaces the welcome screen.
        self.welcome.open = false;
        // UDIM tiles must be contiguous index ranges (projects from before UDIM support).
        mesh.sort_by_udim();
        self.bounds = mesh.bounds();
        self.camera.frame(self.bounds);
        self.uv_camera.zoom = 0.0;
        log::debug!(
            "mesh {name}: {} vertices, {} triangles, {} texture set(s), UVs {}",
            mesh.vertices.len(),
            mesh.indices.len() / 3,
            mesh.parts.len(),
            if mesh.has_uvs { "yes" } else { "generated" }
        );
        self.viewport.set_mesh(&self.device, &mesh);
        self.selected_part = 0;
        self.selected = None;
        self.mesh_label = format!(
            "{name} — {} vertices, {} triangles",
            mesh.vertices.len(),
            mesh.triangle_count()
        );
        self.doc = doc.unwrap_or_else(|| {
            let names: Vec<String> = mesh.parts.iter().map(|p| p.material.clone()).collect();
            Document::new(None, &names)
        });
        // Texture sets whose UVs leave the 0..1 tile become UDIM sets.
        for (k, set) in self.doc.texture_sets.iter_mut().enumerate() {
            let Some(_) = mesh.parts.get(k) else { continue };
            let tiles: Vec<u32> = mesh.udim_tiles(k).into_iter().map(|(t, _)| t).collect();
            if tiles != [reetched_doc::FIRST_UDIM] {
                for t in tiles {
                    if !set.udim.iter().any(|u| u.number == t) {
                        set.udim.push(reetched_doc::UdimTile {
                            number: t,
                            name: String::new(),
                            resolution: None,
                        });
                    }
                }
                set.udim.sort_by_key(|u| u.number);
            }
        }
        self.resources = Resources::default();
        self.history = History::new(&self.doc);
        self.session.mark_new();
        self.stroke = None;
        self.topology = None;
        self.clone_source = None;
        self.clone_offset = None;
        self.path_points.clear();
        self.viewport.clear_paint_cache();
        self.select_top_layer();
        self.export.include = vec![true; self.doc.texture_sets.len()];
        self.mesh = mesh;
        self.info(format!("Loaded {}", self.mesh_label));
    }

    /// The mesh map a view is showing, if any (the 3D view wins).
    pub fn preview_map(&self) -> Option<reetched_doc::MeshMap> {
        [self.view_mode, self.uv_mode]
            .into_iter()
            .find_map(|m| match m {
                ViewMode::Mesh(map) => Some(map),
                _ => None,
            })
    }

    pub fn stack(&self) -> Option<&Vec<Node>> {
        self.doc
            .texture_sets
            .get(self.selected_part)
            .map(|s| &s.stack)
    }

    pub fn stack_mut(&mut self) -> Option<&mut Vec<Node>> {
        self.doc
            .texture_sets
            .get_mut(self.selected_part)
            .map(|s| &mut s.stack)
    }

    /// Inserts a new layer above the selected one (or at the top) and selects it.
    pub fn add_node(&mut self, node: Node) {
        let id = node.id;
        let above = self.selected.map(|s| s.node);
        let Some(stack) = self.stack_mut() else {
            return;
        };
        let leftover = match above {
            Some(target) => insert_node(stack, node, Placement::Above(target)).err(),
            None => Some(node),
        };
        if let Some(node) = leftover {
            stack.insert(0, node);
        }
        self.selected = Some(Selected {
            node: id,
            mask: false,
        });
    }

    pub fn delete_selected(&mut self) {
        let Some(sel) = self.selected else { return };
        let Some(stack) = self.stack_mut() else {
            return;
        };
        if sel.mask {
            if let Some(node) = reetched_doc::find_node_mut(stack, sel.node) {
                node.mask = None;
            }
            self.selected = Some(Selected { mask: false, ..sel });
        } else {
            take_node(stack, sel.node);
            self.selected = None;
        }
    }

    pub fn duplicate_selected(&mut self) {
        let Some(sel) = self.selected else { return };
        let Some(mut copy) = self.stack().and_then(|s| find_node(s, sel.node)).cloned() else {
            return;
        };
        copy.name = format!("{} copy", copy.name);
        self.doc.reassign_ids(&mut copy);
        self.add_node(copy);
    }

    /// Adds an image file to the project; returns its resource id.
    pub fn import_resource(&mut self, path: &Path) -> Option<reetched_doc::ResourceId> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.error(format!("Could not read {}: {e}", path.display()));
                return None;
            }
        };
        if let Err(e) = crate::images::decode(&bytes) {
            self.error(format!("{} isn't a readable image: {e}", file_name(path)));
            return None;
        }
        let id = self.resources.add(bytes);
        self.doc.resources.insert(
            id.clone(),
            reetched_doc::ResourceInfo {
                name: file_name(path),
            },
        );
        Some(id)
    }

    pub fn pick_environment(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("HDR images", ENV_EXTENSIONS)
            .pick_file()
        {
            self.load_environment(&path);
        }
    }

    pub(crate) fn load_environment(&mut self, path: &Path) {
        match EnvImage::load(path) {
            Ok(env) => {
                self.env_path = Some(path.to_owned());
                self.preview_env = PreviewEnv::new(&env);
                self.thumbs.clear(); // regenerated next frame with the new lighting
                self.viewport
                    .set_environment(&self.device, &self.queue, env);
                self.info(format!("Environment: {}", path.display()));
            }
            Err(e) => self.error(format!("Could not open environment: {e:#}")),
        }
    }

    fn refresh_thumbnails(&mut self, ctx: &egui::Context) {
        self.thumbs = PRESETS
            .iter()
            .map(|p| {
                let img = self.preview_env.render(&p.look(), THUMB_SIZE);
                ctx.load_texture(p.name, img, egui::TextureOptions::LINEAR)
            })
            .collect();
        let grey = crate::preview::Look {
            base: [0.2; 3],
            metallic: 0.0,
            roughness: 0.5,
        };
        let looks = crate::smart_presets::BUILTIN
            .iter()
            .map(|p| (p.name.to_string(), &p.look))
            .chain(
                self.smart_imports
                    .iter()
                    .map(|(sm, _)| (sm.name.clone(), &grey)),
            );
        self.smart_thumbs = looks
            .map(|(name, look)| {
                ctx.load_texture(
                    name,
                    self.preview_env.render(look, THUMB_SIZE),
                    egui::TextureOptions::LINEAR,
                )
            })
            .collect();
    }

    /// Adds a fill layer with a material preset to the selected texture set (like dropping
    /// a material onto Painter's layer stack).
    pub fn apply_preset(&mut self, index: usize) {
        if index >= PROC_OFFSET {
            self.apply_procedural(index - PROC_OFFSET);
            return;
        }
        if index >= SMART_OFFSET {
            self.apply_smart(index - SMART_OFFSET);
            return;
        }
        let Some(preset) = PRESETS.get(index) else {
            return;
        };
        let node = self.doc.new_fill(preset.name.to_owned(), preset.fill());
        self.add_node(node);
    }

    /// Adds smart material `k` (built-ins first, then imports) to the selected texture set.
    fn apply_smart(&mut self, k: usize) {
        let builtin = crate::smart_presets::BUILTIN;
        let node = if let Some(p) = builtin.get(k) {
            (p.build)(&mut self.doc)
        } else if let Some((sm, res)) = self.smart_imports.get(k - builtin.len()) {
            sm.instantiate(&mut self.doc, res, &mut self.resources)
        } else {
            return;
        };
        let name = node.name.clone();
        self.add_node(node);
        if !self
            .doc
            .texture_sets
            .get(self.selected_part)
            .is_some_and(|s| !s.mesh_maps.is_empty())
        {
            self.warn(format!(
                "{name}: bake mesh maps so its generators have curvature, AO, … to work with"
            ));
        }
    }

    /// The asset list for the current category: (name, thumbnail, PresetDrag index).
    pub fn asset_items(&self) -> Vec<(String, Option<egui::TextureId>, usize)> {
        match self.asset_category {
            AssetCategory::Materials => PRESETS
                .iter()
                .enumerate()
                .map(|(i, p)| (p.name.to_string(), self.thumbs.get(i).map(|t| t.id()), i))
                .collect(),
            AssetCategory::SmartMaterials => crate::smart_presets::BUILTIN
                .iter()
                .map(|p| p.name.to_string())
                .chain(self.smart_imports.iter().map(|(sm, _)| sm.name.clone()))
                .enumerate()
                .map(|(k, name)| {
                    (
                        name,
                        self.smart_thumbs.get(k).map(|t| t.id()),
                        SMART_OFFSET + k,
                    )
                })
                .collect(),
            AssetCategory::Library(_) => Vec::new(),
            AssetCategory::Procedural => self
                .procedural
                .items()
                .into_iter()
                .enumerate()
                .map(|(k, (label, ..))| {
                    (
                        label,
                        self.procedural.thumbs.get(k).map(|t| t.id()),
                        PROC_OFFSET + k,
                    )
                })
                .collect(),
        }
    }

    pub fn save_smart(&mut self) {
        let Some(sel) = self.selected else { return };
        let Some(node) = self.stack().and_then(|s| find_node(s, sel.node)).cloned() else {
            return;
        };
        let sm = reetched_doc::smart::SmartMaterial::capture(&node, &self.doc);
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Smart material", &[reetched_doc::smart::EXTENSION])
            .set_file_name(format!("{}.{}", node.name, reetched_doc::smart::EXTENSION))
            .save_file()
        else {
            return;
        };
        match reetched_doc::smart::save(&path, &sm, &self.resources) {
            Ok(()) => self.info(format!("Saved smart material {}", path.display())),
            Err(e) => self.error(format!("Could not save the smart material: {e:#}")),
        }
    }

    pub fn import_smart(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("Smart material", &[reetched_doc::smart::EXTENSION])
            .pick_files()
        else {
            return;
        };
        for path in paths {
            self.load_smart_import(&path);
        }
        self.asset_category = AssetCategory::SmartMaterials;
    }

    /// Adds a smart material file to the Assets panel, and remembers it for next time.
    fn load_smart_import(&mut self, path: &Path) -> bool {
        match reetched_doc::smart::load(path) {
            Ok(item) => {
                self.info(format!("Imported smart material {}", item.0.name));
                self.smart_imports.push(item);
                remember(&mut self.settings.imported_smart, path);
                self.thumbs.clear(); // rebuild thumbnails, including the new ones
                true
            }
            Err(e) => {
                self.error(format!("Could not import {}: {e:#}", file_name(path)));
                false
            }
        }
    }

    /// Imports the smart materials and .sbs files of earlier sessions again.
    /// Files that can no longer be read are forgotten.
    fn reload_imports(&mut self) {
        let category = self.asset_category;
        for path in std::mem::take(&mut self.settings.imported_smart) {
            self.load_smart_import(&path);
        }
        for path in std::mem::take(&mut self.settings.imported_sbs) {
            self.import_sbs(&path);
        }
        self.asset_category = category;
    }

    fn push_log(&mut self, text: String, error: bool) {
        let secs = self.started.elapsed().as_secs();
        self.log.push(LogLine {
            time: format!("{:02}:{:02}", secs / 60, secs % 60),
            text,
            error,
        });
    }

    pub fn info(&mut self, text: String) {
        log::info!("{text}");
        self.push_log(text, false);
    }

    pub fn warn(&mut self, text: String) {
        log::warn!("{text}");
        self.push_log(text, false);
    }

    pub fn error(&mut self, text: String) {
        log::error!("{text}");
        self.push_log(text, true);
    }

    fn status_line(&self) -> &str {
        self.log.last().map_or("", |l| l.text.as_str())
    }

    /// GPU memory the rebuildable caches hold, for the status bar.
    fn cache_mb(&mut self) -> u64 {
        self.viewport.compositor().cache_bytes() / (1024 * 1024)
    }

    fn tool_strip_ui(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            for (tool, icon, name) in Tool::ALL {
                if icon_button(ui, icon, self.tool == tool, name).clicked() {
                    self.tool = tool;
                    self.properties_tool = true;
                }
            }
        });
    }
}

impl egui_tiles::Behavior<Pane> for State {
    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        theme::header(pane.title()).into()
    }

    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile: TileId,
        pane: &mut Pane,
    ) -> egui_tiles::UiResponse {
        ui.painter().rect_filled(ui.max_rect(), 0.0, theme::PANEL);
        match pane {
            Pane::View3d => self.view3d_ui(ui),
            Pane::View2d => self.view2d_ui(ui),
            pane => {
                Frame::new().inner_margin(6.0).show(ui, |ui| match pane {
                    Pane::Assets => self.assets_ui(ui),
                    Pane::TextureSets => self.texture_sets_ui(ui),
                    Pane::Layers => self.layers_ui(ui),
                    Pane::TextureSetSettings => self.texture_set_settings_ui(ui),
                    Pane::Properties => self.properties_ui(ui),
                    Pane::DisplaySettings => self.display_settings_ui(ui),
                    Pane::Log => self.log_ui(ui),
                    Pane::View3d | Pane::View2d => {}
                });
            }
        }
        egui_tiles::UiResponse::None
    }

    fn is_tab_closable(&self, _tiles: &Tiles<Pane>, _tile_id: TileId) -> bool {
        true
    }

    /// Closing hides the panel; the Window menu brings it back.
    fn on_tab_close(&mut self, tiles: &mut Tiles<Pane>, tile_id: TileId) -> bool {
        tiles.set_visible(tile_id, false);
        false
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..Default::default()
        }
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        26.0
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        3.0
    }

    fn tab_bar_color(&self, _visuals: &egui::Visuals) -> egui::Color32 {
        theme::HEADER
    }

    fn tab_bg_color(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile_id: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Color32 {
        if state.active {
            theme::PANEL
        } else {
            theme::HEADER
        }
    }

    fn tab_outline_stroke(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile_id: TileId,
        _state: &egui_tiles::TabState,
    ) -> egui::Stroke {
        egui::Stroke::NONE
    }

    fn tab_bar_hline_stroke(&self, _visuals: &egui::Visuals) -> egui::Stroke {
        egui::Stroke::NONE
    }

    fn tab_text_color(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile_id: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Color32 {
        if state.active {
            theme::TEXT
        } else {
            theme::WEAK
        }
    }
}

pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y_deg: f32,
    pub orthographic: bool,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.1,
            distance: 4.0,
            fov_y_deg: 40.0,
            orthographic: false,
        }
    }
}

impl OrbitCamera {
    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cp * sy, sp, cp * cy) * self.distance
    }

    pub fn right_up(&self) -> (Vec3, Vec3) {
        let forward = (self.target - self.eye()).normalize();
        let right = forward.cross(Vec3::Y).normalize();
        (right, right.cross(forward))
    }

    pub fn frame(&mut self, (lo, hi): (Vec3, Vec3)) {
        let radius = ((hi - lo).length() * 0.5).max(1e-4);
        self.target = (lo + hi) * 0.5;
        self.distance = radius / (self.fov_y_deg.to_radians() * 0.5).sin() * 1.1;
    }

    pub fn view_proj(&self, aspect: f32, scene_radius: f32) -> Mat4 {
        let (view, proj) = self.view_and_proj(aspect, scene_radius);
        proj * view
    }

    /// View and projection matrices separately (depth of field shifts the view).
    pub fn view_and_proj(&self, aspect: f32, scene_radius: f32) -> (Mat4, Mat4) {
        let near = (self.distance - scene_radius * 2.0).max(self.distance * 0.001);
        let far = self.distance * 20.0 + scene_radius * 20.0;
        let fov = self.fov_y_deg.to_radians();
        // wgpu clip space: z in [0, 1], y up — glam's "directx" convention.
        use glam::camera::rh::proj::directx;
        let proj = if self.orthographic {
            let h = self.distance * (fov * 0.5).tan();
            directx::orthographic(-h * aspect, h * aspect, -h, h, near, far)
        } else {
            directx::perspective(fov, aspect, near, far)
        };
        let view = glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y);
        (view, proj)
    }
}

/// Pan/zoom for the 2D UV view.
#[derive(Default)]
pub struct UvCamera {
    /// UV coordinate at the center of the view.
    pub center: Vec2,
    /// Points per UV unit; 0 means "frame the 0–1 tile on the next draw".
    pub zoom: f32,
    /// Shader transform: ndc = uv * xy + zw.
    pub xform: [f32; 4],
}

impl UvCamera {
    pub fn update(&mut self, size: egui::Vec2) {
        if self.zoom <= 0.0 {
            self.center = Vec2::splat(0.5);
            self.zoom = size.x.min(size.y) * 0.85;
        }
        let (sx, sy) = (2.0 * self.zoom / size.x, 2.0 * self.zoom / size.y);
        self.xform = [sx, sy, -self.center.x * sx, -self.center.y * sy];
    }

    pub fn to_screen(&self, rect: egui::Rect, uv: Vec2) -> egui::Pos2 {
        let d = (uv - self.center) * self.zoom;
        rect.center() + vec2(d.x, -d.y)
    }
}

/// Drives the whole app headlessly with simulated input. Skipped without a GPU.
#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use reetched_doc::NodeKind;

    struct Harness {
        device: wgpu::Device,
        queue: wgpu::Queue,
        ctx: egui::Context,
        renderer: egui_wgpu::Renderer,
        app: App,
        _turn: std::sync::MutexGuard<'static, ()>,
    }

    impl Harness {
        fn new() -> Option<Self> {
            let turn = crate::gpu_test_lock();
            let (adapter, device, queue) = crate::test_gpu()?;
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let mut renderer = egui_wgpu::Renderer::new(
                &device,
                wgpu::TextureFormat::Rgba8Unorm,
                Default::default(),
            );
            let app = App::new(
                &device,
                &queue,
                &mut renderer,
                &adapter.get_info(),
                false,
                None,
            );
            Some(Self {
                device,
                queue,
                ctx,
                renderer,
                app,
                _turn: turn,
            })
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(1600.0, 900.0),
                )),
                events,
                ..Default::default()
            };
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(input, |ui| app.ui(ui));
            for (id, deltas) in out.textures_delta.set.drain() {
                for d in &deltas {
                    self.renderer
                        .update_texture(&self.device, &self.queue, id, d);
                }
            }
            out.textures_delta.clear();
            let mut enc = self.device.create_command_encoder(&Default::default());
            self.app.render_viewport(&mut self.renderer, &mut enc);
            self.queue.submit([enc.finish()]);
        }

        fn base_color_tiles(&self) -> usize {
            let node = &self.app.state.doc.texture_sets[0].stack[0];
            match &node.kind {
                NodeKind::Paint(p) => p
                    .channels
                    .get(&Channel::BaseColor)
                    .map_or(0, |m| m.tiles.len()),
                _ => panic!("expected the default paint layer"),
            }
        }

        fn stroke(&mut self, from: egui::Pos2, to: egui::Pos2) {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            self.frame(vec![egui::Event::PointerMoved(from)]);
            self.frame(vec![button(from, true)]);
            for i in 1..=5 {
                self.frame(vec![egui::Event::PointerMoved(
                    from.lerp(to, i as f32 / 5.0),
                )]);
            }
            self.frame(vec![button(to, false)]);
            self.frame(vec![]);
        }

        fn click(&mut self, at: egui::Pos2) {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            self.frame(vec![egui::Event::PointerMoved(at)]);
            self.frame(vec![button(true)]);
            self.frame(vec![button(false)]);
            self.frame(vec![]);
        }

        fn key(&mut self, key: egui::Key) {
            let ev = |pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Default::default(),
            };
            self.frame(vec![ev(true)]);
            self.frame(vec![ev(false)]);
        }
    }

    #[test]
    fn welcome_screen_closes_on_escape_and_when_something_opens() {
        let Some(mut h) = Harness::new() else { return };
        h.app.state.welcome.open = true;
        h.frame(vec![]);
        assert!(h.app.state.welcome.open);
        h.key(egui::Key::Escape);
        assert!(!h.app.state.welcome.open, "Esc closes it");
        h.app.state.welcome.open = true;
        h.frame(vec![]);
        let sphere = reetched_io::Mesh::uv_sphere(1.0, 8, 4);
        h.app.state.set_mesh(sphere, "Sphere".into(), None);
        assert!(!h.app.state.welcome.open, "opening a mesh replaces it");
    }

    #[test]
    fn layout_round_trips_through_settings() {
        let tree = default_layout();
        let json = serde_json::to_value(&tree).unwrap();
        let back: egui_tiles::Tree<Pane> = serde_json::from_value(json).unwrap();
        assert_eq!(back.tiles.len(), tree.tiles.len());
    }

    #[test]
    fn dropped_image_adds_a_fill_and_unsaved_work_asks_before_quitting() {
        let Some(mut h) = Harness::new() else { return };
        h.frame(vec![]);
        h.app.request_quit();
        assert!(h.app.wants_quit(), "nothing to save: quits at once");
        h.app.state.quit = false;

        let png = std::env::temp_dir().join("reetched-drop-test.png");
        image::RgbaImage::new(2, 2).save(&png).unwrap();
        let layers = |h: &Harness| h.app.state.stack().map_or(0, |s| s.len());
        let before = layers(&h);
        h.app.state.open_any(&png);
        assert_eq!(layers(&h), before + 1, "an image becomes a fill layer");
        h.frame(vec![]);

        h.app.request_quit();
        assert!(!h.app.wants_quit(), "unsaved changes: asks first");
        assert!(h.app.state.discard.is_some());
        h.app.state.discard = None; // Cancel
        h.app.state.open_new_project();
        assert!(
            h.app.state.new_project.is_none(),
            "asks before a new project too"
        );
        let _ = std::fs::remove_file(png);
    }

    #[test]
    fn painting_on_the_sphere_creates_tiles_and_undoes() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        for _ in 0..3 {
            h.frame(vec![]); // let the layout settle
        }
        let rect = h.app.state.view3d_rect;
        assert!(rect.width() > 100.0, "3D view laid out: {rect:?}");
        assert!(h.app.state.selected.is_some(), "Layer 1 is selected");
        assert_eq!(h.base_color_tiles(), 0);

        h.stroke(
            rect.center() - vec2(20.0, 0.0),
            rect.center() + vec2(20.0, 10.0),
        );
        assert!(h.base_color_tiles() > 0, "the stroke painted Layer 1");
        assert!(h.app.state.history.can_undo());

        h.app.state.history.undo(&mut h.app.state.doc);
        h.frame(vec![]);
        assert_eq!(h.base_color_tiles(), 0, "undo removes the stroke");

        h.app.state.history.redo(&mut h.app.state.doc);
        h.frame(vec![]);
        let painted = h.base_color_tiles();
        assert!(painted > 0);

        // A big eraser stroke over the same spot clears it again.
        h.app.state.tool = Tool::Eraser;
        h.app.state.brush.size = 150.0;
        h.stroke(
            rect.center() - vec2(40.0, 0.0),
            rect.center() + vec2(40.0, 0.0),
        );
        assert!(h.base_color_tiles() < painted || h.base_color_tiles() == 0);
    }

    /// Unit cube faces: (normal, u axis, v axis), with u × v = normal.
    const FACES: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];

    /// A cube in [-1, 1]³ whose faces are `n`×`n` grids; `round` rounds the edges with that radius.
    /// UVs: each face gets a cell of a 3×2 atlas (inset a little).
    fn cube(n: u32, round: f32) -> Mesh {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for (f, (normal, u, v)) in FACES.iter().enumerate() {
            let (normal, u, v) = (Vec3::from(*normal), Vec3::from(*u), Vec3::from(*v));
            let cell = Vec2::new((f % 3) as f32 / 3.0, (f / 3) as f32 / 2.0);
            let base = vertices.len() as u32;
            for j in 0..=n {
                for i in 0..=n {
                    let (a, b) = (
                        i as f32 / n as f32 * 2.0 - 1.0,
                        j as f32 / n as f32 * 2.0 - 1.0,
                    );
                    let p = normal + u * a + v * b;
                    let (pos, nrm) = if round > 0.0 {
                        let q = p.clamp(Vec3::splat(-(1.0 - round)), Vec3::splat(1.0 - round));
                        let dir = (p - q).normalize();
                        (q + dir * round, dir)
                    } else {
                        (p, normal)
                    };
                    let uv = cell
                        + Vec2::new(
                            (a + 1.0) * 0.5 * 0.3 + 0.0167,
                            (b + 1.0) * 0.5 * 0.45 + 0.025,
                        ) * Vec2::new(1.0, 1.0);
                    vertices.push(reetched_io::Vertex {
                        position: pos.into(),
                        normal: nrm.into(),
                        uv: uv.into(),
                        tangent: [u.x, u.y, u.z, 1.0],
                        uv1: [0.0; 2],
                    });
                }
            }
            for j in 0..n {
                for i in 0..n {
                    let k = base + j * (n + 1) + i;
                    indices.extend([k, k + 1, k + n + 2, k, k + n + 2, k + n + 1]);
                }
            }
        }
        let count = indices.len() as u32;
        Mesh {
            vertices,
            indices,
            parts: vec![reetched_io::MeshPart {
                material: "cube".into(),
                indices: 0..count,
            }],
            has_uvs: true,
        }
    }

    #[test]
    fn baked_edge_wear_shows_steel_on_bevelled_edges() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let (low, high) = (cube(1, 0.0), cube(48, 0.15));
        h.app.state.set_mesh(low.clone(), "cube".into(), None);
        h.app.state.doc.texture_sets[0].resolution = 256;
        let settings = reetched_bake::Settings {
            resolution: 256,
            max_frontal: 0.05,
            max_rear: 0.05,
            ao_rays: 16,
            maps: vec![
                reetched_bake::MapKind::Normal,
                reetched_bake::MapKind::Curvature,
                reetched_bake::MapKind::AmbientOcclusion,
            ],
            ..Default::default()
        };
        let maps = reetched_bake::bake(&low, 0, 1001, Some(&high), &settings, &Default::default())
            .unwrap();
        for (kind, img) in maps {
            let id = h
                .app
                .state
                .resources
                .add(crate::bake_ui::encode_png16(&img, kind).unwrap());
            h.app.state.doc.texture_sets[0]
                .mesh_maps
                .insert(crate::bake_ui::mesh_map(kind), id);
        }
        h.app.state.apply_preset(SMART_OFFSET); // Painted Steel Worn
        h.frame(vec![]);
        let d = h
            .app
            .state
            .viewport
            .read_back(&h.device, &h.queue, 0, reetched_doc::FIRST_UDIM)
            .unwrap();
        // Walk across the middle of face 0's cell: painted in the middle, steel at the bevels.
        let row = (256.0 * (1.0 - (0.025 + 0.225))) as u32;
        let (x0, x1) = (
            (256.0 * 0.0167) as u32 + 1,
            (256.0 * (0.0167 + 0.3)) as u32 - 1,
        );
        let metal = |x: u32| d.metallic[(row * 256 + x) as usize];
        let mid = (x0 + x1) / 2;
        assert!(
            metal(mid) < 0.1,
            "face centre keeps its paint: {}",
            metal(mid)
        );
        let edge = (x0..x0 + 6)
            .chain(x1 - 6..=x1)
            .filter(|&x| metal(x) > 0.5)
            .count();
        assert!(
            edge >= 4,
            "steel shows through near the bevelled edges ({edge} texels)"
        );
    }

    /// Frames until procedural renders finish.
    fn settle(h: &mut Harness) {
        for _ in 0..500 {
            h.frame(vec![]);
            if !h.app.state.procedural.busy() {
                h.frame(vec![]); // composite with the finished render
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("procedural render did not finish");
    }

    #[test]
    fn procedural_bricks_render_into_the_layer_stack() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        h.app.state.doc.texture_sets[0].resolution = 256;
        let bricks = h
            .app
            .state
            .procedural
            .items()
            .iter()
            .position(|(_, _, g)| g == "bricks")
            .unwrap();
        h.app.state.apply_preset(PROC_OFFSET + bricks);
        settle(&mut h);
        let read = |h: &Harness| {
            h.app
                .state
                .viewport
                .read_back(&h.device, &h.queue, 0, reetched_doc::FIRST_UDIM)
                .unwrap()
        };
        let d = read(&h);
        let covered: Vec<[f32; 3]> = d
            .base_color
            .iter()
            .zip(&d.coverage)
            .filter(|(_, c)| **c)
            .map(|(b, _)| *b)
            .collect();
        assert!(covered.len() > 1000);
        let reddish = covered.iter().filter(|c| c[0] > c[2] * 2.0).count();
        let greyish = covered
            .iter()
            .filter(|c| c[0] < c[2] * 1.6 && c[0] > 0.2)
            .count();
        assert!(
            reddish > covered.len() / 2,
            "mostly brick ({reddish}/{})",
            covered.len()
        );
        assert!(
            greyish > covered.len() / 50,
            "with mortar lines ({greyish})"
        );

        // Changing a parameter re-renders: blue bricks.
        let Some(Selected { node, .. }) = h.app.state.selected else {
            panic!("the new layer is selected")
        };
        let stack = &mut h.app.state.doc.texture_sets[0].stack;
        let NodeKind::Fill(fill) = &mut reetched_doc::find_node_mut(stack, node).unwrap().kind
        else {
            panic!()
        };
        fill.procedural
            .as_mut()
            .unwrap()
            .params
            .insert("brick".into(), [0.1, 0.2, 0.8, 0.0]);
        settle(&mut h);
        let d = read(&h);
        let blue = d
            .base_color
            .iter()
            .zip(&d.coverage)
            .filter(|(b, c)| **c && b[2] > b[0] * 2.0)
            .count();
        assert!(blue > covered.len() / 2, "bricks turned blue ({blue})");
        assert!(
            h.app
                .state
                .procedural
                .warnings
                .values()
                .all(|w| w.is_empty())
        );
    }

    #[test]
    fn imported_sbs_renders_and_survives_save_and_reload() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        h.app.state.doc.texture_sets[0].resolution = 64;
        let sbs =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../reetched-substance/tests/data/main.sbs");
        h.app.state.import_sbs(&sbs);
        assert_eq!(h.app.state.procedural.imports.len(), 1);
        let main = h
            .app
            .state
            .procedural
            .items()
            .iter()
            .position(|(_, _, g)| g == "main")
            .unwrap();
        h.app.state.apply_preset(PROC_OFFSET + main);
        settle(&mut h);
        let warnings = |h: &Harness| -> Vec<String> {
            h.app
                .state
                .procedural
                .warnings
                .values()
                .flatten()
                .cloned()
                .collect()
        };
        let w = warnings(&h);
        assert!(w.iter().any(|w| w.contains("pattern_missing")), "{w:?}");

        // The .sbs and its dependency are stored in the project.
        let names: Vec<&str> = h
            .app
            .state
            .doc
            .resources
            .values()
            .map(|r| r.name.as_str())
            .collect();
        assert!(
            names.contains(&"main.sbs") && names.contains(&"lib/shapes.sbs"),
            "{names:?}"
        );

        let dir = std::env::temp_dir().join("reetched-sbs-project-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.retx");
        h.app.state.project_path = Some(path.clone());
        h.app.state.save(false);
        // Forget loaded packages and renders, so they come back from the file.
        h.app.state.procedural = Default::default();
        h.app
            .state
            .viewport
            .compositor()
            .retain_procedural(&HashSet::new());
        h.app.state.open_project(&path);
        settle(&mut h);
        let set = &h.app.state.doc.texture_sets[0];
        let NodeKind::Fill(fill) = &set.stack[0].kind else {
            panic!("the fill layer came back")
        };
        let key = fill
            .procedural
            .as_ref()
            .unwrap()
            .render_key(set.resolution, &set.mesh_maps);
        assert!(h.app.state.viewport.compositor().has_procedural(&key));
        let w = warnings(&h);
        assert_eq!(w.len(), 1, "only the missing library package: {w:?}");
    }

    #[test]
    fn painter_bridge_bundle_imports_and_checks_against_painter() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../reetched-substance/tests/data/bridge");
        let checks = h.app.state.import_bridge(&dir);
        let s = &h.app.state;
        assert_eq!(s.doc.texture_sets[0].stack.len(), 3);
        assert_eq!(checks.len(), 7);
        for c in &checks {
            eprintln!(
                "{} {:?}: mean {:.4}, off {:.3}",
                c.set, c.channel, c.mean, c.off
            );
        }
        // The fake Painter in the plugin test blends like ours for these channels.
        for ch in [
            Channel::Metallic,
            Channel::Roughness,
            Channel::Emissive,
            Channel::Opacity,
        ] {
            let c = checks.iter().find(|c| c.channel == ch).unwrap();
            assert!(c.passed(), "{ch:?}: mean {}, off {}", c.mean, c.off);
        }
        h.frame(vec![]);
    }

    #[test]
    fn polygon_fill_and_path_tools_paint_through_the_ui() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        for _ in 0..3 {
            h.frame(vec![]);
        }
        let rect = h.app.state.view3d_rect;
        h.app.state.doc.texture_sets[0].resolution = 512;
        h.frame(vec![]);

        // Polygon Fill, whole mesh: the sphere is one connected mesh, so every tile gets paint.
        h.app.state.tool = Tool::PolygonFill;
        h.app.state.polygon_mode = reetched_io::topology::Selection::Mesh;
        h.click(rect.center());
        assert_eq!(h.base_color_tiles(), 4, "all of a 512² texture");
        h.app.state.history.undo(&mut h.app.state.doc);
        h.frame(vec![]);
        assert_eq!(h.base_color_tiles(), 0);

        // Path: two clicks, then Enter paints along them.
        h.app.state.tool = Tool::Path;
        h.click(rect.center() - vec2(30.0, 0.0));
        h.click(rect.center() + vec2(30.0, 10.0));
        assert_eq!(h.app.state.path_points.len(), 2);
        h.key(egui::Key::Enter);
        assert!(h.base_color_tiles() > 0, "the path painted");
        assert!(h.app.state.path_points.is_empty());
    }

    #[test]
    fn reimporting_with_new_uvs_keeps_the_paint() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        for _ in 0..3 {
            h.frame(vec![]);
        }
        h.app.state.doc.texture_sets[0].resolution = 256;
        h.frame(vec![]);
        let rect = h.app.state.view3d_rect;
        h.stroke(
            rect.center() - vec2(20.0, 0.0),
            rect.center() + vec2(20.0, 0.0),
        );
        let painted = |h: &Harness| -> Vec<(u32, u32)> {
            let s = &h.app.state;
            let NodeKind::Paint(p) = &s.doc.texture_sets[0].stack[0].kind else {
                panic!("paint layer")
            };
            let map = &p.channels[&Channel::BaseColor];
            (0..256 * 256)
                .map(|i| (i % 256, i / 256))
                .filter(|&(x, y)| map.texel(&s.resources, x, y)[3] > 0.99)
                .collect()
        };
        let before = painted(&h);
        assert!(before.len() > 50, "the stroke painted texels");

        // Same sphere, UVs mirrored left-right.
        let mut mirrored = h.app.state.mesh.clone();
        mirrored
            .vertices
            .iter_mut()
            .for_each(|v| v.uv[0] = 1.0 - v.uv[0]);
        h.app.state.reimport_with(mirrored, "mirrored".into());
        h.frame(vec![]);
        let after = painted(&h);
        let moved = before
            .iter()
            .filter(|&&(x, y)| after.contains(&(255 - x, y)))
            .count();
        assert!(
            moved * 10 >= before.len() * 8,
            "{moved} of {} painted texels found mirrored",
            before.len()
        );
    }

    #[test]
    fn emissive_glows_and_cutout_hides_in_the_3d_view() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let center = |h: &Harness| {
            let ([w, ht], px) = h.app.state.viewport.read_display(&h.device, &h.queue);
            let i = ((ht / 2 * w + w / 2) * 4) as usize;
            [px[i], px[i + 1], px[i + 2]]
        };
        let set_fill = |h: &mut Harness, emissive: [f32; 3], opacity: f32| {
            let mut fill = reetched_doc::Fill::new([0.0; 3], 0.0, 0.9);
            for (c, v) in [
                (Channel::Emissive, emissive),
                (Channel::Opacity, [opacity; 3]),
            ] {
                let src = fill.channels.get_mut(&c).unwrap();
                src.enabled = true;
                src.value = v;
            }
            let node = h.app.state.doc.new_fill("test".into(), fill);
            h.app.state.doc.texture_sets[0].stack = vec![node];
            for _ in 0..4 {
                h.frame(vec![]);
            }
        };
        for _ in 0..3 {
            h.frame(vec![]);
        }
        // A black, glowing red sphere.
        set_fill(&mut h, [1.0, 0.0, 0.0], 1.0);
        let c = center(&h);
        assert!(c[0] > 150 && c[1] < 60, "emissive red: {c:?}");

        // Fully transparent with alpha cutout: the background shows (#262626).
        h.app.state.render.alpha = crate::viewport::AlphaMode::Cutout;
        set_fill(&mut h, [1.0, 0.0, 0.0], 0.0);
        let c = center(&h);
        assert!(
            c.iter().all(|&v| (0x20..=0x2c).contains(&v)),
            "background: {c:?}"
        );
    }

    #[test]
    fn exports_usd_and_obj_with_their_textures() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        h.app.state.doc.texture_sets[0].resolution = 64;
        h.frame(vec![]);
        let dir = std::env::temp_dir().join("reetched-mesh-export-app");
        let _ = std::fs::remove_dir_all(&dir);
        h.app.state.export.dir = dir.display().to_string();
        h.app.state.export.displacement = 0.1;
        for f in crate::stack_ui::MeshFormat::ALL {
            h.app.state.export.mesh_format = f;
            h.app.state.export_mesh();
        }
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let has = |suffix: &str| names.iter().any(|n| n.ends_with(suffix));
        for suffix in [
            ".usda",
            ".obj",
            ".mtl",
            "_BaseColor.png",
            "_Normal.png",
            "_Emissive.png",
        ] {
            assert!(has(suffix), "missing *{suffix} in {names:?}");
        }
        let usda = names.iter().find(|n| n.ends_with(".usda")).unwrap();
        let text = std::fs::read_to_string(dir.join(usda)).unwrap();
        let base = names
            .iter()
            .find(|n| n.ends_with("_BaseColor.png"))
            .unwrap();
        assert!(text.contains(&format!("@{base}@")), "USD references {base}");
    }

    #[test]
    fn layered_psd_keeps_layers_folders_and_masks() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let s = &mut h.app.state;
        s.doc.texture_sets[0].resolution = 64;
        // Folder [ Red fill (multiply 50%, black mask) ], over a grey fill.
        let mut red = s.doc.new_fill(
            "Red".into(),
            reetched_doc::Fill::new([1.0, 0.0, 0.0], 0.0, 0.5),
        );
        red.blend.insert(
            Channel::BaseColor,
            reetched_doc::ChannelBlend {
                mode: reetched_doc::BlendMode::Multiply,
                opacity: 0.5,
            },
        );
        red.mask = Some(reetched_doc::Mask::new(reetched_doc::MaskBackground::Black));
        let mut folder = s.doc.new_folder("Group".into());
        if let NodeKind::Folder { children } = &mut folder.kind {
            children.push(red);
        }
        let grey = s
            .doc
            .new_fill("Grey".into(), reetched_doc::Fill::new([0.5; 3], 0.0, 0.5));
        s.doc.texture_sets[0].stack = vec![folder, grey];
        let dir = std::env::temp_dir().join("reetched-psd-app");
        let _ = std::fs::remove_dir_all(&dir);
        s.export.dir = dir.display().to_string();
        s.export.include = vec![true];
        h.frame(vec![]);
        h.app.state.export_psd();

        let file = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.to_string_lossy().ends_with("_BaseColor.psd"))
            .expect("a base color PSD");
        let psd = psd::Psd::from_bytes(&std::fs::read(file).unwrap()).unwrap();
        assert_eq!((psd.width(), psd.height()), (64, 64));
        assert_eq!(psd.groups().len(), 1);
        let red = psd.layer_by_name("Red").unwrap();
        assert_eq!(format!("{:?}", red.blend_mode()), "Multiply");
        assert_eq!(red.opacity(), 128);
        let mid = ((32 * 64 + 32) * 4) as usize;
        assert_eq!(&red.rgba()[mid..mid + 4], &[255, 0, 0, 255]);
        let grey = psd.layer_by_name("Grey").unwrap();
        assert!(
            (grey.rgba()[mid] as i32 - 188).abs() <= 2,
            "linear 0.5 → sRGB 188"
        );
        // Several channels were written: base color, metallic, roughness.
        let n = std::fs::read_dir(&dir).unwrap().count();
        assert!(n >= 3, "{n} files");
    }

    #[test]
    fn projection_tool_and_marquee_fill_paint_the_layer() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        for _ in 0..3 {
            h.frame(vec![]);
        }
        h.app.state.doc.texture_sets[0].resolution = 256;
        h.frame(vec![]);
        let rect = h.app.state.view3d_rect;
        let base = |h: &Harness| -> Vec<[f32; 4]> {
            let s = &h.app.state;
            let NodeKind::Paint(p) = &s.doc.texture_sets[0].stack[0].kind else {
                panic!("paint layer")
            };
            p.channels.get(&Channel::BaseColor).map_or(Vec::new(), |m| {
                (0..256 * 256)
                    .map(|k| m.texel(&s.resources, k % 256, k / 256))
                    .filter(|t| t[3] > 0.99)
                    .collect()
            })
        };

        // Projection: a big solid green image; the stroke paints green, whatever the brush color.
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 255, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let s = &mut h.app.state;
        s.stencil.image = Some(s.resources.add(png));
        s.stencil.size = 3.0;
        s.tool = Tool::Projection;
        h.stroke(
            rect.center() - vec2(20.0, 0.0),
            rect.center() + vec2(20.0, 0.0),
        );
        let painted = base(&h);
        assert!(painted.len() > 50, "projected texels: {}", painted.len());
        assert!(
            painted.iter().all(|t| t[1] > 0.9 && t[0] < 0.05),
            "green, not the brush color: {:?}",
            painted[0]
        );
        h.app.state.history.undo(&mut h.app.state.doc);
        h.frame(vec![]);

        // Polygon Fill marquee over the middle of the view.
        h.app.state.tool = Tool::PolygonFill;
        h.stroke(
            rect.center() - vec2(60.0, 60.0),
            rect.center() + vec2(60.0, 60.0),
        );
        let filled = base(&h).len();
        assert!(
            (200..20_000).contains(&filled),
            "the rectangle fills some front faces, not the whole texture: {filled}"
        );
    }

    /// Two quads side by side facing +z: x in -1..0 uses UDIM tile 1001, x in 0..1 tile 1002.
    fn two_tiles() -> Mesh {
        let v = |x: f32, y: f32| reetched_io::Vertex {
            position: [x, y, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [x + 1.0, (y + 1.0) * 0.5],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv1: [0.0; 2],
        };
        let vertices = vec![
            v(-1.0, -1.0),
            v(0.0, -1.0),
            v(1.0, -1.0),
            v(-1.0, 1.0),
            v(0.0, 1.0),
            v(1.0, 1.0),
        ];
        let indices = vec![0, 1, 4, 0, 4, 3, 1, 2, 5, 1, 5, 4];
        let mut mesh = Mesh {
            vertices,
            indices,
            parts: vec![reetched_io::MeshPart {
                material: "panel".into(),
                indices: 0..12,
            }],
            has_uvs: true,
        };
        mesh.sort_by_udim();
        mesh
    }

    fn center_pixel(h: &Harness) -> [u8; 3] {
        let ([w, ht], px) = h.app.state.viewport.read_display(&h.device, &h.queue);
        let i = ((ht / 2 * w + w / 2) * 4) as usize;
        [px[i], px[i + 1], px[i + 2]]
    }

    #[test]
    fn udim_sets_paint_and_export_per_tile() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        h.app.state.set_mesh(two_tiles(), "panel".into(), None);
        let s = &mut h.app.state;
        let tiles: Vec<u32> = s.doc.texture_sets[0]
            .udim
            .iter()
            .map(|t| t.number)
            .collect();
        assert_eq!(tiles, [1001, 1002]);
        assert_eq!(s.viewport.tiles(0), [1001, 1002]);
        s.doc.texture_sets[0].resolution = 256;
        for _ in 0..3 {
            h.frame(vec![]);
        }
        // Polygon Fill on the whole mesh paints both tiles.
        let rect = h.app.state.view3d_rect;
        h.app.state.tool = Tool::PolygonFill;
        h.app.state.polygon_mode = reetched_io::topology::Selection::Mesh;
        h.click(rect.center());
        let s = &h.app.state;
        let NodeKind::Paint(p) = &s.doc.texture_sets[0].stack[0].kind else {
            panic!("paint layer")
        };
        assert!(
            p.map(1001, Channel::BaseColor)
                .is_some_and(|m| !m.tiles.is_empty())
        );
        assert!(
            p.map(1002, Channel::BaseColor)
                .is_some_and(|m| !m.tiles.is_empty())
        );
        // Each tile composites on its own.
        for tile in [1001, 1002] {
            let d = s.viewport.read_back(&h.device, &h.queue, 0, tile).unwrap();
            assert!(
                d.coverage.iter().filter(|c| **c).count() > 1000,
                "tile {tile}"
            );
        }

        // Textures come out per tile; the mesh references them through <UDIM>.
        let dir = std::env::temp_dir().join("reetched-udim-export");
        let _ = std::fs::remove_dir_all(&dir);
        h.app.state.export.dir = dir.display().to_string();
        h.app.state.export.include = vec![true];
        h.app.state.export.mesh_format = crate::stack_ui::MeshFormat::Usda;
        h.app.state.export_mesh();
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        for want in ["_BaseColor.1001.png", "_BaseColor.1002.png"] {
            assert!(
                names.iter().any(|n| n.ends_with(want)),
                "{want} in {names:?}"
            );
        }
        let usda = names.iter().find(|n| n.ends_with(".usda")).unwrap();
        let text = std::fs::read_to_string(dir.join(usda)).unwrap();
        assert!(text.contains("_BaseColor.<UDIM>.png"), "{text}");

        // Baking runs per tile.
        let settings = reetched_bake::Settings {
            resolution: 64,
            maps: vec![reetched_bake::MapKind::Position],
            ..Default::default()
        };
        let mesh = h.app.state.mesh.clone();
        let maps =
            reetched_bake::bake(&mesh, 0, 1002, None, &settings, &Default::default()).unwrap();
        let (_, img) = &maps[0];
        // Tile 1002 holds the x > 0 half: its baked positions lean to +x.
        let mid = &img.pixels[(32 * 64 + 32) as usize];
        assert!(mid[0] > 0.5, "position map of the right half: {mid:?}");
    }

    #[test]
    fn shader_models_ocio_and_painter_shaders_render() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        h.app.state.doc.texture_sets[0].resolution = 128;
        let mut fill = reetched_doc::Fill::new([0.05, 0.05, 0.05], 0.0, 0.9);
        for (c, v) in [
            (Channel::Coat, [1.0; 3]),
            (Channel::CoatRoughness, [0.02; 3]),
        ] {
            let src = fill.channels.get_mut(&c).unwrap();
            src.enabled = true;
            src.value = v;
        }
        let node = h.app.state.doc.new_fill("coated".into(), fill);
        h.app.state.doc.texture_sets[0].stack = vec![node];
        h.app.state.render.accumulate = false;
        for _ in 0..4 {
            h.frame(vec![]);
        }
        let plain = center_pixel(&h);

        // ASM with its channels: the coat adds a sharp reflection on the dark rough base.
        let set = &mut h.app.state.doc.texture_sets[0];
        set.shader = reetched_doc::ShaderModel::Asm;
        set.extra_channels = reetched_doc::ShaderModel::Asm.channels().to_vec();
        for _ in 0..3 {
            h.frame(vec![]);
        }
        let coated = center_pixel(&h);
        let sum = |p: [u8; 3]| p.iter().map(|&v| v as u32).sum::<u32>();
        assert!(
            sum(coated) > sum(plain) + 10,
            "coat: {plain:?} → {coated:?}"
        );
        // OpenPBR shades too.
        h.app.state.doc.texture_sets[0].shader = reetched_doc::ShaderModel::OpenPbr;
        h.frame(vec![]);
        h.frame(vec![]);

        // An OCIO display transform (ACES 2.0 SDR) is baked into the LUT.
        let (name, _) = reetched_color::builtin_configs()
            .iter()
            .find(|(n, _)| n.contains("aces-v2.0"))
            .unwrap();
        let config = reetched_color::Config::builtin(name).unwrap();
        h.app.state.color.ocio = Some(crate::color_ui::Ocio::new(
            config,
            format!("builtin:{name}"),
        ));
        h.app.state.apply_ocio();
        assert_eq!(h.app.state.render.tone_map, crate::viewport::ToneMap::Ocio);
        h.frame(vec![]);
        h.frame(vec![]);
        let aces2 = center_pixel(&h);
        assert!(sum(aces2) > 0, "the display transform shows something");

        // A Painter shader: the toon example from the translator's tests.
        let toon = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../reetched-shader/tests/shaders/toon_unlit.glsl");
        h.app.state.load_custom_shader_file(&toon);
        for _ in 0..3 {
            h.frame(vec![]);
        }
        let reetched_doc::ShaderModel::Custom { shader, .. } =
            h.app.state.doc.texture_sets[0].shader.clone()
        else {
            panic!("custom shader set")
        };
        match h.app.state.viewport.custom_shader(&shader) {
            Some(Ok(c)) => assert!(c.shader.params.iter().any(|p| p.name == "bands")),
            Some(Err(e)) => panic!("toon shader: {e}"),
            None => panic!("not compiled"),
        }
        let toon_px = center_pixel(&h);
        assert_ne!(toon_px, aces2, "the custom shader draws the mesh");
    }

    #[test]
    fn transparency_scattering_and_depth_of_field_render() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let set_fill = |h: &mut Harness, opacity: f32, scatter: f32| {
            let mut fill = reetched_doc::Fill::new([0.9, 0.2, 0.1], 0.0, 0.5);
            for (c, v) in [
                (Channel::Opacity, [opacity; 3]),
                (Channel::Scattering, [scatter; 3]),
            ] {
                let src = fill.channels.get_mut(&c).unwrap();
                src.enabled = true;
                src.value = v;
            }
            let node = h.app.state.doc.new_fill("test".into(), fill);
            let set = &mut h.app.state.doc.texture_sets[0];
            set.resolution = 128;
            set.stack = vec![node];
            set.shader = reetched_doc::ShaderModel::Asm;
            set.extra_channels = vec![Channel::Scattering, Channel::ScatteringColor];
            for _ in 0..4 {
                h.frame(vec![]);
            }
        };
        h.app.state.render.accumulate = false;
        set_fill(&mut h, 1.0, 0.0);
        let opaque = center_pixel(&h);
        // Half transparent, blended: between the surface and the background.
        h.app.state.render.alpha = crate::viewport::AlphaMode::Blend;
        set_fill(&mut h, 0.5, 0.0);
        let half = center_pixel(&h);
        assert!(
            half[0] < opaque[0] && half[0] > 0x26,
            "blended: {opaque:?} → {half:?}"
        );
        // Scattering on and off light the surface about the same overall.
        h.app.state.render.alpha = crate::viewport::AlphaMode::Opaque;
        set_fill(&mut h, 1.0, 1.0);
        let with_sss = center_pixel(&h);
        assert!(
            with_sss[0] > 0x40,
            "scattered light reaches the screen: {with_sss:?}"
        );
        // Depth of field accumulates lens samples.
        h.app.state.render.accumulate = true;
        h.app.state.render.dof = true;
        h.app.state.render.aperture = 0.1;
        for _ in 0..8 {
            h.frame(vec![]);
        }
        assert!(h.app.state.viewport.accumulating(&h.app.state.render));
    }

    #[test]
    fn loop_ribbon_filled_path_dynamics_and_text() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        h.app.state.set_mesh(cube(4, 0.0), "cube".into(), None);
        h.app.state.doc.texture_sets[0].resolution = 256;
        h.app.state.camera.yaw = 0.0;
        h.app.state.camera.pitch = 0.0;
        for _ in 0..3 {
            h.frame(vec![]);
        }
        let rect = h.app.state.view3d_rect;
        let painted = |h: &Harness| -> usize {
            let s = &h.app.state;
            let NodeKind::Paint(p) = &s.doc.texture_sets[0].stack[0].kind else {
                panic!("paint layer")
            };
            p.channels.get(&Channel::BaseColor).map_or(0, |m| {
                (0..256 * 256)
                    .filter(|k| m.texel(&s.resources, k % 256, k / 256)[3] > 0.5)
                    .count()
            })
        };
        let undo = |h: &mut Harness| {
            h.app.state.history.undo(&mut h.app.state.doc);
            h.frame(vec![]);
        };

        // Loop: a ring of quads around the cube, many times one triangle.
        h.app.state.tool = Tool::PolygonFill;
        h.app.state.polygon_mode = reetched_io::topology::Selection::Triangle;
        h.click(rect.center() + vec2(5.0, 5.0));
        let one = painted(&h);
        assert!(one > 0);
        undo(&mut h);
        h.app.state.polygon_mode = reetched_io::topology::Selection::Loop;
        h.click(rect.center() + vec2(5.0, 5.0));
        let ring = painted(&h);
        // 4 quads per face × 4 faces = 32 triangles.
        assert!(ring >= one * 24, "loop {ring} vs triangle {one}");
        undo(&mut h);

        // Ribbon, filled and smudge paths.
        h.app.state.tool = Tool::Path;
        for mode in [
            crate::tools_ui::PathMode::Ribbon,
            crate::tools_ui::PathMode::Fill,
        ] {
            h.app.state.path_mode = mode;
            for d in [vec2(-60.0, -40.0), vec2(60.0, -40.0), vec2(0.0, 60.0)] {
                h.click(rect.center() + d);
            }
            h.key(egui::Key::Enter);
            assert!(painted(&h) > 0, "{mode:?} painted");
            undo(&mut h);
        }

        // A dynamic stroke scatters dabs around the stroke.
        h.app.state.tool = Tool::Paint;
        h.app.state.brush.dynamics = Some(crate::dynamics::Dynamics::presets()[0].clone());
        h.stroke(
            rect.center() - vec2(30.0, 0.0),
            rect.center() + vec2(30.0, 0.0),
        );
        assert!(painted(&h) > 0, "dynamic stroke painted");

        // Text as a fill bitmap.
        let svg = crate::images::text_svg(&crate::images::TextStyle {
            text: "Hi".into(),
            family: "Ubuntu".into(),
            ..Default::default()
        })
        .unwrap();
        let s = &mut h.app.state;
        let node = s
            .doc
            .new_fill("text".into(), reetched_doc::Fill::new([0.0; 3], 0.0, 0.5));
        let id = node.id;
        s.doc.texture_sets[0].stack.insert(0, node);
        let rid = s.resources.add(svg.into_bytes());
        s.doc.resources.insert(
            rid.clone(),
            reetched_doc::ResourceInfo {
                name: "Text.svg".into(),
            },
        );
        let ctx = h.ctx.clone();
        h.app.state.use_resource(
            crate::text_ui::TextTarget::Fill(id, Channel::BaseColor),
            rid,
            &ctx,
        );
        h.frame(vec![]);
        let d = h
            .app
            .state
            .viewport
            .read_back(&h.device, &h.queue, 0, reetched_doc::FIRST_UDIM)
            .unwrap();
        let white = d
            .base_color
            .iter()
            .zip(&d.coverage)
            .filter(|(c, cov)| **cov && c[0] > 0.8)
            .count();
        assert!(white > 50, "the text shows up white: {white}");
    }

    #[test]
    fn library_assets_are_found_and_used() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let dir = crate::settings::config_dir().join("library");
        let _ = std::fs::remove_dir_all(dir.join("textures"));
        let _ = std::fs::remove_dir_all(dir.join("alphas"));
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        std::fs::create_dir_all(dir.join("alphas")).unwrap();
        let png = |path: &Path, c: [u8; 4]| {
            image::RgbaImage::from_pixel(4, 4, image::Rgba(c))
                .save(path)
                .unwrap();
        };
        png(&dir.join("textures/rusty_plate.png"), [200, 40, 20, 255]);
        png(&dir.join("alphas/dots.png"), [255, 255, 255, 255]);
        let s = &mut h.app.state;
        s.library.lib.rescan();
        s.library.poll();
        let found: Vec<_> = s
            .library
            .lib
            .query("rusty", Some(reetched_library::AssetKind::Texture), false)
            .into_iter()
            .cloned()
            .collect();
        assert_eq!(found.len(), 1, "the texture is found by name");
        let alpha = s
            .library
            .lib
            .query("", Some(reetched_library::AssetKind::Alpha), false)
            .into_iter()
            .next()
            .cloned()
            .expect("files in alphas/ are alphas");

        // Double-clicking a texture puts it on the selected fill's shown channel.
        let fill = s
            .doc
            .new_fill("f".into(), reetched_doc::Fill::new([1.0; 3], 0.0, 0.5));
        let id = fill.id;
        s.doc.texture_sets[0].stack.insert(0, fill);
        s.selected = Some(Selected {
            node: id,
            mask: false,
        });
        s.use_asset(&found[0]);
        let NodeKind::Fill(f) = &s.doc.texture_sets[0].stack[0].kind else {
            panic!("fill")
        };
        assert!(f.channels[&Channel::BaseColor].bitmap.is_some());
        s.use_asset(&alpha);
        assert!(s.brush.alpha.is_some());

        // Brush presets keep the settings and the alpha image.
        s.brush.size = 77.0;
        let preset = crate::library_ui::BrushPreset {
            brush: s.brush.clone(),
            alpha: Some("89504e47".into()),
        };
        let path = std::env::temp_dir().join("reetched-test.rbrush");
        std::fs::write(&path, serde_json::to_vec(&preset).unwrap()).unwrap();
        s.brush = crate::brush::Brush::default();
        s.load_brush_preset(&path);
        assert_eq!(s.brush.size, 77.0);
        let a = s.brush.alpha.clone().unwrap();
        assert_eq!(s.resources.files[&a].as_slice(), &[0x89, 0x50, 0x4e, 0x47]);
    }

    #[test]
    fn python_scripts_drive_the_app() {
        if let Err(e) = reetched_script::find_python() {
            eprintln!("no Python; skipping: {e}");
            return;
        }
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        // Build the bridge into its own target dir (ours is locked while tests run).
        let exe = std::env::current_exe().unwrap();
        let target = exe
            .ancestors()
            .nth(3)
            .unwrap()
            .join("reetched-python-tests");
        let status = std::process::Command::new(env!("CARGO"))
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(["build", "-p", "reetched-python", "--target-dir"])
            .arg(&target)
            .status()
            .unwrap();
        assert!(status.success(), "building reetched-python failed");
        let lib = target.join("debug").join(reetched_script::library_name());
        // SAFETY: no other test reads this, and Python isn't running yet.
        unsafe { std::env::set_var("REETCHED_PYTHON_LIB", lib) };
        let s = &mut h.app.state;
        s.start_scripting();
        assert!(s.script.wants_ticks(), "{}", s.script.status);
        let out = std::env::temp_dir().join("reetched-script-export");
        let _ = std::fs::remove_dir_all(&out);
        s.run_console(
            "from reetched import layerstack as ls, textureset as ts, export, event\n\
             seen = []\n\
             event.DISPATCHER.connect(event.Event, lambda e: seen.append(e.name))\n",
        );
        s.run_console(&format!(
            r#"s = ts.get_active()
fill = ls.insert_fill(ls.InsertPosition.from_texture_set_stack(s))
fill.name = "Rust"
fill.set_channel_value("basecolor", (1.0, 0.0, 0.0))
group = ls.insert_group(ls.InsertPosition.above_node(fill))
ls.move_node(fill, ls.InsertPosition.inside_node(group))
assert ls.get_root_layer_nodes(s)[0] == group
assert group.children()[0].channel_value("basecolor") == (1.0, 0.0, 0.0)
try:
    ls.move_node(group, ls.InsertPosition.inside_node(group))
    raise SystemExit("moving a group into itself must fail")
except Exception as e:
    print("refused:", e)
r = export.export_project_textures({{"exportPath": r"{}", "exportParameters": {{"fileFormat": "png"}}}})
assert r.all_files(), r
"#,
            out.display()
        ));
        s.run_console("print('events', seen)");
        let console = s.script.output.clone();
        assert!(!console.contains("Traceback"), "{console}");
        assert!(console.contains("refused:"), "{console}");
        assert!(console.contains("export_finished"), "{console}");
        let top = &s.doc.texture_sets[0].stack[0];
        let NodeKind::Folder { children } = &top.kind else {
            panic!("the group is on top");
        };
        assert_eq!(children[0].name, "Rust");
        assert!(
            std::fs::read_dir(&out).unwrap().count() > 0,
            "textures written"
        );
        // Each call was its own undo step: undo takes the move back out of the group.
        s.history.undo(&mut s.doc);
        assert!(matches!(
            &s.doc.texture_sets[0].stack[1].kind,
            NodeKind::Fill(_)
        ));
    }

    #[test]
    fn path_tracer_renders_the_material_view() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let s = &mut h.app.state;
        let node = s.doc.new_fill(
            "red".into(),
            reetched_doc::Fill::new([0.8, 0.05, 0.05], 0.0, 0.5),
        );
        s.add_node(node);
        for _ in 0..3 {
            h.frame(vec![]);
        }
        let raster = center_pixel(&h);
        h.app.state.render.path_trace = true;
        for _ in 0..24 {
            h.frame(vec![]);
        }
        let s = &h.app.state;
        assert!(
            s.viewport.trace_samples() >= 16,
            "{}",
            s.viewport.trace_samples()
        );
        assert!(
            s.viewport.accumulating(&s.render),
            "keeps refining up to the sample count"
        );
        let traced = center_pixel(&h);
        assert!(
            traced[0] > traced[1].saturating_add(40) && traced[0] > traced[2].saturating_add(40),
            "the red sphere shows: traced {traced:?}, raster {raster:?}"
        );
        // Same material, so roughly the same brightness as the rasterizer.
        let diff = (traced[0] as i32 - raster[0] as i32).abs();
        assert!(diff < 70, "traced {traced:?} vs raster {raster:?}");
    }

    /// Converts the user's own Painter projects (folder in `REETCHED_SPP_DIR`;
    /// never part of the repository) and compares them with Painter's output.
    #[test]
    fn painter_projects_convert_and_compare() {
        let Some(dir) = std::env::var_os("REETCHED_SPP_DIR") else {
            eprintln!("REETCHED_SPP_DIR not set; skipping");
            return;
        };
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "spp"))
            .filter(|p| p.metadata().is_ok_and(|m| m.len() < 400_000_000))
            .collect();
        files.sort();
        for path in files {
            let checks = h.app.state.import_spp(&path);
            // With Painter's result layer on top, the composite is Painter's.
            let refs = reetched_spp::convert::convert(&path)
                .unwrap()
                .imported
                .references;
            for c in h.app.state.verify(&refs) {
                assert!(
                    c.mean < 0.02,
                    "{}: with the Painter result layer, {} differs by {}",
                    path.display(),
                    c.channel.label(),
                    c.mean
                );
            }
            let s = &h.app.state;
            for l in s.log.iter().filter(|l| l.text.contains("troke")) {
                eprintln!("    log: {}", l.text);
            }
            assert!(!s.doc.texture_sets.is_empty(), "{}", path.display());
            eprintln!("{}", path.file_name().unwrap().to_string_lossy());
            // Side-by-side images (ours | Painter's) for looking at differences.
            // (ours without Painter's result layer | Painter's)
            if let Some(out) = std::env::var_os("REETCHED_SPP_OUT") {
                for set in &mut h.app.state.doc.texture_sets {
                    for n in &mut set.stack {
                        if n.name == reetched_spp::convert::RESULT_LAYER {
                            n.visible = false;
                        }
                    }
                }
                h.app.state.verify(&refs);
                let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
                for r in refs.iter().filter(|r| {
                    matches!(
                        r.channel,
                        Channel::BaseColor | Channel::Normal | Channel::Roughness
                    )
                }) {
                    let s = &h.app.state;
                    let data = s
                        .viewport
                        .read_back(&h.device, &h.queue, r.set, reetched_doc::FIRST_UDIM)
                        .unwrap();
                    if (data.width, data.height) != (r.width, r.height) {
                        continue;
                    }
                    let (w, ht) = (r.width, r.height);
                    let ours = |i: usize| -> [f32; 3] {
                        match r.channel {
                            Channel::BaseColor => {
                                data.base_color[i].map(reetched_doc::export::linear_to_srgb)
                            }
                            Channel::Normal => data.normal[i].map(|v| v * 0.5 + 0.5),
                            _ => [data.roughness[i]; 3],
                        }
                    };
                    let img = image::RgbImage::from_fn(w * 2, ht, |x, y| {
                        let i = (y * w + (x % w)) as usize;
                        let v = if x < w { ours(i) } else { r.pixels[i] };
                        image::Rgb(v.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8))
                    });
                    let file = format!("{stem}_{}_{:?}.png", r.set, r.channel);
                    img.save(std::path::Path::new(&out).join(file)).unwrap();
                }
            }
            for c in &checks {
                eprintln!(
                    "    {} / {}: mean {:.3}, {:.1}% off{}",
                    c.set,
                    c.channel.label(),
                    c.mean,
                    c.off * 100.0,
                    if c.passed() { "  MATCH" } else { "" }
                );
            }
        }
    }
}
