//! Embedded CPython for ReEtched scripting.
//!
//! This cdylib is the only part of ReEtched that links Python. `reetched-script`
//! loads it at runtime (after locating a Python install), so the app itself
//! starts fine without Python. The whole C ABI is one dispatch function plus a
//! free function; the engine logic (console, plugins, events, timers) lives in
//! the embedded Python package `reetched._engine`.
//!
//! ABI (version 1):
//! ```c
//! typedef struct {
//!     void *ctx;
//!     // Returns 0 on success, non-zero on error. *out receives a NUL-terminated
//!     // UTF-8 string allocated by the host (JSON result, or error text) that this
//!     // DLL hands back to `free` once it has copied it.
//!     int (*call)(void *ctx, const char *method, const char *args_json, char **out);
//!     void (*free)(void *ctx, char *s);
//! } ReEtchedHost;
//! uint32_t reetched_abi_version(void);
//! // Returns {"ok": result} or {"error": "text"} as JSON; free with reetched_free.
//! char *reetched_dispatch(const ReEtchedHost *host, const char *op, const char *args_json);
//! void reetched_free(char *s);
//! ```
//! `host` is only valid for the duration of one `reetched_dispatch` call; Python
//! code calling the host at any other time (or from another thread) gets a
//! RuntimeError instead of touching a dangling pointer.

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};
use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Once;

#[repr(C)]
pub struct Host {
    pub ctx: *mut c_void,
    pub call:
        unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *mut *mut c_char) -> c_int,
    pub free: unsafe extern "C" fn(*mut c_void, *mut c_char),
}

thread_local! {
    // Thread-local on purpose: a Python thread other than the one inside
    // reetched_dispatch sees null here and cannot reach the host.
    static HOST: Cell<*const Host> = const { Cell::new(ptr::null()) };
}

/// Restores the previous host pointer even if the dispatch unwinds, so nested
/// dispatches (host -> app event -> Python) see the right host afterwards.
struct HostGuard(*const Host);
impl Drop for HostGuard {
    fn drop(&mut self) {
        HOST.with(|h| h.set(self.0));
    }
}

macro_rules! sources {
    ($($name:literal, $pkg:literal, $file:literal;)*) => {
        &[$(($name, $pkg, include_str!(concat!("../python/", $file)))),*]
    };
}

/// (module name, is package, source) of the embedded API packages.
const SOURCES: &[(&str, bool, &str)] = sources! {
    "reetched", true, "reetched/__init__.py";
    "reetched._core", false, "reetched/_core.py";
    "reetched._engine", false, "reetched/_engine.py";
    "reetched.project", false, "reetched/project.py";
    "reetched.textureset", false, "reetched/textureset.py";
    "reetched.layerstack", false, "reetched/layerstack.py";
    "reetched.resource", false, "reetched/resource.py";
    "reetched.export", false, "reetched/export.py";
    "reetched.baking", false, "reetched/baking.py";
    "reetched.display", false, "reetched/display.py";
    "reetched.event", false, "reetched/event.py";
    "reetched.logging", false, "reetched/logging.py";
    "reetched.ui", false, "reetched/ui.py";
    "substance_painter", true, "substance_painter/__init__.py";
    "substance_painter.exception", false, "substance_painter/exception.py";
    "substance_painter.colormanagement", false, "substance_painter/colormanagement.py";
    "substance_painter.project", false, "substance_painter/project.py";
    "substance_painter.textureset", false, "substance_painter/textureset.py";
    "substance_painter.layerstack", false, "substance_painter/layerstack.py";
    "substance_painter.resource", false, "substance_painter/resource.py";
    "substance_painter.export", false, "substance_painter/export.py";
    "substance_painter.event", false, "substance_painter/event.py";
    "substance_painter.logging", false, "substance_painter/logging.py";
    "substance_painter.ui", false, "substance_painter/ui.py";
    "substance_painter.js", false, "substance_painter/js.py";
};

/// Installs a meta-path finder serving the embedded sources. `get_source` lets
/// tracebacks show the lines of embedded modules too.
const BOOTSTRAP: &CStr = c"
import sys, importlib.abc, importlib.util, _reetched

