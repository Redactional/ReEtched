//! The 3D viewport and the 2D UV view: image-based lighting with the texture
//! sets' shader models, rendered into offscreen textures that egui displays.

use crate::{
    compositor::{Compositor, GroupSets, MeshPart, PaintKey, VERTEX_ATTRIBUTES},
    env::EnvImage,
    paint_gpu::{BrushShape, Camera, DAB_UV_SPACE, Dab, PaintGpu, StrokeTarget},
};
use glam::{Mat4, Vec2, Vec3};
use reetched_doc::{
    Channel, MeshMap, ResourceId, Resources, ShaderModel, TextureSet, TileMap, export::ChannelData,
};
use reetched_io::{Mesh, Vertex};
use std::{collections::BTreeSet, ops::Range};
use wgpu::util::DeviceExt;

const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const REVEAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
/// egui displays these as-is, so shaders write sRGB-encoded values into a plain UNORM target.
const DISPLAY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Prefiltered env mips; level i holds roughness i / (ENV_LEVELS - 1).
const ENV_LEVELS: u32 = 6;
const PREFILTER_SAMPLES: u32 = 128;
/// Viewport background (#262626), display-encoded. BACKGROUND in viewport.wgsl is the linear twin.
const BACKGROUND: f64 = 0x26 as f64 / 255.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    params: [f32; 4],
    view: [f32; 4],
    uv_xform: [f32; 4],
    sh: [[f32; 4]; 9],
    /// x: tone map, y: bloom strength, z: vignette, w: emissive intensity
    post: [f32; 4],
    /// x: alpha mode, y: alpha cutoff, z: parallax depth, w: frame number
    extra: [f32; 4],
    /// x: bloom threshold, y: accumulation weight of this frame
    accum: [f32; 4],
    /// x: scattering radius, y: pixels per world unit at distance 1, z: on
    sss: [f32; 4],
    /// Display LUT shaper: log2 min, log2 max, size
    lut: [f32; 4],
    /// Camera forward (world), vertical field of view (0 = orthographic)
    camera: [f32; 4],
    /// Width, height, 1/width, 1/height
    screen: [f32; 4],
    /// Time (s), scene radius, perspective (for custom shaders)
    misc: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MaterialUniform {
    tile: [f32; 4],
    model: [f32; 4],
    /// Channels present, by Painter shader id: x ids 0-31, y ids 32-63.
    present: [u32; 4],
}

/// A compiled Painter shader.
pub struct CustomShader {
    pub shader: reetched_shader::Shader,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

/// The host header Painter shaders compile against.
const PAINTER_HOST: &str = include_str!("shaders/painter_host.glsl");

/// How the 3D view maps HDR light to the screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToneMap {
    Aces,
    Agx,
    Neutral,
    Linear,
    /// The loaded OCIO config's display/view, baked into a LUT.
    Ocio,
}

impl ToneMap {
    pub const ALL: [ToneMap; 5] = [
        ToneMap::Aces,
        ToneMap::Agx,
        ToneMap::Neutral,
        ToneMap::Linear,
        ToneMap::Ocio,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ToneMap::Aces => "ACES (filmic)",
            ToneMap::Agx => "AgX",
            ToneMap::Neutral => "Khronos PBR Neutral",
            ToneMap::Linear => "Linear (clamped)",
            ToneMap::Ocio => "OCIO display / view",
        }
    }
}

/// How the opacity channel shows in the 3D view.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlphaMode {
    Opaque,
    /// Alpha test against a cutoff.
    Cutout,
    /// Stochastic transparency, smoothed by progressive accumulation.
    Dithered,
    /// Order-independent transparency (weighted blended).
    Blend,
}

impl AlphaMode {
    pub const ALL: [AlphaMode; 4] = [
        AlphaMode::Opaque,
        AlphaMode::Cutout,
        AlphaMode::Dithered,
        AlphaMode::Blend,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AlphaMode::Opaque => "Opaque",
            AlphaMode::Cutout => "Cutout (alpha test)",
            AlphaMode::Dithered => "Dithered",
            AlphaMode::Blend => "Blend (order independent)",
        }
    }
}

/// 3D view rendering options (Display settings).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RenderSettings {
    pub tone_map: ToneMap,
    pub bloom: f32,
    /// Brightness (after exposure) where bloom starts.
    pub bloom_threshold: f32,
    pub vignette: f32,
    pub emissive_intensity: f32,
    pub alpha: AlphaMode,
    pub alpha_cutoff: f32,
    /// Parallax displacement depth from the height channel (0 = off).
    pub parallax: f32,
    /// Progressive anti-aliasing: jitter and average frames while nothing changes.
    pub accumulate: bool,
    /// Screen-space subsurface scattering.
    pub sss: bool,
    /// How far light scatters under the surface, in world units.
    pub sss_radius: f32,
    /// Depth of field (needs progressive accumulation).
    pub dof: bool,
    /// Lens radius in world units.
    pub aperture: f32,
    /// Focus distance; 0 focuses on the orbit target.
    pub focus: f32,
    /// Render the material view with the GPU path tracer.
    pub path_trace: bool,
    pub bounces: u32,
    /// Samples per pixel the path tracer stops at.
    pub trace_samples: u32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            tone_map: ToneMap::Aces,
            bloom: 0.0,
            bloom_threshold: 1.0,
            vignette: 0.0,
            emissive_intensity: 1.0,
            alpha: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            parallax: 0.0,
            accumulate: true,
            sss: true,
            sss_radius: 0.02,
            dof: false,
            aperture: 0.02,
            focus: 0.0,
            path_trace: false,
            bounces: reetched_trace::FrameInputs::DEFAULT_BOUNCES,
            trace_samples: 512,
        }
    }
}

/// Frames accumulated before the view stops redrawing.
const ACCUMULATE_FRAMES: u32 = 32;
/// With depth of field: more frames, so the lens is sampled well.
const DOF_FRAMES: u32 = 96;

/// Everything that, when it changes, restarts accumulation.
#[derive(Clone, PartialEq)]
struct FrameKey {
    view: Mat4,
    proj: Mat4,
    globals: [f32; 12],
    render: RenderSettings,
    wire: bool,
    scene: u64,
    size: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PrefilterParams {
    roughness: f32,
    src_texel_solid_angle: f32,
    samples: u32,
    _pad: u32,
}

/// What the views display. Keep in sync with the MODE_* constants in viewport.wgsl.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    Material,
    BaseColor,
    Roughness,
    Metallic,
    Normal,
    Uv,
    Emissive,
    Opacity,
    /// A baked mesh map, shown as-is.
    Mesh(MeshMap),
    /// One of the extra channels.
    Channel(Channel),
}

impl ViewMode {
    pub const CHANNELS: [ViewMode; 8] = [
        ViewMode::Material,
        ViewMode::BaseColor,
        ViewMode::Roughness,
        ViewMode::Metallic,
        ViewMode::Normal,
        ViewMode::Emissive,
        ViewMode::Opacity,
        ViewMode::Uv,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Material => "Material",
            ViewMode::BaseColor => "Base color",
            ViewMode::Roughness => "Roughness",
            ViewMode::Metallic => "Metallic",
            ViewMode::Normal => "Normal (mesh)",
            ViewMode::Uv => "UV coordinates",
            ViewMode::Emissive => "Emissive",
            ViewMode::Opacity => "Opacity",
            ViewMode::Mesh(m) => m.label(),
            ViewMode::Channel(c) => c.label(),
        }
    }

    /// (mode, solo channel index) for the shader.
    fn shader_index(self) -> (f32, f32) {
        let mode = match self {
            ViewMode::Material => 0.0,
            ViewMode::BaseColor => 1.0,
            ViewMode::Roughness => 2.0,
            ViewMode::Metallic => 3.0,
            ViewMode::Normal => 4.0,
            ViewMode::Uv => 5.0,
            ViewMode::Mesh(_) => 6.0,
            ViewMode::Emissive => 7.0,
            ViewMode::Opacity => 8.0,
            ViewMode::Channel(c) => return (9.0, c.index() as f32),
        };
        (mode, 0.0)
    }
}

