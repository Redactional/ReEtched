//! Loads Substance 3D Painter custom shaders (`.glsl` written against Painter's
//! documented shader API) and turns them into a validated naga fragment-shader
//! module for the ReEtched viewport.
//!
//! The final GLSL handed to naga is, in order:
//! `#version 450`, the host header (see `HOST.md`), `lib-sparse` (types, outputs,
//! sampling), a generated prelude (parameter block, user textures, globals),
//! the Painter libraries the shader imports, the user's code with its
//! annotations stripped line-for-line, and a generated `main()`.

use std::collections::HashSet;
use std::fmt;
use std::fmt::Write as _;

/// ReEtched channel ids, as passed to the host's `rx_channel`.
pub mod channel {
    pub const BASE_COLOR: i32 = 0;
    pub const METALLIC: i32 = 1;
    pub const ROUGHNESS: i32 = 2;
    pub const NORMAL: i32 = 3;
    pub const HEIGHT: i32 = 4;
    pub const EMISSIVE: i32 = 5;
    pub const OPACITY: i32 = 6;
    pub const SPECULAR_LEVEL: i32 = 7;
    pub const SPECULAR_COLOR: i32 = 8;
    pub const ANISOTROPY: i32 = 9;
    pub const ANISOTROPY_ANGLE: i32 = 10;
    pub const TRANSMISSION: i32 = 11;
    pub const SCATTERING: i32 = 12;
    pub const SCATTERING_COLOR: i32 = 13;
    pub const COAT: i32 = 14;
    pub const COAT_COLOR: i32 = 15;
    pub const COAT_ROUGHNESS: i32 = 16;
    pub const SHEEN: i32 = 17;
    pub const SHEEN_COLOR: i32 = 18;
    pub const SHEEN_ROUGHNESS: i32 = 19;
    pub const AMBIENT_OCCLUSION: i32 = 20;
    // Baked mesh maps, read through the same host function.
    pub const MESH_AMBIENT_OCCLUSION: i32 = 32;
    pub const MESH_CURVATURE: i32 = 33;
    pub const MESH_THICKNESS: i32 = 34;
    pub const MESH_NORMAL: i32 = 35;
    pub const MESH_WORLD_NORMAL: i32 = 36;
    pub const MESH_POSITION: i32 = 37;
    pub const MESH_ID: i32 = 38;
}

/// Maps a Painter `channel_*` / `texture_*` tag to a ReEtched channel id.
/// `Some(-1)` = a Painter tag ReEtched has no data for (reads as empty).
fn sparse_tag(tag: &str) -> Option<i32> {
    use channel::*;
    let id = match tag {
        "channel_basecolor" => BASE_COLOR,
        "channel_metallic" => METALLIC,
        "channel_roughness" => ROUGHNESS,
        "channel_normal" => NORMAL,
        "channel_height" => HEIGHT,
        "channel_emissive" => EMISSIVE,
        "channel_opacity" => OPACITY,
        "channel_specularlevel" => SPECULAR_LEVEL,
        "channel_specularedgecolor" => SPECULAR_COLOR,
        "channel_anisotropylevel" => ANISOTROPY,
        "channel_anisotropyangle" => ANISOTROPY_ANGLE,
        "channel_transmissive" => TRANSMISSION,
        "channel_scattering" => SCATTERING,
        "channel_scatteringcolor" => SCATTERING_COLOR,
        "channel_coatopacity" | "channel_coat" => COAT,
        "channel_coatcolor" => COAT_COLOR,
        "channel_coatroughness" => COAT_ROUGHNESS,
        "channel_sheenopacity" | "channel_sheen" => SHEEN,
        "channel_sheencolor" => SHEEN_COLOR,
        "channel_sheenroughness" => SHEEN_ROUGHNESS,
        "channel_ambientocclusion" | "channel_ao" => AMBIENT_OCCLUSION,
        "texture_ambientocclusion" | "texture_ao" => MESH_AMBIENT_OCCLUSION,
        "texture_curvature" => MESH_CURVATURE,
        "texture_thickness" => MESH_THICKNESS,
        "texture_normal" => MESH_NORMAL,
        "texture_normal_ws" => MESH_WORLD_NORMAL,
        "texture_position" => MESH_POSITION,
        "texture_id" => MESH_ID,
        _ if tag.starts_with("channel_") || tag.starts_with("texture_") => -1,
        _ => return None,
    };
    Some(id)
}

