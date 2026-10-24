//! Progressive GPU path tracer for the 3D view (our answer to Painter's Iray).
//!
//! One compute thread per pixel traces `samples_per_frame` full paths each frame and
//! adds them to an accumulation buffer; the averaged image is linear HDR radiance in
//! an `Rgba16Float` texture whose alpha says how much environment is visible (1 on the
//! mesh, `env_opacity` where the camera sees the sky), ready for the viewport's post
//! passes. Geometry lives in a flattened binary BVH (see `bvh`), materials in four
//! texture arrays (`Materials`), and the environment is importance sampled through a
//! CDF built on the GPU whenever the environment view changes.

mod bvh;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use reetched_io::Mesh;
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

/// Format of the averaged image `PathTracer::render` returns.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Format of the material channel arrays (same as the compositor's channel sets).
const CHANNEL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Material layers the hidden-layer bitmask covers.
pub const MAX_LAYERS: u32 = 256;
/// Environment CDF grid; keep in sync with env.wgsl and trace.wgsl.
const ENV_W: u32 = 1024;
const ENV_H: u32 = 512;
const WORKGROUP: u32 = 8;

/// Everything one frame needs. Set `reset` whenever the camera, the scene, the
/// materials or any setting changes; the accumulation then starts over.
pub struct FrameInputs<'a> {
    pub width: u32,
    pub height: u32,
    /// Inverse of projection × view (wgpu clip space, z in 0..1). Perspective or
    /// orthographic; no jitter (the tracer jitters every sample itself).
    pub inv_view_proj: Mat4,
    pub eye: Vec3,
    /// Lens radius in world units; 0 = pinhole.
    pub aperture: f32,
    /// Distance from the eye to the plane in focus, along the view direction.
    pub focus_distance: f32,
    /// Equirect environment (mip 0 = the sharp image) and its sampler.
    pub env: &'a wgpu::TextureView,
    pub env_sampler: &'a wgpu::Sampler,
    /// Radians around Y, as `env_frame` in viewport.wgsl.
    pub env_rotation: f32,
    /// Alpha where the camera sees the environment.
    pub env_opacity: f32,
    /// Environment mip the camera sees directly (background blur); 0 = sharp.
    pub background_lod: f32,
    /// Surface interactions per path (1 = direct lighting only).
    pub max_bounces: u32,
    pub samples_per_frame: u32,
    /// Max luminance of a sample carried by indirect light (firefly clamp); 0 = off.
    pub indirect_clamp: f32,
    pub reset: bool,
    /// Varies the random sequence between runs; any counter will do.
    pub frame: u32,
    /// Hidden material layers (texture sets / tiles), by layer index.
    pub hidden: &'a [bool],
    pub materials: &'a Materials,
}

impl FrameInputs<'_> {
    pub const DEFAULT_BOUNCES: u32 = 6;
    pub const DEFAULT_CLAMP: f32 = 10.0;
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    inv_view_proj: [[f32; 4]; 4],
    // w: 1 for an orthographic camera
    eye: [f32; 4],
    // width, height, seed, samples per frame
    size: [u32; 4],
    // aperture, focus distance, env rotation, env opacity
    lens: [f32; 4],
    // max bounces, indirect clamp, background lod, samples accumulated before this frame
    misc: [f32; 4],
    hidden: [[u32; 4]; 8],
}

/// Composited channels of every material layer, resampled to one size so they fit in
/// texture arrays (t0..t3 as in compositor.wgsl).
pub struct Materials {
    textures: [wgpu::Texture; 4],
    views: [wgpu::TextureView; 4],
    sampler: wgpu::Sampler,
    blitter: wgpu::util::TextureBlitter,
}