/// One UDIM tile of a texture set.
pub struct Tile {
    pub number: u32,
    indices: Range<u32>,
    /// World-space bounds of the tile's triangles (to skip tiles a brush can't reach).
    bounds: (Vec3, Vec3),
    sets: GroupSets,
    bind_group: Option<wgpu::BindGroup>,
    material: wgpu::Buffer,
    /// (AO map, previewed mesh map, shader model) the bind group was built with.
    bound: (Option<ResourceId>, Option<ResourceId>, u32),
    /// A custom shader's parameters and textures (bind group 2), with its shader.
    custom: Option<(ResourceId, wgpu::BindGroup)>,
}

pub struct Part {
    pub name: String,
    /// Texture set visibility in the 3D view.
    pub visible: bool,
    pub triangles: u32,
    pub tiles: Vec<Tile>,
    /// The texture set as last composited, to detect changes.
    composited: Option<TextureSet>,
    /// Tiles whose paint changed under the same document (strokes in progress).
    dirty: BTreeSet<u32>,
}

impl Part {
    pub fn tile_numbers(&self) -> Vec<u32> {
        self.tiles.iter().map(|t| t.number).collect()
    }
}

pub struct FrameParams {
    pub view: Mat4,
    pub proj: Mat4,
    pub eye: Vec3,
    /// Distance from the eye to the orbit target (the automatic focus distance).
    pub target_distance: f32,
    /// Vertical field of view (radians); 0 for orthographic.
    pub fov_y: f32,
    pub exposure_ev: f32,
    pub env_rotation: f32,
    /// 0 = sharp background, 1 = fully blurred.
    pub background_blur: f32,
    /// How much of the environment shows behind the model (0 = flat background).
    pub env_opacity: f32,
    pub show_wireframe: bool,
    /// UV island edges in the 2D view.
    pub show_uv_wireframe: bool,
    pub view_mode: ViewMode,
    pub uv_mode: ViewMode,
    /// 2D view transform: ndc = uv * xy + zw.
    pub uv_xform: [f32; 4],
    /// Texture set shown in the 2D view.
    pub uv_part: Option<usize>,
    pub render: RenderSettings,
}

/// An offscreen texture egui displays, resized to whatever the UI asks for.
pub struct ViewTarget {
    pub texture_id: egui::TextureId,
    /// Size in physical pixels the UI wants; applied at the next render. Zero = hidden.
    pub requested_size: [u32; 2],
    size: [u32; 2],
}

impl ViewTarget {
    fn needs_resize(&self) -> bool {
        self.visible() && self.size != self.requested_size
    }

    fn visible(&self) -> bool {
        self.requested_size[0] > 0 && self.requested_size[1] > 0
    }
}

struct Targets {
    size: [u32; 2],
    hdr: wgpu::TextureView,
    /// Light that scatters under the surface (this frame), and its blurred versions.
    sss: [wgpu::TextureView; 3],
    /// Transparency: premultiplied color sum and revealage.
    oit_accum: wgpu::TextureView,
    oit_reveal: wgpu::TextureView,
    depth: wgpu::TextureView,
    display: wgpu::TextureView,
    /// Accumulated frames, ping-ponged.
    history: [wgpu::TextureView; 2],
    /// Quarter-resolution bloom buffers.
    bloom: [wgpu::TextureView; 2],
    oit_bg: wgpu::BindGroup,
    sss_h_bg: wgpu::BindGroup,
    sss_v_bg: wgpu::BindGroup,
    /// Writes history[i] from the new frame, history[1 - i] and the scattering.
    accum_bg: [wgpu::BindGroup; 2],
    /// Bright pass from history[i] into bloom[0].
    bloom_down_bg: [wgpu::BindGroup; 2],
    blur_h_bg: wgpu::BindGroup,
    blur_v_bg: wgpu::BindGroup,
    /// history[i] + bloom → display.
    tonemap_bg: [wgpu::BindGroup; 2],
}

struct Pipelines {
    sky: wgpu::RenderPipeline,
    pbr: wgpu::RenderPipeline,
    oit: wgpu::RenderPipeline,
    oit_resolve: wgpu::RenderPipeline,
    wire: Option<wgpu::RenderPipeline>,
    sss_h: wgpu::RenderPipeline,
    sss_v: wgpu::RenderPipeline,
    accumulate: wgpu::RenderPipeline,
    bloom_down: wgpu::RenderPipeline,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    tonemap: wgpu::RenderPipeline,
    uv_fill: wgpu::RenderPipeline,
    uv_wire: Option<wgpu::RenderPipeline>,
}

/// The OCIO display transform as a 3D LUT.
pub struct DisplayLut {
    pub size: u32,
    /// log2 stops below / above middle grey that the shaper covers.
    pub log2_range: (f32, f32),
    /// Display-encoded RGB, red fastest.
    pub data: Vec<[f32; 3]>,
}

/// Material textures the path tracer samples are this size (any tile resolution).
// ponytail: fixed 1024² (32 MB per tile); follow the tile resolution if detail matters.
const TRACE_MATERIAL_SIZE: u32 = 1024;

/// The path tracer, created the first time it is turned on.
struct Trace {
    tracer: reetched_trace::PathTracer,
    materials: reetched_trace::Materials,
    /// The current mesh is uploaded (cleared by `set_mesh`).
    has_mesh: bool,
    /// `scene_version` the material layers were copied at.
    scene: Option<u64>,
}

pub struct Viewport {
    pub view3d: ViewTarget,
    pub view2d: ViewTarget,
    pub parts: Vec<Part>,
    pub wireframe_supported: bool,
    targets: Targets,
    uv_display: wgpu::TextureView,
    pipelines: Pipelines,
    globals: wgpu::Buffer,
    globals_layout: wgpu::BindGroupLayout,
    globals_bind_group: wgpu::BindGroup,
    material_layout: wgpu::BindGroupLayout,
    post_layout: wgpu::BindGroupLayout,
    post_sampler: wgpu::Sampler,
    /// Frames accumulated so far, and which history buffer the next frame writes.
    accum: u32,
    history_cur: usize,
    last_key: Option<FrameKey>,
    /// Bumped whenever a texture set is re-composited.
    scene_version: u64,
    env_view: wgpu::TextureView,
    env_sampler: wgpu::Sampler,
    env_sh: [[f32; 4]; 9],
    prefilter: wgpu::ComputePipeline,
    /// The display LUT and its shaper (log2 min, max, size).
    lut: (wgpu::TextureView, [f32; 3]),
    /// The viewport's WGSL module (custom shaders reuse its vertex stage).
    module: wgpu::ShaderModule,
    /// Painter shaders by resource, compiled (or why they didn't).
    custom: std::collections::HashMap<ResourceId, Result<CustomShader, String>>,
    started: std::time::Instant,
    mesh: Option<(wgpu::Buffer, wgpu::Buffer)>,
    /// Mesh bounding-box diagonal.
    mesh_scale: f32,
    white: wgpu::TextureView,
    compositor: Compositor,
    paint: PaintGpu,
    texture_sampler: wgpu::Sampler,
    trace: Option<Trace>,
}