/// Engine values for `//: param auto <id>` on non-texture uniforms:
/// (GLSL type, expression built on host functions).
fn engine_value(id: &str) -> Option<(&'static str, &'static str)> {
    Some(match id {
        "world_eye_position" => ("vec3", "rx_eye_position()"),
        "world_camera_direction" => ("vec3", "rx_camera_direction()"),
        "camera_view_matrix" => ("mat4", "rx_view_matrix()"),
        "camera_view_matrix_it" => ("mat4", "rx_view_matrix_it()"),
        "camera_vp_matrix_inverse" => ("mat4", "rx_inv_view_proj_matrix()"),
        // ponytail: ReEtched draws meshes untransformed, so model = identity.
        "mvp_matrix" => ("mat4", "rx_view_proj_matrix()"),
        "is_perspective_projection" => ("bool", "rx_is_perspective()"),
        "fovy" => ("float", "rx_fovy()"),
        "aspect_ratio" => ("float", "(rx_screen_size().x * rx_screen_size().w)"),
        "screen_size" => ("vec4", "rx_screen_size()"),
        "environment_exposure" => ("float", "rx_env_exposure()"),
        "environment_rotation" => ("float", "rx_env_rotation()"),
        "environment_max_lod" => ("float", "rx_env_max_lod()"),
        "is_2d_view" => ("bool", "rx_is_2d_view()"),
        "main_light" => ("vec4", "rx_main_light()"),
        "scene_original_radius" => ("float", "rx_scene_radius()"),
        "facing" => ("int", "(gl_FrontFacing ? 1 : -1)"),
        "random_seed" => ("int", "int(rx_time() * 60.0)"),
        // ReEtched extension: seconds since start.
        "time" => ("float", "rx_time()"),
        // Settings Painter feeds its own libraries; fixed in ReEtched.
        "ao_blending_mode" => ("int", "4"), // BlendingMode_Multiply
        "normal_blending_mode" => ("int", "26"), // BlendingMode_NM_Combine
        "normal_y_coeff" => ("float", "1.0"), // OpenGL-style normals
        "conversion_linear_to_srgb" => ("bool", "false"),
        "shadow_mask_enable" => ("bool", "false"),
        "shadow_mask_opacity" => ("float", "0.0"),
        _ => return None,
    })
}

struct LibModule {
    name: &'static str,
    deps: &'static [&'static str],
    source: &'static str,
}

/// Painter's libraries, dependencies before dependents. `lib-sparse` is always included.
const LIBRARY: &[LibModule] = &[
    LibModule {
        name: "lib-sparse",
        deps: &[],
        source: include_str!("painter/lib-sparse.glsl"),
    },
    LibModule {
        name: "lib-defines",
        deps: &[],
        source: include_str!("painter/lib-defines.glsl"),
    },
    LibModule {
        name: "lib-random",
        deps: &["lib-defines"],
        source: include_str!("painter/lib-random.glsl"),
    },
    LibModule {
        name: "lib-bayer",
        deps: &[],
        source: include_str!("painter/lib-bayer.glsl"),
    },
    LibModule {
        name: "lib-utils",
        deps: &["lib-bayer"],
        source: include_str!("painter/lib-utils.glsl"),
    },
    LibModule {
        name: "lib-env",
        deps: &["lib-defines"],
        source: include_str!("painter/lib-env.glsl"),
    },
    LibModule {
        name: "lib-emissive",
        deps: &[],
        source: include_str!("painter/lib-emissive.glsl"),
    },
    LibModule {
        name: "lib-normal",
        deps: &["lib-defines"],
        source: include_str!("painter/lib-normal.glsl"),
    },
    LibModule {
        name: "lib-vectors",
        deps: &["lib-normal"],
        source: include_str!("painter/lib-vectors.glsl"),
    },
    LibModule {
        name: "lib-sampler",
        deps: &["lib-defines"],
        source: include_str!("painter/lib-sampler.glsl"),
    },
    LibModule {
        name: "lib-pbr",
        deps: &["lib-env", "lib-emissive", "lib-random", "lib-vectors"],
        source: include_str!("painter/lib-pbr.glsl"),
    },
    LibModule {
        name: "lib-pbr-aniso",
        deps: &["lib-pbr"],
        source: include_str!("painter/lib-pbr-aniso.glsl"),
    },
    LibModule {
        name: "lib-alpha",
        deps: &["lib-sampler", "lib-random"],
        source: include_str!("painter/lib-alpha.glsl"),
    },
    LibModule {
        name: "lib-pom",
        deps: &["lib-normal", "lib-sampler"],
        source: include_str!("painter/lib-pom.glsl"),
    },
    LibModule {
        name: "lib-sss",
        deps: &["lib-sampler"],
        source: include_str!("painter/lib-sss.glsl"),
    },
];

/// Names of the Painter libraries this crate implements (without `.glsl`).
pub fn library_names() -> impl Iterator<Item = &'static str> {
    LIBRARY.iter().map(|m| m.name)
}

// ---------------------------------------------------------------- public types

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// Where a diagnostic points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    /// 1-based line in the user's shader file.
    User {
        line: usize,
    },
    /// Inside our implementation of a Painter library (1-based line in that module).
    Library {
        module: &'static str,
        line: usize,
    },
    /// Inside the host header (1-based line).
    HostHeader {
        line: usize,
    },
    /// In code this crate generated (parameter prelude or `main`).
    Generated,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub location: Location,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        match &self.location {
            Location::User { line } => write!(f, "line {line}: {sev}: {}", self.message),
            Location::Library { module, line } => {
                write!(
                    f,
                    "{sev} inside ReEtched's {module}.glsl (line {line}): {}",
                    self.message
                )
            }
            Location::HostHeader { line } => write!(
                f,
                "{sev} inside the host header (line {line}): {}",
                self.message
            ),
            Location::Generated => write!(f, "{sev} in generated code: {}", self.message),
            Location::Unknown => write!(f, "{sev}: {}", self.message),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    Bool,
    Int,
    IVec2,
    IVec3,
    IVec4,
    Float,
    Vec2,
    Vec3,
    Vec4,
}

