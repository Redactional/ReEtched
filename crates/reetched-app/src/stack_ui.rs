//! Layer stack, layer properties, texture set settings and the export window.

use crate::{
    app::{PresetDrag, Selected, State, file_name, icon_button},
    theme,
};
use egui::{
    Align, Color32, ComboBox, DragValue, Frame, Layout, RichText, Sense, Slider, Stroke,
    StrokeKind, Ui, vec2,
};
use egui_material_icons::icons as ic;
use reetched_doc::{
    BlendMode, Channel, ChannelBlend, ChannelSource, Fill, Mask, MaskBackground, Node, NodeId,
    NodeKind, Placement, Projection, ProjectionMode, RESOLUTIONS, ResourceId,
    export::{
        BitDepth, FileFormat, Padding, expand_name, export_texture_set, presets_from_spexp,
        presets_to_spexp,
    },
    find_node_mut, move_node,
};
use std::collections::HashSet;
use std::path::PathBuf;

/// Payload when dragging a layer to reorder it.
struct LayerDrag(NodeId);

/// Edits that need the whole stack, applied after the rows are drawn.
enum StackAction {
    Select(Selected),
    Move(NodeId, Placement),
    ToggleCollapsed(NodeId),
    AddPreset(usize, Option<NodeId>),
}

/// A bitmap the user chose in the properties panel, applied after the UI pass.
enum BitmapSlot {
    Channel(Channel),
    Mask,
}

struct RowCtx<'a> {
    selected: Option<Selected>,
    collapsed: &'a HashSet<NodeId>,
    channel: Channel,
    actions: Vec<StackAction>,
}

