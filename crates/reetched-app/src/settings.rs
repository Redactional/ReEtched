//! Preferences kept between sessions (`settings.json` in the config folder),
//! and where that folder is.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The folder for settings, autosaves, templates, plugins and caches:
/// `REETCHED_CONFIG_DIR`, else the OS config folder (e.g. `%APPDATA%/ReEtched`).
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("REETCHED_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    // Tests never touch the user's real settings, autosaves or plugins.
    if cfg!(test) {
        return std::env::temp_dir().join("reetched-test-config");
    }
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("ReEtched")
}

/// A folder of assets shown in the Assets panel.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct LibraryRoot {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Minutes between autosaves; 0 turns autosave off.
    pub autosave_minutes: f32,
    /// Most recent first.
    pub recent_projects: Vec<PathBuf>,
    pub library_roots: Vec<LibraryRoot>,
    /// Python plugins to start with the app.
    pub plugins_enabled: Vec<String>,
    /// A Python install to use instead of searching for one.
    pub python: Option<PathBuf>,
    /// GPU memory the compositor's caches may use, in MB (0 = automatic).
    pub vram_budget_mb: u32,
    pub keymap: crate::keymap::KeymapSettings,
    /// Size of the whole interface (1 = 100 %).
    pub ui_scale: f32,
    /// Look for a newer release at startup (notify only).
    pub check_updates: bool,
    /// Show the welcome screen at startup.
    pub show_welcome: bool,
    /// Developer: show the console window with the debug log.
    pub show_console: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            autosave_minutes: 5.0,
            recent_projects: Vec::new(),
            library_roots: Vec::new(),
            plugins_enabled: Vec::new(),
            python: None,
            vram_budget_mb: 0,
            keymap: Default::default(),
            ui_scale: 1.0,
            check_updates: true,
            show_welcome: true,
            show_console: false,
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        config_dir().join("settings.json")
    }

    /// The saved settings, or defaults (a missing or unreadable file isn't an error).
    pub fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, serde_json::to_vec_pretty(self)?)?;
        log::debug!("settings saved to {}", path.display());
        Ok(())
    }

    /// Puts `path` at the top of the recent projects.
    pub fn add_recent(&mut self, path: &Path) {
        self.recent_projects.retain(|p| p != path);
        self.recent_projects.insert(0, path.to_owned());
        self.recent_projects.truncate(10);
    }

    /// The compositor's GPU cache budget in bytes.
    pub fn vram_budget(&self) -> u64 {
        let mb = if self.vram_budget_mb == 0 {
            3072
        } else {
            self.vram_budget_mb
        };
        mb as u64 * 1024 * 1024
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_tolerate_old_files() {
        let mut s = Settings::default();
        s.add_recent(Path::new("a.retx"));
        s.add_recent(Path::new("b.retx"));
        s.add_recent(Path::new("a.retx"));
        assert_eq!(
            s.recent_projects,
            [PathBuf::from("a.retx"), "b.retx".into()]
        );
        let back: Settings = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        // Fields added later fall back to their defaults.
        let old: Settings = serde_json::from_str(r#"{"autosave_minutes": 2.0}"#).unwrap();
        assert_eq!(old.autosave_minutes, 2.0);
        assert!(old.library_roots.is_empty());
    }
}