impl ParamType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "bool" => Self::Bool,
            "int" => Self::Int,
            "ivec2" => Self::IVec2,
            "ivec3" => Self::IVec3,
            "ivec4" => Self::IVec4,
            "float" => Self::Float,
            "vec2" => Self::Vec2,
            "vec3" => Self::Vec3,
            "vec4" => Self::Vec4,
            _ => return None,
        })
    }

    /// Number of scalar components.
    pub fn components(self) -> usize {
        match self {
            Self::Bool | Self::Int | Self::Float => 1,
            Self::IVec2 | Self::Vec2 => 2,
            Self::IVec3 | Self::Vec3 => 3,
            Self::IVec4 | Self::Vec4 => 4,
        }
    }

    /// True when stored as 32-bit signed ints in the block (bools are 0/1 ints).
    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Self::Bool | Self::Int | Self::IVec2 | Self::IVec3 | Self::IVec4
        )
    }

    fn block_glsl(self) -> &'static str {
        match self {
            Self::Bool | Self::Int => "int",
            Self::IVec2 => "ivec2",
            Self::IVec3 => "ivec3",
            Self::IVec4 => "ivec4",
            Self::Float => "float",
            Self::Vec2 => "vec2",
            Self::Vec3 => "vec3",
            Self::Vec4 => "vec4",
        }
    }

    /// std140 (alignment, size) in bytes.
    fn std140(self) -> (u32, u32) {
        match self.components() {
            1 => (4, 4),
            2 => (8, 8),
            3 => (16, 12),
            _ => (16, 16),
        }
    }
}

/// A custom (`//: param custom`) value parameter, stored in the std140 block at
/// `layout(set = 2, binding = 0)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub label: String,
    pub ty: ParamType,
    /// One value per component; bools are 0.0 / 1.0.
    pub default: Vec<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    /// `"color"`, `"combobox"`, ... as written in the JSON.
    pub widget: Option<String>,
    /// Combobox entries (label, value), sorted by value.
    pub values: Vec<(String, f64)>,
    pub group: Option<String>,
    pub description: Option<String>,
    pub visible: bool,
    /// Byte offset and size in the uniform block.
    pub offset: u32,
    pub size: u32,
    /// Declared by one of our Painter libraries (e.g. "AO Intensity") rather than the user.
    pub from_library: bool,
}

/// A custom texture (`uniform sampler2D` with `//: param custom`), bound as a
/// `texture2D` at `layout(set = 2, binding = binding)` and sampled with the
/// shared sampler at binding 9.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureParam {
    pub name: String,
    pub label: String,
    pub binding: u32,
    /// `"texture"` or `"environment"`.
    pub usage: Option<String>,
    /// Name of a shelf resource in Painter.
    pub default: Option<String>,
    pub default_color: Option<[f32; 4]>,
    pub group: Option<String>,
    pub description: Option<String>,
}

pub const PARAMS_BINDING: u32 = 0;
pub const FIRST_TEXTURE_BINDING: u32 = 1;
pub const MAX_TEXTURES: u32 = 8;
pub const USER_SAMPLER_BINDING: u32 = 9;
pub const USER_BIND_GROUP: u32 = 2;

/// A translated, validated Painter shader.
#[derive(Debug)]
pub struct Shader {
    pub module: naga::Module,
    pub info: naga::valid::ModuleInfo,
    /// The fragment entry point name in `module`.
    pub entry_point: &'static str,
    pub params: Vec<Param>,
    /// Size of the std140 parameter block; 0 means no block is declared.
    pub uniform_size: u32,
    pub textures: Vec<TextureParam>,
    /// ReEtched channel / mesh-map ids the shader (and its libraries) can read.
    pub channels: Vec<i32>,
    /// Every `param auto` id used by the shader and its libraries.
    pub auto_params: Vec<String>,
    /// `//: state <name> <value>` lines (cull_face, blend, ...).
    pub states: Vec<(String, String)>,
    /// The shader defines `DISABLE_FRAMEBUFFER_SRGB_CONVERSION`: it writes sRGB itself.
    pub disable_srgb_conversion: bool,
    pub warnings: Vec<Diagnostic>,
    /// The full GLSL given to naga, for debugging.
    pub glsl: String,
}