impl Viewport {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
        wireframe_supported: bool,
        env: EnvImage,
    ) -> Self {
        use wgpu::ShaderStages as S;
        let filtering = wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering);
        let globals_layout = bind_group_layout(
            device,
            &[
                (S::VERTEX_FRAGMENT, uniform_binding()),
                (S::FRAGMENT, texture_binding(true)),
                (S::FRAGMENT, filtering),
                (
                    S::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                ),
            ],
        );
        // Composited tile: core t0..t2, sampler, baked AO, previewed mesh map, core t3,
        // groups 1 and 2 t0..t2, material uniform.
        let mut material = vec![
            (S::FRAGMENT, texture_binding(true)),
            (S::FRAGMENT, texture_binding(true)),
            (S::FRAGMENT, texture_binding(true)),
            (S::FRAGMENT, filtering),
        ];
        material.extend([(S::FRAGMENT, texture_binding(true)); 9]);
        material.push((S::FRAGMENT, uniform_binding()));
        let material_layout = bind_group_layout(device, &material);
        // Post passes: two inputs, a sampler, a third input, the scene depth.
        let post_layout = bind_group_layout(
            device,
            &[
                (S::FRAGMENT, texture_binding(true)),
                (S::FRAGMENT, texture_binding(true)),
                (S::FRAGMENT, filtering),
                (S::FRAGMENT, texture_binding(true)),
                (
                    S::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
            ],
        );
        let post_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let module = device.create_shader_module(wgpu::include_wgsl!("shaders/viewport.wgsl"));
        let scene_layout = pipeline_layout(device, &[&globals_layout, &material_layout]);
        let globals_only = pipeline_layout(device, &[&globals_layout]);
        let post_pipeline_layout = pipeline_layout(device, &[&globals_layout, &post_layout]);
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &VERTEX_ATTRIBUTES,
        };
        let mesh_buffers = [Some(vertex_layout)];
        let target = |format| Some(wgpu::ColorTargetState::from(format));
        // The scene pass writes color and the scattering buffer.
        let scene = [target(HDR_FORMAT), target(HDR_FORMAT)];
        let hdr = [target(HDR_FORMAT)];
        let display = [target(DISPLAY_FORMAT)];
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let reveal = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::OneMinusSrc,
            operation: wgpu::BlendOperation::Add,
        };
        let oit_targets = [
            Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: Some(wgpu::BlendState {
                    color: add,
                    alpha: add,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            }),
            Some(wgpu::ColorTargetState {
                format: REVEAL_FORMAT,
                blend: Some(wgpu::BlendState {
                    color: reveal,
                    alpha: reveal,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            }),
        ];
        let over = [Some(wgpu::ColorTargetState {
            format: HDR_FORMAT,
            blend: Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let depth = |write, compare| Some((write, compare));
        let base = PipelineDesc {
            module: &module,
            buffers: &[],
            polygon_mode: wgpu::PolygonMode::Fill,
            depth: None,
            targets: &hdr,
        };
        let mesh = PipelineDesc {
            buffers: &mesh_buffers,
            targets: &scene,
            ..base
        };
        let lines = PipelineDesc {
            polygon_mode: wgpu::PolygonMode::Line,
            ..mesh
        };
        let pipelines = Pipelines {
            sky: PipelineDesc {
                targets: &scene,
                ..base
            }
            .with(
                "vs_fullscreen",
                "fs_sky",
                &globals_only,
                depth(false, wgpu::CompareFunction::Always),
            )
            .build(device),
            pbr: mesh
                .with(
                    "vs_mesh",
                    "fs_pbr",
                    &scene_layout,
                    depth(true, wgpu::CompareFunction::Less),
                )
                .build(device),
            oit: PipelineDesc {
                targets: &oit_targets,
                ..mesh
            }
            .with(
                "vs_mesh",
                "fs_oit",
                &scene_layout,
                depth(false, wgpu::CompareFunction::LessEqual),
            )
            .build(device),
            oit_resolve: PipelineDesc {
                targets: &over,
                ..base
            }
            .with(
                "vs_fullscreen",
                "fs_oit_resolve",
                &post_pipeline_layout,
                None,
            )
            .build(device),
            wire: wireframe_supported.then(|| {
                lines
                    .with(
                        "vs_wire",
                        "fs_wire",
                        &globals_only,
                        depth(false, wgpu::CompareFunction::LessEqual),
                    )
                    .build(device)
            }),
            sss_h: base
                .with("vs_fullscreen", "fs_sss_h", &post_pipeline_layout, None)
                .build(device),
            sss_v: base
                .with("vs_fullscreen", "fs_sss_v", &post_pipeline_layout, None)
                .build(device),
            accumulate: base
                .with(
                    "vs_fullscreen",
                    "fs_accumulate",
                    &post_pipeline_layout,
                    None,
                )
                .build(device),
            bloom_down: base
                .with(
                    "vs_fullscreen",
                    "fs_bloom_down",
                    &post_pipeline_layout,
                    None,
                )
                .build(device),
            blur_h: base
                .with("vs_fullscreen", "fs_blur_h", &post_pipeline_layout, None)
                .build(device),
            blur_v: base
                .with("vs_fullscreen", "fs_blur_v", &post_pipeline_layout, None)
                .build(device),
            tonemap: PipelineDesc {
                targets: &display,
                ..base
            }
            .with("vs_fullscreen", "fs_tonemap", &post_pipeline_layout, None)
            .build(device),
            uv_fill: PipelineDesc {
                targets: &display,
                ..mesh
            }
            .with("vs_uv", "fs_uv", &scene_layout, None)
            .build(device),
            uv_wire: wireframe_supported.then(|| {
                PipelineDesc {
                    targets: &display,
                    ..lines
                }
                .with("vs_uv_wire", "fs_uv_wire", &globals_only, None)
                .build(device)
            }),
        };

        let prefilter = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("env prefilter"),
            layout: None,
            module: &device.create_shader_module(wgpu::include_wgsl!("shaders/prefilter.wgsl")),
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: crate::gpu_cache::get(),
        });
        let env_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("env"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let targets = Targets::new(device, &post_layout, &post_sampler, [64, 64]);
        let uv_display = display_texture(device, [64, 64]);
        let mut target = |view: &wgpu::TextureView| ViewTarget {
            texture_id: egui.register_native_texture(device, view, wgpu::FilterMode::Linear),
            requested_size: [64, 64],
            size: [64, 64],
        };
        let view3d = target(&targets.display);
        let view2d = target(&uv_display);
        let env_sh = env.diffuse_sh();
        let env_view = prefilter_env(device, queue, &prefilter, &env_sampler, env);
        let lut = (
            lut_texture(device, queue, 2, &[[0.0; 3]; 8]),
            [-8.0, 8.0, 2.0],
        );
        let globals_bind_group = globals_bind_group(
            device,
            &globals_layout,
            &globals,
            &env_view,
            &env_sampler,
            &lut.0,
        );

        Self {
            view3d,
            view2d,
            parts: Vec::new(),
            wireframe_supported,
            targets,
            uv_display,
            pipelines,
            globals,
            globals_layout,
            globals_bind_group,
            material_layout,
            post_layout,
            post_sampler,
            accum: 0,
            history_cur: 0,
            last_key: None,
            scene_version: 0,
            env_view,
            env_sampler,
            env_sh,
            prefilter,
            lut,
            module,
            custom: Default::default(),
            started: std::time::Instant::now(),
            mesh: None,
            mesh_scale: 1.0,
            white: device
                .create_texture_with_data(
                    queue,
                    &wgpu::TextureDescriptor {
                        label: Some("white"),
                        size: wgpu::Extent3d {
                            width: 1,
                            height: 1,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &[255; 4],
                )
                .create_view(&Default::default()),
            compositor: Compositor::new(device, queue),
            paint: PaintGpu::new(device, queue),
            texture_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("channel textures"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                anisotropy_clamp: 1,
                ..Default::default()
            }),
            trace: None,
        }
    }

    pub fn set_mesh(&mut self, device: &wgpu::Device, mesh: &Mesh) {
        let buffer = |contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents,
                usage,
            })
        };
        self.mesh = Some((
            buffer(
                bytemuck::cast_slice(&mesh.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            buffer(
                bytemuck::cast_slice(&mesh.indices),
                wgpu::BufferUsages::INDEX,
            ),
        ));
        let (lo, hi) = mesh.bounds();
        self.mesh_scale = (hi - lo).length().max(1e-6);
        if let Some(t) = &mut self.trace {
            t.has_mesh = false;
        }
        let bounds = |r: &Range<u32>| {
            mesh.indices[r.start as usize..r.end as usize].iter().fold(
                (Vec3::MAX, Vec3::MIN),
                |(lo, hi), &i| {
                    let p = Vec3::from(mesh.vertices[i as usize].position);
                    (lo.min(p), hi.max(p))
                },
            )
        };
        self.parts = mesh
            .parts
            .iter()
            .enumerate()
            .map(|(k, p)| Part {
                name: p.material.clone(),
                visible: true,
                triangles: (p.indices.end - p.indices.start) / 3,
                tiles: mesh
                    .udim_tiles(k)
                    .into_iter()
                    .map(|(number, indices)| Tile {
                        number,
                        bounds: bounds(&indices),
                        indices,
                        sets: Default::default(),
                        bind_group: None,
                        material: device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("material"),
                            size: size_of::<MaterialUniform>() as u64,
                            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        }),
                        bound: (None, None, u32::MAX),
                        custom: None,
                    })
                    .collect(),
                composited: None,
                dirty: BTreeSet::new(),
            })
            .collect();
    }

    /// Starts a brush stroke on `targets` (each with its tiles' current paint).
    #[allow(clippy::too_many_arguments)]
    pub fn paint_begin(
        &mut self,
        targets: Vec<StrokeTarget>,
        set: &TextureSet,
        mode: crate::paint_gpu::StrokeMode,
        opacity: f32,
    ) {
        self.paint.begin(targets, set, mode, opacity);
    }

    /// Projection tool: the image (placed by `u`) this stroke paints, projected from
    /// the 3D view onto each tile as the stroke reaches it.
    pub fn paint_project(&mut self, image: wgpu::TextureView, u: crate::paint_gpu::ProjectUniform) {
        self.paint.set_projection(image, u);
    }

    /// An image resource as an sRGB-decoded texture.
    pub fn color_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &Resources,
        id: &ResourceId,
    ) -> Option<wgpu::TextureView> {
        self.compositor
            .bitmap_view_srgb(device, queue, resources, id)
    }

    /// Polygon Fill: fills triangles (three vertex indices each) of tile `tile` of
    /// texture set `part`.
    pub fn paint_fill(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        part: usize,
        tile: u32,
        indices: &[u32],
        resources: &Resources,
    ) {
        let (Some((vertices, ib)), Some(p)) = (&self.mesh, self.parts.get_mut(part)) else {
            return;
        };
        let Some(t) = p.tiles.iter().find(|t| t.number == tile) else {
            return;
        };
        let mesh = MeshPart {
            vertices,
            indices: ib,
            range: t.indices.clone(),
            scale: self.mesh_scale,
            tile,
        };
        let depth = (self.view3d.size == self.view3d.requested_size).then_some(&self.targets.depth);
        self.paint.fill_triangles(
            device,
            queue,
            &mut self.compositor,
            &mesh,
            indices,
            depth,
            resources,
        );
        p.dirty.insert(tile);
    }

    /// Stamps dabs on texture set `part`. `occlusion`: test against the 3D view's depth.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_dabs(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        part: usize,
        camera: &Camera,
        occlusion: bool,
        shape: &BrushShape,
        dabs: &[Dab],
        motion: [f32; 2],
        resources: &Resources,
    ) {
        let (Some((vertices, ib)), Some(p)) = (&self.mesh, self.parts.get_mut(part)) else {
            return;
        };
        let depth = (occlusion && self.view3d.size == self.view3d.requested_size)
            .then_some(&self.targets.depth);
        // Only the tiles some dab can reach.
        for t in p
            .tiles
            .iter()
            .filter(|t| dabs.iter().any(|d| dab_reaches(d, t)))
        {
            let mesh = MeshPart {
                vertices,
                indices: ib,
                range: t.indices.clone(),
                scale: self.mesh_scale,
                tile: t.number,
            };
            self.paint.dabs(
                device,
                queue,
                &mut self.compositor,
                &mesh,
                camera,
                depth,
                shape,
                dabs,
                motion,
                resources,
            );
            p.dirty.insert(t.number);
        }
    }

    /// Ends the stroke; returns each painted buffer's new tiles.
    pub fn paint_end(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &mut Resources,
    ) -> anyhow::Result<Vec<(PaintKey, TileMap)>> {
        self.paint
            .end(device, queue, &mut self.compositor, resources)
    }

    /// Keeps the compositor's caches within `budget` bytes (between frames).
    pub fn end_frame(&mut self, budget: u64) {
        let painting = self.paint.active();
        self.compositor.trim(budget, painting);
    }

    /// Brush alpha image as a GPU texture.
    pub fn brush_alpha(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &Resources,
        id: &ResourceId,
    ) -> Option<wgpu::TextureView> {
        self.compositor.bitmap_view(device, queue, resources, id)
    }

    pub fn compositor(&mut self) -> &mut crate::compositor::Compositor {
        &mut self.compositor
    }

    /// Re-composites every texture set on the next sync (e.g. a procedural
    /// material finished rendering).
    pub fn invalidate(&mut self) {
        for part in &mut self.parts {
            part.composited = None;
        }
    }

    /// Forgets GPU paint data (a different document was loaded).
    pub fn clear_paint_cache(&mut self) {
        self.compositor.clear_paint();
    }

    /// Re-composites texture set tiles whose layer stack (or paint) changed, and binds
    /// their baked AO and the previewed mesh map. Returns warnings.
    pub fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sets: &[TextureSet],
        resources: &Resources,
        preview: Option<MeshMap>,
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        let Some((vertices, indices)) = &self.mesh else {
            return warnings;
        };
        // Compile the Painter shaders texture sets use.
        for set in sets {
            if let ShaderModel::Custom { shader, .. } = &set.shader
                && !self.custom.contains_key(shader)
            {
                let compiled = compile_custom(
                    device,
                    &self.module,
                    &self.globals_layout,
                    &self.material_layout,
                    resources,
                    shader,
                );
                if let Err(e) = &compiled {
                    warnings.push(format!("Shader of {}: {e}", set.name));
                }
                self.custom.insert(shader.clone(), compiled);
            }
        }
        for (part, set) in self.parts.iter_mut().zip(sets) {
            let changed = part.composited.as_ref() != Some(set);
            let model = match set.shader {
                ShaderModel::MetalRough | ShaderModel::Custom { .. } => 0,
                ShaderModel::Asm => 1,
                ShaderModel::OpenPbr => 2,
            };
            // Channels a Painter shader sees: the set's, and the baked AO.
            let mut present = [0u32; 4];
            for c in set.channels() {
                present[0] |= 1 << c.index();
            }
            for tile in &mut part.tiles {
                let maps = set.mesh_maps_for(tile.number);
                let bound = (
                    maps.get(&MeshMap::AmbientOcclusion).cloned(),
                    preview.and_then(|m| maps.get(&m).cloned()),
                    model,
                );
                let recomposite = changed || part.dirty.contains(&tile.number);
                if changed {
                    tile.custom = match &set.shader {
                        ShaderModel::Custom {
                            shader,
                            params,
                            textures,
                            ..
                        } => match self.custom.get(shader) {
                            Some(Ok(c)) => Some((
                                shader.clone(),
                                custom_group(
                                    device,
                                    queue,
                                    c,
                                    params,
                                    textures,
                                    resources,
                                    &mut self.compositor,
                                    &self.white,
                                ),
                            )),
                            _ => None,
                        },
                        _ => None,
                    };
                }
                if !recomposite && tile.bound == bound && tile.bind_group.is_some() {
                    continue;
                }
                let mut present = present;
                if bound.0.is_some() {
                    present[1] |= 1; // id 32: baked AO
                }
                if recomposite {
                    let mesh = MeshPart {
                        vertices,
                        indices,
                        range: tile.indices.clone(),
                        scale: self.mesh_scale,
                        tile: tile.number,
                    };
                    warnings.extend(self.compositor.composite(
                        device,
                        queue,
                        &mesh,
                        set,
                        resources,
                        &mut tile.sets,
                    ));
                }
                let map_view = |id: &Option<ResourceId>, comp: &mut Compositor| {
                    id.as_ref()
                        .and_then(|id| comp.bitmap_view(device, queue, resources, id))
                };
                let ao =
                    map_view(&bound.0, &mut self.compositor).unwrap_or_else(|| self.white.clone());
                let shown =
                    map_view(&bound.1, &mut self.compositor).unwrap_or_else(|| self.white.clone());
                let [u, v] = reetched_doc::udim_origin(tile.number);
                queue.write_buffer(
                    &tile.material,
                    0,
                    bytemuck::bytes_of(&MaterialUniform {
                        tile: [u, v, 0.0, 0.0],
                        model: [model as f32, 0.0, 0.0, 0.0],
                        present,
                    }),
                );
                let comp = &self.compositor;
                let core = comp.group_views(&tile.sets, 0);
                let g1 = comp.group_views(&tile.sets, 1);
                let g2 = comp.group_views(&tile.sets, 2);
                let views = [
                    &core[0], &core[1], &core[2], &ao, &shown, &core[3], &g1[0], &g1[1], &g1[2],
                    &g2[0], &g2[1], &g2[2],
                ];
                let mut entries: Vec<wgpu::BindGroupEntry> = Vec::new();
                for (i, view) in views.iter().enumerate() {
                    // Binding 3 is the sampler.
                    let binding = if i < 3 { i } else { i + 1 } as u32;
                    entries.push(wgpu::BindGroupEntry {
                        binding,
                        resource: wgpu::BindingResource::TextureView(view),
                    });
                }
                entries.push(wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.texture_sampler),
                });
                entries.push(wgpu::BindGroupEntry {
                    binding: 13,
                    resource: tile.material.as_entire_binding(),
                });
                tile.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("texture set tile"),
                    layout: &self.material_layout,
                    entries: &entries,
                }));
                tile.bound = bound;
                self.scene_version += 1;
            }
            part.composited = Some(set.clone());
            part.dirty.clear();
        }
        warnings
    }

    /// Composites `set` on texture set `part`'s tile `tile` into a scratch target and
    /// reads it back (e.g. one layer alone, for PSD export).
    pub fn composite_temp(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        part: usize,
        tile: u32,
        set: &TextureSet,
        resources: &Resources,
    ) -> anyhow::Result<ChannelData> {
        let (Some((vertices, indices)), Some(t)) = (
            &self.mesh,
            self.parts
                .get(part)
                .and_then(|p| p.tiles.iter().find(|t| t.number == tile)),
        ) else {
            anyhow::bail!("texture set {part} has no tile {tile}");
        };
        let mesh = MeshPart {
            vertices,
            indices,
            range: t.indices.clone(),
            scale: self.mesh_scale,
            tile,
        };
        let mut out = GroupSets::default();
        let warnings = self
            .compositor
            .composite(device, queue, &mesh, set, resources, &mut out);
        if let Some(w) = warnings.first() {
            anyhow::bail!("{w}");
        }
        self.compositor.read_back(device, queue, &out)
    }

    /// Reads tile `tile` of texture set `part` back from the GPU (for export).
    pub fn read_back(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        part: usize,
        tile: u32,
    ) -> anyhow::Result<ChannelData> {
        let t = self
            .parts
            .get(part)
            .and_then(|p| p.tiles.iter().find(|t| t.number == tile))
            .ok_or_else(|| anyhow::anyhow!("texture set {part} has no tile {tile}"))?;
        self.compositor.read_back(device, queue, &t.sets)
    }

    /// UDIM tiles of texture set `part`.
    pub fn tiles(&self, part: usize) -> Vec<u32> {
        self.parts
            .get(part)
            .map_or_else(Vec::new, Part::tile_numbers)
    }

    /// The path tracer needs the mesh (first use, or the mesh changed).
    pub fn trace_needs_mesh(&self) -> bool {
        self.trace.as_ref().is_none_or(|t| !t.has_mesh)
    }

    /// Builds the path tracer's BVH for `mesh` (the one last passed to `set_mesh`).
    pub fn set_trace_mesh(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &Mesh,
    ) -> anyhow::Result<()> {
        // One material layer per (texture set, UDIM tile), in `parts` order.
        let mut layer_of_triangle = vec![0u32; mesh.indices.len() / 3];
        let tiles = self.parts.iter().flat_map(|p| &p.tiles);
        let mut layers = 0;
        for (k, tile) in tiles.enumerate() {
            let tris = tile.indices.start as usize / 3..tile.indices.end as usize / 3;
            layer_of_triangle[tris].fill(k as u32);
            layers = k as u32 + 1;
        }
        let layers = layers.max(1);
        let trace = self.trace.get_or_insert_with(|| Trace {
            tracer: reetched_trace::PathTracer::new(device),
            materials: reetched_trace::Materials::new(device, queue, TRACE_MATERIAL_SIZE, layers),
            has_mesh: false,
            scene: None,
        });
        if trace.materials.layers() < layers {
            trace.materials =
                reetched_trace::Materials::new(device, queue, TRACE_MATERIAL_SIZE, layers);
        }
        let took = trace.tracer.set_mesh(device, mesh, &layer_of_triangle)?;
        log::info!("path tracer BVH built in {took:?}");
        trace.has_mesh = true;
        trace.scene = None;
        self.last_key = None;
        Ok(())
    }

    /// Samples per pixel the path tracer has so far (0 when it isn't running).
    pub fn trace_samples(&self) -> u32 {
        self.trace.as_ref().map_or(0, |t| t.tracer.samples())
    }

    pub fn set_environment(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, env: EnvImage) {
        self.env_sh = env.diffuse_sh();
        self.env_view = prefilter_env(device, queue, &self.prefilter, &self.env_sampler, env);
        self.rebind_globals(device);
    }

    /// Sets the OCIO display LUT (used by `ToneMap::Ocio`).
    pub fn set_display_lut(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        lut: &DisplayLut,
    ) {
        let view = lut_texture(device, queue, lut.size, &lut.data);
        self.lut = (view, [lut.log2_range.0, lut.log2_range.1, lut.size as f32]);
        self.rebind_globals(device);
        self.last_key = None;
    }

    fn rebind_globals(&mut self, device: &wgpu::Device) {
        self.globals_bind_group = globals_bind_group(
            device,
            &self.globals_layout,
            &self.globals,
            &self.env_view,
            &self.env_sampler,
            &self.lut.0,
        );
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
        encoder: &mut wgpu::CommandEncoder,
        p: &FrameParams,
    ) {
        if self.view3d.needs_resize() {
            self.targets = Targets::new(
                device,
                &self.post_layout,
                &self.post_sampler,
                self.view3d.requested_size,
            );
            egui.update_egui_texture_from_wgpu_texture(
                device,
                &self.targets.display,
                wgpu::FilterMode::Linear,
                self.view3d.texture_id,
            );
            self.view3d.size = self.view3d.requested_size;
        }
        if self.view2d.needs_resize() {
            self.uv_display = display_texture(device, self.view2d.requested_size);
            egui.update_egui_texture_from_wgpu_texture(
                device,
                &self.uv_display,
                wgpu::FilterMode::Linear,
                self.view2d.texture_id,
            );
            self.view2d.size = self.view2d.requested_size;
        }

        let max_lod = (ENV_LEVELS - 1) as f32;
        let params = [
            p.exposure_ev.exp2(),
            p.env_rotation,
            max_lod,
            p.background_blur * max_lod,
        ];
        let (mode3d, solo3d) = p.view_mode.shader_index();
        let (mode2d, solo2d) = p.uv_mode.shader_index();
        // One solo channel slot: the 3D view's when it shows one, else the 2D view's.
        let solo = if mode3d == 9.0 { solo3d } else { solo2d };
        let view = [mode3d, p.env_opacity, mode2d, solo];
        // Restart accumulation when anything that shows in the 3D view changes.
        let key = FrameKey {
            view: p.view,
            proj: p.proj,
            globals: [
                params[0], params[1], params[3], view[0], view[1], view[3], p.eye.x, p.eye.y,
                p.eye.z, 0.0, 0.0, 0.0,
            ],
            render: p.render,
            wire: p.show_wireframe,
            scene: self.scene_version,
            size: self.targets.size,
        };
        if self.last_key.as_ref() != Some(&key) {
            self.accum = 0;
            self.last_key = Some(key);
        }
        let tracing = p.render.path_trace
            && p.view_mode == ViewMode::Material
            && self.trace.as_ref().is_some_and(|t| t.has_mesh);
        let accumulate = p.render.accumulate && p.view_mode == ViewMode::Material;
        let n = if accumulate || tracing { self.accum } else { 0 };
        // Sub-pixel jitter (Halton 2, 3), none on the first frame so a still image is sharp.
        let jitter = if n == 0 {
            Vec2::ZERO
        } else {
            Vec2::new(halton(n, 2), halton(n, 3)) - 0.5
        };
        let [w, h] = self.targets.size.map(|s| s.max(1) as f32);
        // Depth of field: each frame looks through another point of the lens (Halton 5, 7
        // on a disk), sheared so the focus plane stays put.
        let r = &p.render;
        let mut view_m = p.view;
        if r.dof && accumulate && n > 0 {
            let f = if r.focus > 0.0 {
                r.focus
            } else {
                p.target_distance
            }
            .max(1e-4);
            let (a, b) = (halton(n, 5), halton(n, 7));
            let (s, c) = (a * std::f32::consts::TAU).sin_cos();
            let o = Vec2::new(c, s) * b.sqrt() * r.aperture;
            let shear = Mat4::from_cols(
                glam::Vec4::X,
                glam::Vec4::Y,
                glam::Vec4::new(-o.x / f, -o.y / f, 1.0, 0.0),
                glam::Vec4::new(-o.x, -o.y, 0.0, 1.0),
            );
            view_m = shear * p.view;
        }
        let view_proj =
            Mat4::from_translation(Vec3::new(jitter.x * 2.0 / w, jitter.y * 2.0 / h, 0.0))
                * p.proj
                * view_m;
        // Scattering radius → pixels: the projection's scale at distance 1.
        let px_per_unit = if p.fov_y > 0.0 {
            h * 0.5 / (p.fov_y * 0.5).tan()
        } else {
            h * 0.5 * p.proj.y_axis.y
        };
        let globals = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            eye: p.eye.extend(1.0).into(),
            params,
            view,
            uv_xform: p.uv_xform,
            sh: self.env_sh,
            post: [
                r.tone_map as u32 as f32,
                r.bloom,
                r.vignette,
                r.emissive_intensity,
            ],
            extra: [
                r.alpha as u32 as f32,
                r.alpha_cutoff,
                r.parallax,
                (n % 64) as f32,
            ],
            accum: [r.bloom_threshold, 1.0 / (n + 1) as f32, 0.0, 0.0],
            sss: [
                r.sss_radius,
                px_per_unit,
                if r.sss { 1.0 } else { 0.0 },
                0.0,
            ],
            lut: [self.lut.1[0], self.lut.1[1], self.lut.1[2], 0.0],
            camera: [
                -p.view.x_axis.z,
                -p.view.y_axis.z,
                -p.view.z_axis.z,
                p.fov_y,
            ],
            screen: [w, h, 1.0 / w, 1.0 / h],
            misc: [
                self.started.elapsed().as_secs_f32(),
                self.mesh_scale * 0.5,
                if p.fov_y > 0.0 { 1.0 } else { 0.0 },
                0.0,
            ],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let traced = if tracing && self.view3d.visible() {
            let trace = self.trace.as_mut().expect("tracing");
            if trace.scene != Some(self.scene_version) {
                let tiles = self.parts.iter().flat_map(|p| &p.tiles);
                for (k, tile) in tiles.enumerate() {
                    if let Some(set) = &tile.sets[0] {
                        let v = &set.views;
                        trace.materials.update_layer(
                            device,
                            encoder,
                            k as u32,
                            [&v[0], &v[1], &v[2], &v[3]],
                        );
                    }
                }
                trace.scene = Some(self.scene_version);
            }
            if n < r.trace_samples {
                let hidden: Vec<bool> = self
                    .parts
                    .iter()
                    .flat_map(|part| part.tiles.iter().map(|_| !part.visible))
                    .collect();
                let focus = if r.focus > 0.0 {
                    r.focus
                } else {
                    p.target_distance
                };
                let inputs = reetched_trace::FrameInputs {
                    width: self.targets.size[0].max(1),
                    height: self.targets.size[1].max(1),
                    inv_view_proj: (p.proj * p.view).inverse(),
                    eye: p.eye,
                    aperture: if r.dof { r.aperture } else { 0.0 },
                    focus_distance: focus.max(1e-4),
                    env: &self.env_view,
                    env_sampler: &self.env_sampler,
                    env_rotation: p.env_rotation,
                    env_opacity: p.env_opacity,
                    background_lod: params[3],
                    max_bounces: r.bounces,
                    samples_per_frame: 1,
                    indirect_clamp: reetched_trace::FrameInputs::DEFAULT_CLAMP,
                    reset: n == 0,
                    frame: n,
                    hidden: &hidden,
                    materials: &trace.materials,
                };
                trace.tracer.render(device, encoder, &inputs);
            }
            trace.tracer.output().map(|v| v.texture().clone())
        } else {
            None
        };
        if self.view3d.visible() {
            self.render_3d(encoder, p, traced.as_ref());
            self.history_cur = 1 - self.history_cur;
            let frames = if tracing {
                r.trace_samples
            } else if r.dof {
                DOF_FRAMES
            } else {
                ACCUMULATE_FRAMES
            };
            self.accum = (n + 1).min(frames);
        }
        if self.view2d.visible() {
            self.render_2d(encoder, p);
        }
    }

    fn visible_tiles(&self) -> impl Iterator<Item = &Tile> {
        self.parts
            .iter()
            .filter(|p| p.visible)
            .flat_map(|p| p.tiles.iter())
            .filter(|t| t.bind_group.is_some())
    }

    /// `traced`: the path tracer's image, which replaces the rasterized one
    /// (the scene is still drawn for the depth buffer brushes test against).
    fn render_3d(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        p: &FrameParams,
        traced: Option<&wgpu::Texture>,
    ) {
        let t = &self.targets;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[
                    Some(color_attachment(&t.hdr, wgpu::Color::TRANSPARENT)),
                    Some(color_attachment(&t.sss[0], wgpu::Color::TRANSPARENT)),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_pipeline(&self.pipelines.sky);
            pass.draw(0..3, 0..1);
            if let Some((vertices, indices)) = &self.mesh {
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                let material = p.view_mode == ViewMode::Material;
                for tile in self.visible_tiles() {
                    let custom = tile
                        .custom
                        .as_ref()
                        .filter(|_| material)
                        .and_then(|(id, group)| Some((self.custom.get(id)?.as_ref().ok()?, group)));
                    match custom {
                        Some((c, group)) => {
                            pass.set_pipeline(&c.pipeline);
                            pass.set_bind_group(2, group, &[]);
                        }
                        None => pass.set_pipeline(&self.pipelines.pbr),
                    }
                    pass.set_bind_group(1, tile.bind_group.as_ref(), &[]);
                    pass.draw_indexed(tile.indices.clone(), 0, 0..1);
                }
                if p.show_wireframe
                    && let Some(wire) = &self.pipelines.wire
                {
                    pass.set_pipeline(wire);
                    for tile in self.visible_tiles() {
                        pass.draw_indexed(tile.indices.clone(), 0, 0..1);
                    }
                }
            }
        }
        let blended = p.render.alpha == AlphaMode::Blend && p.view_mode == ViewMode::Material;
        if blended && let Some((vertices, indices)) = &self.mesh {
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("transparency"),
                    color_attachments: &[
                        Some(color_attachment(&t.oit_accum, wgpu::Color::TRANSPARENT)),
                        Some(color_attachment(&t.oit_reveal, wgpu::Color::WHITE)),
                    ],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &t.depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_pipeline(&self.pipelines.oit);
                for tile in self.visible_tiles() {
                    pass.set_bind_group(1, tile.bind_group.as_ref(), &[]);
                    pass.draw_indexed(tile.indices.clone(), 0, 0..1);
                }
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("transparency resolve"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.hdr,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipelines.oit_resolve);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_bind_group(1, &t.oit_bg, &[]);
            pass.draw(0..3, 0..1);
        }
        let cur = self.history_cur;
        let fullscreen = |encoder: &mut wgpu::CommandEncoder,
                          label: &str,
                          target: &wgpu::TextureView,
                          pipeline: &wgpu::RenderPipeline,
                          group: &wgpu::BindGroup| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(color_attachment(target, wgpu::Color::TRANSPARENT))],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_bind_group(1, group, &[]);
            pass.draw(0..3, 0..1);
        };
        if let Some(traced) = traced {
            // The tracer accumulates by itself.
            encoder.copy_texture_to_texture(
                traced.as_image_copy(),
                t.history[cur].texture().as_image_copy(),
                traced.size(),
            );
        } else {
            fullscreen(
                encoder,
                "sss h",
                &t.sss[1],
                &self.pipelines.sss_h,
                &t.sss_h_bg,
            );
            fullscreen(
                encoder,
                "sss v",
                &t.sss[2],
                &self.pipelines.sss_v,
                &t.sss_v_bg,
            );
            fullscreen(
                encoder,
                "accumulate",
                &t.history[cur],
                &self.pipelines.accumulate,
                &t.accum_bg[cur],
            );
        }
        fullscreen(
            encoder,
            "bloom down",
            &t.bloom[0],
            &self.pipelines.bloom_down,
            &t.bloom_down_bg[cur],
        );
        fullscreen(
            encoder,
            "bloom blur h",
            &t.bloom[1],
            &self.pipelines.blur_h,
            &t.blur_h_bg,
        );
        fullscreen(
            encoder,
            "bloom blur v",
            &t.bloom[0],
            &self.pipelines.blur_v,
            &t.blur_v_bg,
        );
        fullscreen(
            encoder,
            "tonemap",
            &t.display,
            &self.pipelines.tonemap,
            &t.tonemap_bg[cur],
        );
    }

    /// The 3D view as displayed (RGBA8, sRGB-encoded), for tests.
    #[cfg(test)]
    pub fn read_display(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> ([u32; 2], Vec<u8>) {
        let [w, h] = self.targets.size;
        let bytes = crate::paint_gpu::read_region(
            device,
            queue,
            self.targets.display.texture(),
            0,
            0,
            w,
            h,
            4,
        )
        .unwrap();
        ([w, h], bytes)
    }

    /// The 3D view is still refining (progressive anti-aliasing); keep redrawing.
    pub fn accumulating(&self, settings: &RenderSettings) -> bool {
        if settings.path_trace && self.trace.as_ref().is_some_and(|t| t.has_mesh) {
            return self.view3d.visible() && self.accum < settings.trace_samples;
        }
        let frames = if settings.dof {
            DOF_FRAMES
        } else {
            ACCUMULATE_FRAMES
        };
        settings.accumulate && self.view3d.visible() && self.accum < frames
    }

    fn render_2d(&self, encoder: &mut wgpu::CommandEncoder, p: &FrameParams) {
        let bg = wgpu::Color {
            r: BACKGROUND,
            g: BACKGROUND,
            b: BACKGROUND,
            a: 1.0,
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("uv view"),
            color_attachments: &[Some(color_attachment(&self.uv_display, bg))],
            ..Default::default()
        });
        let (Some((vertices, indices)), Some(part)) =
            (&self.mesh, p.uv_part.and_then(|i| self.parts.get(i)))
        else {
            return;
        };
        pass.set_bind_group(0, &self.globals_bind_group, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        for tile in part.tiles.iter().filter(|t| t.bind_group.is_some()) {
            pass.set_bind_group(1, tile.bind_group.as_ref(), &[]);
            pass.set_pipeline(&self.pipelines.uv_fill);
            pass.draw_indexed(tile.indices.clone(), 0, 0..1);
            if p.show_uv_wireframe
                && let Some(wire) = &self.pipelines.uv_wire
            {
                pass.set_pipeline(wire);
                pass.draw_indexed(tile.indices.clone(), 0, 0..1);
            }
        }
    }
}

