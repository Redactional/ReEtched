//! Update check (notify only, never downloads) and the About window.

use std::sync::mpsc;

const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Default)]
pub struct Updates {
    pending: Option<mpsc::Receiver<Result<String, String>>>,
    /// The user asked (Help menu): also report "up to date" and errors.
    manual: bool,
    /// What to tell the user; None = nothing to show.
    notice: Option<Notice>,
    pub about_open: bool,
    started: bool,
}

enum Notice {
    Newer(String),
    UpToDate,
    Failed(String),
}

impl Updates {
    pub fn check(&mut self, ctx: &egui::Context, manual: bool) {
        if self.pending.is_some() {
            return;
        }
        self.manual = manual;
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(latest_tag());
            ctx.request_repaint();
        });
        self.pending = Some(rx);
    }

    fn poll(&mut self) {
        let Some(rx) = &self.pending else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.pending = None;
        self.notice = match result {
            Ok(tag) if newer(&tag, VERSION) => Some(Notice::Newer(tag)),
            Ok(_) if self.manual => Some(Notice::UpToDate),
            Err(e) if self.manual => Some(Notice::Failed(e)),
            _ => None,
        };
    }

    /// `auto`: check once at startup (Settings can turn it off).
    pub fn ui(&mut self, ctx: &egui::Context, auto: bool) {
        if auto && !self.started && !cfg!(test) {
            self.check(ctx, false);
        }
        self.started = true;
        self.poll();
        if let Some(notice) = &self.notice {
            let mut open = true;
            let mut close = false;
            egui::Window::new("Updates")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    match notice {
                        Notice::Newer(tag) => {
                            ui.label(format!("ReEtched {tag} is available (you have {VERSION})."));
                            if ui.button("Open the download page").clicked() {
                                ctx.open_url(egui::OpenUrl::new_tab(format!(
                                    "{REPO}/releases/latest"
                                )));
                                close = true;
                            }
                        }
                        Notice::UpToDate => {
                            ui.label(format!("ReEtched {VERSION} is the latest version."));
                        }
                        Notice::Failed(e) => {
                            ui.label(format!("Could not check for updates: {e}"));
                        }
                    }
                    ui.label(
                        egui::RichText::new("Automatic checks can be turned off in Settings.")
                            .small()
                            .weak(),
                    );
                });
            if !open || close {
                self.notice = None;
            }
        }
        if self.about_open {
            egui::Window::new("About ReEtched")
                .open(&mut self.about_open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.heading(format!("ReEtched {VERSION}"));
                    ui.label("Open-source 3D texture painter.");
                    ui.label(format!("License: {}", env!("CARGO_PKG_LICENSE")));
                    ui.hyperlink(REPO);
                });
        }
    }
}

/// The latest release's tag, from GitHub's API.
// ponytail: uses the system's curl (Windows 10+, macOS and Linux ship it)
// instead of bundling an HTTP/TLS stack; swap for ureq if curl ever falls short.
fn latest_tag() -> Result<String, String> {
    let api = REPO.replace("https://github.com/", "https://api.github.com/repos/");
    let out = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "10",
            "-H",
            "Accept: application/vnd.github+json",
        ])
        .arg(format!("{api}/releases/latest"))
        .output()
        .map_err(|e| format!("curl is not available ({e})"))?;
    if !out.status.success() {
        return Err("no release found, or no connection".into());
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    json["tag_name"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "unexpected reply".into())
}

/// Is release `tag` ("v1.2.0") newer than `current` ("1.1.3")?
fn newer(tag: &str, current: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split(['.', '-', '+'])
            .map_while(|p| p.parse().ok())
            .collect()
    };
    parts(tag) > parts(current)
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions_compare_numerically() {
        use super::newer;
        assert!(newer("v0.10.0", "0.9.5"));
        assert!(newer("1.0.0", "0.1.0"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("v0.1.0-beta", "0.1.0"));
        assert!(!newer("garbage", "0.1.0"));
    }
}