impl Shader {
    /// The parameter block filled with every parameter's default value.
    pub fn default_uniform_data(&self) -> Vec<u8> {
        let mut data = vec![0u8; self.uniform_size as usize];
        for p in &self.params {
            for (i, v) in p.default.iter().enumerate() {
                let at = p.offset as usize + 4 * i;
                let bytes = if p.ty.is_integer() {
                    (*v as i32).to_le_bytes()
                } else {
                    (*v as f32).to_le_bytes()
                };
                data[at..at + 4].copy_from_slice(&bytes);
            }
        }
        data
    }
}

// ------------------------------------------------------------- preprocessing

#[derive(Clone, Copy, PartialEq)]
enum Unit {
    User,
    Library(&'static str),
}

impl Unit {
    fn at(self, line0: usize) -> Location {
        match self {
            Unit::User => Location::User { line: line0 + 1 },
            Unit::Library(module) => Location::Library {
                module,
                line: line0 + 1,
            },
        }
    }
}

enum DeclKind {
    Custom(Param),
    Texture(TextureParam),
    /// Engine value or channel; `init` is the GLSL expression assigned in main.
    Auto {
        init: String,
    },
}

struct Decl {
    name: String,
    /// GLSL type of the private global (`SamplerSparse` already lowered to `int`).
    glsl_type: String,
    kind: DeclKind,
    unit: Unit,
    line: usize,
}

#[derive(Default)]
struct Ctx {
    decls: Vec<Decl>,
    imports: Vec<&'static str>,
    states: Vec<(String, String)>,
    channels: Vec<i32>,
    auto_params: Vec<String>,
    diags: Vec<Diagnostic>,
    disable_srgb: bool,
    /// Names known to be SamplerSparse, for rewriting `.size`.
    sparse_names: HashSet<String>,
}

impl Ctx {
    fn error(&mut self, location: Location, message: impl Into<String>) {
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            location,
            message: message.into(),
        });
    }
    fn warn(&mut self, location: Location, message: impl Into<String>) {
        self.diags.push(Diagnostic {
            severity: Severity::Warning,
            location,
            message: message.into(),
        });
    }

    fn add_decl(&mut self, decl: Decl) {
        if let Some(old) = self.decls.iter().find(|d| d.name == decl.name) {
            // The user re-declaring a library uniform (e.g. `ao_intensity`) is
            // fine as long as the type agrees; the user's declaration wins.
            let user_vs_lib = old.unit == Unit::User && decl.unit != Unit::User;
            if user_vs_lib && old.glsl_type == decl.glsl_type {
                return;
            }
            let msg = if user_vs_lib {
                format!(
                    "uniform `{}` is also declared by {}.glsl as `{}`",
                    decl.name,
                    match decl.unit {
                        Unit::Library(m) => m,
                        Unit::User => "",
                    },
                    decl.glsl_type
                )
            } else {
                format!("uniform `{}` is declared twice", decl.name)
            };
            let at = old.unit.at(old.line);
            self.error(at, msg);
            return;
        }
        self.decls.push(decl);
    }
}

fn process(src: &str, unit: Unit, ctx: &mut Ctx) -> String {
    let mut out = String::with_capacity(src.len() + 64);
    // `//: ...` directives may span several `//:` lines.
    let mut directive: Option<(String, usize)> = None;
    let mut annotation: Option<(String, usize)> = None;

    for (i, raw) in src.lines().enumerate() {
        let t = raw.trim();
        if let Some(rest) = t.strip_prefix("//:") {
            let rest = rest.trim();
            let word = rest.split_whitespace().next().unwrap_or("");
            if matches!(word, "param" | "state" | "metadata") {
                if let Some(d) = directive.take() {
                    finish_directive(d, unit, ctx, &mut annotation);
                }
                directive = Some((rest.to_string(), i));
            } else if let Some(d) = directive.as_mut() {
                d.0.push(' ');
                d.0.push_str(rest);
            } else if !rest.is_empty() {
                ctx.warn(
                    unit.at(i),
                    format!("ignored unknown directive `//: {rest}`"),
                );
            }
            out.push_str(raw);
            out.push('\n');
            continue;
        }
        if let Some(d) = directive.take() {
            finish_directive(d, unit, ctx, &mut annotation);
        }

        let line = if t.is_empty() || t.starts_with("//") {
            raw.to_string()
        } else if let Some(name) = t.strip_prefix("import ") {
            let name = name.trim().trim_end_matches(';').trim();
            let name = name.strip_suffix(".glsl").unwrap_or(name);
            match LIBRARY.iter().find(|m| m.name == name) {
                Some(m) => {
                    if !ctx.imports.contains(&m.name) {
                        ctx.imports.push(m.name);
                    }
                }
                None => ctx.error(unit.at(i), format!("unknown library `{name}.glsl`; only Painter's built-in libraries can be imported")),
            }
            String::new()
        } else if t.starts_with("#version") {
            ctx.warn(
                unit.at(i),
                "ignored #version: the shader is compiled as GLSL 450",
            );
            String::new()
        } else if t.starts_with("uniform ") {
            uniform(t, i, annotation.take(), unit, ctx);
            String::new()
        } else {
            if let Some((_, at)) = annotation.take() {
                ctx.warn(
                    unit.at(at),
                    "`//: param` annotation is not followed by a uniform; ignored",
                );
            }
            if t.starts_with("#define")
                && t.split_whitespace().nth(1) == Some("DISABLE_FRAMEBUFFER_SRGB_CONVERSION")
            {
                ctx.disable_srgb = true;
            }
            if unit == Unit::User {
                rewrite_user_line(raw, &mut ctx.sparse_names)
            } else {
                raw.to_string()
            }
        };
        out.push_str(&line);
        out.push('\n');
    }
    if let Some(d) = directive.take() {
        finish_directive(d, unit, ctx, &mut annotation);
    }
    if let Some((_, at)) = annotation {
        ctx.warn(
            unit.at(at),
            "`//: param` annotation is not followed by a uniform; ignored",
        );
    }
    out
}