impl Materials {
    /// `size`² texels per layer (1024 costs 32 MB per layer, 2048 four times that).
    /// Layers start as a plain grey dielectric until `update_layer` fills them.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, size: u32, layers: u32) -> Self {
        let layers = layers.clamp(1, MAX_LAYERS.min(device.limits().max_texture_array_layers));
        let textures = [0, 1, 2, 3].map(|i| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(["trace t0", "trace t1", "trace t2", "trace t3"][i]),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: layers,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: CHANNEL_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        });
        // Base grey, metal 0 | flat normal, roughness 0.5 | height 0, covered, filled,
        // opaque | no emission.
        let clears = [
            [0.5, 0.5, 0.5, 0.0],
            [0.0, 0.0, 1.0, 0.5],
            [0.0, 1.0, 1.0, 1.0],
            [0.0; 4],
        ];
        let mut enc = device.create_command_encoder(&Default::default());
        for layer in 0..layers {
            for (tex, c) in textures.iter().zip(clears) {
                let view = layer_view(tex, layer);
                enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("trace material clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: c[0],
                                g: c[1],
                                b: c[2],
                                a: c[3],
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            }
        }
        queue.submit([enc.finish()]);
        let views = textures.each_ref().map(|t| {
            t.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        });
        // Repeat, like the rasterizer's texture set sampler: a tile's UVs then need
        // no origin (integer offsets wrap away).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trace materials"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // ponytail: bilinear resampling, so sources more than twice the atlas size
        // alias a little; blit from a mip chain if that shows.
        let blitter = wgpu::util::TextureBlitterBuilder::new(device, CHANNEL_FORMAT)
            .sample_type(wgpu::FilterMode::Linear)
            .build();
        Self {
            textures,
            views,
            sampler,
            blitter,
        }
    }

    pub fn size(&self) -> u32 {
        self.textures[0].width()
    }

    pub fn layers(&self) -> u32 {
        self.textures[0].depth_or_array_layers()
    }

    /// Copies one layer's composited t0..t3 (any size, filterable float views) in.
    pub fn update_layer(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        layer: u32,
        views: [&wgpu::TextureView; 4],
    ) {
        if layer >= self.layers() {
            log::warn!("trace: material layer {layer} out of {}", self.layers());
            return;
        }
        for (tex, src) in self.textures.iter().zip(views) {
            self.blitter
                .copy(device, encoder, src, &layer_view(tex, layer));
        }
    }
}

fn layer_view(tex: &wgpu::Texture, layer: u32) -> wgpu::TextureView {
    tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2),
        base_array_layer: layer,
        array_layer_count: Some(1),
        ..Default::default()
    })
}

struct Target {
    width: u32,
    height: u32,
    accum: wgpu::Buffer,
    view: wgpu::TextureView,
}

pub struct PathTracer {
    trace: wgpu::ComputePipeline,
    env_cells: wgpu::ComputePipeline,
    env_rows: wgpu::ComputePipeline,
    env_marginal: wgpu::ComputePipeline,
    env_data: wgpu::Buffer,
    /// The environment the CDF was built from.
    env_key: Option<wgpu::TextureView>,
    scene: wgpu::BindGroup,
    target: Option<Target>,
    samples: u32,
    dirty: bool,
}

impl PathTracer {
    pub fn new(device: &wgpu::Device) -> Self {
        let pipeline = |module: &wgpu::ShaderModule, entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let trace_module = device.create_shader_module(wgpu::include_wgsl!("trace.wgsl"));
        let env_module = device.create_shader_module(wgpu::include_wgsl!("env.wgsl"));
        let trace = pipeline(&trace_module, "trace");
        // Cell weights, per-row CDFs, the row CDF and the total (see env.wgsl).
        let env_floats = 2 * ENV_W * ENV_H + ENV_H + 1;
        let env_data = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trace env cdf"),
            size: env_floats as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let scene = scene_bind_group(device, &trace, &bvh::build(&[], &[]), &[0.0; 9], &[0; 3]);
        Self {
            trace,
            env_cells: pipeline(&env_module, "cells"),
            env_rows: pipeline(&env_module, "rows"),
            env_marginal: pipeline(&env_module, "marginal"),
            env_data,
            env_key: None,
            scene,
            target: None,
            samples: 0,
            dirty: true,
        }
    }

