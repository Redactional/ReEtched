//! Importing a ReEtched Bridge bundle (a Painter project exported by the
//! plugin in `tools/painter-bridge`) and checking it against Painter's export.

use crate::app::State;
use reetched_doc::{Channel, export::linear_to_srgb};
use reetched_substance::bridge::{self, Reference};
use std::path::Path;

/// How one channel of one texture set compares with Painter's export.
pub struct Check {
    pub set: String,
    pub channel: Channel,
    /// Mean absolute difference over covered texels (0..1 values as exported).
    pub mean: f32,
    /// Share of covered texels more than 0.1 off.
    pub off: f32,
}

impl Check {
    pub fn passed(&self) -> bool {
        self.mean < 0.01 && self.off < 0.01
    }
}

impl State {
    pub fn import_bridge_dialog(&mut self) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("Choose the folder the ReEtched Bridge plugin exported")
            .pick_folder()
        {
            self.import_bridge(&dir);
        }
    }

    /// Opens a bundle folder (or its `bridge.json`) and reports how it matches Painter.
    pub fn import_bridge(&mut self, path: &Path) -> Vec<Check> {
        let dir = if path.is_file() {
            path.parent().unwrap_or(Path::new("."))
        } else {
            path
        };
        let imported = match bridge::import(dir) {
            Ok(i) => i,
            Err(e) => {
                self.error(format!("Could not import the Painter bridge: {e:#}"));
                return Vec::new();
            }
        };
        let name = crate::app::file_name(dir);
        self.set_mesh(imported.mesh, name.clone(), Some(imported.doc));
        self.resources = imported.resources;
        self.project_path = None;
        self.info(format!("Imported Painter project {name}"));
        for line in imported.log {
            self.warn(format!("Bridge: {line}"));
        }
        let checks = self.verify(&imported.references);
        let failed = checks.iter().filter(|c| !c.passed()).count();
        for c in &checks {
            let text = format!(
                "{} / {}: {} (mean difference {:.4}, {:.1}% of texels off)",
                c.set,
                c.channel.label(),
                if c.passed() {
                    "matches Painter"
                } else {
                    "differs from Painter"
                },
                c.mean,
                c.off * 100.0
            );
            if c.passed() {
                self.info(text);
            } else {
                self.warn(text);
            }
        }
        if !checks.is_empty() {
            self.info(format!(
                "Bridge check: {} of {} channels match Painter's export",
                checks.len() - failed,
                checks.len()
            ));
        }
        checks
    }

    /// Re-composites the document and compares it with Painter's exports.
    fn verify(&mut self, refs: &[Reference]) -> Vec<Check> {
        for w in self.viewport.sync(
            &self.device,
            &self.queue,
            &self.doc.texture_sets,
            &self.resources,
            None,
        ) {
            self.warn(w);
        }
        let mut checks = Vec::new();
        for r in refs {
            let Ok(data) =
                self.viewport
                    .read_back(&self.device, &self.queue, r.set, reetched_doc::FIRST_UDIM)
            else {
                continue;
            };
            if (data.width, data.height) != (r.width, r.height) {
                continue;
            }
            // Ours, encoded like Painter's export.
            let ours = |i: usize| -> [f32; 3] {
                match r.channel {
                    Channel::BaseColor => data.base_color[i].map(linear_to_srgb),
                    Channel::Normal => data.normal[i].map(|v| v * 0.5 + 0.5),
                    Channel::Metallic => [data.metallic[i]; 3],
                    Channel::Roughness => [data.roughness[i]; 3],
                    Channel::Height => [data.height_map[i]; 3],
                    Channel::Emissive => data.emissive[i].map(linear_to_srgb),
                    Channel::Opacity => [data.opacity[i]; 3],
                    c => {
                        let v = data.extra.get(&c).map_or(c.empty_value(), |v| v[i]);
                        if c.is_color() {
                            v.map(linear_to_srgb)
                        } else {
                            v
                        }
                    }
                }
            };
            let comps = if r.channel.is_vector() { 3 } else { 1 };
            let (mut sum, mut off, mut n) = (0.0f64, 0usize, 0usize);
            for (i, theirs) in r.pixels.iter().enumerate() {
                if !data.coverage[i] {
                    continue;
                }
                let o = ours(i);
                let d = (0..comps)
                    .map(|k| (o[k] - theirs[k]).abs())
                    .fold(0.0f32, f32::max);
                sum += d as f64;
                off += (d > 0.1) as usize;
                n += 1;
            }
            if n == 0 {
                continue;
            }
            checks.push(Check {
                set: self.doc.texture_sets[r.set].name.clone(),
                channel: r.channel,
                mean: (sum / n as f64) as f32,
                off: off as f32 / n as f32,
            });
        }
        checks
    }
}
