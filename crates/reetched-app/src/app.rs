//! Editor UI: docked panels around the 3D viewport.

use crate::{
    env::EnvImage,
    viewport::{FrameParams, Viewport},
};
use egui::{Color32, PointerButton, Rect, Sense, pos2};
use glam::{Mat4, Vec3};
use reetched_io::Mesh;
use std::path::{Path, PathBuf};

const MESH_EXTENSIONS: &[&str] = &["fbx", "obj", "gltf", "glb"];
const ENV_EXTENSIONS: &[&str] = &["hdr", "exr"];

pub struct App {
    tree: egui_tiles::Tree<Pane>,
    state: State,
}

enum Pane {
    Viewport,
    Assets,
    TextureSets,
    LayerStack,
    Properties,
}

struct State {
    device: wgpu::Device,
    queue: wgpu::Queue,
    viewport: Viewport,
    camera: OrbitCamera,
    bounds: (Vec3, Vec3),
    mesh_label: String,
    selected_part: usize,
    exposure_ev: f32,
    env_rotation_deg: f32,
    background_blur: f32,
    show_grid: bool,
    show_wireframe: bool,
    status: String,
    adapter: String,
    quit: bool,
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
        let mut state = State {
            device: device.clone(),
            queue: queue.clone(),
            viewport: Viewport::new(device, queue, egui, wireframe),
            camera: OrbitCamera::default(),
            bounds: (Vec3::splat(-1.0), Vec3::ONE),
            mesh_label: String::new(),
            selected_part: 0,
            exposure_ev: 0.0,
            env_rotation_deg: 0.0,
            background_blur: 0.6,
            show_grid: true,
            show_wireframe: false,
            status: String::new(),
            adapter: format!("{} ({:?})", adapter.name, adapter.backend),
            quit: false,
        };
        state.set_mesh(Mesh::uv_sphere(1.0, 64, 32), "Preview sphere".into());
        if let Some(path) = startup_mesh {
            state.load_mesh(&path);
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
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| self.state.menu_ui(ui));
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.state.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(&self.state.adapter)
                });
            });
        });
        egui::CentralPanel::no_frame().show(ui, |ui| self.tree.ui(&mut self.state, ui));
    }

    pub fn render_viewport(
        &mut self,
        egui: &mut egui_wgpu::Renderer,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let s = &mut self.state;
        let [w, h] = s.viewport.requested_size;
        let aspect = w.max(1) as f32 / h.max(1) as f32;
        let radius = (s.bounds.1 - s.bounds.0).length() * 0.5;
        let params = FrameParams {
            view_proj: s.camera.view_proj(aspect, radius),
            eye: s.camera.eye(),
            exposure_ev: s.exposure_ev,
            env_rotation: s.env_rotation_deg.to_radians(),
            background_blur: s.background_blur,
            show_grid: s.show_grid,
            show_wireframe: s.show_wireframe,
            grid_height: s.bounds.0.y,
            grid_cell: 10f32.powf(radius.max(1e-4).log10().floor()) * 0.1,
            grid_extent: radius * 10.0 + s.camera.distance * 3.0,
        };
        s.viewport
            .render(&s.device, &s.queue, egui, encoder, &params);
    }
}

fn default_layout() -> egui_tiles::Tree<Pane> {
    let mut tiles = egui_tiles::Tiles::default();
    let mut pane = |p| tiles.insert_pane(p);
    let (viewport, assets) = (pane(Pane::Viewport), pane(Pane::Assets));
    let (sets, layers, props) = (
        pane(Pane::TextureSets),
        pane(Pane::LayerStack),
        pane(Pane::Properties),
    );
    let left = tiles.insert_vertical_tile(vec![viewport, assets]);
    let right = tiles.insert_vertical_tile(vec![sets, layers, props]);
    let root = tiles.insert_horizontal_tile(vec![left, right]);
    let mut share = |container, shares: &[(egui_tiles::TileId, f32)]| {
        if let Some(egui_tiles::Tile::Container(egui_tiles::Container::Linear(l))) =
            tiles.get_mut(container)
        {
            for &(id, s) in shares {
                l.shares.set_share(id, s);
            }
        }
    };
    share(left, &[(viewport, 4.0), (assets, 1.0)]);
    share(right, &[(sets, 1.0), (layers, 2.0), (props, 2.0)]);
    share(root, &[(left, 3.0), (right, 1.0)]);
    egui_tiles::Tree::new(egui::Id::unique("main_layout"), root, tiles)
}

