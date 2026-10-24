//! Color management: an OCIO config (built-in ACES ones or a file), whose
//! display/view becomes the 3D view's display transform (baked into a LUT), and
//! whose color spaces bitmaps can be read in.

use crate::{app::State, theme, viewport::DisplayLut};
use egui::{ComboBox, RichText, Ui};
use reetched_color::{Config, Shaper};
use std::sync::Arc;

/// LUT size for the viewport (65³: ~1 % error on saturated ACES 2.0 colors).
const LUT_SIZE: u32 = 65;

/// The config and the choices made in it.
#[derive(Clone)]
pub struct Ocio {
    pub config: Arc<Config>,
    /// "builtin:<name>" or a file path.
    pub source: String,
    /// The color space ReEtched renders and composites in (linear Rec.709 primaries).
    pub working: String,
    pub display: String,
    pub view: String,
    /// "" = the view's own looks.
    pub look: String,
}

#[derive(Default)]
pub struct ColorUi {
    pub ocio: Option<Ocio>,
}

/// The config's color space for linear Rec.709 / sRGB primaries, which is what
/// ReEtched renders in; else its scene_linear role.
fn find_working(config: &Config) -> String {
    let names = config.color_space_names();
    let wanted = [
        "lin_rec709_srgb",
        "Linear Rec.709 (sRGB)",
        "lin_srgb",
        "Utility - Linear - sRGB",
        "linear",
    ];
    for w in wanted {
        if let Some(cs) = config.color_space(w) {
            return cs.name.clone();
        }
    }
    config
        .role("scene_linear")
        .map(str::to_owned)
        .or_else(|| names.first().map(|n| n.to_string()))
        .unwrap_or_default()
}

impl Ocio {
    pub fn new(config: Config, source: String) -> Self {
        let working = find_working(&config);
        let display = config.default_display().unwrap_or_default().to_owned();
        let view = config.default_view(&display).unwrap_or_default().to_owned();
        Self {
            config: Arc::new(config),
            source,
            working,
            display,
            view,
            look: String::new(),
        }
    }

    /// The display transform as a LUT over scene-linear input.
    pub fn bake(&self) -> anyhow::Result<DisplayLut> {
        let looks = (!self.look.is_empty()).then_some(self.look.as_str());
        let p = self
            .config
            .display_processor(&self.working, &self.display, &self.view, looks)?;
        let shaper = Shaper::default();
        let lut = p.bake_lut3d(LUT_SIZE, shaper);
        Ok(DisplayLut {
            size: lut.size,
            log2_range: (shaper.min_stop, shaper.max_stop),
            data: lut.data,
        })
    }

    /// Converts linear `rgba` pixels from color space `from` to the working space.
    pub fn to_working(&self, from: &str, pixels: &mut [f32]) -> anyhow::Result<()> {
        let p = self.config.processor(from, &self.working)?;
        p.apply_interleaved(pixels, 4);
        Ok(())
    }
}

impl State {
    /// Applies the current OCIO choices: display LUT and bitmap conversion.
    pub fn apply_ocio(&mut self) {
        let Some(ocio) = self.color.ocio.clone() else {
            if self.render.tone_map == crate::viewport::ToneMap::Ocio {
                self.render.tone_map = crate::viewport::ToneMap::Aces;
            }
            self.viewport.compositor().set_ocio(None);
            self.viewport.invalidate();
            return;
        };
        match ocio.bake() {
            Ok(lut) => {
                self.viewport
                    .set_display_lut(&self.device, &self.queue, &lut);
                self.render.tone_map = crate::viewport::ToneMap::Ocio;
            }
            Err(e) => self.error(format!("OCIO display transform failed: {e:#}")),
        }
        self.viewport.compositor().set_ocio(Some(ocio));
        self.viewport.invalidate();
    }

    pub fn color_management_ui(&mut self, ui: &mut Ui) {
        ui.label(theme::header("COLOR MANAGEMENT"));
        let mut changed = false;
        let current = match &self.color.ocio {
            None => "None (built-in tone mapping)".to_owned(),
            Some(o) => o.source.strip_prefix("builtin:").map_or_else(
                || crate::app::file_name(std::path::Path::new(&o.source)),
                str::to_owned,
            ),
        };
        let mut load_file = false;
        let mut pick_builtin: Option<&str> = None;
        let mut turn_off = false;
        ui.horizontal(|ui| {
            ui.label("OCIO config");
            ComboBox::from_id_salt("ocio_config")
                .selected_text(current)
                .width(230.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.color.ocio.is_none(), "None (built-in tone mapping)")
                        .clicked()
                    {
                        turn_off = true;
                    }
                    for (name, _) in reetched_color::builtin_configs() {
                        if ui.button(*name).clicked() {
                            pick_builtin = Some(name);
                        }
                    }
                    if ui.button("Load .ocio file…").clicked() {
                        load_file = true;
                    }
                });
        });
        if turn_off && self.color.ocio.is_some() {
            self.color.ocio = None;
            changed = true;
        }
        if let Some(name) = pick_builtin {
            match Config::builtin(name) {
                Ok(c) => {
                    self.color.ocio = Some(Ocio::new(c, format!("builtin:{name}")));
                    changed = true;
                }
                Err(e) => self.error(format!("Could not load {name}: {e:#}")),
            }
        }
        if load_file
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("OCIO config", &["ocio"])
                .pick_file()
        {
            match Config::from_file(&path) {
                Ok(c) => {
                    self.color.ocio = Some(Ocio::new(c, path.display().to_string()));
                    changed = true;
                }
                Err(e) => self.error(format!("Could not load {}: {e:#}", path.display())),
            }
        }
        if let Some(o) = &mut self.color.ocio {
            let config = o.config.clone();
            egui::Grid::new("ocio")
                .num_columns(2)
                .spacing(egui::vec2(12.0, 6.0))
                .show(ui, |ui| {
                    let combo = |ui: &mut Ui, id: &str, value: &mut String, items: Vec<&str>| {
                        let mut picked = false;
                        ComboBox::from_id_salt(id)
                            .selected_text(if value.is_empty() { "(view default)" } else { value.as_str() })
                            .width(230.0)
                            .show_ui(ui, |ui| {
                                for item in items {
                                    picked |= ui.selectable_value(value, item.to_owned(), item).changed();
                                }
                            });
                        picked
                    };
                    ui.label("Rendering space").on_hover_text(
                        "The config's name for linear Rec.709 / sRGB primaries, which ReEtched renders in",
                    );
                    changed |= combo(ui, "ocio_working", &mut o.working, config.color_space_names());
                    ui.end_row();
                    ui.label("Display");
                    if combo(ui, "ocio_display", &mut o.display, config.displays()) {
                        o.view = config.default_view(&o.display).unwrap_or_default().to_owned();
                        changed = true;
                    }
                    ui.end_row();
                    ui.label("View");
                    changed |= combo(ui, "ocio_view", &mut o.view, config.views(&o.display));
                    ui.end_row();
                    ui.label("Look");
                    let mut looks = vec![""];
                    looks.extend(config.looks());
                    changed |= combo(ui, "ocio_look", &mut o.look, looks);
                    ui.end_row();
                });
            ui.label(
                RichText::new("Bitmaps can also be read in any of this config's color spaces (fill layer properties).")
                    .color(theme::WEAK),
            );
        }
        if changed {
            self.apply_ocio();
        }
    }
}
