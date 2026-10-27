//! The developer console: a separate window with the debug log (Settings >
//! Developer). Off by default; on Windows the app is a GUI program without one.
//! Elsewhere the log goes to the terminal the app was started from.

/// Shows or hides the console, and logs at debug level while it's shown.
pub fn set_visible(on: bool) {
    log::set_max_level(if on {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Warn
    });
    #[cfg(windows)]
    windows_console(on);
    if on {
        log::debug!("debug log on (ReEtched {})", env!("CARGO_PKG_VERSION"));
    }
}

#[cfg(windows)]
fn windows_console(on: bool) {
    use windows_sys::Win32::System::Console::{
        AllocConsole, FreeConsole, GetConsoleWindow, SetConsoleCtrlHandler, SetConsoleTitleW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DeleteMenu, GetSystemMenu, MF_BYCOMMAND, SC_CLOSE,
    };
    // SAFETY: plain Win32 console calls on this process's own console.
    unsafe {
        let has = !GetConsoleWindow().is_null();
        if on && !has && AllocConsole() != 0 {
            let title: Vec<u16> = "ReEtched debug log\0".encode_utf16().collect();
            SetConsoleTitleW(title.as_ptr());
            // Closing a console window (or Ctrl+C in it) ends the whole process,
            // unsaved work included: remove its close button and ignore Ctrl+C.
            // It's closed from Settings instead.
            DeleteMenu(GetSystemMenu(GetConsoleWindow(), 0), SC_CLOSE, MF_BYCOMMAND);
            SetConsoleCtrlHandler(None, 1);
        } else if !on && has {
            FreeConsole();
        }
    }
}