impl Viewport {
    /// A Painter shader's compile result, once a texture set used it.
    pub fn custom_shader(&self, id: &ResourceId) -> Option<&Result<CustomShader, String>> {
        self.custom.get(id)
    }

    /// Forgets a compiled Painter shader (to recompile it).
    pub fn forget_custom_shader(&mut self, id: &ResourceId) {
        self.custom.remove(id);
        self.invalidate();
    }
}

/// Translates and compiles the Painter shader in resource `id`.
fn compile_custom(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    globals: &wgpu::BindGroupLayout,
    material: &wgpu::BindGroupLayout,
    resources: &Resources,
    id: &ResourceId,
) -> Result<CustomShader, String> {
    let bytes = resources
        .files
        .get(id)
        .ok_or("the shader file is missing")?;
    let source = String::from_utf8_lossy(bytes);
    let shader = reetched_shader::translate(&source, PAINTER_HOST).map_err(|d| {
        d.iter()
            .filter(|d| d.severity == reetched_shader::Severity::Error)
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    use wgpu::ShaderStages as S;
    let mut entries = vec![(
        reetched_shader::PARAMS_BINDING,
        S::FRAGMENT,
        uniform_binding(),
    )];
    for t in &shader.textures {
        entries.push((t.binding, S::FRAGMENT, texture_binding(true)));
    }
    if !shader.textures.is_empty() {
        entries.push((
            reetched_shader::USER_SAMPLER_BINDING,
            S::FRAGMENT,
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        ));
    }
    let entries: Vec<_> = entries
        .into_iter()
        .map(|(binding, visibility, ty)| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        })
        .collect();
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("painter shader params"),
        entries: &entries,
    });
    // wgpu validates again; collect its error instead of panicking.
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let fragment = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("painter shader"),
        source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(shader.module.clone())),
    });
    let pipeline_layout = pipeline_layout(device, &[globals, material, &layout]);
    let vertex_layout = wgpu::VertexBufferLayout {
        array_stride: size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &VERTEX_ATTRIBUTES,
    };
    let target = Some(wgpu::ColorTargetState::from(HDR_FORMAT));
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("painter shader"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_mesh"),
            compilation_options: Default::default(),
            buffers: &[Some(vertex_layout)],
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &fragment,
            entry_point: Some(shader.entry_point),
            compilation_options: Default::default(),
            targets: &[target.clone(), target],
        }),
        multiview_mask: None,
        cache: crate::gpu_cache::get(),
    });
    if let Some(e) = pollster::block_on(scope.pop()) {
        return Err(format!("the GPU rejected the translated shader: {e}"));
    }
    Ok(CustomShader {
        shader,
        pipeline,
        layout,
    })
}