    /// Uploads the mesh and builds its BVH; returns the build time. `layer_of_triangle`
    /// gives each triangle's material layer (missing entries use layer 0).
    pub fn set_mesh(
        &mut self,
        device: &wgpu::Device,
        mesh: &Mesh,
        layer_of_triangle: &[u32],
    ) -> anyhow::Result<Duration> {
        let start = Instant::now();
        let p = |i: u32| Vec3::from(mesh.vertices[i as usize].position);
        let tris: Vec<[Vec3; 3]> = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[a, b, c]| [p(a), p(b), p(c)])
            .collect();
        let flat = bvh::build(&tris, layer_of_triangle);
        let took = start.elapsed();
        // Shading attributes only: normal, uv, tangent (positions live in the BVH).
        let verts: Vec<f32> = mesh
            .vertices
            .iter()
            .flat_map(|v| {
                let [nx, ny, nz] = v.normal;
                let [u, w] = v.uv;
                let [tx, ty, tz, tw] = v.tangent;
                [nx, ny, nz, u, w, tx, ty, tz, tw]
            })
            .collect();
        let limit = device.limits().max_storage_buffer_binding_size;
        let biggest = [
            std::mem::size_of_val(flat.nodes.as_slice()),
            std::mem::size_of_val(flat.tris.as_slice()),
            verts.len() * 4,
            mesh.indices.len() * 4,
        ]
        .into_iter()
        .max()
        .unwrap_or(0) as u64;
        anyhow::ensure!(
            biggest <= limit,
            "mesh too large for the path tracer ({biggest} byte buffer, the device allows {limit}; \
             request adapter.limits() when creating the device)"
        );
        let indices: &[u32] = if mesh.indices.is_empty() {
            &[0; 3]
        } else {
            &mesh.indices
        };
        let verts: &[f32] = if verts.is_empty() { &[0.0; 9] } else { &verts };
        self.scene = scene_bind_group(device, &self.trace, &flat, verts, indices);
        self.dirty = true;
        log::info!(
            "trace: BVH over {} triangles in {:.1} ms ({} nodes)",
            tris.len(),
            took.as_secs_f64() * 1e3,
            flat.nodes.len()
        );
        Ok(took)
    }

    /// Samples per pixel accumulated so far.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// The averaged image of the last `render`, if any.
    pub fn output(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.view)
    }

    /// Adds `samples_per_frame` samples per pixel and returns the averaged image
    /// (`OUTPUT_FORMAT`, `width`×`height`, rgb = linear radiance, a = environment
    /// visibility as described in the crate docs).
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        f: &FrameInputs,
    ) -> &wgpu::TextureView {
        let (w, h) = (f.width.max(1), f.height.max(1));
        if self
            .target
            .as_ref()
            .is_none_or(|t| (t.width, t.height) != (w, h))
        {
            self.target = Some(new_target(device, w, h));
            self.dirty = true;
        }
        if f.reset || self.dirty {
            self.samples = 0;
            self.dirty = false;
        }
        if self.env_key.as_ref() != Some(f.env) {
            self.build_env_cdf(device, encoder, f.env);
        }

        let mut hidden = [[0u32; 4]; 8];
        for (i, _) in f
            .hidden
            .iter()
            .enumerate()
            .take(MAX_LAYERS as usize)
            .filter(|(_, h)| **h)
        {
            hidden[i / 128][(i / 32) % 4] |= 1 << (i % 32);
        }
        let spp = f.samples_per_frame.max(1);
        let uniform = FrameUniform {
            inv_view_proj: f.inv_view_proj.to_cols_array_2d(),
            eye: f
                .eye
                .extend(if is_orthographic(f.inv_view_proj) {
                    1.0
                } else {
                    0.0
                })
                .into(),
            size: [
                w,
                h,
                self.samples.wrapping_add(f.frame.wrapping_mul(0x9e37_79b9)),
                spp,
            ],
            lens: [
                f.aperture.max(0.0),
                f.focus_distance.max(1e-6),
                f.env_rotation,
                f.env_opacity,
            ],
            misc: [
                f.max_bounces as f32,
                f.indirect_clamp.max(0.0),
                f.background_lod,
                self.samples as f32,
            ],
            hidden,
        };
        // A fresh uniform buffer per frame, so several renders before one submit
        // each keep their own settings.
        let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("trace frame"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let target = self.target.as_ref().unwrap();
        let frame = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trace frame"),
            layout: &self.trace.get_bind_group_layout(0),
            entries: &[
                entry(0, ubuf.as_entire_binding()),
                entry(1, target.accum.as_entire_binding()),
                entry(2, wgpu::BindingResource::TextureView(&target.view)),
                entry(3, wgpu::BindingResource::TextureView(f.env)),
                entry(4, wgpu::BindingResource::Sampler(f.env_sampler)),
                entry(5, self.env_data.as_entire_binding()),
            ],
        });
        let m = f.materials;
        let materials = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trace materials"),
            layout: &self.trace.get_bind_group_layout(2),
            entries: &[
                entry(0, wgpu::BindingResource::TextureView(&m.views[0])),
                entry(1, wgpu::BindingResource::TextureView(&m.views[1])),
                entry(2, wgpu::BindingResource::TextureView(&m.views[2])),
                entry(3, wgpu::BindingResource::TextureView(&m.views[3])),
                entry(4, wgpu::BindingResource::Sampler(&m.sampler)),
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("trace"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.trace);
            pass.set_bind_group(0, &frame, &[]);
            pass.set_bind_group(1, &self.scene, &[]);
            pass.set_bind_group(2, &materials, &[]);
            pass.dispatch_workgroups(w.div_ceil(WORKGROUP), h.div_ceil(WORKGROUP), 1);
        }
        self.samples += spp;
        &target.view
    }

    fn build_env_cdf(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        env: &wgpu::TextureView,
    ) {
        let group = |p: &wgpu::ComputePipeline, with_env: bool| {
            let mut entries = vec![entry(1, self.env_data.as_entire_binding())];
            if with_env {
                entries.push(entry(0, wgpu::BindingResource::TextureView(env)));
            }
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("trace env cdf"),
                layout: &p.get_bind_group_layout(0),
                entries: &entries,
            })
        };
        let groups = [
            group(&self.env_cells, true),
            group(&self.env_rows, false),
            group(&self.env_marginal, false),
        ];
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("trace env cdf"),
            timestamp_writes: None,
        });
        let steps = [
            (&self.env_cells, (ENV_W / 8, ENV_H / 8)),
            (&self.env_rows, (ENV_H / 64, 1)),
            (&self.env_marginal, (1, 1)),
        ];
        for ((p, (x, y)), g) in steps.into_iter().zip(&groups) {
            pass.set_pipeline(p);
            pass.set_bind_group(0, g, &[]);
            pass.dispatch_workgroups(x, y, 1);
        }
        drop(pass);
        self.env_key = Some(env.clone());
        // Old samples were lit by the old environment.
        self.samples = 0;
    }
}

