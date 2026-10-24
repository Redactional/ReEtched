//! Dark, compact look modelled on Substance Painter.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, vec2};

pub const ACCENT: Color32 = Color32::from_rgb(0x4a, 0x90, 0xd9);
pub const PANEL: Color32 = Color32::from_rgb(0x2c, 0x2c, 0x2c);
pub const HEADER: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
pub const STRIP: Color32 = Color32::from_rgb(0x25, 0x25, 0x25);
pub const GAP: Color32 = Color32::from_rgb(0x17, 0x17, 0x17);
pub const ROW_SELECTED: Color32 = Color32::from_rgb(0x34, 0x3a, 0x42);
pub const TEXT: Color32 = Color32::from_rgb(0xd4, 0xd4, 0xd4);
pub const WEAK: Color32 = Color32::from_rgb(0x8c, 0x8c, 0x8c);
pub const ERROR: Color32 = Color32::from_rgb(0xe0, 0x6c, 0x5c);
pub const WARN: Color32 = Color32::from_rgb(0xd9, 0xa6, 0x4e);

pub fn apply(ctx: &egui::Context) {
    egui_material_icons::initialize(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        let radius = CornerRadius::same(2);
        let v = &mut s.visuals;
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.extreme_bg_color = Color32::from_rgb(0x1e, 0x1e, 0x1e);
        v.faint_bg_color = Color32::from_rgb(0x30, 0x30, 0x30);
        v.window_stroke = Stroke::new(1.0, GAP);
        v.window_corner_radius = CornerRadius::same(3);
        v.menu_corner_radius = CornerRadius::same(3);
        v.selection.bg_fill = Color32::from_rgb(0x2b, 0x52, 0x7c);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;

        let w = &mut v.widgets;
        let shade =
            |fill: Color32, stroke: Color32, text: Color32, wv: &mut egui::style::WidgetVisuals| {
                wv.bg_fill = fill;
                wv.weak_bg_fill = fill;
                wv.bg_stroke = Stroke::new(1.0, stroke);
                wv.fg_stroke = Stroke::new(1.0, text);
                wv.corner_radius = radius;
            };
        shade(PANEL, GAP, TEXT, &mut w.noninteractive);
        shade(
            Color32::from_rgb(0x3a, 0x3a, 0x3a),
            Color32::from_rgb(0x45, 0x45, 0x45),
            TEXT,
            &mut w.inactive,
        );
        shade(
            Color32::from_rgb(0x46, 0x46, 0x46),
            Color32::from_rgb(0x5a, 0x5a, 0x5a),
            Color32::WHITE,
            &mut w.hovered,
        );
        shade(
            Color32::from_rgb(0x52, 0x52, 0x52),
            ACCENT,
            Color32::WHITE,
            &mut w.active,
        );
        shade(
            Color32::from_rgb(0x3e, 0x3e, 0x3e),
            Color32::from_rgb(0x55, 0x55, 0x55),
            TEXT,
            &mut w.open,
        );

        s.spacing.item_spacing = vec2(6.0, 4.0);
        s.spacing.button_padding = vec2(6.0, 2.0);
        s.spacing.interact_size.y = 20.0;
        s.spacing.slider_width = 120.0;
        s.text_styles = [
            (
                TextStyle::Small,
                FontId::new(10.5, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(12.5, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(12.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(13.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.0, FontFamily::Monospace),
            ),
        ]
        .into();
    });
}

/// Small, bold, upper-case panel title like Painter's headers.
pub fn header(text: &str) -> egui::RichText {
    egui::RichText::new(text).size(11.0).strong()
}