/// Bind group 2 of a Painter shader: its parameter block (defaults, overridden by
/// `params`) and its textures (`textures`, else white).
#[allow(clippy::too_many_arguments)]
fn custom_group(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    c: &CustomShader,
    params: &std::collections::BTreeMap<String, Vec<f32>>,
    textures: &std::collections::BTreeMap<String, ResourceId>,
    resources: &Resources,
    compositor: &mut Compositor,
    white: &wgpu::TextureView,
) -> wgpu::BindGroup {
    let mut data = c.shader.default_uniform_data();
    for p in &c.shader.params {
        let Some(values) = params.get(&p.name) else {
            continue;
        };
        for (k, v) in values.iter().take(p.ty.components()).enumerate() {
            let at = p.offset as usize + k * 4;
            let bytes = if p.ty.is_integer() {
                (v.round() as i32).to_le_bytes()
            } else {
                v.to_le_bytes()
            };
            if let Some(dst) = data.get_mut(at..at + 4) {
                dst.copy_from_slice(&bytes);
            }
        }
    }
    // A uniform binding can't be empty.
    data.resize(data.len().max(16).next_multiple_of(16), 0);
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("painter shader params"),
        contents: &data,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let views: Vec<(u32, wgpu::TextureView)> = c
        .shader
        .textures
        .iter()
        .map(|t| {
            let view = textures
                .get(&t.name)
                .and_then(|id| compositor.bitmap_view_srgb(device, queue, resources, id))
                .unwrap_or_else(|| white.clone());
            (t.binding, view)
        })
        .collect();
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    let mut entries = vec![wgpu::BindGroupEntry {
        binding: reetched_shader::PARAMS_BINDING,
        resource: buffer.as_entire_binding(),
    }];
    for (binding, view) in &views {
        entries.push(wgpu::BindGroupEntry {
            binding: *binding,
            resource: wgpu::BindingResource::TextureView(view),
        });
    }
    if !views.is_empty() {
        entries.push(wgpu::BindGroupEntry {
            binding: reetched_shader::USER_SAMPLER_BINDING,
            resource: wgpu::BindingResource::Sampler(&sampler),
        });
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("painter shader params"),
        layout: &c.layout,
        entries: &entries,
    })
}