class _EmbeddedFinder(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    def find_spec(self, name, path=None, target=None):
        entry = _reetched.SOURCES.get(name)
        if entry is None:
            return None
        return importlib.util.spec_from_loader(name, self, is_package=entry[0])

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        name = module.__name__
        is_pkg, src = _reetched.SOURCES[name]
        module.__file__ = '<embedded>/' + name.replace('.', '/') + ('/__init__.py' if is_pkg else '.py')
        exec(compile(src, module.__file__, 'exec'), module.__dict__)

    def get_source(self, name):
        return _reetched.SOURCES[name][1]

sys.meta_path.insert(0, _EmbeddedFinder())
import reetched._engine
";

/// Calls the current host. Returns (ok, text): text is the JSON result on
/// success and the error message otherwise.
#[pyfunction]
fn call(method: &str, args: &str) -> PyResult<(bool, String)> {
    let host = HOST.with(Cell::get);
    if host.is_null() {
        return Err(PyRuntimeError::new_err(
            "ReEtched can only be called from the main script thread while the app is running Python \
             (use reetched.ui.call_later to get back onto it)",
        ));
    }
    // SAFETY: non-null only while reetched_dispatch runs on this thread, and the
    // caller guarantees the struct outlives that call.
    let host = unsafe { &*host };
    let (m, a) = (cstring(method), cstring(args));
    let mut out: *mut c_char = ptr::null_mut();
    let rc = unsafe { (host.call)(host.ctx, m.as_ptr(), a.as_ptr(), &mut out) };
    let text = if out.is_null() {
        String::new()
    } else {
        let s = unsafe { CStr::from_ptr(out) }
            .to_string_lossy()
            .into_owned();
        unsafe { (host.free)(host.ctx, out) };
        s
    };
    Ok((rc == 0, text))
}

/// True when host calls are possible right now on this thread.
#[pyfunction]
fn has_host() -> bool {
    !HOST.with(Cell::get).is_null()
}

fn cstring(s: &str) -> CString {
    // Interior NULs cannot cross a C string boundary; replace rather than fail.
    CString::new(s.replace('\0', "\u{FFFD}")).unwrap_or_default()
}

fn init_python() -> Result<(), String> {
    static INIT: Once = Once::new();
    let mut result = Ok(());
    INIT.call_once(|| {
        // PYTHONHOME is set by reetched-script before this DLL is loaded.
        Python::initialize();
        result = Python::attach(|py| -> PyResult<()> {
            let m = PyModule::new(py, "_reetched")?;
            m.add_function(wrap_pyfunction!(call, &m)?)?;
            m.add_function(wrap_pyfunction!(has_host, &m)?)?;
            let sources = PyDict::new(py);
            for (name, pkg, src) in SOURCES {
                sources.set_item(name, (pkg, src))?;
            }
            m.add("SOURCES", sources)?;
            py.import("sys")?
                .getattr("modules")?
                .set_item("_reetched", m)?;
            py.run(BOOTSTRAP, None, None)
        })
        .map_err(|e| Python::attach(|py| format_err(py, e)));
    });
    result
}

fn format_err(py: Python<'_>, e: PyErr) -> String {
    let tb = e
        .traceback(py)
        .and_then(|tb| tb.format().ok())
        .unwrap_or_default();
    format!("{tb}{e}")
}

fn dispatch(op: &str, args: &str) -> Result<String, String> {
    if op == "init" {
        init_python()?;
    } else if unsafe { pyo3::ffi::Py_IsInitialized() } == 0 {
        return Err("Python engine not initialized".into());
    }
    Python::attach(|py| {
        let f = py.import("reetched._engine")?.getattr("dispatch")?;
        f.call1((op, args))?.extract::<String>()
    })
    .map_err(|e| Python::attach(|py| format_err(py, e)))
}

#[unsafe(no_mangle)]
pub extern "C" fn reetched_abi_version() -> u32 {
    1
}

/// # Safety
/// `op` and `args` must be valid NUL-terminated strings; `host` null or valid
/// for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn reetched_dispatch(
    host: *const Host,
    op: *const c_char,
    args: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let op = unsafe { CStr::from_ptr(op) }.to_string_lossy();
        let args = unsafe { CStr::from_ptr(args) }.to_string_lossy();
        let _guard = HostGuard(HOST.with(|h| h.replace(host)));
        dispatch(&op, &args)
    }));
    let json = match result {
        // The engine already wraps its answer as {"ok": ...} / {"error": ...}.
        Ok(Ok(s)) => s,
        Ok(Err(e)) => error_json(&e),
        Err(_) => error_json("panic inside reetched_python"),
    };
    cstring(&json).into_raw()
}

/// # Safety
/// `s` must come from `reetched_dispatch` (or be null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn reetched_free(s: *mut c_char) {
    if !s.is_null() {
        let _ = catch_unwind(|| drop(unsafe { CString::from_raw(s) }));
    }
}

fn error_json(msg: &str) -> String {
    let mut out = String::from("{\"error\":\"");
    for c in msg.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push_str("\"}");
    out
}
