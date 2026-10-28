//! Procedural materials in fill layers: package loading, background rendering,
//! `.sbs` import, the asset shelf and the Properties editor.

use crate::app::{State, file_name};
use crate::theme;
use egui::{RichText, Ui};
use reetched_doc::{
    Channel, Fill, MeshMap, NodeKind, Procedural, ProceduralSource, ResourceId, ResourceInfo,
    Resources, TextureSet,
};
use reetched_graph::{Args, Graph, Image, InputKind, OutputDef, Package, Ty, V, Widget, library};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
};

/// An imported `.sbs` file, kept with the files it references until applied.
pub struct SbsImport {
    pub name: String,
    pub bytes: Vec<u8>,
    /// Relative path → bytes of every file the package loaded.
    pub files: BTreeMap<String, Vec<u8>>,
    pub package: Arc<Package>,
}

struct Job {
    key: String,
    /// The layer it is for; newer jobs for the same layer replace queued ones.
    slot: u64,
    package: Arc<Package>,
    graph: String,
    args: Args,
}

struct Done {
    key: String,
    /// `None`: skipped because a newer job for the layer came in.
    outputs: Option<Vec<(Channel, image::RgbaImage)>>,
    warnings: Vec<String>,
}

pub struct Procedurals {
    packages: HashMap<String, Result<Arc<Package>, String>>,
    jobs: mpsc::Sender<Job>,
    done: mpsc::Receiver<Done>,
    pending: HashSet<String>,
    /// Latest render warnings (or load error) per render key.
    pub warnings: HashMap<String, Vec<String>>,
    pub imports: Vec<SbsImport>,
    pub thumbs: Vec<egui::TextureHandle>,
    mesh_maps: HashMap<ResourceId, Arc<Image>>,
}

impl Default for Procedurals {
    fn default() -> Self {
        let (jobs, rx) = mpsc::channel::<Job>();
        let (tx, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("procedural".into())
            // Deep graphs recurse; give them room.
            .stack_size(64 << 20)
            .spawn(move || worker(rx, tx))
            .expect("start the procedural render thread");
        Self {
            packages: HashMap::new(),
            jobs,
            done,
            pending: HashSet::new(),
            warnings: HashMap::new(),
            imports: Vec::new(),
            thumbs: Vec::new(),
            mesh_maps: HashMap::new(),
        }
    }
}

fn worker(rx: mpsc::Receiver<Job>, tx: mpsc::Sender<Done>) {
    while let Ok(first) = rx.recv() {
        // Keep only the newest queued job per layer (slider drags queue many).
        let mut latest: Vec<Job> = vec![first];
        while let Ok(j) = rx.try_recv() {
            if let Some(old) = latest.iter_mut().find(|o| o.slot == j.slot) {
                let stale = std::mem::replace(old, j);
                let _ = tx.send(Done {
                    key: stale.key,
                    outputs: None,
                    warnings: Vec::new(),
                });
            } else {
                latest.push(j);
            }
        }
        for job in latest {
            let r = reetched_graph::render(&job.package, &job.graph, &job.args);
            let mut outputs = Vec::new();
            for (o, img) in &r.outputs {
                if let Some(c) = channel_for(o)
                    && !outputs.iter().any(|(oc, _)| *oc == c)
                {
                    let rgba =
                        image::RgbaImage::from_raw(img.w as u32, img.h as u32, img.to_rgba8())
                            .expect("size matches");
                    outputs.push((c, rgba));
                }
            }
            if tx
                .send(Done {
                    key: job.key,
                    outputs: Some(outputs),
                    warnings: r.warnings,
                })
                .is_err()
            {
                return;
            }
        }
    }
}

/// Which channel a graph output feeds, by usage or identifier.
pub fn channel_for(o: &OutputDef) -> Option<Channel> {
    let name = o.usage.as_deref().unwrap_or(&o.id).to_lowercase();
    Some(match name.as_str() {
        "basecolor" | "diffuse" | "albedo" | "color" => Channel::BaseColor,
        "metallic" | "metalness" => Channel::Metallic,
        "roughness" => Channel::Roughness,
        "normal" => Channel::Normal,
        "height" | "displacement" => Channel::Height,
        "emissive" | "emission" => Channel::Emissive,
        "opacity" | "alpha" => Channel::Opacity,
        "ambientocclusion" | "ao" => Channel::AmbientOcclusion,
        "specular" | "specularlevel" => Channel::SpecularLevel,
        "anisotropy" | "anisotropylevel" => Channel::Anisotropy,
        "coat" | "coatweight" | "coatopacity" => Channel::Coat,
        "sheen" | "sheenweight" | "sheenopacity" => Channel::Sheen,
        "transmission" | "transmissive" => Channel::Transmission,
        "subsurface" | "scattering" => Channel::Scattering,
        other => return Channel::from_id(other),
    })
}

/// Usage name Designer/Painter graphs use for each mesh map input.
fn usage_name(m: MeshMap) -> &'static str {
    match m {
        MeshMap::Normal => "normal",
        MeshMap::WorldNormal => "worldspacenormal",
        MeshMap::Id => "id",
        MeshMap::AmbientOcclusion => "ambientocclusion",
        MeshMap::Curvature => "curvature",
        MeshMap::Position => "position",
        MeshMap::Thickness => "thickness",
        MeshMap::Height => "height",
        MeshMap::Opacity => "opacity",
    }
}

