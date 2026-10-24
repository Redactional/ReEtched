//! Text resources: a small window that turns a line of text into an SVG image
//! resource and hands it to whatever asked for it (a fill or mask bitmap, the
//! brush alpha, the Projection tool's image).

use crate::{
    app::State,
    images::{TextStyle, font_families, text_svg},
    theme,
};
use egui::{ComboBox, RichText};
use reetched_doc::{Channel, NodeId, NodeKind, ResourceId, ResourceInfo, find_node_mut};

/// Where a new text resource goes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TextTarget {
    /// A fill layer channel's bitmap.
    Fill(NodeId, Channel),
    /// A layer mask's bitmap.
    Mask(NodeId),
    BrushAlpha,
    Stencil,
}

pub struct TextUi {
    pub target: TextTarget,
    pub style: TextStyle,
    /// Loaded when the window first opens (scanning system fonts takes a moment).
    families: Vec<String>,
}

impl TextUi {
    pub fn new(target: TextTarget) -> Self {
        let families = font_families();
        let mut style = TextStyle::default();
        if !families.contains(&style.family)
            && let Some(f) = families.first()
        {
            style.family = f.clone();
        }
        Self {
            target,
            style,
            families,
        }
    }
}

impl State {
    pub fn text_window(&mut self, ctx: &egui::Context) {
        let Some(t) = &mut self.text_ui else { return };
        let mut open = true;
        let mut create = false;
        egui::Window::new(theme::header("TEXT"))
            .open(&mut open)
            .collapsible(false)
            .default_width(320.0)
            .show(ctx, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut t.style.text)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
                egui::Grid::new("text_style")
                    .num_columns(2)
                    .spacing(egui::vec2(12.0, 6.0))
                    .show(ui, |ui| {
                        ui.label("Font");
                        ComboBox::from_id_salt("text_font")
                            .selected_text(&t.style.family)
                            .width(200.0)
                            .show_ui(ui, |ui| {
                                for f in &t.families {
                                    ui.selectable_value(&mut t.style.family, f.clone(), f);
                                }
                            });
                        ui.end_row();
                        ui.label("Style");
                        ui.horizontal(|ui| {
                            ui.toggle_value(&mut t.style.bold, RichText::new("B").strong());
                            ui.toggle_value(&mut t.style.italic, RichText::new("I").italics());
                        });
                        ui.end_row();
                        ui.label("Align");
                        ui.horizontal(|ui| {
                            for (k, name) in ["Left", "Center", "Right"].into_iter().enumerate() {
                                ui.selectable_value(&mut t.style.align, k as u8, name);
                            }
                        });
                        ui.end_row();
                        ui.label("Line spacing");
                        ui.add(egui::Slider::new(&mut t.style.line_height, 0.8..=2.0));
                        ui.end_row();
                    });
                ui.label(
                    RichText::new(
                        "White text on a transparent background, kept sharp at any size.",
                    )
                    .color(theme::WEAK),
                );
                create = ui.button("Create").clicked();
            });
        if !open {
            self.text_ui = None;
            return;
        }
        if !create {
            return;
        }
        let Some(t) = self.text_ui.take() else { return };
        match text_svg(&t.style) {
            Ok(svg) => {
                let id = self.resources.add(svg.into_bytes());
                let first = t.style.text.lines().next().unwrap_or("").trim();
                let name = format!("Text {}.svg", first.chars().take(24).collect::<String>());
                self.doc
                    .resources
                    .insert(id.clone(), ResourceInfo { name: name.clone() });
                self.use_resource(t.target, id, ctx);
                self.info(format!("Created {name}"));
            }
            Err(e) => {
                self.error(format!("Could not make the text image: {e:#}"));
                self.text_ui = Some(t);
            }
        }
    }

    /// Puts image resource `id` where `target` says.
    pub fn use_resource(&mut self, target: TextTarget, id: ResourceId, ctx: &egui::Context) {
        match target {
            TextTarget::BrushAlpha => self.brush.alpha = Some(id),
            TextTarget::Stencil => {
                if let Some(bytes) = self.resources.files.get(&id).cloned() {
                    self.set_stencil(ctx, &bytes, "text");
                }
            }
            TextTarget::Fill(node, c) => {
                if let Some(n) = self.stack_mut().and_then(|s| find_node_mut(s, node))
                    && let NodeKind::Fill(fill) = &mut n.kind
                    && let Some(src) = fill.channels.get_mut(&c)
                {
                    src.bitmap = Some(id);
                    src.enabled = true;
                }
            }
            TextTarget::Mask(node) => {
                if let Some(mask) = self
                    .stack_mut()
                    .and_then(|s| find_node_mut(s, node))
                    .and_then(|n| n.mask.as_mut())
                {
                    mask.bitmap = Some(id);
                }
            }
        }
    }
}
