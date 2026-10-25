//! Python scripting for ReEtched.
//!
//! Scripts run on the user's installed CPython. This crate never links Python:
//! it finds an install ([`find_python`]), loads the `reetched_python` bridge
//! library at runtime and talks to it through a tiny C ABI. Without Python the
//! app runs normally and [`find_python`] just returns an error to show.
//!
//! The app implements [`ScriptHost`]; Python reaches the app only through
//! `ScriptHost::call` with the methods listed in PROTOCOL.md, and only while
//! one of the [`Engine`] methods taking a host is running.

mod find;

pub use find::{PythonInstall, find_python, python_at};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

/// Implemented by the app. Methods are named like "layers.insert"; args/results are JSON.
pub trait ScriptHost {
    fn call(&mut self, method: &str, args: Value) -> Result<Value, String>;
    /// Python's stdout arrives as Info, stderr as Error.
    fn log(&mut self, level: LogLevel, text: &str);
}

/// Outcome of running code. Output printed by the code is in `output` (it is
/// not also sent to `ScriptHost::log`); `error` holds the traceback text.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RunResult {
    pub output: String,
    pub error: Option<String>,
}

impl RunResult {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    /// The plugin's .py file or package directory.
    pub path: PathBuf,
    /// The user wants it on.
    pub enabled: bool,
    /// start_plugin() ran successfully and close_plugin() has not run since.
    pub started: bool,
    /// Traceback of the last failed start/close.
    pub error: Option<String>,
}

/// A menu entry registered from Python.
#[derive(Clone, Debug, Deserialize)]
pub struct Action {
    pub id: u64,
    /// Menu path inside the app's Python menu; '/' separates submenus ("My Tools/Bake All").
    pub label: String,
    /// Plugin that registered it (None for console scripts).
    pub owner: Option<String>,
}

/// Name of the bridge library ("reetched_python.dll", "libreetched_python.so", ...).
pub fn library_name() -> String {
    format!(
        "{}reetched_python{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )
}

/// Where the bridge library is: `REETCHED_PYTHON_LIB` if set, else next to the
/// executable, one level up (test and example binaries live in
/// `target/<profile>/deps` and `.../examples`), or where installers put
/// resources (`Contents/Resources` in a macOS app, `usr/lib` on Linux).
pub fn find_library() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("REETCHED_PYTHON_LIB") {
        return Some(p.into());
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let up = dir.parent()?;
    let name = library_name();
    [
        dir.to_path_buf(),
        up.to_path_buf(),
        up.join("Resources"),
        up.join("lib"),
        up.join("lib/reetched"),
    ]
    .into_iter()
    .map(|d| d.join(&name))
    .find(|p| p.is_file())
}

// ---------------------------------------------------------------- C ABI

#[repr(C)]
struct HostVTable {
    ctx: *mut c_void,
    call:
        unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *mut *mut c_char) -> c_int,
    free: unsafe extern "C" fn(*mut c_void, *mut c_char),
}

type DispatchFn =
    unsafe extern "C" fn(*const HostVTable, *const c_char, *const c_char) -> *mut c_char;
type FreeFn = unsafe extern "C" fn(*mut c_char);
type VersionFn = unsafe extern "C" fn() -> u32;

const ABI_VERSION: u32 = 1;

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "\u{FFFD}")).unwrap_or_default()
}

// `ctx` points at a `&mut dyn ScriptHost` living on the stack of Engine::dispatch.
unsafe extern "C" fn host_call(
    ctx: *mut c_void,
    method: *const c_char,
    args: *const c_char,
    out: *mut *mut c_char,
) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Value, String> {
        let host = unsafe { &mut **(ctx as *mut &mut dyn ScriptHost) };
        let method = unsafe { CStr::from_ptr(method) }.to_string_lossy();
        let args = unsafe { CStr::from_ptr(args) }.to_string_lossy();
        let args: Value = serde_json::from_str(&args).map_err(|e| e.to_string())?;
        if method == "log" {
            // Routed to ScriptHost::log so the app doesn't implement it twice.
            let level = match args["level"].as_str() {
                Some("warning") => LogLevel::Warning,
                Some("error") => LogLevel::Error,
                _ => LogLevel::Info,
            };
            host.log(level, args["text"].as_str().unwrap_or_default());
            return Ok(Value::Null);
        }
        host.call(&method, args)
    }));
    let (rc, text) = match result {
        Ok(Ok(v)) => (0, v.to_string()),
        Ok(Err(e)) => (1, e),
        Err(_) => (1, "the app panicked while handling this call".to_string()),
    };
    unsafe { *out = cstring(&text).into_raw() };
    rc
}