fn finish_directive(
    (text, line): (String, usize),
    unit: Unit,
    ctx: &mut Ctx,
    annotation: &mut Option<(String, usize)>,
) {
    let mut words = text.split_whitespace();
    match words.next() {
        Some("param") => {
            if let Some((_, at)) = annotation.take() {
                ctx.warn(
                    unit.at(at),
                    "`//: param` annotation is not followed by a uniform; ignored",
                );
            }
            *annotation = Some((text["param".len()..].trim().to_string(), line));
        }
        Some("state") => {
            let name = words.next().unwrap_or("").to_string();
            let value = words.collect::<Vec<_>>().join(" ");
            ctx.states.push((name, value));
        }
        _ => ctx.warn(unit.at(line), "`//: metadata` is ignored by ReEtched"),
    }
}

/// Handles one `uniform <type> <name>;` line and its annotation.
fn uniform(t: &str, line: usize, annotation: Option<(String, usize)>, unit: Unit, ctx: &mut Ctx) {
    let at = unit.at(line);
    let decl_text = t.split(';').next().unwrap_or("");
    let words: Vec<&str> = decl_text
        .split_whitespace()
        .skip(1)
        .filter(|w| !matches!(*w, "lowp" | "mediump" | "highp"))
        .collect();
    let (ty, name) = match words.as_slice() {
        [ty, name] if !name.contains(['[', ',']) => (*ty, name.to_string()),
        _ => {
            ctx.error(at, "unsupported uniform declaration; expected `uniform <type> <name>;` (no arrays or lists)");
            return;
        }
    };
    // JSON errors point at the annotation, everything else at the uniform.
    let json_at = annotation.as_ref().map_or(at, |(_, l)| unit.at(*l));
    let (kind_word, body) = match &annotation {
        Some((text, _)) => {
            let text = text.trim();
            let word = text.split_whitespace().next().unwrap_or("");
            (word, text[word.len()..].trim())
        }
        None => ("", ""),
    };

    match (ty, kind_word) {
        ("SamplerSparse", "auto") => {
            ctx.sparse_names.insert(name.clone());
            ctx.auto_params.push(body.to_string());
            // `channel_a or channel_b`: the first one present in the texture set.
            let mut init = "-1".to_string();
            for tag in body
                .split(" or ")
                .map(str::trim)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                let id = match sparse_tag(tag) {
                    Some(id) => id,
                    None => {
                        ctx.error(at, format!("`{tag}` is not a channel or mesh map"));
                        -1
                    }
                };
                if id < 0 {
                    ctx.warn(
                        at,
                        format!("`{tag}` has no ReEtched equivalent; `{name}` reads as empty"),
                    );
                    continue;
                }
                if !ctx.channels.contains(&id) {
                    ctx.channels.push(id);
                }
                init = if init == "-1" {
                    id.to_string()
                } else {
                    format!("(rx_channel_present({id}) ? {id} : {init})")
                };
            }
            ctx.add_decl(Decl {
                name,
                glsl_type: "int".into(),
                kind: DeclKind::Auto { init },
                unit,
                line,
            });
        }
        ("SamplerSparse", _) => {
            ctx.sparse_names.insert(name.clone());
            ctx.warn(at, format!("SamplerSparse `{name}` has no `//: param auto channel_*` annotation; it reads as empty"));
            ctx.add_decl(Decl {
                name,
                glsl_type: "int".into(),
                kind: DeclKind::Auto { init: "-1".into() },
                unit,
                line,
            });
        }
        ("sampler2D", "auto") => {
            ctx.error(at, format!("engine texture `{body}` is not available in ReEtched; use the lib-env / lib-random functions instead"));
        }
        ("sampler2D", _) => {
            let json = parse_json(body, json_at, ctx);
            if kind_word != "custom" {
                ctx.warn(
                    at,
                    format!("texture `{name}` has no `//: param custom` annotation"),
                );
            }
            let s = |k: &str| json.get(k).and_then(|v| v.as_str()).map(str::to_string);
            let default_color = json
                .get("default_color")
                .and_then(|v| v.as_array())
                .map(|a| {
                    let mut c = [0.0f32, 0.0, 0.0, 1.0];
                    for (i, v) in a.iter().take(4).enumerate() {
                        c[i] = v.as_f64().unwrap_or(0.0) as f32;
                    }
                    c
                });
            let tex = TextureParam {
                label: s("label").unwrap_or_else(|| name.clone()),
                name: name.clone(),
                binding: 0,
                usage: s("usage"),
                default: s("default").filter(|d| !d.is_empty()),
                default_color,
                group: s("group"),
                description: s("description"),
            };
            ctx.add_decl(Decl {
                name,
                glsl_type: "sampler2D".into(),
                kind: DeclKind::Texture(tex),
                unit,
                line,
            });
        }
        (_, "auto") => {
            ctx.auto_params.push(body.to_string());
            let init = if let Some((want, expr)) = engine_value(body) {
                if want != ty {
                    ctx.error(at, format!("`param auto {body}` is a `{want}`, not `{ty}`"));
                }
                expr.to_string()
            } else if let Some(id) = body
                .strip_suffix("_size")
                .and_then(sparse_tag)
                .filter(|_| ty == "vec4")
            {
                format!("rx_sparse_size({id})")
            } else {
                ctx.warn(
                    at,
                    format!("`param auto {body}` is not supported by ReEtched; `{name}` is zero"),
                );
                zero_of(ty).unwrap_or("0").to_string()
            };
            if zero_of(ty).is_none() {
                ctx.error(at, format!("unsupported uniform type `{ty}`"));
                return;
            }
            ctx.add_decl(Decl {
                name,
                glsl_type: ty.into(),
                kind: DeclKind::Auto { init },
                unit,
                line,
            });
        }
        _ => {
            let Some(pty) = ParamType::parse(ty) else {
                ctx.error(
                    at,
                    format!("unsupported type `{ty}` for a custom parameter"),
                );
                return;
            };
            if kind_word != "custom" {
                ctx.warn(
                    at,
                    format!(
                        "uniform `{name}` has no `//: param custom` annotation; it defaults to 0"
                    ),
                );
            }
            let json = parse_json(body, json_at, ctx);
            let param = custom_param(&name, pty, &json, unit != Unit::User);
            ctx.add_decl(Decl {
                name,
                glsl_type: ty.into(),
                kind: DeclKind::Custom(param),
                unit,
                line,
            });
        }
    }
}

