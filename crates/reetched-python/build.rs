//! macOS: leave Python's symbols undefined in the bridge; they resolve against
//! the libpython the app loads first (see `extension-module` in Cargo.toml).

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
}