impl State {
    pub fn layers_ui(&mut self, ui: &mut Ui) {
        let has_sel = self.selected.is_some();
        let sel_is_layer = self.selected.is_some_and(|s| !s.mask);
        ui.horizontal(|ui| {
            ComboBox::from_id_salt("layer_channel")
                .selected_text(self.layer_channel.label())
                .width(100.0)
                .show_ui(ui, |ui| {
                    for c in Channel::ALL {
                        ui.selectable_value(&mut self.layer_channel, c, c.label());
                    }
                });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled_ui(has_sel, |ui| {
                        icon_button(ui, ic::ICON_DELETE, false, "Delete (Del)")
                    })
                    .inner
                    .clicked()
                {
                    self.delete_selected();
                }
                if ui
                    .add_enabled_ui(sel_is_layer, |ui| {
                        icon_button(ui, ic::ICON_CONTENT_COPY, false, "Duplicate")
                    })
                    .inner
                    .clicked()
                {
                    self.duplicate_selected();
                }
                if icon_button(ui, ic::ICON_CREATE_NEW_FOLDER, false, "Add folder").clicked() {
                    let node = self.doc.new_folder("Folder 1".into());
                    self.add_node(node);
                }
                if icon_button(ui, ic::ICON_FORMAT_COLOR_FILL, false, "Add fill layer").clicked() {
                    let node = self
                        .doc
                        .new_fill("Fill layer".into(), Fill::new([0.8; 3], 0.0, 0.5));
                    self.add_node(node);
                }
                ui.add_enabled_ui(sel_is_layer, |ui| {
                    ui.menu_button(ic::ICON_MASKED_TRANSITIONS.rich_text().size(18.0), |ui| {
                        for (label, bg) in [
                            ("Add white mask", MaskBackground::White),
                            ("Add black mask", MaskBackground::Black),
                        ] {
                            if ui.button(label).clicked() {
                                self.add_mask(bg);
                            }
                        }
                    })
                    .response
                    .on_hover_text("Add mask");
                });
                if icon_button(ui, ic::ICON_BRUSH, false, "Add paint layer").clicked() {
                    let n = self.stack().map_or(0, |s| s.len()) + 1;
                    let node = self.doc.new_paint(format!("Layer {n}"));
                    self.add_node(node);
                }
                ui.add_enabled_ui(false, |ui| {
                    icon_button(ui, ic::ICON_AUTO_FIX_HIGH, false, "Add effect")
                        .on_disabled_hover_text("Effects arrive in Phase 3");
                });
            });
        });
        ui.separator();

        let Some(set) = self.doc.texture_sets.get_mut(self.selected_part) else {
            ui.label(RichText::new("No texture set").color(theme::WEAK));
            return;
        };
        let mut ctx = RowCtx {
            selected: self.selected,
            collapsed: &self.collapsed,
            channel: self.layer_channel,
            actions: Vec::new(),
        };
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                node_rows(ui, &mut set.stack, 0, &mut ctx);
                if set.stack.is_empty() {
                    ui.label(
                        RichText::new("Empty stack — add a fill layer or drop a material here")
                            .color(theme::WEAK),
                    );
                }
                let rest = ui.allocate_rect(ui.available_rect_before_wrap(), Sense::click());
                if let Some(p) = rest.dnd_release_payload::<PresetDrag>() {
                    ctx.actions.push(StackAction::AddPreset(p.0, None));
                }
                if rest.clicked() {
                    ctx.actions.push(StackAction::Select(Selected {
                        node: 0,
                        mask: false,
                    }));
                }
            });
        let actions = std::mem::take(&mut ctx.actions);
        for action in actions {
            match action {
                StackAction::Select(sel) => {
                    self.selected = (sel.node != 0).then_some(sel);
                    self.properties_tool = false;
                }
                StackAction::Move(id, placement) => {
                    if let Some(stack) = self.stack_mut() {
                        move_node(stack, id, placement);
                    }
                }
                StackAction::ToggleCollapsed(id) => {
                    if !self.collapsed.remove(&id) {
                        self.collapsed.insert(id);
                    }
                }
                StackAction::AddPreset(i, above) => {
                    if let Some(above) = above {
                        self.selected = Some(Selected {
                            node: above,
                            mask: false,
                        });
                    }
                    self.apply_preset(i);
                }
            }
        }
    }

    fn add_mask(&mut self, background: MaskBackground) {
        let Some(sel) = self.selected else { return };
        if let Some(node) = self.stack_mut().and_then(|s| find_node_mut(s, sel.node)) {
            node.mask = Some(Mask::new(background));
            self.selected = Some(Selected {
                node: sel.node,
                mask: true,
            });
        }
    }

    pub fn texture_set_settings_ui(&mut self, ui: &mut Ui) {
        let triangles = self
            .viewport
            .parts
            .get(self.selected_part)
            .map_or(0, |p| p.triangles);
        let Some(set) = self.doc.texture_sets.get_mut(self.selected_part) else {
            ui.label(RichText::new("No texture set").color(theme::WEAK));
            return;
        };
        egui::Grid::new("tss")
            .num_columns(2)
            .spacing(vec2(12.0, 6.0))
            .show(ui, |ui| {
                ui.label("Texture set");
                ui.label(RichText::new(&set.name).strong());
                ui.end_row();
                ui.label("Size");
                ComboBox::from_id_salt("tss_size")
                    .selected_text(format!("{0} × {0}", set.resolution))
                    .show_ui(ui, |ui| {
                        for r in RESOLUTIONS {
                            ui.selectable_value(&mut set.resolution, r, format!("{r} × {r}"));
                        }
                    });
                ui.end_row();
                ui.label("Triangles");
                ui.label(triangles.to_string());
                ui.end_row();
                ui.label("UV tiles");
                ui.label("1001");
                ui.end_row();
                ui.label("Normal map format");
                ui.label("OpenGL");
                ui.end_row();
            });
        if set.resolution >= 8192 {
            ui.label(
                RichText::new("8K uses about 1.5 GB of video memory per texture set.")
                    .color(theme::WEAK),
            );
        }
        ui.add_space(8.0);
        ui.label(theme::header("CHANNELS"));
        egui::Grid::new("channels")
            .num_columns(2)
            .spacing(vec2(12.0, 4.0))
            .show(ui, |ui| {
                for (name, format) in [
                    ("Base color", "sRGB8"),
                    ("Metallic", "L8"),
                    ("Roughness", "L8"),
                    ("Normal", "RGB16F"),
                    ("Height", "L16F"),
                ] {
                    ui.label(name);
                    ui.label(RichText::new(format).color(theme::WEAK));
                    ui.end_row();
                }
            });
    }

    pub fn properties_ui(&mut self, ui: &mut Ui) {
        if self.properties_tool
            && matches!(
                self.tool,
                crate::app::Tool::Paint | crate::app::Tool::Eraser
            )
        {
            self.brush_ui(ui);
            return;
        }
        let Some(sel) = self.selected else {
            ui.label(RichText::new("Select a layer to edit its properties").color(theme::WEAK));
            return;
        };
        let names = self.doc.resources.clone();
        let mut pick: Option<BitmapSlot> = None;
        let mut clear: Option<BitmapSlot> = None;
        let mut show_brush = false;
        let Some(node) = self.stack_mut().and_then(|s| find_node_mut(s, sel.node)) else {
            self.selected = None;
            return;
        };
        let resource_name = |id: &Option<ResourceId>| {
            id.as_ref().map(|id| {
                names
                    .get(id)
                    .map_or("(missing image)", |r| r.name.as_str())
                    .to_owned()
            })
        };
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                if sel.mask {
                    let Some(mask) = &mut node.mask else { return };
                    ui.label(theme::header("MASK"));
                    egui::Grid::new("mask")
                        .num_columns(2)
                        .spacing(vec2(12.0, 6.0))
                        .show(ui, |ui| {
                            ui.label("Enabled");
                            ui.checkbox(&mut mask.enabled, "");
                            ui.end_row();
                            ui.label("Background");
                            ComboBox::from_id_salt("mask_bg")
                                .selected_text(match mask.background {
                                    MaskBackground::White => "White",
                                    MaskBackground::Black => "Black",
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut mask.background,
                                        MaskBackground::White,
                                        "White",
                                    );
                                    ui.selectable_value(
                                        &mut mask.background,
                                        MaskBackground::Black,
                                        "Black",
                                    );
                                });
                            ui.end_row();
                            ui.label("Invert");
                            ui.checkbox(&mut mask.invert, "");
                            ui.end_row();
                            ui.label("Bitmap");
                            bitmap_buttons(
                                ui,
                                resource_name(&mask.bitmap),
                                BitmapSlot::Mask,
                                &mut pick,
                                &mut clear,
                            );
                            ui.end_row();
                        });
                    if mask.bitmap.is_some() {
                        ui.add_space(8.0);
                        projection_ui(ui, &mut mask.projection, "mask_proj");
                    }
                    return;
                }

                ui.label(theme::header(match node.kind {
                    NodeKind::Fill(_) => "FILL",
                    NodeKind::Paint(_) => "PAINT",
                    NodeKind::Folder { .. } => "FOLDER",
                }));
                egui::Grid::new("layer")
                    .num_columns(2)
                    .spacing(vec2(12.0, 6.0))
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(&mut node.name);
                        ui.end_row();
                    });
                let NodeKind::Fill(fill) = &mut node.kind else {
                    let hint = if matches!(node.kind, NodeKind::Paint(_)) {
                        "Paint on this layer with the Paint tool (1) and erase with the Eraser (2), in the 3D or 2D view. Blend mode and opacity per channel are in the layer stack."
                    } else {
                        "Set a folder's blend mode and opacity per channel in the layer stack."
                    };
                    ui.label(RichText::new(hint).color(theme::WEAK));
                    if matches!(node.kind, NodeKind::Paint(_)) && ui.button("Brush settings").clicked() {
                        show_brush = true;
                    }
                    return;
                };
                ui.add_space(8.0);
                projection_ui(ui, &mut fill.projection, "fill_proj");
                ui.add_space(8.0);
                ui.label(theme::header("MATERIAL"));
                ui.horizontal_wrapped(|ui| {
                    for c in Channel::ALL {
                        let src = fill.channels.entry(c).or_insert_with(|| ChannelSource {
                            enabled: false,
                            value: c.empty_value(),
                            bitmap: None,
                        });
                        ui.toggle_value(&mut src.enabled, c.label());
                    }
                });
                ui.add_space(4.0);
                egui::Grid::new("channels")
                    .num_columns(2)
                    .spacing(vec2(12.0, 6.0))
                    .show(ui, |ui| {
                        for c in Channel::ALL {
                            let Some(src) = fill.channels.get_mut(&c).filter(|s| s.enabled) else {
                                continue;
                            };
                            ui.label(c.label());
                            ui.horizontal(|ui| {
                                if src.bitmap.is_none() {
                                    channel_value_ui(ui, c, &mut src.value);
                                }
                                bitmap_buttons(
                                    ui,
                                    resource_name(&src.bitmap),
                                    BitmapSlot::Channel(c),
                                    &mut pick,
                                    &mut clear,
                                );
                            });
                            ui.end_row();
                        }
                    });
            });

        if show_brush {
            self.properties_tool = true;
            if !matches!(
                self.tool,
                crate::app::Tool::Paint | crate::app::Tool::Eraser
            ) {
                self.tool = crate::app::Tool::Paint;
            }
        }
        if let Some(slot) = clear {
            self.set_bitmap(sel.node, slot, None);
        }
        if let Some(slot) = pick
            && let Some(path) = rfd::FileDialog::new()
                .add_filter(
                    "Images",
                    &["png", "jpg", "jpeg", "tga", "tif", "tiff", "exr", "hdr"],
                )
                .pick_file()
            && let Some(id) = self.import_resource(&path)
        {
            self.set_bitmap(sel.node, slot, Some(id));
        }
    }

    fn set_bitmap(&mut self, node: NodeId, slot: BitmapSlot, id: Option<ResourceId>) {
        let Some(node) = self.stack_mut().and_then(|s| find_node_mut(s, node)) else {
            return;
        };
        match (slot, &mut node.kind) {
            (BitmapSlot::Mask, _) => {
                if let Some(mask) = &mut node.mask {
                    mask.bitmap = id;
                }
            }
            (BitmapSlot::Channel(c), NodeKind::Fill(fill)) => {
                if let Some(src) = fill.channels.get_mut(&c) {
                    src.bitmap = id;
                }
            }
            _ => {}
        }
    }

    pub fn export_window(&mut self, ctx: &egui::Context) {
        if !self.export.open {
            return;
        }
        let mut open = true;
        let mut run = false;
        egui::Window::new(theme::header("EXPORT TEXTURES"))
            .open(&mut open)
            .default_width(480.0)
            .collapsible(false)
            .show(ctx, |ui| {
                let e = &mut self.export;
                if e.dir.is_empty()
                    && let Some(dir) = self.project_path.as_ref().and_then(|p| p.parent())
                {
                    e.dir = dir.join("textures").display().to_string();
                }
                egui::Grid::new("export")
                    .num_columns(2)
                    .spacing(vec2(12.0, 6.0))
                    .show(ui, |ui| {
                        ui.label("Output directory");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut e.dir).desired_width(260.0));
                            if ui.button("Browse…").clicked()
                                && let Some(dir) = rfd::FileDialog::new().pick_folder()
                            {
                                e.dir = dir.display().to_string();
                            }
                        });
                        ui.end_row();

                        ui.label("Output template");
                        ui.horizontal(|ui| {
                            let current = e
                                .presets
                                .get(e.preset)
                                .map_or("", |p| p.name.as_str())
                                .to_owned();
                            ComboBox::from_id_salt("export_preset")
                                .selected_text(current)
                                .width(220.0)
                                .show_ui(ui, |ui| {
                                    for (i, p) in e.presets.iter().enumerate() {
                                        ui.selectable_value(&mut e.preset, i, &p.name);
                                    }
                                });
                        });
                        ui.end_row();

                        ui.label("File type");
                        ui.horizontal(|ui| {
                            ComboBox::from_id_salt("export_format")
                                .selected_text(e.settings.format.extension().to_uppercase())
                                .show_ui(ui, |ui| {
                                    for f in FileFormat::ALL {
                                        ui.selectable_value(
                                            &mut e.settings.format,
                                            f,
                                            f.extension().to_uppercase(),
                                        );
                                    }
                                });
                            let depths = e.settings.format.depths();
                            if !depths.contains(&e.settings.depth) {
                                e.settings.depth = depths[0];
                            }
                            ComboBox::from_id_salt("export_depth")
                                .selected_text(e.settings.depth.label())
                                .show_ui(ui, |ui| {
                                    for &d in depths {
                                        ui.selectable_value(&mut e.settings.depth, d, d.label());
                                    }
                                });
                        });
                        ui.end_row();

                        ui.label("Padding");
                        ui.horizontal(|ui| {
                            let label = match e.settings.padding {
                                Padding::None => "No padding",
                                Padding::Infinite => "Dilation infinite",
                                Padding::Dilation {
                                    transparent: true, ..
                                } => "Dilation + transparent",
                                Padding::Dilation {
                                    transparent: false, ..
                                } => "Dilation + default background",
                            };
                            ComboBox::from_id_salt("export_padding")
                                .selected_text(label)
                                .width(200.0)
                                .show_ui(ui, |ui| {
                                    let p = &mut e.settings.padding;
                                    ui.selectable_value(p, Padding::None, "No padding");
                                    ui.selectable_value(p, Padding::Infinite, "Dilation infinite");
                                    ui.selectable_value(
                                        p,
                                        Padding::Dilation {
                                            pixels: 16,
                                            transparent: true,
                                        },
                                        "Dilation + transparent",
                                    );
                                    ui.selectable_value(
                                        p,
                                        Padding::Dilation {
                                            pixels: 16,
                                            transparent: false,
                                        },
                                        "Dilation + default background",
                                    );
                                });
                            if let Padding::Dilation { pixels, .. } = &mut e.settings.padding {
                                ui.add(DragValue::new(pixels).range(1..=256).suffix(" px"));
                            }
                        });
                        ui.end_row();
                    });
                if e.settings.depth == BitDepth::Float32 {
                    ui.label(
                        RichText::new(
                            "Float files keep base color linear; 8/16-bit files are sRGB.",
                        )
                        .color(theme::WEAK),
                    );
                }

                ui.horizontal(|ui| {
                    if ui.button("Import Painter preset (.spexp)…").clicked() {
                        self.pending_preset_import = true;
                    }
                    if ui.button("Save preset as .spexp…").clicked() {
                        self.pending_preset_save = true;
                    }
                });

                ui.separator();
                ui.label(theme::header("TEXTURE SETS"));
                let names = (self.project_name(), mesh_name(&self.doc.mesh_source));
                let e = &mut self.export;
                e.include.resize(self.doc.texture_sets.len(), true);
                for (set, on) in self.doc.texture_sets.iter().zip(e.include.iter_mut()) {
                    ui.checkbox(on, format!("{}  —  {} px", set.name, set.resolution));
                }
                if let (Some(preset), Some(set)) =
                    (e.presets.get(e.preset), self.doc.texture_sets.first())
                {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Files for the first texture set:").color(theme::WEAK));
                    for map in &preset.maps {
                        let name = expand_name(&map.file_name, &names.0, &names.1, &set.name);
                        ui.label(
                            RichText::new(format!("{name}.{}", e.settings.format.extension()))
                                .monospace()
                                .color(theme::WEAK),
                        );
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let ready = !e.dir.trim().is_empty() && e.include.iter().any(|&b| b);
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new(RichText::new("  Export  ").strong()),
                        )
                        .clicked()
                    {
                        run = true;
                    }
                });
            });
        self.export.open = open;
        if std::mem::take(&mut self.pending_preset_import) {
            self.import_spexp();
        }
        if std::mem::take(&mut self.pending_preset_save) {
            self.save_spexp();
        }
        if run {
            self.run_export();
        }
    }

    fn import_spexp(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Painter export preset", &["spexp", "json"])
            .pick_file()
        else {
            return;
        };
        let parsed = std::fs::read(&path)
            .map_err(anyhow::Error::from)
            .and_then(|b| Ok(serde_json::from_slice::<serde_json::Value>(&b)?))
            .and_then(|v| presets_from_spexp(&v));
        match parsed {
            Ok((presets, warnings)) => {
                for w in warnings {
                    self.warn(w);
                }
                let first = self.export.presets.len();
                let count = presets.len();
                self.export.presets.extend(presets);
                if count > 0 {
                    self.export.preset = first;
                }
                self.info(format!(
                    "Imported {count} export preset(s) from {}",
                    file_name(&path)
                ));
            }
            Err(e) => self.error(format!("Could not read {}: {e:#}", file_name(&path))),
        }
    }

    fn save_spexp(&mut self) {
        let Some(preset) = self.export.presets.get(self.export.preset).cloned() else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Painter export preset", &["spexp"])
            .set_file_name(format!("{}.spexp", preset.name))
            .save_file()
        else {
            return;
        };
        let json = serde_json::to_vec_pretty(&presets_to_spexp(std::slice::from_ref(&preset)));
        match json
            .map_err(anyhow::Error::from)
            .and_then(|j| Ok(std::fs::write(&path, j)?))
        {
            Ok(()) => self.info(format!("Saved preset {}", path.display())),
            Err(e) => self.error(format!("Could not save preset: {e:#}")),
        }
    }

    fn run_export(&mut self) {
        let dir = PathBuf::from(self.export.dir.trim());
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.error(format!("Could not create {}: {e}", dir.display()));
            return;
        }
        let Some(preset) = self.export.presets.get(self.export.preset).cloned() else {
            return;
        };
        // Make sure every composite reflects the current document.
        let warnings = self.viewport.sync(
            &self.device,
            &self.queue,
            &self.doc.texture_sets,
            &self.resources,
        );
        for w in warnings {
            self.warn(w);
        }
        let project = self.project_name();
        let mesh = mesh_name(&self.doc.mesh_source);
        let mut written = 0;
        for (i, set) in self.doc.texture_sets.clone().iter().enumerate() {
            if !self.export.include.get(i).copied().unwrap_or(false) {
                continue;
            }
            let result = self
                .viewport
                .read_back(&self.device, &self.queue, i)
                .and_then(|data| {
                    export_texture_set(
                        &preset,
                        &self.export.settings,
                        &data,
                        &dir,
                        (&project, &mesh, &set.name),
                    )
                });
            match result {
                Ok(files) => written += files.len(),
                Err(e) => {
                    self.error(format!("Export of {} failed: {e:#}", set.name));
                    return;
                }
            }
        }
        self.info(format!("Exported {written} textures to {}", dir.display()));
    }
}

