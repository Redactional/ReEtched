//! Locating an installed CPython 3.8+.

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A CPython installation the engine can embed.
#[derive(Clone, Debug, Deserialize)]
pub struct PythonInstall {
    /// The base install (PYTHONHOME): holds the standard library.
    pub home: PathBuf,
    /// "3.14.0"
    pub version: String,
    /// The python executable that was probed (a venv's, if REETCHED_PYTHON points at one).
    pub executable: PathBuf,
    /// The shared Python library to preload (python3XY.dll, libpython3.X.so, ...).
    pub library: PathBuf,
    /// site-packages directories of the probed interpreter, so a venv's packages are importable.
    pub site_packages: Vec<PathBuf>,
}

// Prints what the engine needs to embed this interpreter.
const PROBE: &str = r#"
import json, os, sys, sysconfig
v = sys.version_info
if os.name == "nt":
    lib = os.path.join(sys.base_prefix, "python%d%d.dll" % v[:2])
else:
    name = sysconfig.get_config_var("LDLIBRARY") or ""
    base = sysconfig.get_config_var("PYTHONFRAMEWORKPREFIX") or sysconfig.get_config_var("LIBDIR") or ""
    lib = os.path.join(base, name) if name else ""
print(json.dumps({
    "home": sys.base_prefix,
    "version": "%d.%d.%d" % v[:3],
    "executable": sys.executable,
    "library": lib,
    "site_packages": [p for p in sys.path if p.endswith("site-packages") and os.path.isdir(p)],
}))
"#;

/// Probes the given python executable (or install directory).
pub fn python_at(path: &Path) -> Result<PythonInstall> {
    let exe = if path.is_dir() {
        exe_in(path)
    } else {
        path.to_owned()
    };
    probe(&exe, &[])
}

/// Finds Python 3.8+: `REETCHED_PYTHON` (a python executable or install dir)
/// first, then `py -3`, `python3`, `python` on PATH, then (Windows) the
/// registry and the standard install directories.
pub fn find_python() -> Result<PythonInstall> {
    if let Some(p) = std::env::var_os("REETCHED_PYTHON") {
        let p = PathBuf::from(p);
        let exe = if p.is_dir() { exe_in(&p) } else { p.clone() };
        return probe(&exe, &[]).map_err(|e| anyhow!("REETCHED_PYTHON={}: {e}", p.display()));
    }
    let mut candidates: Vec<(PathBuf, &[&str])> = vec![
        ("py".into(), &["-3"]),
        ("python3".into(), &[]),
        ("python".into(), &[]),
    ];
    if cfg!(windows) {
        candidates.extend(windows_installs().into_iter().map(|p| (p, &[] as &[&str])));
    }
    for (exe, pre) in &candidates {
        if let Ok(install) = probe(exe, pre) {
            return Ok(install);
        }
    }
    bail!(
        "Python 3.8+ not found (install it from python.org, or set REETCHED_PYTHON to a python executable)"
    )
}

fn exe_in(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dir.join("python.exe")
    } else {
        dir.join("bin/python3")
    }
}

fn probe(exe: &Path, pre: &[&str]) -> Result<PythonInstall> {
    let mut cmd = Command::new(exe);
    cmd.args(pre)
        .args(["-c", PROBE])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashing up from a GUI app.
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        bail!("{} failed to run", exe.display());
    }
    let install: PythonInstall = serde_json::from_slice(&out.stdout)?;
    let mut parts = install
        .version
        .split('.')
        .map(|s| s.parse::<u32>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    if (major, minor) < (3, 8) {
        bail!("Python {} is too old (3.8+ needed)", install.version);
    }
    let lib = install.library.to_string_lossy();
    if !install.library.is_file() || lib.ends_with(".a") {
        bail!(
            "Python {} has no shared library to embed ({lib})",
            install.version
        );
    }
    Ok(install)
}

/// python.exe paths from the PEP 514 registry keys and the usual install folders.
fn windows_installs() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in ["HKCU", "HKLM"] {
        // `reg` ships with Windows; avoids a registry crate for one query.
        let Ok(out) = Command::new("reg")
            .args([
                "query",
                &format!(r"{root}\SOFTWARE\Python\PythonCore"),
                "/s",
                "/v",
                "ExecutablePath",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        else {
            continue;
        };
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some((_, path)) = line.split_once("REG_SZ") {
                found.push(PathBuf::from(path.trim()));
            }
        }
    }
    let mut dirs = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        dirs.push(local.join(r"Programs\Python"));
        dirs.push(local.join("Python"));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        dirs.push(PathBuf::from(pf));
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut subdirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with("python"))
            })
            .collect();
        // Newest first. ponytail: lexical order ranks Python39 above Python310;
        // parse versions if that pairing ever matters.
        subdirs.sort();
        subdirs.reverse();
        found.extend(
            subdirs
                .into_iter()
                .map(|d| d.join("python.exe"))
                .filter(|p| p.is_file()),
        );
    }
    found
}
