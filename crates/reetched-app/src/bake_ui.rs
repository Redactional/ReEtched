//! The Bake Mesh Maps window: runs `reetched-bake` on a worker thread and
//! stores the results as image resources on the texture sets.

use crate::{app::State, theme};
use egui::{ComboBox, RichText, Slider, vec2};
use reetched_bake::{MapKind, Progress, Settings};
use reetched_doc::{MeshMap, RESOLUTIONS};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
};

/// Encoded maps for one texture set tile: (set index, UDIM tile, [(map, PNG bytes)]).
type SetMaps = (usize, u32, Vec<(MeshMap, Vec<u8>)>);

enum BakeMessage {
    Done(Vec<SetMaps>),
    Failed(String),
    Cancelled,
}

pub struct BakeJob {
    progress: Arc<Progress>,
    rx: mpsc::Receiver<BakeMessage>,
    sets: usize,
    /// Which set (of `sets`) is baking now, for the progress label.
    current: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Default)]
pub struct BakeUi {
    pub open: bool,
    pub high_poly: String,
    pub settings: Settings,
    /// Which texture sets to bake.
    pub include: Vec<bool>,
    pub job: Option<BakeJob>,
    /// UDIM tile whose maps Texture Set Settings lists (0 = the set's first).
    pub map_tile: u32,
}

pub fn mesh_map(kind: MapKind) -> MeshMap {
    match kind {
        MapKind::Normal => MeshMap::Normal,
        MapKind::WorldNormal => MeshMap::WorldNormal,
        MapKind::Id => MeshMap::Id,
        MapKind::AmbientOcclusion => MeshMap::AmbientOcclusion,
        MapKind::Curvature => MeshMap::Curvature,
        MapKind::Position => MeshMap::Position,
        MapKind::Thickness => MeshMap::Thickness,
        MapKind::Height => MeshMap::Height,
        MapKind::Opacity => MeshMap::Opacity,
    }
}

/// 16-bit PNG: RGB for vector maps, grey for the rest.
pub fn encode_png16(img: &reetched_bake::Image, kind: MapKind) -> anyhow::Result<Vec<u8>> {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
    let rgb = matches!(
        kind,
        MapKind::Normal | MapKind::WorldNormal | MapKind::Id | MapKind::Position
    );
    let dynamic = if rgb {
        let raw = img
            .pixels
            .iter()
            .flat_map(|p| [q(p[0]), q(p[1]), q(p[2])])
            .collect();
        image::DynamicImage::ImageRgb16(
            image::ImageBuffer::from_raw(img.width, img.height, raw)
                .ok_or_else(|| anyhow::anyhow!("bad image size"))?,
        )
    } else {
        let raw = img.pixels.iter().map(|p| q(p[0])).collect();
        image::DynamicImage::ImageLuma16(
            image::ImageBuffer::from_raw(img.width, img.height, raw)
                .ok_or_else(|| anyhow::anyhow!("bad image size"))?,
        )
    };
    let mut out = Vec::new();
    dynamic.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}