fn mesh_name(source: &Option<String>) -> String {
    source
        .as_deref()
        .map(std::path::Path::new)
        .and_then(|p| p.file_stem())
        .map_or_else(|| "mesh".into(), |s| s.to_string_lossy().into_owned())
}

fn node_rows(ui: &mut Ui, nodes: &mut [Node], depth: usize, ctx: &mut RowCtx) {
    for node in nodes.iter_mut() {
        node_row(ui, node, depth, ctx);
        if let NodeKind::Folder { children } = &mut node.kind
            && !ctx.collapsed.contains(&node.id)
        {
            node_rows(ui, children, depth + 1, ctx);
        }
    }
}

fn node_row(ui: &mut Ui, node: &mut Node, depth: usize, ctx: &mut RowCtx) {
    let id = node.id;
    let selected = ctx.selected
        == Some(Selected {
            node: id,
            mask: false,
        });
    let mask_selected = ctx.selected
        == Some(Selected {
            node: id,
            mask: true,
        });
    let folder = matches!(node.kind, NodeKind::Folder { .. });
    let row = Frame::new()
        .inner_margin(vec2(4.0, 2.0))
        .corner_radius(2)
        .fill(if selected || mask_selected {
            theme::ROW_SELECTED
        } else {
            Color32::TRANSPARENT
        })
        .stroke(if selected {
            Stroke::new(1.0, theme::ACCENT)
        } else {
            Stroke::NONE
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 14.0);
                if folder {
                    let open = !ctx.collapsed.contains(&id);
                    let icon = if open {
                        ic::ICON_EXPAND_MORE
                    } else {
                        ic::ICON_CHEVRON_RIGHT
                    };
                    if ui
                        .add(egui::Button::new(icon.rich_text()).frame(false))
                        .clicked()
                    {
                        ctx.actions.push(StackAction::ToggleCollapsed(id));
                    }
                }
                let eye = if node.visible {
                    ic::ICON_VISIBILITY
                } else {
                    ic::ICON_VISIBILITY_OFF
                };
                if icon_button(ui, eye, false, "Show / hide").clicked() {
                    node.visible = !node.visible;
                }
                // Thumbnail.
                let (thumb, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                match &node.kind {
                    NodeKind::Fill(fill) => {
                        ui.painter()
                            .rect_filled(thumb, 2.0, swatch(fill, ctx.channel));
                    }
                    NodeKind::Paint(_) | NodeKind::Folder { .. } => {
                        let icon = if folder {
                            ic::ICON_FOLDER
                        } else {
                            ic::ICON_BRUSH
                        };
                        ui.painter()
                            .rect_filled(thumb, 2.0, Color32::from_gray(0x3a));
                        ui.painter().text(
                            thumb.center(),
                            egui::Align2::CENTER_CENTER,
                            icon.codepoint,
                            egui::FontId::new(18.0, icon.font_family()),
                            theme::WEAK,
                        );
                    }
                }
                ui.painter().rect_stroke(
                    thumb,
                    2.0,
                    Stroke::new(1.0, theme::GAP),
                    StrokeKind::Inside,
                );
                // Mask thumbnail.
                if let Some(mask) = &node.mask {
                    let (m, resp) = ui.allocate_exact_size(vec2(22.0, 28.0), Sense::click());
                    let fill = match (mask.bitmap.is_some(), mask.background) {
                        (true, _) => Color32::from_gray(128),
                        (false, MaskBackground::White) => Color32::WHITE,
                        (false, MaskBackground::Black) => Color32::BLACK,
                    };
                    ui.painter().rect_filled(
                        m,
                        2.0,
                        if mask.enabled {
                            fill
                        } else {
                            fill.gamma_multiply(0.3)
                        },
                    );
                    let stroke = if mask_selected {
                        Stroke::new(1.5, theme::ACCENT)
                    } else {
                        Stroke::new(1.0, theme::GAP)
                    };
                    ui.painter().rect_stroke(m, 2.0, stroke, StrokeKind::Inside);
                    if resp
                        .on_hover_text("Mask (Shift+click to disable)")
                        .clicked()
                    {
                        if ui.input(|i| i.modifiers.shift) {
                            if let Some(mask) = &mut node.mask {
                                mask.enabled = !mask.enabled;
                            }
                        } else {
                            ctx.actions.push(StackAction::Select(Selected {
                                node: id,
                                mask: true,
                            }));
                        }
                    }
                }
                let name = ui.add(
                    egui::Label::new(RichText::new(&node.name).color(if node.visible {
                        theme::TEXT
                    } else {
                        theme::WEAK
                    }))
                    .sense(Sense::click_and_drag())
                    .truncate()
                    .selectable(false),
                );
                if name.clicked() {
                    ctx.actions.push(StackAction::Select(Selected {
                        node: id,
                        mask: false,
                    }));
                }
                if name.drag_started() {
                    name.dnd_set_drag_payload(LayerDrag(id));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let channel = ctx.channel;
                    let mut b = node.blend(channel);
                    let mut percent = (b.opacity * 100.0).round();
                    if ui
                        .add(
                            DragValue::new(&mut percent)
                                .range(0.0..=100.0)
                                .speed(0.5)
                                .max_decimals(0),
                        )
                        .on_hover_text(format!("{} opacity", channel.label()))
                        .changed()
                    {
                        b.opacity = percent / 100.0;
                    }
                    ComboBox::from_id_salt(("blend", id))
                        .selected_text(RichText::new(b.mode.short()).small())
                        .width(52.0)
                        .show_ui(ui, |ui| {
                            for m in BlendMode::ALL {
                                if folder || m.allowed_on_layers() {
                                    ui.selectable_value(&mut b.mode, m, m.label());
                                }
                            }
                        })
                        .response
                        .on_hover_text(format!(
                            "{} blend mode: {}",
                            channel.label(),
                            b.mode.label()
                        ));
                    if b != node.blend(channel) {
                        node.blend.insert(
                            channel,
                            ChannelBlend {
                                mode: b.mode,
                                opacity: b.opacity,
                            },
                        );
                    }
                });
            });
        })
        .response;

    // Drops: layers reorder (into folders, otherwise above), materials add a fill layer here.
    if let Some(drag) = row.dnd_hover_payload::<LayerDrag>()
        && drag.0 != id
    {
        let y = if folder {
            row.rect.center().y
        } else {
            row.rect.top()
        };
        ui.painter()
            .hline(row.rect.x_range(), y, Stroke::new(2.0, theme::ACCENT));
    }
    if let Some(drag) = row.dnd_release_payload::<LayerDrag>()
        && drag.0 != id
    {
        let placement = if folder {
            Placement::Into(id)
        } else {
            Placement::Above(id)
        };
        ctx.actions.push(StackAction::Move(drag.0, placement));
    }
    if let Some(p) = row.dnd_release_payload::<PresetDrag>() {
        ctx.actions.push(StackAction::AddPreset(p.0, Some(id)));
    }
}