fn parse_json(body: &str, at: Location, ctx: &mut Ctx) -> serde_json::Value {
    if body.is_empty() {
        return serde_json::Value::Null;
    }
    match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            ctx.error(at, format!("invalid parameter JSON: {e}"));
            serde_json::Value::Null
        }
    }
}

fn custom_param(name: &str, ty: ParamType, json: &serde_json::Value, from_library: bool) -> Param {
    let num = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_bool().map(|b| b as u8 as f64));
    let n = ty.components();
    let mut default = vec![0.0; n];
    match json.get("default") {
        Some(serde_json::Value::Array(a)) => {
            for (d, v) in default.iter_mut().zip(a) {
                *d = num(v).unwrap_or(0.0);
            }
        }
        Some(v) => default = vec![num(v).unwrap_or(0.0); n],
        None => {}
    }
    let s = |k: &str| json.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let f = |k: &str| json.get(k).and_then(|v| v.as_f64());
    // ponytail: serde_json maps don't keep key order (preserve_order would flip
    // it for the whole workspace), so combobox entries are sorted by value.
    let mut values: Vec<(String, f64)> = json
        .get("values")
        .and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_f64()?)))
                .collect()
        })
        .unwrap_or_default();
    values.sort_by(|a, b| a.1.total_cmp(&b.1));
    let (_, size) = ty.std140();
    Param {
        name: name.to_string(),
        label: s("label").unwrap_or_else(|| name.to_string()),
        ty,
        default,
        min: f("min"),
        max: f("max"),
        step: f("step"),
        widget: s("widget"),
        values,
        group: s("group"),
        description: s("description"),
        visible: json
            .get("visible")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        offset: 0,
        size,
        from_library,
    }
}

fn zero_of(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "bool" => "false",
        "int" => "0",
        "float" => "0.0",
        "vec2" => "vec2(0.0)",
        "vec3" => "vec3(0.0)",
        "vec4" => "vec4(0.0)",
        "ivec2" => "ivec2(0)",
        "ivec3" => "ivec3(0)",
        "ivec4" => "ivec4(0)",
        "mat2" => "mat2(0.0)",
        "mat3" => "mat3(0.0)",
        "mat4" => "mat4(0.0)",
        _ => return None,
    })
}

