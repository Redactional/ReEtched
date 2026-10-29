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
    NodeKind, Placement, Projection, ProjectionMode, RESOLUTIONS, ResourceId, ShaderModel,
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
#[derive(Clone, Copy)]
enum BitmapSlot {
    Channel(Channel),
    Mask,
}

struct RowCtx<'a> {
    selected: Option<Selected>,
    collapsed: &'a HashSet<NodeId>,
    channel: Channel,
    actions: Vec<StackAction>,
    /// Content thumbnails by (layer, mask?).
    thumbs: std::collections::HashMap<(NodeId, bool), egui::TextureId>,
}

impl State {
    pub fn layers_ui(&mut self, ui: &mut Ui) {
        let has_sel = self.selected.is_some();
        let sel_is_layer = self.selected.is_some_and(|s| !s.mask);
        let channels = self
            .doc
            .texture_sets
            .get(self.selected_part)
            .map_or_else(|| Channel::CORE.to_vec(), |s| s.channels());
        if !channels.contains(&self.layer_channel) {
            self.layer_channel = Channel::BaseColor;
        }
        ui.horizontal(|ui| {
            ComboBox::from_id_salt("layer_channel")
                .selected_text(self.layer_channel.label())
                .width(100.0)
                .show_ui(ui, |ui| {
                    for &c in &channels {
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
                if ui
                    .add_enabled_ui(sel_is_layer, |ui| {
                        icon_button(ui, ic::ICON_SAVE, false, "Save as smart material…")
                    })
                    .inner
                    .clicked()
                {
                    self.save_smart();
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
            });
        });
        ui.separator();

        let thumbs = self.layer_thumbnails(ui.ctx(), self.layer_channel);
        let Some(set) = self.doc.texture_sets.get_mut(self.selected_part) else {
            ui.label(RichText::new("No texture set").color(theme::WEAK));
            return;
        };
        let mut ctx = RowCtx {
            selected: self.selected,
            collapsed: &self.collapsed,
            channel: self.layer_channel,
            actions: Vec::new(),
            thumbs,
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
        let tiles = self.viewport.tiles(self.selected_part);
        let mut load_shader = false;
        if self.doc.texture_sets.get(self.selected_part).is_none() {
            ui.label(RichText::new("No texture set").color(theme::WEAK));
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            let set = &mut self.doc.texture_sets[self.selected_part];
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
                    ui.label("Shader");
                    ComboBox::from_id_salt("tss_shader")
                        .selected_text(set.shader.label())
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            for m in [
                                ShaderModel::MetalRough,
                                ShaderModel::Asm,
                                ShaderModel::OpenPbr,
                            ] {
                                let label = m.label();
                                ui.selectable_value(&mut set.shader, m, label);
                            }
                            if ui.button("Painter GLSL shader…").clicked() {
                                load_shader = true;
                            }
                        });
                    ui.end_row();
                    ui.label("Normal map format");
                    ui.label("OpenGL");
                    ui.end_row();
                });
            let missing: Vec<Channel> = set
                .shader
                .channels()
                .iter()
                .copied()
                .filter(|c| !set.has_channel(*c))
                .collect();
            if !missing.is_empty()
                && ui
                    .button(format!(
                        "Add the {} channels this shader reads",
                        missing.len()
                    ))
                    .clicked()
            {
                set.extra_channels.extend(missing);
            }
            if set.resolution >= 8192 {
                ui.label(
                    RichText::new("8K uses about 1.5 GB of video memory per texture set.")
                        .color(theme::WEAK),
                );
            }

            ui.add_space(8.0);
            ui.label(theme::header("CHANNELS"));
            let mut remove = None;
            egui::Grid::new("channels")
                .num_columns(3)
                .spacing(vec2(12.0, 4.0))
                .show(ui, |ui| {
                    for c in set.channels() {
                        ui.label(c.label());
                        let format = match c {
                            Channel::Normal => "RGB16F",
                            Channel::Height => "L16F",
                            c if c.is_color() => "sRGB8",
                            _ => "L8",
                        };
                        ui.label(RichText::new(format).color(theme::WEAK));
                        if !c.is_core()
                            && icon_button(ui, ic::ICON_CLOSE, false, "Remove this channel")
                                .clicked()
                        {
                            remove = Some(c);
                        }
                        ui.end_row();
                    }
                });
            if let Some(c) = remove {
                set.extra_channels.retain(|&x| x != c);
            }
            let addable: Vec<Channel> = Channel::ALL
                .into_iter()
                .filter(|c| !set.has_channel(*c))
                .collect();
            if !addable.is_empty() {
                ui.menu_button("Add channel", |ui| {
                    for c in addable {
                        if ui.button(c.label()).clicked() {
                            set.extra_channels.push(c);
                            ui.close();
                        }
                    }
                });
            }

            if tiles.len() > 1 || set.is_udim() {
                ui.add_space(8.0);
                ui.label(theme::header("UDIM TILES"));
                // Every tile the mesh uses gets an entry.
                for &t in &tiles {
                    if !set.udim.iter().any(|u| u.number == t) {
                        set.udim.push(reetched_doc::UdimTile {
                            number: t,
                            name: String::new(),
                            resolution: None,
                        });
                    }
                }
                set.udim.sort_by_key(|u| u.number);
                let base = set.resolution;
                egui::Grid::new("udim")
                    .num_columns(3)
                    .spacing(vec2(12.0, 4.0))
                    .show(ui, |ui| {
                        for tile in &mut set.udim {
                            ui.label(tile.number.to_string());
                            ui.add(
                                egui::TextEdit::singleline(&mut tile.name)
                                    .hint_text("name")
                                    .desired_width(110.0),
                            );
                            let label = tile
                                .resolution
                                .map_or(format!("{base} (set)"), |r| r.to_string());
                            ComboBox::from_id_salt(("udim_res", tile.number))
                                .selected_text(label)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut tile.resolution,
                                        None,
                                        "Same as the set",
                                    );
                                    for r in RESOLUTIONS {
                                        ui.selectable_value(
                                            &mut tile.resolution,
                                            Some(r),
                                            r.to_string(),
                                        );
                                    }
                                });
                            ui.end_row();
                        }
                    });
            }
            self.custom_shader_ui(ui);
            ui.add_space(8.0);
            self.mesh_maps_ui(ui);
        });
        if load_shader {
            self.load_custom_shader();
        }
    }

    pub fn properties_ui(&mut self, ui: &mut Ui) {
        if self.properties_tool {
            self.brush_ui(ui);
            return;
        }
        let Some(sel) = self.selected else {
            ui.label(RichText::new("Select a layer to edit its properties").color(theme::WEAK));
            return;
        };
        let names = self.doc.resources.clone();
        let maps = self
            .doc
            .texture_sets
            .get(self.selected_part)
            .map(|s| s.mesh_maps.clone())
            .unwrap_or_default();
        let mut effect_req = crate::effects_ui::EffectRequests::default();
        // The selected fill's procedural material: package, render warnings, rendering?
        let proc_view = self
            .stack()
            .and_then(|s| reetched_doc::find_node(s, sel.node))
            .and_then(|n| match &n.kind {
                NodeKind::Fill(f) => f.procedural.clone(),
                _ => None,
            })
            .and_then(|p| {
                let set = self.doc.texture_sets.get(self.selected_part)?;
                let key = p.render_key(set.resolution, &set.mesh_maps);
                let pkg = self.procedural.package(&p.source, &self.resources);
                let warnings = self
                    .procedural
                    .warnings
                    .get(&key)
                    .cloned()
                    .unwrap_or_default();
                Some((pkg, warnings, self.procedural.is_pending(&key)))
            });
        let mut pick: Option<BitmapSlot> = None;
        let mut clear: Option<BitmapSlot> = None;
        let mut text: Option<BitmapSlot> = None;
        let mut save_mask = false;
        let mut show_brush = false;
        let channels = self
            .doc
            .texture_sets
            .get(self.selected_part)
            .map_or_else(|| Channel::CORE.to_vec(), |s| s.channels());
        let tiles = self.viewport.tiles(self.selected_part);
        let ocio_spaces: Option<Vec<String>> = self.color.ocio.as_ref().map(|o| {
            o.config
                .color_space_names()
                .into_iter()
                .map(str::to_owned)
                .collect()
        });
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
                    ui.label(RichText::new(format!("Mask of “{}”", node.name)).strong().size(14.0));
                    ui.add_space(6.0);
                    theme::section(ui, "Mask", |_| {}, |ui| {
                        theme::grid(ui, "mask", |ui| {
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
                                    ui.selectable_value(&mut mask.background, MaskBackground::White, "White");
                                    ui.selectable_value(&mut mask.background, MaskBackground::Black, "Black");
                                });
                            ui.end_row();
                            ui.label("Invert");
                            ui.checkbox(&mut mask.invert, "");
                            ui.end_row();
                            ui.label("Image");
                            ui.horizontal(|ui| {
                                bitmap_buttons(
                                    ui,
                                    resource_name(&mask.bitmap),
                                    BitmapSlot::Mask,
                                    &mut pick,
                                    &mut clear,
                                    &mut text,
                                );
                            });
                            ui.end_row();
                        });
                    });
                    if mask.bitmap.is_some() {
                        theme::section(ui, "Projection", |_| {}, |ui| {
                            projection_ui(ui, &mut mask.projection, "mask_proj");
                        });
                    }
                    theme::section(ui, "Effects", |_| {}, |ui| {
                        effect_req = crate::effects_ui::mask_effects_ui(ui, mask, &maps);
                    });
                    ui.add_space(4.0);
                    if ui
                        .add_sized([ui.available_width(), 24.0], egui::Button::new("Save as smart mask…"))
                        .clicked()
                    {
                        save_mask = true;
                    }
                    return;
                }

                let kind = match node.kind {
                    NodeKind::Fill(_) => "Fill layer",
                    NodeKind::Paint(_) => "Paint layer",
                    NodeKind::Folder { .. } => "Folder",
                };
                ui.label(RichText::new(kind).strong().size(14.0));
                ui.add_space(6.0);
                theme::section(ui, "Layer", |_| {}, |ui| {
                    theme::grid(ui, "layer", |ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut node.name).desired_width(f32::INFINITY));
                        ui.end_row();
                        if tiles.len() > 1 {
                            ui.label("UDIM tiles")
                                .on_hover_text("Geometry mask: the tiles this layer applies to");
                            ui.horizontal_wrapped(|ui| {
                                for &t in &tiles {
                                    let mut on = node.on_tile(t);
                                    if ui.toggle_value(&mut on, t.to_string()).changed() {
                                        if node.udim_mask.is_empty() {
                                            node.udim_mask = tiles.clone();
                                        }
                                        node.udim_mask.retain(|&x| x != t);
                                        if on {
                                            node.udim_mask.push(t);
                                            node.udim_mask.sort_unstable();
                                        }
                                        if node.udim_mask.len() == tiles.len() {
                                            node.udim_mask.clear();
                                        }
                                    }
                                }
                            });
                            ui.end_row();
                        }
                    });
                    if !matches!(node.kind, NodeKind::Fill(_)) {
                        ui.add_space(4.0);
                        theme::hint(ui, "Blend mode and opacity per channel are set in the layer stack.");
                    }
                });
                let NodeKind::Fill(fill) = &mut node.kind else {
                    if matches!(node.kind, NodeKind::Paint(_)) {
                        theme::section(ui, "Painting", |_| {}, |ui| {
                            theme::hint(
                                ui,
                                "Paint on this layer with the Paint tool (1) and erase with the Eraser (2), in the 3D or 2D view.",
                            );
                            ui.add_space(4.0);
                            if ui
                                .add_sized([ui.available_width(), 24.0], egui::Button::new("Brush settings"))
                                .clicked()
                            {
                                show_brush = true;
                            }
                        });
                    }
                    return;
                };
                if let (Some(p), Some((pkg, warnings, pending))) = (&mut fill.procedural, &proc_view) {
                    let graph = pkg.as_ref().ok().and_then(|pk| pk.graph(&p.graph));
                    let status = match pkg {
                        Ok(_) => Ok(warnings.as_slice()),
                        Err(e) => Err(e.as_str()),
                    };
                    let mut detach = false;
                    theme::section(
                        ui,
                        "Procedural material",
                        |ui| {
                            if ui
                                .small_button("Detach")
                                .on_hover_text("Turn into a plain fill layer")
                                .clicked()
                            {
                                detach = true;
                            }
                            if *pending {
                                ui.spinner();
                            }
                        },
                        |ui| crate::procedural::procedural_ui(ui, p, graph, status),
                    );
                    if detach {
                        fill.procedural = None;
                    }
                }
                theme::section(ui, "Projection", |_| {}, |ui| {
                    projection_ui(ui, &mut fill.projection, "fill_proj");
                });
                theme::section(ui, "Material", |_| {}, |ui| {
                    for &c in &channels {
                        fill.channels.entry(c).or_insert_with(|| ChannelSource {
                            enabled: false,
                            value: c.empty_value(),
                            bitmap: None,
                            color_space: Default::default(),
                            ocio: None,
                        });
                    }
                    theme::chips(
                        ui,
                        fill.channels
                            .iter_mut()
                            .filter(|(c, _)| channels.contains(c))
                            .map(|(c, src)| (c.label(), &mut src.enabled)),
                    );
                    ui.add_space(4.0);
                    if fill.procedural.is_some() {
                        theme::hint(ui, "Channel values come from the procedural material.");
                        return;
                    }
                    // Room for the image and text buttons after each value.
                    ui.spacing_mut().slider_width -= 60.0;
                    theme::grid(ui, "channels", |ui| {
                        for &c in &channels {
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
                                    &mut text,
                                );
                                if src.bitmap.is_some() {
                                    let label = match &src.ocio {
                                        Some(n) if ocio_spaces.is_some() => n.clone(),
                                        _ => src.color_space.label().to_owned(),
                                    };
                                    ComboBox::from_id_salt(("color_space", c))
                                        .width(90.0)
                                        .selected_text(label)
                                        .show_ui(ui, |ui| {
                                            for cs in reetched_doc::ColorSpace::ALL {
                                                if ui.selectable_label(src.ocio.is_none() && src.color_space == cs, cs.label()).clicked() {
                                                    src.color_space = cs;
                                                    src.ocio = None;
                                                }
                                            }
                                            if let Some(spaces) = &ocio_spaces {
                                                ui.separator();
                                                ui.label(theme::header("OCIO"));
                                                for n in spaces {
                                                    if ui.selectable_label(src.ocio.as_ref() == Some(n), n).clicked() {
                                                        src.ocio = Some(n.clone());
                                                    }
                                                }
                                            }
                                        })
                                        .response
                                        .on_hover_text("How the image's values are read");
                                }
                            });
                            ui.end_row();
                        }
                    });
                });
            });

        if save_mask {
            self.save_smart_mask();
        }
        if let Some(i) = effect_req.pick_color {
            self.id_pick = Some((sel.node, i));
            self.info("Click the model to pick an ID color".into());
        }
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
        if let Some(slot) = text {
            let target = match slot {
                BitmapSlot::Channel(c) => crate::text_ui::TextTarget::Fill(sel.node, c),
                BitmapSlot::Mask => crate::text_ui::TextTarget::Mask(sel.node),
            };
            self.text_ui = Some(crate::text_ui::TextUi::new(target));
        }
        if let Some(slot) = pick
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", crate::images::IMAGE_EXTENSIONS)
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
        let mut run_mesh = false;
        let mut run_psd = false;
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
                        let udim = set.is_udim().then_some("1001");
                        let name =
                            expand_name(&map.file_name, &names.0, &names.1, &set.name, udim);
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
                ui.add_space(8.0);
                ui.separator();
                ui.label(theme::header("MESH WITH TEXTURES"));
                ui.horizontal(|ui| {
                    ComboBox::from_id_salt("mesh_format")
                        .selected_text(e.mesh_format.label())
                        .show_ui(ui, |ui| {
                            for f in MeshFormat::ALL {
                                ui.selectable_value(&mut e.mesh_format, f, f.label());
                            }
                        });
                    ui.label("Displacement");
                    ui.add(egui::DragValue::new(&mut e.displacement).speed(0.001).range(0.0..=10.0))
                        .on_hover_text("Moves vertices along their normals by the height channel × this (world units); 0 = off");
                });
                ui.label(
                    RichText::new("Writes the mesh plus PBR Metallic Roughness textures it references, into the folder above.")
                        .color(theme::WEAK),
                );
                let ready = !e.dir.trim().is_empty();
                if ui.add_enabled(ready, egui::Button::new("Export mesh")).clicked() {
                    run_mesh = true;
                }
                ui.add_space(8.0);
                ui.separator();
                ui.label(theme::header("LAYERED PSD"));
                ui.label(
                    RichText::new("One Photoshop file per texture set and channel, with layers, folders, masks, blend modes and opacities.")
                        .color(theme::WEAK),
                );
                if ui.add_enabled(ready, egui::Button::new("Export PSD")).clicked() {
                    run_psd = true;
                }
            });
        self.export.open = open;
        if run_mesh {
            self.export_mesh();
        }
        if run_psd {
            self.export_psd();
        }
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
        let Some(preset) = self.export.presets.get(self.export.preset).cloned() else {
            return;
        };
        let (settings, include) = (self.export.settings.clone(), self.export.include.clone());
        match self.export_textures(&dir, &preset, &settings, &include) {
            Ok(files) => {
                let n: usize = files.values().map(Vec::len).sum();
                self.info(format!("Exported {n} textures to {}", dir.display()));
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    /// Writes the included texture sets' textures into `dir`; returns the files
    /// written per texture set.
    pub(crate) fn export_textures(
        &mut self,
        dir: &std::path::Path,
        preset: &reetched_doc::export::ExportPreset,
        settings: &reetched_doc::export::ExportSettings,
        include: &[bool],
    ) -> anyhow::Result<std::collections::BTreeMap<String, Vec<PathBuf>>> {
        use anyhow::Context;
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Could not create {}", dir.display()))?;
        // Make sure every composite reflects the current document.
        let warnings = self.viewport.sync(
            &self.device,
            &self.queue,
            &self.doc.texture_sets,
            &self.resources,
            self.preview_map(),
        );
        for w in warnings {
            self.warn(w);
        }
        let project = self.project_name();
        let mesh = mesh_name(&self.doc.mesh_source);
        let mut out = std::collections::BTreeMap::new();
        for (i, set) in self.doc.texture_sets.clone().iter().enumerate() {
            if !include.get(i).copied().unwrap_or(false) {
                continue;
            }
            let tiles = self.viewport.tiles(i);
            let udim = tiles.len() > 1 || set.is_udim();
            for tile in tiles {
                let files = self
                    .viewport
                    .read_back(&self.device, &self.queue, i, tile)
                    .map(|mut data| {
                        data.ambient_occlusion = baked_ao(set, tile, &self.resources, data.width);
                        data
                    })
                    .and_then(|data| {
                        export_texture_set(
                            preset,
                            settings,
                            &data,
                            dir,
                            (&project, &mesh, &set.name),
                            udim.then_some(tile),
                        )
                    })
                    .with_context(|| format!("Export of {} failed", set.name))?;
                out.entry(set.name.clone())
                    .or_insert_with(Vec::new)
                    .extend(files);
            }
        }
        Ok(out)
    }
}

impl State {
    /// Writes the mesh (USD, USDZ or OBJ) and the textures it references.
    pub fn export_mesh(&mut self) {
        let dir = PathBuf::from(self.export.dir.trim());
        let format = self.export.mesh_format;
        let name = mesh_name(&self.doc.mesh_source);
        let path = dir.join(format!("{name}.{}", format.extension()));
        match self.export_mesh_to(&path, format) {
            Ok(()) => self.info(format!("Exported {}", path.display())),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    /// Writes the mesh to `path`, with its textures (current preset) beside it.
    pub(crate) fn export_mesh_to(
        &mut self,
        path: &std::path::Path,
        format: MeshFormat,
    ) -> anyhow::Result<()> {
        use anyhow::Context;
        use reetched_doc::export::{UDIM_TOKEN, expand_name};
        use reetched_io::export::{MaterialFiles, write_obj, write_usda, write_usdc, write_usdz};
        let dir = path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_owned();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Could not create {}", dir.display()))?;
        for w in self.viewport.sync(
            &self.device,
            &self.queue,
            &self.doc.texture_sets,
            &self.resources,
            self.preview_map(),
        ) {
            self.warn(w);
        }
        let preset = reetched_doc::export::builtin_presets().remove(0);
        let project = self.project_name();
        let mesh_name = mesh_name(&self.doc.mesh_source);
        let mut materials = Vec::new();
        // Height per (texture set, UDIM tile), for displacement.
        let mut heights = std::collections::HashMap::new();
        for (i, set) in self.doc.texture_sets.clone().iter().enumerate() {
            let tiles = self.viewport.tiles(i);
            let udim = tiles.len() > 1 || set.is_udim();
            for &tile in &tiles {
                let result = self
                    .viewport
                    .read_back(&self.device, &self.queue, i, tile)
                    .and_then(|data| {
                        export_texture_set(
                            &preset,
                            &self.export.settings,
                            &data,
                            &dir,
                            (&project, &mesh_name, &set.name),
                            udim.then_some(tile),
                        )?;
                        Ok(data)
                    });
                let data = result.with_context(|| format!("Export of {} failed", set.name))?;
                heights.insert((i, tile), (data.width, data.height_map));
            }
            // The preset's maps, in order: base color, metallic, roughness, normal, height,
            // emissive, opacity. UDIM sets reference all tiles through the <UDIM> token.
            let ext = self.export.settings.format.extension();
            let file = |k: usize| {
                preset.maps.get(k).map(|m| {
                    let token = udim.then_some(UDIM_TOKEN);
                    let name = expand_name(&m.file_name, &project, &mesh_name, &set.name, token);
                    format!("{name}.{ext}")
                })
            };
            materials.push(MaterialFiles {
                name: set.name.clone(),
                base_color: file(0),
                metallic: file(1),
                roughness: file(2),
                normal: file(3),
                emissive: file(5),
                opacity: file(6),
            });
        }
        let scale = self.export.displacement;
        let displace = |part: usize, uv: [f32; 2]| -> f32 {
            let tile = reetched_doc::udim_of(uv);
            let Some((res, h)) = heights.get(&(part, tile)) else {
                return 0.0;
            };
            let [u0, v0] = reetched_doc::udim_origin(tile);
            let (u, v) = (uv[0] - u0, uv[1] - v0);
            let x = ((u.rem_euclid(1.0) * *res as f32) as u32).min(res - 1);
            let y = (((1.0 - v).rem_euclid(1.0) * *res as f32) as u32).min(res - 1);
            h[(y * res + x) as usize] * scale
        };
        let displace: Option<reetched_io::export::Displace> = (scale > 0.0).then_some(&displace);
        match format {
            MeshFormat::Usda => write_usda(path, &self.mesh, displace, &materials),
            MeshFormat::Usdc => write_usdc(path, &self.mesh, displace, &materials),
            MeshFormat::Usdz => write_usdz(path, &self.mesh, displace, &materials, &dir),
            MeshFormat::Obj => write_obj(path, &self.mesh, displace, &materials),
        }
        .context("Mesh export failed")
    }
}

/// Mesh export file types.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MeshFormat {
    Usda,
    Usdc,
    Usdz,
    Obj,
}

impl MeshFormat {
    pub const ALL: [MeshFormat; 4] = [
        MeshFormat::Usda,
        MeshFormat::Usdc,
        MeshFormat::Usdz,
        MeshFormat::Obj,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            MeshFormat::Usda => "usda",
            MeshFormat::Usdc => "usdc",
            MeshFormat::Usdz => "usdz",
            MeshFormat::Obj => "obj",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MeshFormat::Usda => "USD (.usda)",
            MeshFormat::Usdc => "USD binary (.usdc)",
            MeshFormat::Usdz => "USDZ package",
            MeshFormat::Obj => "OBJ",
        }
    }
}

/// The texture set's baked AO, resized to `res` (empty if there is none).
fn baked_ao(
    set: &reetched_doc::TextureSet,
    tile: u32,
    resources: &reetched_doc::Resources,
    res: u32,
) -> Vec<f32> {
    let Some(bytes) = set
        .mesh_maps_for(tile)
        .get(&reetched_doc::MeshMap::AmbientOcclusion)
        .and_then(|id| resources.files.get(id))
    else {
        return Vec::new();
    };
    match crate::images::decode(bytes) {
        Ok(img) => image::imageops::resize(
            &img.to_luma32f(),
            res,
            res,
            image::imageops::FilterType::Triangle,
        )
        .into_raw(),
        Err(_) => Vec::new(),
    }
}

pub(crate) fn mesh_name(source: &Option<String>) -> String {
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
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                if let Some(tex) = ctx.thumbs.get(&(id, false)) {
                    ui.painter()
                        .rect_filled(thumb, 2.0, Color32::from_gray(0x3a));
                    ui.painter().image(*tex, thumb, uv, Color32::WHITE);
                } else {
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
                    let tint = if mask.enabled {
                        Color32::WHITE
                    } else {
                        Color32::from_gray(80)
                    };
                    ui.painter().rect_filled(
                        m,
                        2.0,
                        fill.gamma_multiply(if mask.enabled { 1.0 } else { 0.3 }),
                    );
                    if let Some(tex) = ctx.thumbs.get(&(id, true)) {
                        let uv =
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                        ui.painter().image(*tex, m, uv, tint);
                    }
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
        Channel::BaseColor | Channel::Emissive => egui::Rgba::from_rgb(x, y, z).into(),
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
        _ if c.is_color() => {
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
        Channel::Height => {
            if ui.add(Slider::new(&mut value[0], -1.0..=1.0)).changed() {
                *value = [value[0]; 3];
            }
        }
        Channel::AnisotropyAngle => {
            let mut deg = value[0] * 360.0;
            if ui
                .add(Slider::new(&mut deg, 0.0..=360.0).suffix("°"))
                .changed()
            {
                *value = [deg / 360.0; 3];
            }
        }
        Channel::Normal => {
            ui.label(RichText::new("flat").color(theme::WEAK))
                .on_hover_text(
                    "Uniform normals are flat (0, 0, 1); load a normal map image to add detail.",
                );
        }
        _ => {
            if ui.add(Slider::new(&mut value[0], 0.0..=1.0)).changed() {
                *value = [value[0]; 3];
            }
        }
    }
}

fn bitmap_buttons(
    ui: &mut Ui,
    current: Option<String>,
    slot: BitmapSlot,
    pick: &mut Option<BitmapSlot>,
    clear: &mut Option<BitmapSlot>,
    text: &mut Option<BitmapSlot>,
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
            if icon_button(ui, ic::ICON_TEXT_FIELDS, false, "Use text…").clicked() {
                *text = Some(slot);
            }
        }
    }
}

