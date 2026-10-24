//! Pure-Rust subset of OpenColorIO v2 for ReEtched.
//!
//! Loads OCIO configs (v1 and v2 YAML, including the ACES 1.3 / 2.x configs), builds CPU
//! processors for color space and display/view conversions, and bakes display transforms
//! into shaper-addressed 3D LUTs for the viewport shader.
//!
//! ```no_run
//! use reetched_color::{Config, Shaper};
//! let config = Config::builtin("cg-config-v4.0.0_aces-v2.0_ocio-v2.5")?;
//! let display = config.default_display().unwrap();
//! let view = config.default_view(display).unwrap();
//! let lut = config.display_processor("scene_linear", display, view, None)?.bake_lut3d(33, Shaper::default());
//! # Ok::<(), anyhow::Error>(())
//! ```

mod aces2;
mod builtins;
mod config;
mod lutfile;
mod ops;
mod processor;

pub use config::{ColorSpace, Config, ReferenceSpace};
pub use processor::{Lut3d, Processor, Shaper};

static BUILTIN_CONFIGS: &[(&str, &str)] = &[
    (
        "cg-config-v2.2.0_aces-v1.3_ocio-v2.4",
        include_str!("../data/cg-config-v2.2.0_aces-v1.3_ocio-v2.4.ocio"),
    ),
    (
        "cg-config-v4.0.0_aces-v2.0_ocio-v2.5",
        include_str!("../data/cg-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio"),
    ),
    (
        "cg-config-v5.0.0_aces-v2.1_ocio-v2.6",
        include_str!("../data/cg-config-v5.0.0_aces-v2.1_ocio-v2.6.ocio"),
    ),
];

/// The vendored ACES "cg" configs as `(name, yaml)`, oldest first.
pub fn builtin_configs() -> &'static [(&'static str, &'static str)] {
    BUILTIN_CONFIGS
}