/// Folders where Substance Designer keeps its `sbs://` library packages.
/// ponytail: fixed install paths (+ REETCHED_SBS_LIBRARY); add a setting if users need others.
fn designer_library(path: &str) -> Option<Vec<u8>> {
    let file = path.strip_prefix("sbs://")?;
    let mut dirs: Vec<PathBuf> = std::env::var_os("REETCHED_SBS_LIBRARY")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    dirs.extend(
        [
            "C:/Program Files/Adobe/Adobe Substance 3D Designer/resources/packages",
            "/Applications/Adobe Substance 3D Designer.app/Contents/Resources/packages",
            "/opt/Adobe/Adobe_Substance_3D_Designer/resources/packages",
        ]
        .map(PathBuf::from),
    );
    dirs.iter().find_map(|d| std::fs::read(d.join(file)).ok())
}

fn package_key(src: &ProceduralSource) -> String {
    match src {
        ProceduralSource::Library => "library".into(),
        ProceduralSource::Sbs { file, .. } => file.to_string(),
    }
}

impl Procedurals {
    /// Loads (once) the package a material comes from.
    pub fn package(
        &mut self,
        src: &ProceduralSource,
        resources: &Resources,
    ) -> Result<Arc<Package>, String> {
        let key = package_key(src);
        if let Some(p) = self.packages.get(&key) {
            return p.clone();
        }
        let p = match src {
            ProceduralSource::Library => Ok(library::package()),
            ProceduralSource::Sbs { file, name, files } => match resources.files.get(file) {
                None => Err(format!("{name} is missing from the project")),
                Some(bytes) => {
                    let load = |path: &str| {
                        files
                            .get(path)
                            .and_then(|id| resources.files.get(id))
                            .map(|b| b.to_vec())
                            .or_else(|| designer_library(path))
                    };
                    reetched_substance::sbs::read(bytes, name, &load)
                        .map(Arc::new)
                        .map_err(|e| format!("{name}: {e:#}"))
                }
            },
        };
        self.packages.insert(key, p.clone());
        p
    }

    /// Asset shelf entries: (label, package, graph id). Library first, then imports.
    pub fn items(&self) -> Vec<(String, Arc<Package>, String)> {
        let lib = library::package();
        let mut items: Vec<_> = lib
            .graphs
            .iter()
            .map(|g| (g.label.clone(), lib.clone(), g.id.clone()))
            .collect();
        for imp in &self.imports {
            for g in imp.package.graphs.iter().filter(|g| !g.outputs.is_empty()) {
                items.push((g.label.clone(), imp.package.clone(), g.id.clone()));
            }
        }
        items
    }

    /// Small base color previews for the shelf.
    pub fn refresh_thumbs(&mut self, ctx: &egui::Context) {
        self.thumbs = self
            .items()
            .iter()
            .map(|(label, pkg, graph)| {
                let r = reetched_graph::render(
                    pkg,
                    graph,
                    &Args {
                        size_log2: [6, 6],
                        ..Default::default()
                    },
                );
                let img = r
                    .outputs
                    .iter()
                    .find(|(o, _)| channel_for(o) == Some(Channel::BaseColor))
                    .or(r.outputs.first())
                    .map(|(_, i)| i.clone());
                let color = match img {
                    Some(i) => egui::ColorImage::from_rgba_unmultiplied([i.w, i.h], &i.to_rgba8()),
                    None => egui::ColorImage::new([1, 1], vec![egui::Color32::GRAY]),
                };
                ctx.load_texture(format!("proc:{label}"), color, egui::TextureOptions::LINEAR)
            })
            .collect();
    }