/// Can `dab` touch tile `t`? World dabs: their sphere against the tile's bounds;
/// UV dabs: their circle against the tile's square.
fn dab_reaches(d: &Dab, t: &Tile) -> bool {
    if d.view_flags[3] as u32 & DAB_UV_SPACE != 0 {
        let [u0, v0] = reetched_doc::udim_origin(t.number);
        let (u, v, r) = (d.uv[0], d.uv[1], d.uv[2]);
        return u + r >= u0 && u - r <= u0 + 1.0 && v + r >= v0 && v - r <= v0 + 1.0;
    }
    let c = Vec3::new(d.center_radius[0], d.center_radius[1], d.center_radius[2]);
    // The splat shader reaches 1.5 radii.
    let r = d.center_radius[3] * 1.5;
    let nearest = c.clamp(t.bounds.0, t.bounds.1);
    nearest.distance_squared(c) <= r * r
}

fn display_texture(device: &wgpu::Device, [width, height]: [u32; 2]) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("view display"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DISPLAY_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// A 3D LUT texture (rgba16f), red fastest.
fn lut_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    data: &[[f32; 3]],
) -> wgpu::TextureView {
    let texels: Vec<half::f16> = data
        .iter()
        .flat_map(|c| [c[0], c[1], c[2], 1.0])
        .map(half::f16::from_f32)
        .collect();
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("display lut"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: size,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytemuck::cast_slice(&texels),
        )
        .create_view(&Default::default())
}