/// Rewrites the few Painter-isms naga's GLSL front end can't take, keeping the
/// line intact otherwise:
/// - `x.is_set` -> `rx_is_set(x)` and `x.size` -> `rx_sparse_size(x)` (SamplerSparse is an int here);
/// - the identifier `sampler` (a type keyword for naga, a common Painter parameter name);
/// - legacy `texture2D(...)` calls.
fn rewrite_user_line(line: &str, sparse_names: &mut HashSet<String>) -> String {
    let b = line.as_bytes();
    let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = String::with_capacity(line.len() + 16);
    let mut prev_ident = String::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            out.push_str(&line[i..]);
            break;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && (is_ident(b[i]) || b[i] == b'.') {
                i += 1;
            }
            out.push_str(&line[start..i]);
            continue;
        }
        if !(c.is_ascii_alphabetic() || c == b'_') {
            out.push(c as char);
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && is_ident(b[i]) {
            i += 1;
        }
        let ident = &line[start..i];
        let after_dot = start > 0 && b[start - 1] == b'.';
        let field = |name: &str| {
            let rest = &line[i..];
            rest.strip_prefix('.')
                .and_then(|r| r.strip_prefix(name))
                .filter(|r| !r.bytes().next().is_some_and(is_ident))
                .is_some()
        };
        if prev_ident == "SamplerSparse" {
            sparse_names.insert(ident.to_string());
        }
        if !after_dot && field("is_set") {
            let _ = write!(out, "rx_is_set({})", rename(ident));
            i += ".is_set".len();
        } else if !after_dot && sparse_names.contains(ident) && field("size") {
            let _ = write!(out, "rx_sparse_size({})", rename(ident));
            i += ".size".len();
        } else if ident == "texture2D" && line[i..].trim_start().starts_with('(') {
            out.push_str("texture");
        } else if after_dot {
            out.push_str(ident);
        } else {
            out.push_str(rename(ident));
        }
        prev_ident = ident.to_string();
    }
    out
}

fn rename(ident: &str) -> &str {
    if ident == "sampler" {
        "rx_sampler"
    } else {
        ident
    }
}

// ------------------------------------------------------------------ assembly

#[derive(Clone, Copy)]
enum Origin {
    Header,
    Host,
    Library(&'static str),
    User,
    Generated,
}

#[derive(Default)]
struct Source {
    text: String,
    /// (first line of the segment in `text`, origin), 0-based lines.
    segments: Vec<(usize, Origin)>,
    lines: usize,
}

impl Source {
    fn push(&mut self, origin: Origin, text: &str) {
        self.segments.push((self.lines, origin));
        self.text.push_str(text);
        if !text.ends_with('\n') {
            self.text.push('\n');
        }
        self.lines += text.lines().count().max(1);
    }

