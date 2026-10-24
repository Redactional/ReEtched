use reetched_shader::{Location, ParamType, Severity, Shader, channel, translate};

const HOST: &str = include_str!("host_stub.glsl");

fn load(name: &str) -> String {
    let path = format!("{}/tests/shaders/{name}.glsl", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap()
}

fn ok(name: &str) -> Shader {
    match translate(&load(name), HOST) {
        Ok(s) => {
            roundtrip_wgsl(name, &s);
            s
        }
        Err(diags) => {
            let list: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
            panic!("{name} failed:\n{}", list.join("\n"));
        }
    }
}

/// The module must survive naga's WGSL backend and re-parse/validate.
fn roundtrip_wgsl(name: &str, s: &Shader) {
    let wgsl =
        naga::back::wgsl::write_string(&s.module, &s.info, naga::back::wgsl::WriterFlags::empty())
            .unwrap_or_else(|e| panic!("{name}: WGSL backend failed: {e}"));
    let module = naga::front::wgsl::parse_str(&wgsl)
        .unwrap_or_else(|e| panic!("{name}: WGSL re-parse failed: {}", e.emit_to_string(&wgsl)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .unwrap_or_else(|e| {
        panic!(
            "{name}: WGSL re-validate failed: {}",
            e.emit_to_string(&wgsl)
        )
    });
}

#[test]
fn pbr_metal_rough() {
    let s = ok("pbr_metal_rough");
    for c in [
        channel::BASE_COLOR,
        channel::ROUGHNESS,
        channel::METALLIC,
        channel::SPECULAR_LEVEL,
        channel::EMISSIVE,
    ] {
        assert!(s.channels.contains(&c), "missing channel {c}");
    }
    // Library parameters show up like Painter's "Common Parameters".
    let ao = s.params.iter().find(|p| p.name == "ao_intensity").unwrap();
    assert!(ao.from_library);
    assert_eq!(ao.default, vec![0.75]);
    let quality = s.params.iter().find(|p| p.name == "nbSamples").unwrap();
    assert_eq!(quality.widget.as_deref(), Some("combobox"));
    assert_eq!(quality.values.first().map(|v| v.1), Some(4.0));
    assert_eq!(quality.values.len(), 6);
    assert!(s.auto_params.iter().any(|a| a == "world_eye_position"));
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    assert!(
        s.module
            .entry_points
            .iter()
            .any(|e| e.name == s.entry_point)
    );
}

#[test]
fn spec_gloss() {
    let s = ok("spec_gloss");
    assert_eq!(s.states, vec![("cull_face".to_string(), "off".to_string())]);
    // diffuse / specular / glossiness have no ReEtched channel: warned, read as defaults.
    assert_eq!(
        s.warnings
            .iter()
            .filter(|w| w.severity == Severity::Warning)
            .count(),
        3
    );
    assert!(
        matches!(s.warnings[0].location, Location::User { line: 6 }),
        "{:?}",
        s.warnings[0]
    );
}

#[test]
fn toon_unlit_params() {
    let s = ok("toon_unlit");
    let names: Vec<&str> = s.params.iter().map(|p| p.name.as_str()).collect();
    // User params first, in declaration order, then library ones.
    assert_eq!(
        &names[..6],
        [
            "bands",
            "outline_color",
            "outline_width",
            "unlit",
            "paper_mode",
            "paper_tiling"
        ]
    );

    let p = |n: &str| s.params.iter().find(|p| p.name == n).unwrap();
    // std140: int @0, vec3 aligned to 16, float packs after the vec3, bool as int,
    // int, vec2 aligned to 8.
    assert_eq!((p("bands").offset, p("bands").ty), (0, ParamType::Int));
    assert_eq!(
        (p("outline_color").offset, p("outline_color").size),
        (16, 12)
    );
    assert_eq!(p("outline_width").offset, 28);
    assert_eq!((p("unlit").offset, p("unlit").ty), (32, ParamType::Bool));
    assert_eq!(p("paper_mode").offset, 36);
    assert_eq!(p("paper_tiling").offset, 40);
    assert_eq!(s.uniform_size % 16, 0);

    assert_eq!(p("bands").default, vec![4.0]);
    assert_eq!(p("bands").step, Some(1.0));
    assert_eq!(p("outline_color").default, vec![0.1, 0.05, 0.2]);
    assert_eq!(p("outline_color").widget.as_deref(), Some("color"));
    assert_eq!(p("outline_width").label, "Outline width");
    assert_eq!(
        (p("outline_width").min, p("outline_width").max),
        (Some(0.0), Some(1.0))
    );
    assert_eq!(p("unlit").default, vec![0.0]);
    assert_eq!(
        p("unlit").description.as_deref(),
        Some("Skip lighting entirely")
    );
    assert_eq!(p("paper_mode").group.as_deref(), Some("Paper"));
    assert_eq!(
        p("paper_mode").values,
        vec![
            ("Off".to_string(), 0.0),
            ("Multiply".to_string(), 1.0),
            ("Overlay".to_string(), 2.0)
        ]
    );
    assert_eq!(p("paper_tiling").default, vec![4.0, 4.0]);

    assert_eq!(s.textures.len(), 1);
    let t = &s.textures[0];
    assert_eq!(
        (t.name.as_str(), t.label.as_str(), t.binding),
        ("paper_tex", "Paper", 1)
    );
    assert_eq!(t.usage.as_deref(), Some("texture"));
    assert_eq!(t.default.as_deref(), Some("paper_grain"));
    assert_eq!(t.default_color, Some([1.0, 1.0, 1.0, 1.0]));

    assert!(s.channels.contains(&channel::MESH_CURVATURE));

    // Defaults land in the block bytes.
    let data = s.default_uniform_data();
    assert_eq!(data.len() as u32, s.uniform_size);
    assert_eq!(i32::from_le_bytes(data[0..4].try_into().unwrap()), 4);
    assert_eq!(f32::from_le_bytes(data[28..32].try_into().unwrap()), 0.25);
    assert_eq!(f32::from_le_bytes(data[44..48].try_into().unwrap()), 4.0);
}

#[test]
fn aniso_sss() {
    let s = ok("aniso_sss");
    for c in [
        channel::ANISOTROPY,
        channel::ANISOTROPY_ANGLE,
        channel::SCATTERING,
        channel::SCATTERING_COLOR,
    ] {
        assert!(s.channels.contains(&c), "missing channel {c}");
    }
    let color = s.params.iter().find(|p| p.name == "sssColor").unwrap();
    assert_eq!(color.ty, ParamType::Vec3);
    assert_eq!(color.offset % 16, 0);
}

#[test]
fn alpha_parallax() {
    let s = ok("alpha_parallax");
    assert!(
        s.states
            .contains(&("blend".to_string(), "over".to_string()))
    );
    assert!(s.params.iter().any(|p| p.name == "alpha_threshold"));
    assert!(s.params.iter().any(|p| p.name == "usePOM"));
    // Painter's POM reads the displacement channel, which ReEtched lacks.
    assert!(!s.channels.contains(&-1));
}

#[test]
fn sparse_helper() {
    let s = ok("sparse_helper");
    assert!(s.disable_srgb_conversion);
    assert!(s.auto_params.iter().any(|a| a == "time"));
    // channel_user0 is warned about at its line.
    assert!(
        s.warnings
            .iter()
            .any(|w| w.location == Location::User { line: 14 }
                && w.message.contains("channel_user0"))
    );
}

#[test]
fn error_line_in_user_file() {
    let diags = translate(&load("broken"), HOST).unwrap_err();
    let first = &diags[0];
    assert_eq!(first.severity, Severity::Error);
    assert_eq!(first.location, Location::User { line: 12 }, "{first}");
}

#[test]
fn unknown_import_and_bad_json() {
    let src = "import lib-nope.glsl\n//: param custom { \"default\": 1, }\nuniform float x;\nvoid shade(V2F inputs) {}\n";
    let diags = translate(src, HOST).unwrap_err();
    assert!(
        diags
            .iter()
            .any(|d| d.location == Location::User { line: 1 } && d.message.contains("lib-nope"))
    );
    assert!(
        diags
            .iter()
            .any(|d| d.location == Location::User { line: 2 } && d.message.contains("JSON"))
    );
}

#[test]
fn user_redeclares_library_param() {
    // The user's own `ao_intensity` replaces lib-sampler's.
    let src = "import lib-sampler.glsl\n//: param custom { \"default\": 1.0, \"label\": \"AO\" }\nuniform float ao_intensity;\nvoid shade(V2F inputs) { diffuseShadingOutput(vec3(getAO(inputs.sparse_coord))); }\n";
    let s = translate(src, HOST).unwrap_or_else(|d| panic!("{d:?}"));
    let ao: Vec<_> = s
        .params
        .iter()
        .filter(|p| p.name == "ao_intensity")
        .collect();
    assert_eq!(ao.len(), 1);
    assert_eq!((ao[0].label.as_str(), ao[0].from_library), ("AO", false));
}

#[test]
fn error_inside_library_is_reported_as_library() {
    // A host header missing a function makes our library fail; say so.
    let host = HOST.replace("vec3 rx_env_specular(", "vec3 rx_env_specular_gone(");
    let diags = translate(&load("pbr_metal_rough"), &host).unwrap_err();
    assert!(
        matches!(
            diags[0].location,
            Location::Library {
                module: "lib-pbr",
                ..
            }
        ),
        "{}",
        diags[0]
    );
    assert!(
        diags[0]
            .to_string()
            .contains("inside ReEtched's lib-pbr.glsl"),
        "{}",
        diags[0]
    );
}
