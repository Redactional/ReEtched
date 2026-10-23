//! Editor shell: menus, toolbars and the dockable panel layout (modelled on Substance Painter).
//! The panels' contents live in `panels.rs`.

use crate::{
    env::EnvImage,
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

pub const MESH_EXTENSIONS: &[&str] = &["fbx", "obj", "gltf", "glb"];
pub const ENV_EXTENSIONS: &[&str] = &["hdr", "exr"];
const THUMB_SIZE: usize = 96;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
            Pane::Properties => "PROPERTIES - FILL",
            Pane::DisplaySettings => "DISPLAY SETTINGS",
            Pane::Log => "LOG",
        }
    }
}

/// Painting tools (shown now, working from Phase 2).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Paint,
    Eraser,
    Projection,
    PolygonFill,
    Smudge,
    Clone,
    Picker,
    Path,
}

impl Tool {
    const ALL: [(Tool, egui_material_icons::MaterialIcon, &'static str); 8] = [
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
        (Tool::Picker, ic::ICON_COLORIZE, "Material picker (P)"),
        (Tool::Path, ic::ICON_POLYLINE, "Path"),
    ];
}

/// Payload when dragging a material preset out of the Assets panel.
pub struct PresetDrag(pub usize);

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
    /// Set by the export window; handled after it draws (file dialogs block).
    pub pending_preset_import: bool,
    pub pending_preset_save: bool,
    pub exposure_ev: f32,
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
    pub asset_search: String,
    pub selected_preset: Option<usize>,
    pub preview_env: PreviewEnv,
    pub thumbs: Vec<egui::TextureHandle>,
    pub log: Vec<LogLine>,
    started: Instant,
    adapter: String,
    quit: bool,
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
            export: ExportUi {
                open: false,
                dir: String::new(),
                presets: builtin_presets(),
                preset: 0,
                settings: ExportSettings::default(),
                include: Vec::new(),
            },
            exposure_ev: 0.0,
            env_rotation_deg: 0.0,
            background_blur: 0.6,
            env_opacity: 0.0,
            show_wireframe: false,
            show_uv_wireframe: true,
            view_mode: ViewMode::Material,
            uv_mode: ViewMode::Material,
            layer_channel: Channel::BaseColor,
            tool: Tool::Paint,
            asset_search: String::new(),
            selected_preset: None,
            thumbs: Vec::new(),
            log: Vec::new(),
            started: Instant::now(),
            adapter: format!("{} ({:?})", adapter.name, adapter.backend),
            quit: false,
        };
        state.set_mesh(Mesh::uv_sphere(1.0, 64, 32), "Preview sphere".into(), None);
        if let Some(path) = startup_mesh {
            state.open_any(&path);
        }
        Self {
            tree: default_layout(),
            state,
        }
    }

    pub fn wants_quit(&self) -> bool {
        self.state.quit
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.state.thumbs.is_empty() {
            self.state.refresh_thumbnails(&ctx);
        }
        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_owned())
                .collect()
        });
        for path in dropped {
            self.state.open_any(&path);
        }
        self.state.shortcuts(&ctx);
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
                                "{}  |  Version: {}",
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

        // A whole drag or text edit becomes one undo step.
        let interacting =
            ctx.input(|i| i.pointer.any_down()) || ctx.memory(|m| m.focused().is_some());
        self.state.history.observe(&self.state.doc, interacting);
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
            if shortcut(ui, "New project from mesh…", "Ctrl+N") {
                self.state.pick_mesh();
            }
            if shortcut(ui, "Open project…", "Ctrl+O") {
                self.state.pick_project();
            }
            if shortcut(ui, "Save", "Ctrl+S") {
                self.state.save(false);
            }
            if shortcut(ui, "Save as…", "Ctrl+Shift+S") {
                self.state.save(true);
            }
            ui.separator();
            if shortcut(ui, "Export textures…", "Ctrl+Shift+E") {
                self.state.export.open = true;
            }
            if ui.button("Load environment…").clicked() {
                self.state.pick_environment();
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                self.state.quit = true;
            }
        });
        ui.menu_button("Edit", |ui| {
            let s = &mut self.state;
            let undo = egui::Button::new("Undo").shortcut_text("Ctrl+Z");
            if ui.add_enabled(s.history.can_undo(), undo).clicked() {
                s.history.undo(&mut s.doc);
            }
            let redo = egui::Button::new("Redo").shortcut_text("Ctrl+Y");
            if ui.add_enabled(s.history.can_redo(), redo).clicked() {
                s.history.redo(&mut s.doc);
            }
        });
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
            if icon_button(ui, ic::ICON_CENTER_FOCUS_STRONG, false, "Fit view (F)").clicked() {
                self.state.camera.frame(self.state.bounds);
                self.state.uv_camera.zoom = 0.0;
            }
            ui.add_enabled_ui(false, |ui| {
                icon_button(ui, ic::ICON_FLIP, false, "Symmetry")
                    .on_disabled_hover_text("Symmetry — arrives with painting in Phase 2");
            });
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
        let s = &mut self.state;
        let warnings = s
            .viewport
            .sync(&s.device, &s.queue, &s.doc.texture_sets, &s.resources);
        for w in warnings {
            s.warn(w);
        }
        let [w, h] = s.viewport.view3d.requested_size;
        let aspect = w.max(1) as f32 / h.max(1) as f32;
        let radius = (s.bounds.1 - s.bounds.0).length() * 0.5;
        let params = FrameParams {
            view_proj: s.camera.view_proj(aspect, radius),
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
        };
        s.viewport
            .render(&s.device, &s.queue, egui, encoder, &params);
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
    /// Opens a project, mesh or environment, by extension.
    pub fn open_any(&mut self, path: &Path) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ENV_EXTENSIONS.contains(&ext.as_str()) {
            self.load_environment(path);
        } else if ext == project::EXTENSION {
            self.open_project(path);
        } else {
            self.load_mesh(path);
        }
    }

    pub fn pick_mesh(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Meshes", MESH_EXTENSIONS)
            .pick_file()
        {
            self.load_mesh(&path);
        }
    }

    pub fn pick_project(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("ReEtched project", &[project::EXTENSION])
            .pick_file()
        {
            self.open_project(&path);
        }
    }

    pub fn open_project(&mut self, path: &Path) {
        match project::load(path) {
            Ok((doc, mesh, resources)) => {
                let name = file_name(path);
                if doc.texture_sets.len() != mesh.parts.len() {
                    self.error(format!("{name}: texture sets don't match the mesh"));
                    return;
                }
                self.set_mesh(mesh, name.clone(), Some(doc));
                self.resources = resources;
                self.project_path = Some(path.to_owned());
                self.info(format!("Opened project {name}"));
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
        match project::save(&path, &self.doc, &self.mesh, &self.resources) {
            Ok(()) => {
                self.info(format!("Saved {}", path.display()));
                self.project_path = Some(path);
            }
            Err(e) => self.error(format!("Could not save: {e:#}")),
        }
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
        use egui::{Key, KeyboardShortcut, Modifiers};
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let pressed = |m, k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        // Shifted versions first so Ctrl+S doesn't swallow Ctrl+Shift+S.
        let redo = pressed(cmd_shift, Key::Z) || pressed(cmd, Key::Y);
        let save_as = pressed(cmd_shift, Key::S);
        let export = pressed(cmd_shift, Key::E);
        let undo = pressed(cmd, Key::Z);
        let save = pressed(cmd, Key::S);
        let open = pressed(cmd, Key::O);
        let new = pressed(cmd, Key::N);
        let typing = ctx.memory(|m| m.focused().is_some());
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
            self.pick_mesh();
        }
        if !typing && ctx.input(|i| i.key_pressed(Key::Delete)) {
            self.delete_selected();
        }
    }

    pub fn load_mesh(&mut self, path: &Path) {
        match reetched_io::load_mesh(path) {
            Ok(mesh) => {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                if !mesh.has_uvs {
                    self.warn(format!(
                        "{name} has no UVs; painting will need UVs (auto-unwrap arrives in Phase 6)"
                    ));
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
    fn set_mesh(&mut self, mesh: Mesh, name: String, doc: Option<Document>) {
        self.bounds = mesh.bounds();
        self.camera.frame(self.bounds);
        self.uv_camera.zoom = 0.0;
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
        self.resources = Resources::default();
        self.history = History::new(&self.doc);
        self.export.include = vec![true; self.doc.texture_sets.len()];
        self.mesh = mesh;
        self.info(format!("Loaded {}", self.mesh_label));
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
        if let Err(e) = image::load_from_memory(&bytes) {
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

    fn load_environment(&mut self, path: &Path) {
        match EnvImage::load(path) {
            Ok(env) => {
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
    }

    /// Adds a fill layer with a material preset to the selected texture set (like dropping
    /// a material onto Painter's layer stack).
    pub fn apply_preset(&mut self, index: usize) {
        let Some(preset) = PRESETS.get(index) else {
            return;
        };
        let node = self.doc.new_fill(preset.name.to_owned(), preset.fill());
        self.add_node(node);
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

    fn tool_strip_ui(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            for (tool, icon, name) in Tool::ALL {
                let tip = format!("{name} — arrives with painting in Phase 2");
                if icon_button(ui, icon, self.tool == tool, &tip).clicked() {
                    self.tool = tool;
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

    fn view_proj(&self, aspect: f32, scene_radius: f32) -> Mat4 {
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
        proj * glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y)
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
