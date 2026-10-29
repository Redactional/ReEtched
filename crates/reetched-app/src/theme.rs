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

/// Label column width of every Properties grid, so rows line up across sections.
pub const LABEL_W: f32 = 104.0;

/// A collapsible Properties section: a full-width header band, then `body`.
/// `extra` draws small buttons at the header's right end. Open state persists.
pub fn section(
    ui: &mut egui::Ui,
    title: &str,
    extra: impl FnOnce(&mut egui::Ui),
    body: impl FnOnce(&mut egui::Ui),
) {
    use egui::{Align, Layout, Sense, UiBuilder, collapsing_header::CollapsingState};
    use egui_material_icons::icons as ic;
    let id = ui.make_persistent_id(("section", title));
    let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, true);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    let hot = resp.hovered();
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(2),
        if hot {
            STRIP.gamma_multiply(1.3)
        } else {
            HEADER
        },
    );
    let icon = if state.is_open() {
        ic::ICON_EXPAND_MORE
    } else {
        ic::ICON_CHEVRON_RIGHT
    };
    ui.painter().text(
        rect.left_center() + vec2(6.0, 0.0),
        egui::Align2::LEFT_CENTER,
        icon.codepoint,
        FontId::new(16.0, icon.font_family()),
        WEAK,
    );
    ui.painter().text(
        rect.left_center() + vec2(26.0, 0.0),
        egui::Align2::LEFT_CENTER,
        title.to_uppercase(),
        FontId::new(11.0, FontFamily::Proportional),
        if hot { Color32::WHITE } else { TEXT },
    );
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(6.0, 0.0)))
            .layout(Layout::right_to_left(Align::Center)),
        extra,
    );
    if resp.clicked() {
        state.toggle(ui);
    }
    state.show_body_unindented(ui, |ui| {
        ui.add_space(6.0);
        // Sliders fill the row after the label column and the value box.
        ui.spacing_mut().slider_width = (ui.available_width() - LABEL_W - 76.0).clamp(60.0, 240.0);
        body(ui);
        ui.add_space(8.0);
    });
    ui.add_space(4.0);
}

/// Two-column label/value grid with the shared label width.
pub fn grid(ui: &mut egui::Ui, id: impl egui::AsIdSalt, rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing(vec2(8.0, 6.0))
        .min_col_width(LABEL_W)
        .show(ui, rows);
}

/// Equal-width toggle buttons, three per row (channel pickers and the like).
pub fn chips<'a>(ui: &mut egui::Ui, items: impl IntoIterator<Item = (&'a str, &'a mut bool)>) {
    let gap = ui.spacing().item_spacing.x;
    let w = ((ui.available_width() - 2.0 * gap) / 3.0).floor();
    ui.horizontal_wrapped(|ui| {
        for (label, on) in items {
            if ui
                .add(
                    egui::Button::new(label)
                        .selected(*on)
                        .min_size(vec2(w, 22.0)),
                )
                .clicked()
            {
                *on = !*on;
            }
        }
    });
}

/// Small grey explanatory text.
pub fn hint(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).color(WEAK).size(11.5));
}
