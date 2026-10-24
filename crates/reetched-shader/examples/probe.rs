//! Debug helper: translate a Painter shader with the test host stub and print
//! the diagnostics, parameters and the resulting WGSL.
//! `cargo run -p reetched-shader --example probe -- path/to/shader.glsl [--glsl]`

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: probe <shader.glsl> [--glsl]");
    let show_glsl = args.next().as_deref() == Some("--glsl");
    let src = std::fs::read_to_string(&path).expect("read shader");
    let host = include_str!("../tests/host_stub.glsl");
    match reetched_shader::translate(&src, host) {
        Ok(s) => {
            for w in &s.warnings {
                println!("{w}");
            }
            for p in &s.params {
                println!(
                    "param {} {:?} @{}+{} default {:?} ({})",
                    p.name, p.ty, p.offset, p.size, p.default, p.label
                );
            }
            for t in &s.textures {
                println!("texture {} binding {}", t.name, t.binding);
            }
            println!(
                "channels {:?}\nauto {:?}\nstates {:?}",
                s.channels, s.auto_params, s.states
            );
            if show_glsl {
                println!("{}", s.glsl);
            } else {
                let wgsl = naga::back::wgsl::write_string(
                    &s.module,
                    &s.info,
                    naga::back::wgsl::WriterFlags::empty(),
                )
                .expect("wgsl");
                println!("{wgsl}");
            }
        }
        Err(diags) => {
            for d in diags {
                println!("{d}");
            }
        }
    }
}