unsafe extern "C" fn host_free(_ctx: *mut c_void, s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

/// The loaded bridge. Python cannot be unloaded or re-initialised, so this is
/// process-wide and lives forever once loaded.
#[derive(Clone, Copy)]
struct Bridge {
    dispatch: DispatchFn,
    free: FreeFn,
}

static BRIDGE: Mutex<Option<(Bridge, PathBuf)>> = Mutex::new(None);

fn load_bridge(library: &Path, install: &PythonInstall) -> Result<Bridge> {
    let mut slot = BRIDGE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((bridge, home)) = &*slot {
        if *home != install.home {
            bail!(
                "Python from {} is already running; restart ReEtched to switch",
                home.display()
            );
        }
        return Ok(*bridge);
    }
    // The interpreter finds its standard library through PYTHONHOME.
    // SAFETY: set once, before Python starts; only read by Python's init.
    unsafe { std::env::set_var("PYTHONHOME", &install.home) };
    preload_python(install)?;
    // SAFETY: loading our own bridge library; leaked because Python state lives inside it.
    let lib: &'static libloading::Library = Box::leak(Box::new(
        unsafe { libloading::Library::new(library) }
            .with_context(|| format!("loading {}", library.display()))?,
    ));
    let bridge = unsafe {
        let version = *lib.get::<VersionFn>(b"reetched_abi_version\0")?;
        if version() != ABI_VERSION {
            bail!(
                "{} speaks ABI {}, expected {ABI_VERSION}",
                library.display(),
                version()
            );
        }
        Bridge {
            dispatch: *lib.get::<DispatchFn>(b"reetched_dispatch\0")?,
            free: *lib.get::<FreeFn>(b"reetched_free\0")?,
        }
    };
    *slot = Some((bridge, install.home.clone()));
    Ok(bridge)
}

/// Loads the Python library by full path so the bridge's own import of it
/// (python3.dll / libpython) resolves without touching PATH.
fn preload_python(install: &PythonInstall) -> Result<()> {
    #[cfg(windows)]
    {
        // python3XY.dll first: python3.dll (what abi3 links) forwards into it.
        for path in [install.library.clone(), install.home.join("python3.dll")] {
            let lib = unsafe { libloading::Library::new(&path) }
                .with_context(|| format!("loading {}", path.display()))?;
            std::mem::forget(lib);
        }
    }
    #[cfg(unix)]
    {
        use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};
        // RTLD_GLOBAL so the bridge and C extension modules see Python's symbols.
        // ponytail: only exercised on Windows so far.
        let lib = unsafe { Library::open(Some(&install.library), RTLD_NOW | RTLD_GLOBAL) }
            .with_context(|| format!("loading {}", install.library.display()))?;
        std::mem::forget(lib);
    }
    Ok(())
}

// ---------------------------------------------------------------- Engine

/// The embedded interpreter. There is one Python per process: starting a
/// second Engine re-initialises the same interpreter (closing its plugins).
/// Everything runs on the calling thread; call it from the UI thread.
pub struct Engine {
    bridge: Bridge,
}

impl Engine {
    /// Loads the bridge library found by [`find_library`] and starts Python.
    /// Plugins are discovered but not started: enable them with
    /// [`Engine::set_plugin_enabled`] (they may call the host when starting).
    pub fn start(install: &PythonInstall, plugin_dirs: &[PathBuf]) -> Result<Engine> {
        let library = find_library()
            .ok_or_else(|| anyhow!("{} not found next to the executable", library_name()))?;
        Self::start_with_library(&library, install, plugin_dirs)
    }

