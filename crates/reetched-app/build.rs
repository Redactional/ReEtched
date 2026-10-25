//! Embeds the app icon and version info into the Windows executable.
//! (macOS and Linux get their icons from the installer; see
//! `[package.metadata.packager]` in Cargo.toml.)

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icons/ReEtched.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icons/ReEtched.ico")
            .set("ProductName", "ReEtched")
            .set("FileDescription", "ReEtched");
        // A missing resource compiler shouldn't stop development builds.
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the app icon: {e}");
        }
    }
}