/// Thumbnail color of a fill layer for one channel.
fn swatch(fill: &Fill, channel: Channel) -> Color32 {
    let Some(src) = fill.channels.get(&channel).filter(|s| s.enabled) else {
        return Color32::from_gray(0x30);
    };
    if src.bitmap.is_some() {
        return Color32::from_gray(0x80);
    }
    let [x, y, z] = src.value;
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    match channel {
        Channel::BaseColor => egui::Rgba::from_rgb(x, y, z).into(),
        Channel::Normal => Color32::from_rgb(
            byte(x * 0.5 + 0.5),
            byte(y * 0.5 + 0.5),
            byte(z * 0.5 + 0.5),
        ),
        Channel::Height => Color32::from_gray(byte(x + 0.5)),
        _ => Color32::from_gray(byte(x)),
    }
}

pub fn channel_value_ui(ui: &mut Ui, c: Channel, value: &mut [f32; 3]) {
    match c {
        Channel::BaseColor => {
            let mut rgba = egui::Rgba::from_rgb(value[0], value[1], value[2]);
            if egui::color_picker::color_edit_button_rgba(
                ui,
                &mut rgba,
                egui::color_picker::Alpha::Opaque,
            )
            .changed()
            {
                *value = [rgba.r(), rgba.g(), rgba.b()];
            }
        }
        Channel::Metallic | Channel::Roughness => {
            if ui.add(Slider::new(&mut value[0], 0.0..=1.0)).changed() {
                *value = [value[0]; 3];
            }
        }
        Channel::Height => {
            if ui.add(Slider::new(&mut value[0], -1.0..=1.0)).changed() {
                *value = [value[0]; 3];
            }
        }
        Channel::Normal => {
            ui.label(RichText::new("flat").color(theme::WEAK))
                .on_hover_text(
                    "Uniform normals are flat (0, 0, 1); load a normal map image to add detail.",
                );
        }
    }
}