fn entry(binding: u32, resource: wgpu::BindingResource<'_>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource }
}

fn new_target(device: &wgpu::Device, width: u32, height: u32) -> Target {
    let accum = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("trace accum"),
        size: width as u64 * height as u64 * 16,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("trace output"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    Target {
        width,
        height,
        accum,
        view: tex.create_view(&Default::default()),
    }
}

fn scene_bind_group(
    device: &wgpu::Device,
    trace: &wgpu::ComputePipeline,
    flat: &bvh::Flat,
    verts: &[f32],
    indices: &[u32],
) -> wgpu::BindGroup {
    let buf = |label, contents: &[u8]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage: wgpu::BufferUsages::STORAGE,
        })
    };
    let nodes = buf("trace nodes", bytemuck::cast_slice(&flat.nodes));
    let tris = buf("trace tris", bytemuck::cast_slice(&flat.tris));
    let verts = buf("trace verts", bytemuck::cast_slice(verts));
    let indices = buf("trace indices", bytemuck::cast_slice(indices));
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("trace scene"),
        layout: &trace.get_bind_group_layout(1),
        entries: &[
            entry(0, nodes.as_entire_binding()),
            entry(1, tris.as_entire_binding()),
            entry(2, verts.as_entire_binding()),
            entry(3, indices.as_entire_binding()),
        ],
    })
}

/// Whether rays through opposite corners of the view are parallel.
fn is_orthographic(inv_view_proj: Mat4) -> bool {
    let dir = |x: f32, y: f32| {
        let near = inv_view_proj.project_point3(Vec3::new(x, y, 0.0));
        let far = inv_view_proj.project_point3(Vec3::new(x, y, 1.0));
        (far - near).normalize()
    };
    dir(-1.0, -1.0).dot(dir(1.0, 1.0)) > 1.0 - 1e-6
}