    fn mesh_map(&mut self, id: &ResourceId, resources: &Resources) -> Option<Arc<Image>> {
        if let Some(i) = self.mesh_maps.get(id) {
            return Some(i.clone());
        }
        let img = Arc::new(reetched_substance::sbs::decode(resources.files.get(id)?)?);
        self.mesh_maps.insert(id.clone(), img.clone());
        Some(img)
    }

    /// Renders are queued or running.
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn is_pending(&self, key: &str) -> bool {
        self.pending.contains(key)
    }
}

/// Input values a material renders with: graph defaults with the layer's overrides.
fn args_for(p: &Procedural, graph: &Graph, size: u8, usages: HashMap<String, Arc<Image>>) -> Args {
    let values = graph
        .inputs
        .iter()
        .filter(|i| i.kind == InputKind::Value)
        .filter_map(|i| {
            p.params.get(&i.id).map(|x| {
                (
                    i.id.clone(),
                    V {
                        ty: i.default.ty,
                        x: *x,
                    },
                )
            })
        })
        .collect();
    Args {
        size_log2: [size as i32; 2],
        seed: p.seed,
        values,
        images: HashMap::new(),
        usages,
    }
}

fn walk_fills<'a>(nodes: &'a [reetched_doc::Node], out: &mut Vec<(u64, &'a Procedural)>) {
    for n in nodes {
        match &n.kind {
            NodeKind::Fill(Fill {
                procedural: Some(p),
                ..
            }) => out.push((n.id, &**p)),
            NodeKind::Folder { children } => walk_fills(children, out),
            _ => {}
        }
    }
}

impl State {
    /// Collects finished renders and queues materials that need one. Call once a frame.
    pub fn update_procedurals(&mut self) {
        let mut fresh = false;
        while let Ok(done) = self.procedural.done.try_recv() {
            self.procedural.pending.remove(&done.key);
            if let Some(outputs) = done.outputs {
                self.viewport.compositor().set_procedural(
                    &self.device,
                    &self.queue,
                    done.key.clone(),
                    &outputs,
                );
                self.procedural.warnings.insert(done.key, done.warnings);
                fresh = true;
            }
        }

        let mut active = HashSet::new();
        let sets: Vec<TextureSet> = self.doc.texture_sets.clone();
        for set in &sets {
            let mut fills = Vec::new();
            walk_fills(&set.stack, &mut fills);
            for (slot, p) in fills {
                let key = p.render_key(set.resolution, &set.mesh_maps);
                active.insert(key.clone());
                if self.viewport.compositor().has_procedural(&key)
                    || self.procedural.pending.contains(&key)
                    || self.procedural.warnings.contains_key(&key)
                {
                    continue;
                }
                let pkg = match self.procedural.package(&p.source, &self.resources) {
                    Ok(pkg) => pkg,
                    Err(e) => {
                        self.procedural.warnings.insert(key, vec![e]);
                        continue;
                    }
                };
                let Some(graph) = pkg.graph(&p.graph) else {
                    self.procedural
                        .warnings
                        .insert(key, vec![format!("{} has no graph {}", pkg.name, p.graph)]);
                    continue;
                };
                let mut usages = HashMap::new();
                for (m, id) in &set.mesh_maps {
                    if let Some(img) = self.procedural.mesh_map(id, &self.resources) {
                        usages.insert(usage_name(*m).to_string(), img);
                    }
                }
                let args = args_for(p, graph, p.size_log2(set.resolution), usages);
                let job = Job {
                    key: key.clone(),
                    slot,
                    package: pkg.clone(),
                    graph: p.graph.clone(),
                    args,
                };
                if self.procedural.jobs.send(job).is_ok() {
                    self.procedural.pending.insert(key);
                }
            }
        }
        if fresh {
            self.viewport.invalidate();
        }
        // Forget renders nothing uses (keep in-flight ones).
        let mut keep = active;
        keep.extend(self.procedural.pending.iter().cloned());
        self.viewport.compositor().retain_procedural(&keep);
        self.procedural.warnings.retain(|k, _| keep.contains(k));
    }