/// Element `i` of the Halton sequence in `base`, in [0, 1).
fn halton(mut i: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0, 0.0);
    while i > 0 {
        f /= base as f32;
        r += f * (i % base) as f32;
        i /= base;
    }
    r
}

impl Targets {
    fn new(
        device: &wgpu::Device,
        post_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        [width, height]: [u32; 2],
    ) -> Self {
        let sized = |format, usage, [width, height]: [u32; 2]| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        use wgpu::TextureUsages as U;
        let full = [width, height];
        let quarter = [(width / 4).max(1), (height / 4).max(1)];
        let rt = U::RENDER_ATTACHMENT | U::TEXTURE_BINDING;
        let hdr = sized(HDR_FORMAT, rt, full);
        let sss = [
            sized(HDR_FORMAT, rt, full),
            sized(HDR_FORMAT, rt, full),
            sized(HDR_FORMAT, rt, full),
        ];
        let oit_accum = sized(HDR_FORMAT, rt, full);
        let oit_reveal = sized(REVEAL_FORMAT, rt, full);
        // Copy destination too: the path tracer's image goes straight into history.
        let history = [
            sized(HDR_FORMAT, rt | U::COPY_DST, full),
            sized(HDR_FORMAT, rt | U::COPY_DST, full),
        ];
        let bloom = [
            sized(HDR_FORMAT, rt, quarter),
            sized(HDR_FORMAT, rt, quarter),
        ];
        // Kept and readable: brush strokes test occlusion against it, scattering too.
        let depth = sized(DEPTH_FORMAT, rt, full);
        let group = |a: &wgpu::TextureView, b: &wgpu::TextureView, c: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: post_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(a),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(b),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(c),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&depth),
                    },
                ],
            })
        };
        Self {
            size: full,
            oit_bg: group(&oit_accum, &oit_reveal, &oit_accum),
            sss_h_bg: group(&sss[0], &sss[0], &sss[0]),
            sss_v_bg: group(&sss[1], &sss[1], &sss[1]),
            accum_bg: [
                group(&hdr, &history[1], &sss[2]),
                group(&hdr, &history[0], &sss[2]),
            ],
            bloom_down_bg: [
                group(&history[0], &hdr, &hdr),
                group(&history[1], &hdr, &hdr),
            ],
            blur_h_bg: group(&bloom[0], &bloom[0], &bloom[0]),
            blur_v_bg: group(&bloom[1], &bloom[1], &bloom[1]),
            tonemap_bg: [
                group(&history[0], &bloom[0], &bloom[0]),
                group(&history[1], &bloom[0], &bloom[0]),
            ],
            hdr,
            sss,
            oit_accum,
            oit_reveal,
            depth,
            display: display_texture(device, full),
            history,
            bloom,
        }
    }
}