fn bitmap_buttons(
    ui: &mut Ui,
    current: Option<String>,
    slot: BitmapSlot,
    pick: &mut Option<BitmapSlot>,
    clear: &mut Option<BitmapSlot>,
) {
    match current {
        Some(name) => {
            ui.label(RichText::new(name).color(theme::TEXT))
                .on_hover_text("Image");
            if icon_button(ui, ic::ICON_CLOSE, false, "Remove image").clicked() {
                *clear = Some(slot);
            }
        }
        None => {
            if icon_button(ui, ic::ICON_IMAGE, false, "Use an image…").clicked() {
                *pick = Some(slot);
            }
        }
    }
}

fn projection_ui(ui: &mut Ui, p: &mut Projection, id: &str) {
    ui.label(theme::header("PROJECTION"));
    egui::Grid::new(id)
        .num_columns(2)
        .spacing(vec2(12.0, 6.0))
        .show(ui, |ui| {
            ui.label("Projection");
            ComboBox::from_id_salt((id, "mode"))
                .selected_text(p.mode.label())
                .show_ui(ui, |ui| {
                    for m in ProjectionMode::ALL {
                        ui.selectable_value(&mut p.mode, m, m.label());
                    }
                });
            ui.end_row();
            ui.label("Scale");
            ui.horizontal(|ui| {
                ui.add(
                    DragValue::new(&mut p.scale[0])
                        .speed(0.01)
                        .range(0.001..=1000.0),
                );
                ui.add(
                    DragValue::new(&mut p.scale[1])
                        .speed(0.01)
                        .range(0.001..=1000.0),
                );
            });
            ui.end_row();
            ui.label("Rotation");
            ui.add(Slider::new(&mut p.rotation_deg, -180.0..=180.0).suffix("°"));
            ui.end_row();
            ui.label("Offset");
            ui.horizontal(|ui| {
                ui.add(DragValue::new(&mut p.offset[0]).speed(0.005));
                ui.add(DragValue::new(&mut p.offset[1]).speed(0.005));
            });
            ui.end_row();
            if p.mode != ProjectionMode::Uv {
                ui.label("Size");
                ui.add(
                    DragValue::new(&mut p.size)
                        .speed(0.01)
                        .range(0.001..=10_000.0),
                )
                .on_hover_text("World units covered by one repeat of the image");
                ui.end_row();
                ui.label("Position");
                ui.horizontal(|ui| {
                    for v in &mut p.position {
                        ui.add(DragValue::new(v).speed(0.01));
                    }
                });
                ui.end_row();
                ui.label("Rotation 3D");
                ui.horizontal(|ui| {
                    for v in &mut p.rotation3d_deg {
                        ui.add(DragValue::new(v).speed(0.5).suffix("°"));
                    }
                });
                ui.end_row();
            }
            if p.mode == ProjectionMode::Triplanar {
                ui.label("Hardness");
                ui.add(Slider::new(&mut p.hardness, 0.0..=1.0));
                ui.end_row();
            }
        });
}