    /// Adds procedural shelf item `k` as a fill layer.
    pub fn apply_procedural(&mut self, k: usize) {
        let items = self.procedural.items();
        let Some((label, pkg, graph_id)) = items.get(k).cloned() else {
            return;
        };
        let lib_count = library::package().graphs.len();
        let source = if k < lib_count {
            ProceduralSource::Library
        } else {
            let Some(imp) = self
                .procedural
                .imports
                .iter()
                .find(|i| Arc::ptr_eq(&i.package, &pkg))
            else {
                return;
            };
            let file = self.resources.add(imp.bytes.clone());
            self.doc.resources.insert(
                file.clone(),
                ResourceInfo {
                    name: imp.name.clone(),
                },
            );
            let mut files = BTreeMap::new();
            for (path, bytes) in &imp.files {
                let id = self.resources.add(bytes.clone());
                self.doc
                    .resources
                    .insert(id.clone(), ResourceInfo { name: path.clone() });
                files.insert(path.clone(), id);
            }
            let source = ProceduralSource::Sbs {
                file,
                name: imp.name.clone(),
                files,
            };
            // Same package, now keyed by its resource id.
            self.procedural
                .packages
                .insert(package_key(&source), Ok(pkg.clone()));
            source
        };
        let Some(graph) = pkg.graph(&graph_id) else {
            return;
        };
        let mut fill = Fill::new([0.5; 3], 0.0, 0.5);
        for c in Channel::ALL {
            let made = graph.outputs.iter().any(|o| channel_for(o) == Some(c));
            if let Some(src) = fill.channels.get_mut(&c) {
                src.enabled = made;
            }
        }
        fill.procedural = Some(Box::new(Procedural {
            source,
            graph: graph_id,
            label: label.clone(),
            params: BTreeMap::new(),
            seed: 0,
            resolution_log2: 0,
        }));
        let node = self.doc.new_fill(label, fill);
        self.add_node(node);
    }

    pub fn import_sbs_dialog(&mut self) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter("Substance Designer graph", &["sbs"])
            .pick_files()
        {
            for p in paths {
                self.import_sbs(&p);
            }
        }
    }

    pub fn import_sbs(&mut self, path: &Path) {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.error(format!("Could not read {}: {e}", path.display()));
                return;
            }
        };
        let dir = path.parent().unwrap_or(Path::new("."));
        let name = file_name(path);
        let loaded = std::cell::RefCell::new(BTreeMap::new());
        let load = |p: &str| {
            if p.starts_with("sbs://") {
                return designer_library(p);
            }
            let b = std::fs::read(dir.join(p)).ok()?;
            loaded.borrow_mut().insert(p.to_string(), b.clone());
            Some(b)
        };
        match reetched_substance::sbs::read(&bytes, &name, &load) {
            Ok(pkg) => {
                let graphs = pkg.graphs.iter().filter(|g| !g.outputs.is_empty()).count();
                for e in pkg.deps.values().filter_map(|d| d.as_ref().err()) {
                    self.warn(format!("{name}: {e}"));
                }
                if graphs == 0 {
                    self.warn(format!("{name} has no graphs with outputs"));
                    return;
                }
                self.info(format!("Imported {name} ({graphs} graph(s))"));
                crate::app::remember(&mut self.settings.imported_sbs, path);
                self.procedural.imports.push(SbsImport {
                    name,
                    bytes,
                    files: loaded.into_inner(),
                    package: Arc::new(pkg),
                });
                self.procedural.thumbs.clear();
                self.asset_category = crate::app::AssetCategory::Procedural;
            }
            Err(e) => self.error(format!("Could not import {name}: {e:#}")),
        }
    }
}