/// Uploads the environment with a full mip chain, then GGX-prefilters it into ENV_LEVELS mips.
fn prefilter_env(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    sampler: &wgpu::Sampler,
    env: EnvImage,
) -> wgpu::TextureView {
    use wgpu::TextureUsages as U;
    let (width, height) = (env.width, env.height);
    let texel_solid_angle = 4.0 * std::f32::consts::PI / (width * height) as f32;
    let mips = env.mip_chain();
    let texture = |mip_level_count, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("environment"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage,
            view_formats: &[],
        })
    };
    let src = texture(mips.len() as u32, U::TEXTURE_BINDING | U::COPY_DST);
    for (level, mip) in mips.iter().enumerate() {
        let data: Vec<half::f16> = mip
            .pixels
            .iter()
            .flatten()
            .map(|&c| half::f16::from_f32(c))
            .collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &src,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(mip.width * 8),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: mip.width,
                height: mip.height,
                depth_or_array_layers: 1,
            },
        );
    }
    let levels = ENV_LEVELS.min(mips.len() as u32);
    let dst = texture(levels, U::TEXTURE_BINDING | U::STORAGE_BINDING);
    let src_view = src.create_view(&Default::default());
    let layout = pipeline.get_bind_group_layout(0);
    let mut encoder = device.create_command_encoder(&Default::default());
    for level in 0..levels {
        let params = PrefilterParams {
            roughness: level as f32 / (ENV_LEVELS - 1) as f32,
            src_texel_solid_angle: texel_solid_angle,
            samples: PREFILTER_SAMPLES,
            _pad: 0,
        };
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: level,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&dst_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        let (w, h) = ((width >> level).max(1), (height >> level).max(1));
        pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
    }
    queue.submit([encoder.finish()]);
    dst.create_view(&Default::default())
}

fn globals_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    env: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    lut: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("globals"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(env),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(lut),
            },
        ],
    })
}

fn color_attachment(
    view: &wgpu::TextureView,
    clear: wgpu::Color,
) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(clear),
            store: wgpu::StoreOp::Store,
        },
    }
}

fn uniform_binding() -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}

fn texture_binding(filterable: bool) -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    }
}

fn bind_group_layout(
    device: &wgpu::Device,
    entries: &[(wgpu::ShaderStages, wgpu::BindingType)],
) -> wgpu::BindGroupLayout {
    let entries: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(i, &(visibility, ty))| wgpu::BindGroupLayoutEntry {
            binding: i as u32,
            visibility,
            ty,
            count: None,
        })
        .collect();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &entries,
    })
}

fn pipeline_layout(
    device: &wgpu::Device,
    layouts: &[&wgpu::BindGroupLayout],
) -> wgpu::PipelineLayout {
    let layouts: Vec<_> = layouts.iter().map(|l| Some(*l)).collect();
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &layouts,
        immediate_size: 0,
    })
}

#[derive(Clone, Copy)]
struct PipelineDesc<'a> {
    module: &'a wgpu::ShaderModule,
    buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    polygon_mode: wgpu::PolygonMode,
    depth: Option<(bool, wgpu::CompareFunction)>,
    targets: &'a [Option<wgpu::ColorTargetState>],
}

struct PipelineWith<'a> {
    desc: PipelineDesc<'a>,
    vs: &'a str,
    fs: &'a str,
    layout: &'a wgpu::PipelineLayout,
}

impl<'a> PipelineDesc<'a> {
    fn with(
        self,
        vs: &'a str,
        fs: &'a str,
        layout: &'a wgpu::PipelineLayout,
        depth: Option<(bool, wgpu::CompareFunction)>,
    ) -> PipelineWith<'a> {
        PipelineWith {
            desc: PipelineDesc { depth, ..self },
            vs,
            fs,
            layout,
        }
    }
}

impl PipelineWith<'_> {
    fn build(self, device: &wgpu::Device) -> wgpu::RenderPipeline {
        let d = self.desc;
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(self.fs),
            layout: Some(self.layout),
            vertex: wgpu::VertexState {
                module: d.module,
                entry_point: Some(self.vs),
                compilation_options: Default::default(),
                buffers: d.buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                polygon_mode: d.polygon_mode,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: d.depth.map(|(write, compare)| wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: d.module,
                entry_point: Some(self.fs),
                compilation_options: Default::default(),
                targets: d.targets,
            }),
            multiview_mask: None,
            cache: crate::gpu_cache::get(),
        })
    }
}