impl State {
    fn open_any(&mut self, path: &Path) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ENV_EXTENSIONS.contains(&ext.as_str()) {
            self.load_environment(path);
        } else {
            self.load_mesh(path);
        }
    }

    fn load_mesh(&mut self, path: &Path) {
        match reetched_io::load_mesh(path) {
            Ok(mesh) => {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                if !mesh.has_uvs {
                    log::warn!(
                        "{name} has no UVs; painting will need UVs (auto-unwrap arrives in Phase 6)"
                    );
                }
                self.set_mesh(mesh, name);
            }
            Err(e) => self.error(format!("Could not open mesh: {e:#}")),
        }
    }

    fn set_mesh(&mut self, mesh: Mesh, name: String) {
        self.bounds = mesh.bounds();
        self.camera.frame(self.bounds);
        self.viewport.set_mesh(&self.device, &mesh);
        self.selected_part = 0;
        self.mesh_label = format!(
            "{name} — {} vertices, {} triangles",
            mesh.vertices.len(),
            mesh.triangle_count()
        );
        self.status = self.mesh_label.clone();
        log::info!("loaded {}", self.mesh_label);
    }

    fn load_environment(&mut self, path: &Path) {
        match EnvImage::load(path) {
            Ok(env) => {
                self.viewport
                    .set_environment(&self.device, &self.queue, env);
                self.status = format!("Environment: {}", path.display());
            }
            Err(e) => self.error(format!("Could not open environment: {e:#}")),
        }
    }

    fn error(&mut self, msg: String) {
        log::error!("{msg}");
        self.status = msg;
    }

    fn menu_ui(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("File", |ui| {
            if ui.button("Open mesh…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Meshes", MESH_EXTENSIONS)
                    .pick_file()
            {
                self.load_mesh(&path);
            }
            if ui.button("Load environment…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("HDR images", ENV_EXTENSIONS)
                    .pick_file()
            {
                self.load_environment(&path);
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                self.quit = true;
            }
        });
        ui.menu_button("View", |ui| self.display_ui(ui));
    }

    fn display_ui(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.show_grid, "Grid");
        ui.add_enabled(
            self.viewport.wireframe_supported,
            egui::Checkbox::new(&mut self.show_wireframe, "Wireframe"),
        )
        .on_disabled_hover_text("This GPU doesn't support line rasterization");
        ui.add(egui::Slider::new(&mut self.camera.fov_y_deg, 10.0..=90.0).text("Field of view"));
    }

    fn viewport_ui(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let ppp = ui.pixels_per_point();
        self.viewport.requested_size = [
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        ];
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        ui.painter().image(
            self.viewport.texture_id,
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );

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
    }

    fn texture_sets_ui(&mut self, ui: &mut egui::Ui) {
        ui.weak(&self.mesh_label);
        ui.separator();
        for (i, part) in self.viewport.parts.iter().enumerate() {
            let label = format!("{}  ({} tris)", part.name, part.triangles);
            if ui
                .selectable_label(self.selected_part == i, label)
                .clicked()
            {
                self.selected_part = i;
            }
        }
    }

    fn properties_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            if let Some(part) = self.viewport.parts.get_mut(self.selected_part) {
                ui.heading(&part.name);
                ui.weak("Flat material (the layer stack replaces this in Phase 1)");
                egui::Grid::new("material").num_columns(2).show(ui, |ui| {
                    let m = &mut part.material;
                    ui.label("Base color");
                    egui::color_picker::color_edit_button_rgba(
                        ui,
                        &mut m.base_color,
                        egui::color_picker::Alpha::Opaque,
                    );
                    ui.end_row();
                    ui.label("Roughness");
                    ui.add(egui::Slider::new(&mut m.roughness, 0.0..=1.0));
                    ui.end_row();
                    ui.label("Metallic");
                    ui.add(egui::Slider::new(&mut m.metallic, 0.0..=1.0));
                    ui.end_row();
                });
                ui.separator();
            }
            ui.heading("Environment");
            if ui.button("Load HDRI…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("HDR images", ENV_EXTENSIONS)
                    .pick_file()
            {
                self.load_environment(&path);
            }
            egui::Grid::new("environment")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Exposure (EV)");
                    ui.add(egui::Slider::new(&mut self.exposure_ev, -6.0..=6.0));
                    ui.end_row();
                    ui.label("Rotation");
                    ui.add(
                        egui::Slider::new(&mut self.env_rotation_deg, -180.0..=180.0).suffix("°"),
                    );
                    ui.end_row();
                    ui.label("Background blur");
                    ui.add(egui::Slider::new(&mut self.background_blur, 0.0..=1.0));
                    ui.end_row();
                });
            ui.separator();
            ui.heading("Display");
            self.display_ui(ui);
        });
    }
}

impl egui_tiles::Behavior<Pane> for State {
    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        match pane {
            Pane::Viewport => "3D View",
            Pane::Assets => "Assets",
            Pane::TextureSets => "Texture Sets",
            Pane::LayerStack => "Layers",
            Pane::Properties => "Properties",
        }
        .into()
    }

    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile: egui_tiles::TileId,
        pane: &mut Pane,
    ) -> egui_tiles::UiResponse {
        if let Pane::Viewport = pane {
            self.viewport_ui(ui);
            return egui_tiles::UiResponse::None;
        }
        egui::Frame::new()
            .inner_margin(8.0)
            .show(ui, |ui| match pane {
                Pane::TextureSets => self.texture_sets_ui(ui),
                Pane::Properties => self.properties_ui(ui),
                _ => {
                    ui.weak("Arrives in Phase 1.");
                }
            });
        egui_tiles::UiResponse::None
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..Default::default()
        }
    }
}

struct OrbitCamera {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    fov_y_deg: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.25,
            distance: 4.0,
            fov_y_deg: 40.0,
        }
    }
}

impl OrbitCamera {
    fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cp * sy, sp, cp * cy) * self.distance
    }

    fn right_up(&self) -> (Vec3, Vec3) {
        let forward = (self.target - self.eye()).normalize();
        let right = forward.cross(Vec3::Y).normalize();
        (right, right.cross(forward))
    }

    fn frame(&mut self, (lo, hi): (Vec3, Vec3)) {
        let radius = ((hi - lo).length() * 0.5).max(1e-4);
        self.target = (lo + hi) * 0.5;
        self.distance = radius / (self.fov_y_deg.to_radians() * 0.5).sin() * 1.1;
    }

    fn view_proj(&self, aspect: f32, scene_radius: f32) -> Mat4 {
        let near = (self.distance - scene_radius * 2.0).max(self.distance * 0.001);
        let far = self.distance * 20.0 + scene_radius * 20.0;
        // wgpu clip space: z in [0, 1], y up — glam's "directx" convention.
        let proj = glam::camera::rh::proj::directx::perspective(
            self.fov_y_deg.to_radians(),
            aspect,
            near,
            far,
        );
        proj * glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y)
    }
}