fn projection_ui(ui: &mut Ui, p: &mut Projection, id: &str) {
    theme::grid(ui, id, |ui| {
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
        ui.label("Repeat");
        ui.checkbox(&mut p.repeat, "")
            .on_hover_text("Off: the image appears once; the fill covers nothing outside it");
        ui.end_row();
        if p.mode == ProjectionMode::Planar {
            ui.label("Front faces only");
            ui.checkbox(&mut p.front_only, "")
                .on_hover_text("Skip surfaces facing away from the projector");
            ui.end_row();
        }
        if !p.mode.is_uv() {
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
        if !p.mode.is_uv() {
            ui.label("Symmetry");
            ui.horizontal(|ui| {
                for (m, axis) in p.mirror.iter_mut().zip(["X", "Y", "Z"]) {
                    ui.toggle_value(m, axis)
                        .on_hover_text(format!("Mirror the projection across {axis} = 0"));
                }
            });
            ui.end_row();
        }
        if p.mode == ProjectionMode::Triplanar {
            ui.label("Hardness");
            ui.add(Slider::new(&mut p.hardness, 0.0..=1.0));
            ui.end_row();
        }
        if p.mode == ProjectionMode::Warp {
            ui.label("Warp grid");
            ui.horizontal(|ui| {
                ui.label(RichText::new("Drag its points in the 2D view").color(theme::WEAK));
                if ui.small_button("Reset").clicked() {
                    p.warp = reetched_doc::warp_identity();
                }
            });
            ui.end_row();
        }
    });
}
