//! The 3D viewport: renders the mesh with PBR + image-based lighting into an
//! offscreen texture that egui displays.

use crate::env::EnvImage;
use glam::{Mat4, Vec3};
use reetched_io::{Mesh, Vertex};
use std::ops::Range;
use wgpu::util::DeviceExt;

const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const DISPLAY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Prefiltered env mips; level i holds roughness i / (ENV_LEVELS - 1).
const ENV_LEVELS: u32 = 6;
const PREFILTER_SAMPLES: u32 = 128;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    params: [f32; 4],
    grid: [f32; 4],
    sh: [[f32; 4]; 9],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MaterialUniform {
    base_color: [f32; 4],
    rough_metal: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PrefilterParams {
    roughness: f32,
    src_texel_solid_angle: f32,
    samples: u32,
    _pad: u32,
}

/// Flat material per texture set. Replaced by the layer stack in Phase 1.
pub struct Material {
    /// Linear RGB.
    pub base_color: egui::Rgba,
    pub roughness: f32,
    pub metallic: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: egui::Rgba::from_rgb(0.6, 0.6, 0.6),
            roughness: 0.45,
            metallic: 0.0,
        }
    }
}

pub struct Part {
    pub name: String,
    pub material: Material,
    pub triangles: u32,
    indices: Range<u32>,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct FrameParams {
    pub view_proj: Mat4,
    pub eye: Vec3,
    pub exposure_ev: f32,
    pub env_rotation: f32,
    /// 0 = sharp background, 1 = fully blurred.
    pub background_blur: f32,
    pub show_grid: bool,
    pub show_wireframe: bool,
    pub grid_height: f32,
    pub grid_cell: f32,
    pub grid_extent: f32,
}

struct Targets {
    hdr: wgpu::TextureView,
    depth: wgpu::TextureView,
    display: wgpu::TextureView,
    hdr_bind_group: wgpu::BindGroup,
}

struct Pipelines {
    sky: wgpu::RenderPipeline,
    pbr: wgpu::RenderPipeline,
    wire: Option<wgpu::RenderPipeline>,
    grid: wgpu::RenderPipeline,
    tonemap: wgpu::RenderPipeline,
}

pub struct Viewport {
    pub texture_id: egui::TextureId,
    /// Size in physical pixels the UI wants; applied at the next render.
    pub requested_size: [u32; 2],
    pub parts: Vec<Part>,
    pub wireframe_supported: bool,
    size: [u32; 2],
    targets: Targets,
    pipelines: Pipelines,
    globals: wgpu::Buffer,
    globals_layout: wgpu::BindGroupLayout,
    globals_bind_group: wgpu::BindGroup,
    material_layout: wgpu::BindGroupLayout,
    hdr_layout: wgpu::BindGroupLayout,
    env_sampler: wgpu::Sampler,
    env_sh: [[f32; 4]; 9],
    prefilter: wgpu::ComputePipeline,
    mesh: Option<(wgpu::Buffer, wgpu::Buffer)>,
}

impl Viewport {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
        wireframe_supported: bool,
    ) -> Self {
        use wgpu::ShaderStages as S;
        let globals_layout = bind_group_layout(
            device,
            &[
                (S::VERTEX_FRAGMENT, uniform_binding()),
                (S::FRAGMENT, texture_binding(true)),
                (
                    S::FRAGMENT,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        );
        let material_layout = bind_group_layout(device, &[(S::FRAGMENT, uniform_binding())]);
        let hdr_layout = bind_group_layout(device, &[(S::FRAGMENT, texture_binding(false))]);

        let module = device.create_shader_module(wgpu::include_wgsl!("shaders/viewport.wgsl"));
        let scene_layout = pipeline_layout(device, &[&globals_layout, &material_layout]);
        let globals_only = pipeline_layout(device, &[&globals_layout]);
        let tonemap_layout = pipeline_layout(device, &[&globals_layout, &hdr_layout]);
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4],
        };
        let mesh_buffers = [Some(vertex_layout)];
        let depth = |write, compare| Some((write, compare));
        let base = PipelineDesc {
            module: &module,
            buffers: &[],
            topology: wgpu::PrimitiveTopology::TriangleList,
            polygon_mode: wgpu::PolygonMode::Fill,
            depth: None,
            blend: None,
            format: HDR_FORMAT,
        };
        let pipelines = Pipelines {
            sky: base
                .with(
                    "vs_fullscreen",
                    "fs_sky",
                    &globals_only,
                    depth(false, wgpu::CompareFunction::Always),
                )
                .build(device),
            pbr: PipelineDesc {
                buffers: &mesh_buffers,
                ..base
            }
            .with(
                "vs_mesh",
                "fs_pbr",
                &scene_layout,
                depth(true, wgpu::CompareFunction::Less),
            )
            .build(device),
            wire: wireframe_supported.then(|| {
                PipelineDesc {
                    buffers: &mesh_buffers,
                    polygon_mode: wgpu::PolygonMode::Line,
                    ..base
                }
                .with(
                    "vs_wire",
                    "fs_wire",
                    &globals_only,
                    depth(false, wgpu::CompareFunction::LessEqual),
                )
                .build(device)
            }),
            grid: PipelineDesc {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                ..base
            }
            .with(
                "vs_grid",
                "fs_grid",
                &globals_only,
                depth(false, wgpu::CompareFunction::Less),
            )
            .build(device),
            tonemap: PipelineDesc {
                format: DISPLAY_FORMAT,
                ..base
            }
            .with("vs_fullscreen", "fs_tonemap", &tonemap_layout, None)
            .build(device),
        };

        let prefilter = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("env prefilter"),
            layout: None,
            module: &device.create_shader_module(wgpu::include_wgsl!("shaders/prefilter.wgsl")),
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
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

        let targets = Targets::new(device, &hdr_layout, [64, 64]);
        let texture_id =
            egui.register_native_texture(device, &targets.display, wgpu::FilterMode::Linear);
        let env = EnvImage::procedural_sky();
        let env_sh = env.diffuse_sh();
        let env_view = prefilter_env(device, queue, &prefilter, &env_sampler, env);
        let globals_bind_group =
            globals_bind_group(device, &globals_layout, &globals, &env_view, &env_sampler);

        Self {
            texture_id,
            requested_size: [64, 64],
            parts: Vec::new(),
            wireframe_supported,
            size: [64, 64],
            targets,
            pipelines,
            globals,
            globals_layout,
            globals_bind_group,
            material_layout,
            hdr_layout,
            env_sampler,
            env_sh,
            prefilter,
            mesh: None,
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
        self.parts = mesh
            .parts
            .iter()
            .map(|p| {
                let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&p.material),
                    size: size_of::<MaterialUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.material_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    }],
                });
                Part {
                    name: p.material.clone(),
                    material: Material::default(),
                    triangles: (p.indices.end - p.indices.start) / 3,
                    indices: p.indices.clone(),
                    uniform,
                    bind_group,
                }
            })
            .collect();
    }

    pub fn set_environment(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, env: EnvImage) {
        self.env_sh = env.diffuse_sh();
        let view = prefilter_env(device, queue, &self.prefilter, &self.env_sampler, env);
        self.globals_bind_group = globals_bind_group(
            device,
            &self.globals_layout,
            &self.globals,
            &view,
            &self.env_sampler,
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
        let [w, h] = self.requested_size;
        if w == 0 || h == 0 {
            return;
        }
        if self.size != [w, h] {
            self.targets = Targets::new(device, &self.hdr_layout, [w, h]);
            egui.update_egui_texture_from_wgpu_texture(
                device,
                &self.targets.display,
                wgpu::FilterMode::Linear,
                self.texture_id,
            );
            self.size = [w, h];
        }

        let max_lod = (ENV_LEVELS - 1) as f32;
        let globals = Globals {
            view_proj: p.view_proj.to_cols_array_2d(),
            inv_view_proj: p.view_proj.inverse().to_cols_array_2d(),
            eye: p.eye.extend(1.0).into(),
            params: [
                p.exposure_ev.exp2(),
                p.env_rotation,
                max_lod,
                p.background_blur * max_lod,
            ],
            grid: [p.grid_height, p.grid_cell, p.grid_extent, 0.0],
            sh: self.env_sh,
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        for part in &self.parts {
            let m = &part.material;
            let uniform = MaterialUniform {
                base_color: m.base_color.to_array(),
                rough_metal: [m.roughness, m.metallic, 0.0, 0.0],
            };
            queue.write_buffer(&part.uniform, 0, bytemuck::bytes_of(&uniform));
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(color_attachment(&self.targets.hdr))],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
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
                pass.set_pipeline(&self.pipelines.pbr);
                for part in &self.parts {
                    pass.set_bind_group(1, &part.bind_group, &[]);
                    pass.draw_indexed(part.indices.clone(), 0, 0..1);
                }
                if p.show_wireframe
                    && let Some(wire) = &self.pipelines.wire
                {
                    pass.set_pipeline(wire);
                    for part in &self.parts {
                        pass.draw_indexed(part.indices.clone(), 0, 0..1);
                    }
                }
            }
            if p.show_grid {
                pass.set_pipeline(&self.pipelines.grid);
                pass.draw(0..4, 0..1);
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tonemap"),
                color_attachments: &[Some(color_attachment(&self.targets.display))],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipelines.tonemap);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_bind_group(1, &self.targets.hdr_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

impl Targets {
    fn new(
        device: &wgpu::Device,
        hdr_layout: &wgpu::BindGroupLayout,
        [width, height]: [u32; 2],
    ) -> Self {
        let view = |format, usage| {
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
        let hdr = view(HDR_FORMAT, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING);
        let hdr_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: hdr_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&hdr),
            }],
        });
        Self {
            hdr,
            depth: view(DEPTH_FORMAT, U::RENDER_ATTACHMENT),
            display: view(DISPLAY_FORMAT, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING),
            hdr_bind_group,
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
        ],
    })
}

fn color_attachment(view: &wgpu::TextureView) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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
    topology: wgpu::PrimitiveTopology,
    polygon_mode: wgpu::PolygonMode,
    depth: Option<(bool, wgpu::CompareFunction)>,
    blend: Option<wgpu::BlendState>,
    format: wgpu::TextureFormat,
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
                topology: d.topology,
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
                targets: &[Some(wgpu::ColorTargetState {
                    format: d.format,
                    blend: d.blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }
}