impl State {
    pub fn bake_window(&mut self, ctx: &egui::Context) {
        self.poll_bake(false);
        if !self.bake.open {
            return;
        }
        let mut open = true;
        let mut start = false;
        let mut cancel = false;
        let set_count = self.doc.texture_sets.len();
        self.bake.include.resize(set_count, true);
        if self.bake.settings.resolution == 0
            || !RESOLUTIONS.contains(&self.bake.settings.resolution)
        {
            self.bake.settings.resolution = 2048;
        }
        egui::Window::new(theme::header("BAKE MESH MAPS"))
            .open(&mut open)
            .default_width(460.0)
            .collapsible(false)
            .show(ctx, |ui| {
                let b = &mut self.bake;
                let busy = b.job.is_some();
                ui.add_enabled_ui(!busy, |ui| {
                    egui::Grid::new("bake").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                        ui.label("High definition mesh");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut b.high_poly).hint_text("none: bake from the low mesh").desired_width(220.0));
                            if ui.button("Browse…").clicked()
                                && let Some(p) = rfd::FileDialog::new()
                                    .add_filter("Meshes", crate::app::MESH_EXTENSIONS)
                                    .pick_file()
                            {
                                b.high_poly = p.display().to_string();
                            }
                        });
                        ui.end_row();
                        let s = &mut b.settings;
                        ui.label("Output size");
                        ComboBox::from_id_salt("bake_size").selected_text(format!("{0} × {0}", s.resolution)).show_ui(ui, |ui| {
                            for r in RESOLUTIONS.into_iter().filter(|&r| r <= 4096) {
                                ui.selectable_value(&mut s.resolution, r, format!("{r} × {r}"));
                            }
                        });
                        ui.end_row();
                        ui.label("Antialiasing");
                        ComboBox::from_id_salt("bake_aa")
                            .selected_text(match s.antialiasing {
                                1 => "None",
                                2 => "Subsampling 2×2",
                                _ => "Subsampling 3×3",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut s.antialiasing, 1, "None");
                                ui.selectable_value(&mut s.antialiasing, 2, "Subsampling 2×2");
                                ui.selectable_value(&mut s.antialiasing, 3, "Subsampling 3×3");
                            });
                        ui.end_row();
                        ui.label("Max frontal distance");
                        ui.add(Slider::new(&mut s.max_frontal, 0.0..=0.2).custom_formatter(|v, _| format!("{:.1}%", v * 100.0)))
                            .on_hover_text("How far outside the low mesh to look for the high mesh (fraction of the mesh size)");
                        ui.end_row();
                        ui.label("Max rear distance");
                        ui.add(Slider::new(&mut s.max_rear, 0.0..=0.2).custom_formatter(|v, _| format!("{:.1}%", v * 100.0)));
                        ui.end_row();
                        ui.label("AO rays");
                        ui.add(Slider::new(&mut s.ao_rays, 4..=256).logarithmic(true));
                        ui.end_row();
                        ui.label("AO / thickness distance");
                        ui.add(Slider::new(&mut s.ao_distance, 0.01..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                        ui.end_row();
                        ui.label("Thickness rays");
                        ui.add(Slider::new(&mut s.thickness_rays, 4..=128).logarithmic(true));
                        ui.end_row();
                    });
                    ui.add_space(6.0);
                    ui.label(theme::header("BAKERS"));
                    ui.horizontal_wrapped(|ui| {
                        for kind in MapKind::ALL {
                            let mut on = b.settings.maps.contains(&kind);
                            if ui.toggle_value(&mut on, mesh_map(kind).label()).changed() {
                                if on {
                                    b.settings.maps.push(kind);
                                    b.settings.maps.sort();
                                } else {
                                    b.settings.maps.retain(|&k| k != kind);
                                }
                            }
                        }
                    });
                    ui.add_space(6.0);
                    ui.label(theme::header("TEXTURE SETS"));
                    for (set, on) in self.doc.texture_sets.iter().zip(b.include.iter_mut()) {
                        ui.checkbox(on, &set.name);
                    }
                });
                ui.add_space(6.0);
                match &b.job {
                    Some(job) => {
                        let current = job.current.load(std::sync::atomic::Ordering::Relaxed) + 1;
                        ui.add(
                            egui::ProgressBar::new(job.progress.fraction())
                                .text(format!("Baking texture set {current} of {}…", job.sets)),
                        );
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    None => {
                        let ready = !b.settings.maps.is_empty() && b.include.iter().any(|&i| i);
                        if ui.add_enabled(ready, egui::Button::new(RichText::new("  Bake selected textures  ").strong())).clicked() {
                            start = true;
                        }
                    }
                }
            });
        self.bake.open = open;
        if cancel && let Some(job) = &self.bake.job {
            job.progress
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if start {
            self.start_bake();
        }
    }

    pub(crate) fn start_bake(&mut self) {
        let low = self.mesh.clone();
        let high_path = Some(self.bake.high_poly.trim().to_owned())
            .filter(|p| !p.is_empty())
            .map(PathBuf::from);
        let sets: Vec<usize> = self
            .bake
            .include
            .iter()
            .enumerate()
            .filter(|(_, on)| **on)
            .map(|(i, _)| i)
            .collect();
        let settings = self.bake.settings.clone();
        let progress = Arc::new(Progress::default());
        let current = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let (p, c) = (progress.clone(), current.clone());
        let count = sets.len();
        std::thread::spawn(move || {
            let high = match high_path.map(|p| reetched_io::load_mesh(&p)) {
                Some(Ok(m)) => Some(m),
                Some(Err(e)) => {
                    let _ = tx.send(BakeMessage::Failed(format!(
                        "Could not load the high-poly mesh: {e:#}"
                    )));
                    return;
                }
                None => None,
            };
            let mut out = Vec::new();
            for (n, set) in sets.into_iter().enumerate() {
                c.store(n, std::sync::atomic::Ordering::Relaxed);
                for (tile, _) in low.udim_tiles(set) {
                    let Some(maps) =
                        reetched_bake::bake(&low, set, tile, high.as_ref(), &settings, &p)
                    else {
                        let _ = tx.send(BakeMessage::Cancelled);
                        return;
                    };
                    let mut encoded = Vec::new();
                    for (kind, img) in maps {
                        match encode_png16(&img, kind) {
                            Ok(bytes) => encoded.push((mesh_map(kind), bytes)),
                            Err(e) => {
                                let _ = tx.send(BakeMessage::Failed(format!(
                                    "Could not encode a baked map: {e:#}"
                                )));
                                return;
                            }
                        }
                    }
                    out.push((set, tile, encoded));
                }
            }
            let _ = tx.send(BakeMessage::Done(out));
        });
        self.bake.job = Some(BakeJob {
            progress,
            rx,
            sets: count,
            current,
        });
        self.info("Baking mesh maps…".into());
    }

    /// Picks up a finished bake (`wait`: blocks until it ends). Some(true) when
    /// a bake finished successfully.
    pub(crate) fn poll_bake(&mut self, wait: bool) -> Option<bool> {
        let job = self.bake.job.as_ref()?;
        let msg = if wait {
            job.rx.recv().map_err(|_| mpsc::TryRecvError::Disconnected)
        } else {
            job.rx.try_recv()
        };
        let msg = match msg {
            Ok(m) => m,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                BakeMessage::Failed("the bake stopped unexpectedly".into())
            }
        };
        self.bake.job = None;
        match msg {
            BakeMessage::Done(sets) => {
                let mut count = 0;
                for (set, tile, maps) in sets {
                    for (map, bytes) in maps {
                        let id = self.resources.add(bytes);
                        let ts = &mut self.doc.texture_sets[set];
                        let name = if tile == reetched_doc::FIRST_UDIM {
                            format!("{}_{}.png", ts.name, map.painter_suffix())
                        } else {
                            format!("{}_{}.{tile}.png", ts.name, map.painter_suffix())
                        };
                        ts.mesh_maps_for_mut(tile).insert(map, id.clone());
                        self.doc
                            .resources
                            .insert(id, reetched_doc::ResourceInfo { name });
                        count += 1;
                    }
                }
                self.info(format!("Baked {count} mesh maps"));
                Some(true)
            }
            BakeMessage::Failed(e) => {
                self.error(e);
                Some(false)
            }
            BakeMessage::Cancelled => {
                self.warn("Bake cancelled".into());
                Some(false)
            }
        }
    }

    /// Mesh map list for Texture Set Settings: status, load, clear, Painter import.
    pub fn mesh_maps_ui(&mut self, ui: &mut egui::Ui) {
        let set_index = self.selected_part;
        let mut import = false;
        ui.horizontal(|ui| {
            ui.label(theme::header("MESH MAPS"));
            if ui.small_button("Bake…").clicked() {
                self.bake.open = true;
            }
            import = ui.small_button("Import Painter maps…").clicked();
        });
        if import {
            self.import_painter_maps();
        }
        let tiles = self.viewport.tiles(set_index);
        if !tiles.contains(&self.bake.map_tile) {
            self.bake.map_tile = tiles.first().copied().unwrap_or(reetched_doc::FIRST_UDIM);
        }
        let tile = self.bake.map_tile;
        if tiles.len() > 1 {
            ComboBox::from_id_salt("map_tile")
                .selected_text(format!("Tile {tile}"))
                .show_ui(ui, |ui| {
                    for t in &tiles {
                        ui.selectable_value(&mut self.bake.map_tile, *t, format!("Tile {t}"));
                    }
                });
        }
        let Some(set) = self.doc.texture_sets.get(set_index) else {
            return;
        };
        let names = self.doc.resources.clone();
        let mut load: Option<MeshMap> = None;
        let mut clear: Option<MeshMap> = None;
        egui::Grid::new("mesh_maps")
            .num_columns(3)
            .spacing(vec2(12.0, 4.0))
            .show(ui, |ui| {
                for map in MeshMap::ALL {
                    ui.label(map.label());
                    match set.mesh_maps_for(tile).get(&map) {
                        Some(id) => {
                            ui.label(
                                RichText::new(names.get(id).map_or("baked", |r| r.name.as_str()))
                                    .color(theme::TEXT),
                            );
                            if ui.small_button("Clear").clicked() {
                                clear = Some(map);
                            }
                        }
                        None => {
                            ui.label(RichText::new("—").color(theme::WEAK));
                            if ui.small_button("Load…").clicked() {
                                load = Some(map);
                            }
                        }
                    }
                    ui.end_row();
                }
            });
        if let Some(map) = clear {
            self.doc.texture_sets[set_index]
                .mesh_maps_for_mut(tile)
                .remove(&map);
        }
        if let Some(map) = load
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "tga", "tif", "tiff", "exr", "jpg"])
                .pick_file()
            && let Some(id) = self.import_resource(&path)
        {
            self.doc.texture_sets[set_index]
                .mesh_maps_for_mut(tile)
                .insert(map, id);
        }
    }

    /// Imports baked maps named like Painter's (`TextureSetName_ambient_occlusion.png`, …).
    fn import_painter_maps(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "tga", "tif", "tiff", "exr", "jpg"])
            .pick_files()
        else {
            return;
        };
        let mut matched = 0;
        for path in paths {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let hit = self
                .doc
                .texture_sets
                .iter()
                .enumerate()
                .find_map(|(i, set)| {
                    MeshMap::ALL
                        .into_iter()
                        .find(|m| {
                            stem == format!("{}_{}", set.name.to_lowercase(), m.painter_suffix())
                        })
                        .map(|m| (i, m))
                });
            match hit {
                Some((set, map)) => {
                    if let Some(id) = self.import_resource(&path) {
                        self.doc.texture_sets[set].mesh_maps.insert(map, id);
                        matched += 1;
                    }
                }
                None => self.warn(format!(
                    "{}: name doesn't match TextureSet_mapname (e.g. {}_ambient_occlusion)",
                    crate::app::file_name(&path),
                    self.doc
                        .texture_sets
                        .first()
                        .map_or("Set", |s| s.name.as_str())
                )),
            }
        }
        self.info(format!("Imported {matched} mesh maps"));
    }
}