/// Editor for a fill's procedural material. `graph`: its definition, if loaded.
pub fn procedural_ui(
    ui: &mut Ui,
    p: &mut Procedural,
    graph: Option<&Graph>,
    status: Result<&[String], &str>,
    pending: bool,
) -> bool {
    let mut remove = false;
    ui.horizontal(|ui| {
        ui.label(theme::header("PROCEDURAL MATERIAL"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Detach")
                .on_hover_text("Turn into a plain fill layer")
                .clicked()
            {
                remove = true;
            }
            if pending {
                ui.spinner();
            }
        });
    });
    ui.label(RichText::new(&p.label).strong());
    match status {
        Err(e) => {
            ui.label(RichText::new(e).color(theme::ERROR));
        }
        Ok(warnings) => {
            for w in warnings {
                ui.label(RichText::new(w).color(theme::WARN));
            }
        }
    }
    egui::Grid::new("proc_common")
        .num_columns(2)
        .spacing(egui::vec2(12.0, 6.0))
        .show(ui, |ui| {
            ui.label("Resolution");
            let label = |v: u8| match v {
                0 => "Texture set".to_string(),
                v => format!("{}", 1u32 << v),
            };
            egui::ComboBox::from_id_salt("proc_res")
                .selected_text(label(p.resolution_log2))
                .show_ui(ui, |ui| {
                    for v in [0, 8, 9, 10, 11, 12] {
                        ui.selectable_value(&mut p.resolution_log2, v, label(v));
                    }
                });
            ui.end_row();
            ui.label("Random seed");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut p.seed));
                if ui.small_button("Random").clicked() {
                    p.seed =
                        (p.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345) >> 8) & 0xffff;
                }
            });
            ui.end_row();
        });
    let Some(graph) = graph else {
        return remove;
    };
    // Inputs in their groups, in file order.
    let mut groups: Vec<&str> = Vec::new();
    for i in graph
        .inputs
        .iter()
        .filter(|i| i.kind == InputKind::Value && !i.is_builtin())
    {
        if !groups.contains(&i.group.as_str()) {
            groups.push(&i.group);
        }
    }
    for group in groups {
        let mut body = |ui: &mut Ui| {
            egui::Grid::new(("proc_inputs", group))
                .num_columns(2)
                .spacing(egui::vec2(12.0, 6.0))
                .show(ui, |ui| {
                    for def in graph.inputs.iter().filter(|i| {
                        i.kind == InputKind::Value && !i.is_builtin() && i.group == group
                    }) {
                        let mut x = p.params.get(&def.id).copied().unwrap_or(def.default.x);
                        ui.label(&def.label);
                        input_widget(ui, def, &mut x);
                        ui.end_row();
                        if x == def.default.x {
                            p.params.remove(&def.id);
                        } else {
                            p.params.insert(def.id.clone(), x);
                        }
                    }
                });
        };
        if group.is_empty() {
            body(ui);
        } else {
            egui::CollapsingHeader::new(group)
                .default_open(true)
                .show(ui, body);
        }
    }
    if !p.params.is_empty() && ui.button("Reset to defaults").clicked() {
        p.params.clear();
    }
    remove
}

fn input_widget(ui: &mut Ui, def: &reetched_graph::InputDef, x: &mut [f32; 4]) {
    let n = def.default.ty.len();
    match (&def.widget, def.default.ty) {
        (Widget::Combo(items), _) => {
            let current = items
                .iter()
                .find(|(v, _)| *v as f32 == x[0])
                .map_or_else(|| format!("{}", x[0]), |(_, l)| l.clone());
            egui::ComboBox::from_id_salt(&def.id)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (v, l) in items {
                        ui.selectable_value(&mut x[0], *v as f32, l);
                    }
                });
        }
        (_, Ty::Bool) => {
            let mut b = x[0] != 0.0;
            ui.checkbox(&mut b, "");
            x[0] = if b { 1.0 } else { 0.0 };
        }
        (Widget::Color, Ty::Float(3 | 4)) => {
            // Designer colors are sRGB values.
            let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            let mut c = [to8(x[0]), to8(x[1]), to8(x[2])];
            if ui.color_edit_button_srgb(&mut c).changed() {
                for i in 0..3 {
                    x[i] = c[i] as f32 / 255.0;
                }
            }
        }
        (_, Ty::Int(1)) => {
            let mut v = x[0] as i32;
            ui.add(egui::Slider::new(
                &mut v,
                def.min as i32..=def.max.max(def.min + 1.0) as i32,
            ));
            x[0] = v as f32;
        }
        (_, Ty::Float(1)) => {
            ui.add(egui::Slider::new(
                &mut x[0],
                def.min..=def.max.max(def.min + 1e-3),
            ));
        }
        (_, ty) => {
            ui.horizontal(|ui| {
                for v in x.iter_mut().take(n) {
                    let mut d = egui::DragValue::new(v).speed(0.01);
                    if matches!(ty, Ty::Int(_)) {
                        d = d.speed(0.1).fixed_decimals(0);
                    }
                    ui.add(d);
                }
            });
        }
    }
}