    pub fn start_with_library(
        library: &Path,
        install: &PythonInstall,
        plugin_dirs: &[PathBuf],
    ) -> Result<Engine> {
        let engine = Engine {
            bridge: load_bridge(library, install)?,
        };
        let args = json!({
            "plugin_dirs": plugin_dirs,
            "executable": install.executable,
            "site_packages": install.site_packages,
        });
        engine
            .dispatch(None, "init", &args)
            .map_err(|e| anyhow!("starting Python: {e}"))?;
        Ok(engine)
    }

    fn dispatch(
        &self,
        host: Option<&mut dyn ScriptHost>,
        op: &str,
        args: &Value,
    ) -> Result<Value, String> {
        let (op, args) = (cstring(op), cstring(&args.to_string()));
        let out = match host {
            Some(mut host) => {
                let vtable = HostVTable {
                    ctx: &mut host as *mut &mut dyn ScriptHost as *mut c_void,
                    call: host_call,
                    free: host_free,
                };
                unsafe { (self.bridge.dispatch)(&vtable, op.as_ptr(), args.as_ptr()) }
            }
            None => unsafe { (self.bridge.dispatch)(std::ptr::null(), op.as_ptr(), args.as_ptr()) },
        };
        if out.is_null() {
            return Err("no answer from the Python bridge".into());
        }
        let text = unsafe { CStr::from_ptr(out) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.bridge.free)(out) };
        let mut v: Value = serde_json::from_str(&text)
            .map_err(|e| format!("bad answer from Python bridge: {e}"))?;
        match v.get("error") {
            Some(e) => Err(e.as_str().unwrap_or("unknown error").to_string()),
            None => Ok(v["ok"].take()),
        }
    }

    fn run_op(&self, host: &mut dyn ScriptHost, op: &str, args: Value) -> RunResult {
        match self.dispatch(Some(host), op, &args) {
            Ok(v) => serde_json::from_value(v).unwrap_or_default(),
            Err(e) => RunResult {
                output: String::new(),
                error: Some(e),
            },
        }
    }

    /// Runs code in the persistent console namespace (a trailing expression's
    /// value is printed, like the Python REPL).
    pub fn run(&mut self, host: &mut dyn ScriptHost, code: &str, filename: &str) -> RunResult {
        self.run_op(host, "run", json!({ "code": code, "filename": filename }))
    }

    /// Runs a script file as `__main__` in a fresh namespace.
    pub fn run_file(&mut self, host: &mut dyn ScriptHost, path: &Path) -> RunResult {
        self.run_op(host, "run_file", json!({ "path": path }))
    }

    /// Tells Python about an app event (see PROTOCOL.md for names and data).
    /// Callback errors are logged through the host, not returned.
    pub fn event(&mut self, host: &mut dyn ScriptHost, name: &str, data: Value) {
        if let Err(e) = self.dispatch(
            Some(&mut *host),
            "event",
            &json!({ "name": name, "data": data }),
        ) {
            host.log(LogLevel::Error, &e);
        }
    }

    pub fn plugins(&mut self) -> Vec<PluginInfo> {
        self.dispatch(None, "plugins", &json!({}))
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    /// Imports + start_plugin() when turning on, close_plugin() when turning off.
    pub fn set_plugin_enabled(
        &mut self,
        host: &mut dyn ScriptHost,
        name: &str,
        on: bool,
    ) -> Result<(), String> {
        self.dispatch(
            Some(host),
            "set_plugin_enabled",
            &json!({ "name": name, "on": on }),
        )
        .map(drop)
    }

    /// close_plugin(), re-import from disk, start_plugin() again if it was enabled.
    pub fn reload_plugin(&mut self, host: &mut dyn ScriptHost, name: &str) -> Result<(), String> {
        self.dispatch(Some(host), "reload_plugin", &json!({ "name": name }))
            .map(drop)
    }

    pub fn actions(&mut self) -> Vec<Action> {
        self.dispatch(None, "actions", &json!({}))
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    pub fn trigger_action(&mut self, host: &mut dyn ScriptHost, id: u64) -> RunResult {
        self.run_op(host, "trigger_action", json!({ "id": id }))
    }

    /// Runs due timers (reetched.ui.call_later / call_every). Call once a frame.
    pub fn tick(&mut self, host: &mut dyn ScriptHost) {
        if let Err(e) = self.dispatch(Some(&mut *host), "tick", &json!({})) {
            host.log(LogLevel::Error, &e);
        }
    }
}
