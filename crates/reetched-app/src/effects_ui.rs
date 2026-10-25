//! Mask effect stack editor (generators, levels, color selection).

use crate::{app::icon_button, theme};
use egui::{ComboBox, RichText, Slider, Ui, vec2};
use egui_material_icons::icons as ic;
use reetched_doc::{
    Generator, GeneratorKind, MAX_MASK_EFFECTS, Mask, MaskBlend, MaskEffect, MaskEffectKind,
    MeshMap,
};
use std::collections::BTreeMap;

/// Things the effect editor asks the app to do after drawing.
#[derive(Default)]
pub struct EffectRequests {
    /// Start picking an ID color from the model for this effect index.
    pub pick_color: Option<usize>,
}

/// Draws the mask's effect stack. `maps`: which mesh maps the texture set has.
pub fn mask_effects_ui(
    ui: &mut Ui,
    mask: &mut Mask,
    maps: &BTreeMap<MeshMap, String>,
) -> EffectRequests {
    let mut req = EffectRequests::default();
    ui.horizontal(|ui| {
        ui.label(theme::header("EFFECTS"));
        let full = mask.effects.len() >= MAX_MASK_EFFECTS;
        ui.add_enabled_ui(!full, |ui| {
            ui.menu_button("Add effect", |ui| {
                ui.label(RichText::new("Generators").color(theme::WEAK));
                for kind in GeneratorKind::ALL {
                    if ui.button(kind.label()).clicked() {
                        mask.effects.push(MaskEffect::new(MaskEffectKind::Generator(
                            Generator::new(kind),
                        )));
                    }
                }
                ui.separator();
                if ui.button("Levels").clicked() {
                    mask.effects.push(MaskEffect::new(MaskEffectKind::Levels {
                        in_min: 0.0,
                        in_max: 1.0,
                        gamma: 1.0,
                        out_min: 0.0,
                        out_max: 1.0,
                    }));
                }
                if ui.button("Color selection").clicked() {
                    mask.effects
                        .push(MaskEffect::new(MaskEffectKind::ColorSelection {
                            colors: Vec::new(),
                            tolerance: 0.1,
                        }));
                }
            })
            .response
            .on_disabled_hover_text(format!("Up to {MAX_MASK_EFFECTS} effects per mask"));
        });
    });
    if mask.effects.is_empty() {
        ui.label(
            RichText::new("No effects. Generators read the baked mesh maps (edge wear, dirt, …).")
                .color(theme::WEAK),
        );
    }

    let mut remove = None;
    let mut swap = None;
    let count = mask.effects.len();
    for (i, e) in mask.effects.iter_mut().enumerate() {
        egui::Frame::new()
            .fill(theme::ROW_SELECTED.gamma_multiply(0.6))
            .inner_margin(6.0)
            .corner_radius(3)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.checkbox(&mut e.enabled, RichText::new(e.kind.label()).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, ic::ICON_DELETE, false, "Remove").clicked() {
                            remove = Some(i);
                        }
                        if i + 1 < count
                            && icon_button(ui, ic::ICON_EXPAND_MORE, false, "Move down").clicked()
                        {
                            swap = Some((i, i + 1));
                        }
                        if i > 0
                            && icon_button(ui, ic::ICON_CHEVRON_RIGHT, false, "Move up").clicked()
                        {
                            swap = Some((i, i - 1));
                        }
                    });
                });
                egui::Grid::new(("fx", i))
                    .num_columns(2)
                    .spacing(vec2(10.0, 4.0))
                    .show(ui, |ui| {
                        ui.label("Blend");
                        ComboBox::from_id_salt(("fx_blend", i))
                            .selected_text(format!("{:?}", e.blend))
                            .show_ui(ui, |ui| {
                                for b in MaskBlend::ALL {
                                    ui.selectable_value(&mut e.blend, b, format!("{b:?}"));
                                }
                            });
                        ui.end_row();
                        ui.label("Opacity");
                        ui.add(Slider::new(&mut e.opacity, 0.0..=1.0));
                        ui.end_row();
                        match &mut e.kind {
                            MaskEffectKind::Generator(g) => {
                                if let Some(need) = g.kind.needs()
                                    && !maps.contains_key(&need)
                                {
                                    ui.label("");
                                    ui.label(
                                        RichText::new(format!(
                                            "Needs a baked {} map",
                                            need.label().to_lowercase()
                                        ))
                                        .color(theme::ERROR),
                                    );
                                    ui.end_row();
                                }
                                ui.label("Type");
                                ComboBox::from_id_salt(("gen_kind", i))
                                    .selected_text(g.kind.label())
                                    .show_ui(ui, |ui| {
                                        for k in GeneratorKind::ALL {
                                            ui.selectable_value(&mut g.kind, k, k.label());
                                        }
                                    });
                                ui.end_row();
                                ui.label("Level");
                                ui.add(Slider::new(&mut g.level, -0.5..=1.5));
                                ui.end_row();
                                ui.label("Contrast");
                                ui.add(Slider::new(&mut g.contrast, 0.001..=1.0).logarithmic(true));
                                ui.end_row();
                                ui.label("Noise");
                                ui.add(Slider::new(&mut g.noise, 0.0..=2.0));
                                ui.end_row();
                                ui.label("Noise scale");
                                ui.add(
                                    Slider::new(&mut g.noise_scale, 0.5..=64.0).logarithmic(true),
                                );
                                ui.end_row();
                                if g.kind == GeneratorKind::PositionGradient {
                                    ui.label("Axis");
                                    ui.horizontal(|ui| {
                                        for (a, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                                            ui.selectable_value(&mut g.axis, a as u8, name);
                                        }
                                    });
                                    ui.end_row();
                                }
                                if g.kind == GeneratorKind::Light {
                                    ui.label("Light direction");
                                    ui.add(Slider::new(&mut g.light[0], 0.0..=1.0).text("around"));
                                    ui.end_row();
                                    ui.label("");
                                    ui.add(Slider::new(&mut g.light[1], -1.0..=1.0).text("height"));
                                    ui.end_row();
                                }
                                ui.label("Invert");
                                ui.checkbox(&mut g.invert, "");
                                ui.end_row();
                            }
                            MaskEffectKind::Levels {
                                in_min,
                                in_max,
                                gamma,
                                out_min,
                                out_max,
                            } => {
                                ui.label("Input");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::DragValue::new(in_min).range(0.0..=1.0).speed(0.005),
                                    );
                                    ui.add(
                                        egui::DragValue::new(in_max).range(0.0..=1.0).speed(0.005),
                                    );
                                });
                                ui.end_row();
                                ui.label("Gamma");
                                ui.add(Slider::new(gamma, 0.1..=10.0).logarithmic(true));
                                ui.end_row();
                                ui.label("Output");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::DragValue::new(out_min).range(0.0..=1.0).speed(0.005),
                                    );
                                    ui.add(
                                        egui::DragValue::new(out_max).range(0.0..=1.0).speed(0.005),
                                    );
                                });
                                ui.end_row();
                            }
                            MaskEffectKind::ColorSelection { colors, tolerance } => {
                                if !maps.contains_key(&MeshMap::Id) {
                                    ui.label("");
                                    ui.label(
                                        RichText::new("Needs a baked ID map").color(theme::ERROR),
                                    );
                                    ui.end_row();
                                }
                                ui.label("Colors");
                                ui.horizontal_wrapped(|ui| {
                                    let mut drop = None;
                                    for (k, c) in colors.iter_mut().enumerate() {
                                        ui.color_edit_button_rgb(c);
                                        if ui.small_button("✕").clicked() {
                                            drop = Some(k);
                                        }
                                    }
                                    if let Some(k) = drop {
                                        colors.remove(k);
                                    }
                                    if colors.len() < 4 && ui.button("Pick from model").clicked() {
                                        req.pick_color = Some(i);
                                    }
                                });
                                ui.end_row();
                                ui.label("Tolerance");
                                ui.add(Slider::new(tolerance, 0.001..=0.5).logarithmic(true));
                                ui.end_row();
                            }
                        }
                    });
            });
    }
    if let Some(i) = remove {
        mask.effects.remove(i);
    }
    if let Some((a, b)) = swap {
        mask.effects.swap(a, b);
    }
    req
}