    fn locate(&self, offset: usize) -> Location {
        let offset = offset.min(self.text.len());
        let line = self.text.as_bytes()[..offset]
            .iter()
            .filter(|&&c| c == b'\n')
            .count();
        let Some(&(start, origin)) = self.segments.iter().rev().find(|(s, _)| *s <= line) else {
            return Location::Unknown;
        };
        let local = line - start + 1;
        match origin {
            Origin::Host => Location::HostHeader { line: local },
            Origin::Library(module) => Location::Library {
                module,
                line: local,
            },
            Origin::User => Location::User { line: local },
            Origin::Header | Origin::Generated => Location::Generated,
        }
    }
}

/// Translates a Painter shader. `host_header` is ReEtched's GLSL host header
/// (varyings, bind groups 0-1 and the `rx_*` functions, see HOST.md); it must
/// not contain `#version`.
pub fn translate(source: &str, host_header: &str) -> Result<Shader, Vec<Diagnostic>> {
    let mut ctx = Ctx::default();

    // The user's file first: it decides which libraries are needed and its
    // uniform declarations win over identical library ones.
    let user = process(source, Unit::User, &mut ctx);
    if !source.contains("shade") {
        ctx.error(
            Location::Unknown,
            "the shader has no `void shade(V2F inputs)` entry point",
        );
    }

    let mut included: Vec<&'static str> = vec!["lib-sparse"];
    let mut stack = ctx.imports.clone();
    while let Some(name) = stack.pop() {
        if !included.contains(&name) {
            included.push(name);
            let m = LIBRARY
                .iter()
                .find(|m| m.name == name)
                .expect("known library");
            stack.extend(m.deps.iter().copied());
        }
    }
    let mut libs = Vec::new();
    for m in LIBRARY.iter().filter(|m| included.contains(&m.name)) {
        libs.push((m.name, process(m.source, Unit::Library(m.name), &mut ctx)));
    }

    // Library warnings are about our own code, not something the user can fix.
    ctx.diags.retain(|d| {
        d.severity == Severity::Error || !matches!(d.location, Location::Library { .. })
    });
    if ctx.diags.iter().any(|d| d.severity == Severity::Error) {
        return Err(ctx.diags);
    }

    // Lay out the parameter block and texture bindings.
    let mut params = Vec::new();
    let mut textures = Vec::new();
    let mut offset = 0u32;
    for d in &mut ctx.decls {
        match &mut d.kind {
            DeclKind::Custom(p) => {
                let (align, size) = p.ty.std140();
                offset = offset.next_multiple_of(align);
                p.offset = offset;
                offset += size;
                params.push(p.clone());
            }
            DeclKind::Texture(t) => {
                t.binding = FIRST_TEXTURE_BINDING + textures.len() as u32;
                textures.push(t.clone());
            }
            DeclKind::Auto { .. } => {}
        }
    }
    let uniform_size = offset.next_multiple_of(16);
    if textures.len() as u32 > MAX_TEXTURES {
        let at = ctx
            .decls
            .iter()
            .filter(|d| matches!(d.kind, DeclKind::Texture(_)))
            .nth(MAX_TEXTURES as usize)
            .map(|d| d.unit.at(d.line));
        return Err(vec![Diagnostic {
            severity: Severity::Error,
            location: at.unwrap_or(Location::Unknown),
            message: format!("too many texture parameters (at most {MAX_TEXTURES})"),
        }]);
    }

    let mut prelude = String::from("// Generated: Painter shader parameters.\n");
    let mut init = String::new();
    if !params.is_empty() {
        let _ = writeln!(
            prelude,
            "layout(set = {USER_BIND_GROUP}, binding = {PARAMS_BINDING}, std140) uniform RxParams {{"
        );
        for p in &params {
            let _ = writeln!(prelude, "  {} {};", p.ty.block_glsl(), p.name);
        }
        prelude.push_str("} rx_params;\n");
    }
    for t in &textures {
        let _ = writeln!(
            prelude,
            "layout(set = {USER_BIND_GROUP}, binding = {}) uniform texture2D rx_tex_{};",
            t.binding, t.name
        );
    }
    if !textures.is_empty() {
        let _ = writeln!(
            prelude,
            "layout(set = {USER_BIND_GROUP}, binding = {USER_SAMPLER_BINDING}) uniform sampler rx_user_sampler;"
        );
    }
    for d in &ctx.decls {
        match &d.kind {
            DeclKind::Texture(_) => {
                let _ = writeln!(
                    prelude,
                    "#define {0} sampler2D(rx_tex_{0}, rx_user_sampler)",
                    d.name
                );
            }
            DeclKind::Custom(p) => {
                let _ = writeln!(prelude, "{} {};", d.glsl_type, d.name);
                if p.ty == ParamType::Bool {
                    let _ = writeln!(init, "  {0} = rx_params.{0} != 0;", d.name);
                } else {
                    let _ = writeln!(init, "  {0} = rx_params.{0};", d.name);
                }
            }
            DeclKind::Auto { init: expr } => {
                let _ = writeln!(prelude, "{} {};", d.glsl_type, d.name);
                let _ = writeln!(init, "  {} = {};", d.name, expr);
            }
        }
    }
    let main = format!(
        "// Generated entry point.\nvoid main() {{\n{init}  rx_reset_outputs();\n  shade(rx_build_inputs());\n  rx_emit_outputs();\n}}\n"
    );

    let mut src = Source::default();
    src.push(Origin::Header, "#version 450\n");
    src.push(Origin::Host, host_header);
    let (core_name, core) = &libs[0];
    src.push(Origin::Library(core_name), core);
    src.push(Origin::Generated, &prelude);
    for (name, text) in &libs[1..] {
        src.push(Origin::Library(name), text);
    }
    src.push(Origin::User, &user);
    src.push(Origin::Generated, &main);

    let module = match naga::front::glsl::Frontend::default().parse(
        &naga::front::glsl::Options::from(naga::ShaderStage::Fragment),
        &src.text,
    ) {
        Ok(m) => m,
        Err(e) => {
            let mut diags: Vec<Diagnostic> = e
                .errors
                .iter()
                .map(|err| Diagnostic {
                    severity: Severity::Error,
                    location: err
                        .meta
                        .to_range()
                        .map_or(Location::Unknown, |r| src.locate(r.start)),
                    message: err.kind.to_string(),
                })
                .collect();
            diags.extend(ctx.diags);
            return Err(diags);
        }
    };

    let info = match naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    {
        Ok(info) => info,
        Err(e) => {
            let mut message = e.as_inner().to_string();
            let mut cause: Option<&dyn std::error::Error> = std::error::Error::source(e.as_inner());
            while let Some(c) = cause {
                let _ = write!(message, ": {c}");
                cause = c.source();
            }
            // Point at the innermost span the validator gave.
            let location = e
                .spans()
                .filter_map(|(span, _)| span.to_range())
                .last()
                .map_or(Location::Unknown, |r| src.locate(r.start));
            let mut diags = vec![Diagnostic {
                severity: Severity::Error,
                location,
                message,
            }];
            diags.extend(ctx.diags);
            return Err(diags);
        }
    };

    Ok(Shader {
        module,
        info,
        entry_point: "main",
        params,
        uniform_size,
        textures,
        channels: ctx.channels,
        auto_params: dedup(ctx.auto_params),
        states: ctx.states,
        disable_srgb_conversion: ctx.disable_srgb,
        warnings: ctx.diags,
        glsl: src.text,
    })
}

fn dedup(v: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|s| seen.insert(s.clone())).collect()
}
