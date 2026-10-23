//! Evaluates a texture set's layer stack on the GPU into a "channel set":
//! three rgba16float textures (see `shaders/compositor.wgsl` for the layout).

use glam::{EulerRot, Mat3};
use reetched_doc::export::ChannelData;
use reetched_doc::{
    Channel, MaskBackground, Node, NodeId, NodeKind, Projection, ProjectionMode, ResourceId,
    Resources, TILE_BYTES, TILE_SIZE, TextureSet, TileMap,
};
use std::collections::HashMap;
use wgpu::util::DeviceExt;

pub const SET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Padding (in texels) grown around UV islands for display.
const DISPLAY_DILATION: u32 = 8;

/// Identifies a paint buffer: a paint layer's channel (slot = channel index) or a mask (slot 5).
pub type PaintKey = (NodeId, u8);
pub const MASK_SLOT: u8 = 5;
pub const PAINT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// GPU copy of one paint buffer, kept in sync with its tiles.
pub struct PaintTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    resolution: u32,
    /// The tiles the texture currently holds.
    synced: TileMap,
}

/// Composited channels of one texture set.
pub struct ChannelSet {
    pub resolution: u32,
    textures: [wgpu::Texture; 3],
    pub views: [wgpu::TextureView; 3],
    /// For reading this set in a compositor pass.
    read: wgpu::BindGroup,
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct ProjUniform {
    a: [f32; 4],
    so: [f32; 4],
    origin: [f32; 4],
    r0: [f32; 4],
    r1: [f32; 4],
    r2: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    values: [[f32; 4]; 5],
    blend: [[f32; 4]; 5],
    mask: [f32; 4],
    proj: ProjUniform,
    mask_proj: ProjUniform,
    misc: [f32; 4],
}

/// The mesh triangles of one texture set.
pub struct MeshPart<'a> {
    pub vertices: &'a wgpu::Buffer,
    pub indices: &'a wgpu::Buffer,
    pub range: std::ops::Range<u32>,
}

struct Pipelines {
    init: wgpu::RenderPipeline,
    fill: wgpu::RenderPipeline,
    folder: wgpu::RenderPipeline,
    paint: wgpu::RenderPipeline,
    finalize: wgpu::RenderPipeline,
    dilate: wgpu::RenderPipeline,
}

pub struct Compositor {
    params_layout: wgpu::BindGroupLayout,
    set_layout: wgpu::BindGroupLayout,
    pipelines: Pipelines,
    sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    /// Decoded bitmaps, keyed by (resource, is sRGB color data).
    bitmaps: HashMap<(ResourceId, bool), Option<wgpu::TextureView>>,
    /// Scratch sets, reused between passes and frames.
    pool: Vec<ChannelSet>,
    /// 1×1 set bound where a pass reads nothing (a pass can't read its own target).
    dummy: ChannelSet,
    /// Paint layers' and masks' painted texels on the GPU.
    paint: HashMap<PaintKey, PaintTexture>,
}

impl Compositor {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let tex = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ];
        entries.extend((2..9).map(|b| tex(b, true)));
        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compositor params"),
            entries: &entries,
        });
        let set_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("channel set"),
            entries: &[tex(0, false), tex(1, false), tex(2, false)],
        });

        let module = device.create_shader_module(wgpu::include_wgsl!("shaders/compositor.wgsl"));
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("compositor"),
            bind_group_layouts: &[Some(&params_layout), Some(&set_layout), Some(&set_layout)],
            immediate_size: 0,
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: size_of::<reetched_io::Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4],
        };
        let targets = [
            Some(SET_FORMAT.into()),
            Some(SET_FORMAT.into()),
            Some(SET_FORMAT.into()),
        ];
        let mesh_buffers = [Some(vertex_layout)];
        let pipeline = |vs: &str, fs: &str, mesh: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: if mesh { &mesh_buffers } else { &[] },
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = Pipelines {
            init: pipeline("vs_uv", "fs_init", true),
            fill: pipeline("vs_uv", "fs_fill", true),
            folder: pipeline("vs_uv", "fs_folder", true),
            paint: pipeline("vs_uv", "fs_paint", true),
            finalize: pipeline("vs_full", "fs_finalize", false),
            dilate: pipeline("vs_full", "fs_dilate", false),
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bitmaps"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let white = upload_rgba8(
            device,
            queue,
            &image::RgbaImage::from_pixel(1, 1, image::Rgba([255; 4])),
            false,
        );
        let dummy = make_set(device, &set_layout, 1);
        Self {
            params_layout,
            set_layout,
            pipelines,
            sampler,
            white,
            bitmaps: HashMap::new(),
            pool: Vec::new(),
            dummy,
            paint: HashMap::new(),
        }
    }

    /// The GPU texture for paint buffer `key`, created and synced with `map` as needed.
    pub fn paint_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: PaintKey,
        map: &TileMap,
        res: u32,
        resources: &Resources,
    ) -> wgpu::TextureView {
        let entry = self
            .paint
            .entry(key)
            .or_insert_with(|| new_paint_texture(device, res));
        if entry.resolution != res {
            *entry = new_paint_texture(device, res);
        }
        if entry.synced != *map {
            upload_tiles(queue, entry, map, resources);
        }
        entry.view.clone()
    }

    pub fn paint_texture(&self, key: PaintKey) -> Option<&PaintTexture> {
        self.paint.get(&key)
    }

    /// After a stroke: the texture already holds `map`'s pixels, so don't re-upload.
    pub fn mark_synced(&mut self, key: PaintKey, map: TileMap) {
        if let Some(entry) = self.paint.get_mut(&key) {
            entry.synced = map;
        }
    }

    /// Drops every paint texture (when a different document is loaded).
    pub fn clear_paint(&mut self) {
        self.paint.clear();
    }

    pub fn new_set(&self, device: &wgpu::Device, resolution: u32) -> ChannelSet {
        make_set(device, &self.set_layout, resolution)
    }

    fn take(&mut self, device: &wgpu::Device, resolution: u32) -> ChannelSet {
        match self.pool.iter().position(|s| s.resolution == resolution) {
            Some(i) => self.pool.swap_remove(i),
            None => self.new_set(device, resolution),
        }
    }

    /// Re-evaluates `set` into `out` (created or resized as needed).
    /// Returns warnings, e.g. bitmaps that couldn't be decoded.
    pub fn composite(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &MeshPart,
        set: &TextureSet,
        resources: &Resources,
        out: &mut Option<ChannelSet>,
    ) -> Vec<String> {
        let res = set.resolution;
        if out.as_ref().is_none_or(|o| o.resolution != res) {
            *out = Some(self.new_set(device, res));
            // Different-sized scratch sets are dead weight now.
            self.pool.retain(|s| s.resolution == res);
        }
        let mut warnings = Vec::new();
        let mut enc = device.create_command_encoder(&Default::default());

        let start = self.take(device, res);
        let default_params = self.params(device, queue, &Params::default(), &Default::default());
        self.mesh_pass(
            &mut enc,
            &self.pipelines.init,
            &default_params,
            &self.dummy,
            &self.dummy,
            &start,
            mesh,
        );
        let result = self.eval(
            device,
            queue,
            &mut enc,
            mesh,
            &set.stack,
            start,
            resources,
            &mut warnings,
        );

        // Height → normal, then grow the islands a few texels, ending in `out`.
        let finalize = Params {
            misc: [res as f32 / 256.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        let finalize = self.params(device, queue, &finalize, &Default::default());
        let mut cur = self.take(device, res);
        self.full_pass(&mut enc, &self.pipelines.finalize, &finalize, &result, &cur);
        self.pool.push(result);
        for i in 0..DISPLAY_DILATION {
            let last = i + 1 == DISPLAY_DILATION;
            if last {
                self.full_pass(
                    &mut enc,
                    &self.pipelines.dilate,
                    &default_params,
                    &cur,
                    out.as_ref().unwrap(),
                );
            } else {
                let next = self.take(device, res);
                self.full_pass(
                    &mut enc,
                    &self.pipelines.dilate,
                    &default_params,
                    &cur,
                    &next,
                );
                self.pool.push(std::mem::replace(&mut cur, next));
            }
        }
        self.pool.push(cur);
        queue.submit([enc.finish()]);
        warnings
    }

    /// Composites `nodes` (top first) on top of `cur`; returns the result.
    #[allow(clippy::too_many_arguments)]
    fn eval(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        mesh: &MeshPart,
        nodes: &[Node],
        mut cur: ChannelSet,
        resources: &Resources,
        warnings: &mut Vec<String>,
    ) -> ChannelSet {
        let res = cur.resolution;
        for node in nodes.iter().rev().filter(|n| n.visible) {
            let (params, bitmaps) = self.node_params(device, queue, node, res, resources, warnings);
            let group = self.params(device, queue, &params, &bitmaps);
            let dst = self.take(device, res);
            match &node.kind {
                NodeKind::Fill(_) => {
                    self.mesh_pass(
                        enc,
                        &self.pipelines.fill,
                        &group,
                        &cur,
                        &self.dummy,
                        &dst,
                        mesh,
                    );
                }
                NodeKind::Paint(_) => {
                    self.mesh_pass(
                        enc,
                        &self.pipelines.paint,
                        &group,
                        &cur,
                        &self.dummy,
                        &dst,
                        mesh,
                    );
                }
                NodeKind::Folder { children } => {
                    let start = self.take(device, res);
                    copy_set(enc, &cur, &start);
                    let inner = self.eval(
                        device, queue, enc, mesh, children, start, resources, warnings,
                    );
                    self.mesh_pass(
                        enc,
                        &self.pipelines.folder,
                        &group,
                        &cur,
                        &inner,
                        &dst,
                        mesh,
                    );
                    self.pool.push(inner);
                }
            }
            self.pool.push(std::mem::replace(&mut cur, dst));
        }
        cur
    }

    fn node_params(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        node: &Node,
        res: u32,
        resources: &Resources,
        warnings: &mut Vec<String>,
    ) -> (Params, [Option<wgpu::TextureView>; 7]) {
        let mut p = Params::default();
        // Slots 0–4: channel bitmaps or paint, 5: mask bitmap, 6: painted mask.
        let mut bitmaps: [Option<wgpu::TextureView>; 7] = Default::default();
        for (i, c) in Channel::ALL.into_iter().enumerate() {
            let b = node.blend(c);
            p.blend[i] = [b.mode.index() as f32, b.opacity, 0.0, 0.0];
        }
        if let NodeKind::Fill(fill) = &node.kind {
            for (i, c) in Channel::ALL.into_iter().enumerate() {
                let Some(src) = fill.channels.get(&c) else {
                    continue;
                };
                let [x, y, z] = src.value;
                p.values[i] = [x, y, z, if src.enabled { 1.0 } else { 0.0 }];
                if let Some(id) = &src.bitmap {
                    bitmaps[i] = self.bitmap(
                        device,
                        queue,
                        resources,
                        id,
                        c == Channel::BaseColor,
                        warnings,
                    );
                    p.blend[i][2] = if bitmaps[i].is_some() { 1.0 } else { 0.0 };
                }
            }
            p.proj = proj_uniform(&fill.projection);
        }
        if let NodeKind::Paint(paint) = &node.kind {
            for (i, c) in Channel::ALL.into_iter().enumerate() {
                if let Some(map) = paint.channels.get(&c).filter(|m| !m.tiles.is_empty()) {
                    bitmaps[i] = Some(self.paint_view(
                        device,
                        queue,
                        (node.id, i as u8),
                        map,
                        res,
                        resources,
                    ));
                    p.blend[i][2] = 1.0;
                }
            }
        }
        if let Some(mask) = &node.mask {
            p.mask = [
                if mask.enabled { 1.0 } else { 0.0 },
                if mask.background == MaskBackground::White {
                    1.0
                } else {
                    0.0
                },
                0.0,
                if mask.invert { 1.0 } else { 0.0 },
            ];
            if let Some(id) = &mask.bitmap {
                bitmaps[5] = self.bitmap(device, queue, resources, id, false, warnings);
                p.mask[2] = if bitmaps[5].is_some() { 1.0 } else { 0.0 };
            }
            p.mask_proj = proj_uniform(&mask.projection);
            if let Some(map) = mask.paint.as_ref().filter(|m| !m.tiles.is_empty()) {
                bitmaps[6] =
                    Some(self.paint_view(device, queue, (node.id, MASK_SLOT), map, res, resources));
                p.misc[1] = 1.0;
            }
        }
        (p, bitmaps)
    }

    /// A linear (non-sRGB) image resource as a texture, e.g. a brush alpha.
    pub fn bitmap_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &Resources,
        id: &ResourceId,
    ) -> Option<wgpu::TextureView> {
        self.bitmap(device, queue, resources, id, false, &mut Vec::new())
    }

    fn bitmap(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &Resources,
        id: &ResourceId,
        srgb: bool,
        warnings: &mut Vec<String>,
    ) -> Option<wgpu::TextureView> {
        self.bitmaps
            .entry((id.clone(), srgb))
            .or_insert_with(|| {
                let bytes = resources.files.get(id)?;
                match image::load_from_memory(bytes) {
                    Ok(img) => Some(upload_rgba8(device, queue, &img.to_rgba8(), srgb)),
                    Err(e) => {
                        warnings.push(format!("could not decode an image: {e}"));
                        None
                    }
                }
            })
            .clone()
    }

    fn params(
        &self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        params: &Params,
        bitmaps: &[Option<wgpu::TextureView>; 7],
    ) -> wgpu::BindGroup {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("layer params"),
            contents: bytemuck::bytes_of(params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let views: Vec<&wgpu::TextureView> = bitmaps
            .iter()
            .map(|b| b.as_ref().unwrap_or(&self.white))
            .collect();
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
        ];
        entries.extend(views.iter().enumerate().map(|(i, v)| wgpu::BindGroupEntry {
            binding: 2 + i as u32,
            resource: wgpu::BindingResource::TextureView(v),
        }));
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.params_layout,
            entries: &entries,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn mesh_pass(
        &self,
        enc: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        params: &wgpu::BindGroup,
        src: &ChannelSet,
        other: &ChannelSet,
        dst: &ChannelSet,
        mesh: &MeshPart,
    ) {
        let mut pass = begin(enc, dst);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, params, &[]);
        pass.set_bind_group(1, &src.read, &[]);
        pass.set_bind_group(2, &other.read, &[]);
        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(mesh.range.clone(), 0, 0..1);
    }

    fn full_pass(
        &self,
        enc: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        params: &wgpu::BindGroup,
        src: &ChannelSet,
        dst: &ChannelSet,
    ) {
        let mut pass = begin(enc, dst);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, params, &[]);
        pass.set_bind_group(1, &src.read, &[]);
        pass.set_bind_group(2, &src.read, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Copies a channel set back to the CPU for export.
    pub fn read_back(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        set: &ChannelSet,
    ) -> anyhow::Result<ChannelData> {
        let res = set.resolution;
        let row = res * 8; // rgba16float
        let padded_row =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffers: Vec<wgpu::Buffer> = (0..3)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("readback"),
                    size: (padded_row * res) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let mut enc = device.create_command_encoder(&Default::default());
        for (texture, buffer) in set.textures.iter().zip(&buffers) {
            enc.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_row),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: res,
                    height: res,
                    depth_or_array_layers: 1,
                },
            );
        }
        queue.submit([enc.finish()]);
        for b in &buffers {
            b.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        }
        device.poll(wgpu::PollType::wait_indefinitely())?;

        let texels = (res * res) as usize;
        let mut planes: Vec<Vec<[f32; 4]>> = Vec::with_capacity(3);
        for b in &buffers {
            let mapped = b
                .slice(..)
                .get_mapped_range()
                .map_err(|e| anyhow::anyhow!("could not read GPU data: {e:?}"))?;
            let mut px = Vec::with_capacity(texels);
            for y in 0..res as usize {
                let start = y * padded_row as usize;
                let line = &mapped[start..start + row as usize];
                px.extend(line.as_chunks::<8>().0.iter().map(|c| {
                    let h: [half::f16; 4] = bytemuck::pod_read_unaligned(c);
                    h.map(f32::from)
                }));
            }
            drop(mapped);
            b.unmap();
            planes.push(px);
        }
        let (t0, t1, t2) = (&planes[0], &planes[1], &planes[2]);
        Ok(ChannelData {
            width: res,
            height: res,
            base_color: t0.iter().map(|p| [p[0], p[1], p[2]]).collect(),
            metallic: t0.iter().map(|p| p[3]).collect(),
            roughness: t1.iter().map(|p| p[3]).collect(),
            normal: t1.iter().map(|p| [p[0], p[1], p[2]]).collect(),
            height_map: t2.iter().map(|p| p[0]).collect(),
            coverage: t2.iter().map(|p| p[1] > 0.5).collect(),
        })
    }
}

fn begin<'a>(enc: &'a mut wgpu::CommandEncoder, dst: &'a ChannelSet) -> wgpu::RenderPass<'a> {
    // Outside the islands: the empty stack's values, coverage 0 (matches empty() in the shader).
    let clears = [
        wgpu::Color {
            r: 0.214,
            g: 0.214,
            b: 0.214,
            a: 0.0,
        },
        wgpu::Color {
            r: 0.0,
            g: 0.0,
            b: 1.0,
            a: 0.5,
        },
        wgpu::Color::TRANSPARENT,
    ];
    let attachments: Vec<_> = dst
        .views
        .iter()
        .zip(clears)
        .map(|(view, clear)| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })
        })
        .collect();
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("composite"),
        color_attachments: &attachments,
        ..Default::default()
    })
}

fn copy_set(enc: &mut wgpu::CommandEncoder, src: &ChannelSet, dst: &ChannelSet) {
    for (s, d) in src.textures.iter().zip(&dst.textures) {
        enc.copy_texture_to_texture(s.as_image_copy(), d.as_image_copy(), s.size());
    }
}

fn proj_uniform(p: &Projection) -> ProjUniform {
    let [rx, ry, rz] = p.rotation3d_deg.map(f32::to_radians);
    let inv = Mat3::from_euler(EulerRot::XYZ, rx, ry, rz).transpose();
    let row = |i: usize| {
        let r = inv.row(i);
        [r.x, r.y, r.z, 0.0]
    };
    let mode = match p.mode {
        ProjectionMode::Uv => 0.0,
        ProjectionMode::Triplanar => 1.0,
        ProjectionMode::Planar => 2.0,
    };
    let [px, py, pz] = p.position;
    ProjUniform {
        a: [
            mode,
            p.rotation_deg.to_radians(),
            1.0 + p.hardness * 15.0,
            1.0 / p.size.max(1e-4),
        ],
        so: [p.scale[0], p.scale[1], p.offset[0], p.offset[1]],
        origin: [px, py, pz, 0.0],
        r0: row(0),
        r1: row(1),
        r2: row(2),
    }
}

/// Uploads an image with a box-filtered mip chain.
fn upload_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    img: &image::RgbaImage,
    srgb: bool,
) -> wgpu::TextureView {
    let mut mips = vec![img.clone()];
    while let Some(last) = mips.last().filter(|m| m.width() > 1 || m.height() > 1) {
        let (w, h) = ((last.width() / 2).max(1), (last.height() / 2).max(1));
        mips.push(image::imageops::resize(
            last,
            w,
            h,
            image::imageops::FilterType::Triangle,
        ));
    }
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("bitmap"),
            size: wgpu::Extent3d {
                width: img.width(),
                height: img.height(),
                depth_or_array_layers: 1,
            },
            mip_level_count: mips.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::MipMajor,
        &mips
            .iter()
            .flat_map(|m| m.as_raw().iter().copied())
            .collect::<Vec<u8>>(),
    );
    texture.create_view(&Default::default())
}

fn make_set(
    device: &wgpu::Device,
    set_layout: &wgpu::BindGroupLayout,
    resolution: u32,
) -> ChannelSet {
    let textures = std::array::from_fn(|i| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(["set t0", "set t1", "set t2"][i]),
            size: wgpu::Extent3d {
                width: resolution,
                height: resolution,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    });
    let views: [wgpu::TextureView; 3] =
        std::array::from_fn(|i: usize| textures[i].create_view(&Default::default()));
    let read = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: set_layout,
        entries: &[0, 1, 2].map(|i| wgpu::BindGroupEntry {
            binding: i,
            resource: wgpu::BindingResource::TextureView(&views[i as usize]),
        }),
    });
    ChannelSet {
        resolution,
        textures,
        views,
        read,
    }
}

fn new_paint_texture(device: &wgpu::Device, res: u32) -> PaintTexture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("paint"),
        size: wgpu::Extent3d {
            width: res,
            height: res,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: PAINT_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    PaintTexture {
        view: texture.create_view(&Default::default()),
        texture,
        resolution: res,
        // New textures are zeroed: transparent everywhere.
        synced: TileMap::new(res),
    }
}

/// Brings a paint texture in line with `map`, uploading only tiles that changed.
fn upload_tiles(queue: &wgpu::Queue, pt: &mut PaintTexture, map: &TileMap, resources: &Resources) {
    let res = pt.resolution;
    let zero = || std::sync::Arc::new(vec![0u8; TILE_BYTES]);
    let write = |x: u32, y: u32, w: u32, h: u32, bytes: &[u8]| {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &pt.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 8),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    };

    if map.resolution == res || map.tiles.is_empty() {
        // Same size: tile-by-tile diff. A full rewrite if the texture held resampled data.
        let full = pt.synced.resolution != res;
        let tiles_per_side = res / TILE_SIZE;
        for ty in 0..tiles_per_side {
            for tx in 0..tiles_per_side {
                let key = TileMap::key(tx, ty);
                let (old, new) = (pt.synced.tiles.get(&key), map.tiles.get(&key));
                if !full && old == new {
                    continue;
                }
                let bytes = new
                    .and_then(|id| resources.tiles.get(id).cloned())
                    .unwrap_or_else(zero);
                write(tx * TILE_SIZE, ty * TILE_SIZE, TILE_SIZE, TILE_SIZE, &bytes);
            }
        }
    } else {
        // Painted at another resolution: assemble, resample, upload everything.
        let src_res = map.resolution;
        let mut img = image::Rgba32FImage::new(src_res, src_res);
        for (key, id) in &map.tiles {
            let (Some((tx, ty)), Some(bytes)) = (TileMap::parse_key(key), resources.tiles.get(id))
            else {
                continue;
            };
            for (i, texel) in bytes.as_chunks::<8>().0.iter().enumerate() {
                let (x, y) = (
                    tx * TILE_SIZE + i as u32 % TILE_SIZE,
                    ty * TILE_SIZE + i as u32 / TILE_SIZE,
                );
                if x < src_res && y < src_res {
                    let h: [half::f16; 4] = bytemuck::pod_read_unaligned(texel);
                    img.put_pixel(x, y, image::Rgba(h.map(f32::from)));
                }
            }
        }
        let resized =
            image::imageops::resize(&img, res, res, image::imageops::FilterType::Triangle);
        let bytes: Vec<u8> = resized
            .pixels()
            .flat_map(|p| p.0.map(half::f16::from_f32))
            .flat_map(|h| h.to_le_bytes())
            .collect();
        write(0, 0, res, res, &bytes);
    }
    pt.synced = map.clone();
}

/// End-to-end checks on a real GPU: a quad covering the whole UV tile is
/// composited and read back. Skipped when no adapter is available (e.g. CI).
#[cfg(test)]
mod tests {
    use super::*;
    use reetched_doc::{
        BlendMode, ChannelBlend, Document, Fill, Mask, Resources,
        export::{ExportSettings, builtin_presets, export_texture_set},
    };
    use reetched_io::Vertex;

    const RES: u32 = 64;

    fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        pollster::block_on(adapter.request_device(&Default::default())).ok()
    }

    fn quad(device: &wgpu::Device) -> (wgpu::Buffer, wgpu::Buffer) {
        let v = |u: f32, w: f32| Vertex {
            position: [u, w, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [u, w],
            tangent: [1.0, 0.0, 0.0, 1.0],
        };
        let vertices = [v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)];
        let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];
        let buf = |bytes: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytes,
                usage,
            })
        };
        (
            buf(bytemuck::cast_slice(&vertices), wgpu::BufferUsages::VERTEX),
            buf(bytemuck::cast_slice(&indices), wgpu::BufferUsages::INDEX),
        )
    }

    /// Composites `stack` (top first) and returns the read-back channels.
    fn run(stack: Vec<Node>, resources: &Resources) -> Option<ChannelData> {
        let (device, queue) = gpu()?;
        let (vb, ib) = quad(&device);
        let mesh = MeshPart {
            vertices: &vb,
            indices: &ib,
            range: 0..6,
        };
        let set = TextureSet {
            name: "t".into(),
            resolution: RES,
            stack,
        };
        let mut comp = Compositor::new(&device, &queue);
        let mut out = None;
        let warnings = comp.composite(&device, &queue, &mesh, &set, resources, &mut out);
        assert!(warnings.is_empty(), "{warnings:?}");
        Some(
            comp.read_back(&device, &queue, out.as_ref().unwrap())
                .unwrap(),
        )
    }

    fn at(d: &ChannelData, x: u32, y: u32) -> usize {
        (y * d.width + x) as usize
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
    }

    #[test]
    fn opacity_multiply_mask_and_folder() {
        let mut doc = Document::new(None, &[]);
        let mut red = doc.new_fill("red".into(), Fill::new([1.0, 0.0, 0.0], 1.0, 0.2));
        red.blend.insert(
            Channel::BaseColor,
            ChannelBlend {
                mode: BlendMode::Normal,
                opacity: 0.5,
            },
        );
        let Some(d) = run(vec![red.clone()], &Resources::default()) else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let i = at(&d, 32, 32);
        assert!(d.coverage[i]);
        assert!(
            close(d.base_color[i], [0.607, 0.107, 0.107]),
            "{:?}",
            d.base_color[i]
        ); // mix(0.214, red, 0.5)
        assert!((d.metallic[i] - 1.0).abs() < 0.01 && (d.roughness[i] - 0.2).abs() < 0.01);

        // Multiply a 50% grey over full red.
        red.blend.insert(
            Channel::BaseColor,
            ChannelBlend {
                mode: BlendMode::Normal,
                opacity: 1.0,
            },
        );
        let mut grey = doc.new_fill("grey".into(), Fill::new([0.5; 3], 0.0, 0.5));
        grey.blend.insert(
            Channel::BaseColor,
            ChannelBlend {
                mode: BlendMode::Multiply,
                opacity: 1.0,
            },
        );
        let d = run(vec![grey.clone(), red.clone()], &Resources::default()).unwrap();
        assert!(
            close(d.base_color[at(&d, 5, 5)], [0.5, 0.0, 0.0]),
            "{:?}",
            d.base_color[at(&d, 5, 5)]
        );

        // A black mask hides the layer entirely.
        grey.mask = Some(Mask::new(reetched_doc::MaskBackground::Black));
        let d = run(vec![grey.clone(), red.clone()], &Resources::default()).unwrap();
        assert!(close(d.base_color[at(&d, 5, 5)], [1.0, 0.0, 0.0]));

        // Layers inside a folder composite onto the layers below it.
        let mut folder = doc.new_folder("f".into());
        grey.mask = None;
        if let NodeKind::Folder { children } = &mut folder.kind {
            children.push(grey);
        }
        let d = run(vec![folder, red], &Resources::default()).unwrap();
        assert!(close(d.base_color[at(&d, 60, 2)], [0.5, 0.0, 0.0]));
    }

    #[test]
    fn bitmap_orientation_matches_image_and_export() {
        // 2×2 image: top row red, green; bottom row blue, white.
        let mut img = image::RgbaImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
        img.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        img.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut resources = Resources::default();
        let id = resources.add(png);
        let mut doc = Document::new(None, &[]);
        let mut fill = Fill::new([1.0; 3], 0.0, 0.5);
        fill.channels.get_mut(&Channel::BaseColor).unwrap().bitmap = Some(id);
        let layer = doc.new_fill("img".into(), fill);
        let Some(d) = run(vec![layer], &resources) else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        // Read-back row 0 is the top of the texture, like the image.
        // (Bilinear filtering blends a little of the neighbouring pixel in.)
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.06);
        assert!(
            near(d.base_color[at(&d, 16, 16)], [1.0, 0.0, 0.0]),
            "top-left {:?}",
            d.base_color[at(&d, 16, 16)]
        );
        assert!(near(d.base_color[at(&d, 48, 16)], [0.0, 1.0, 0.0]));
        assert!(near(d.base_color[at(&d, 16, 48)], [0.0, 0.0, 1.0]));

        // Exported files keep that orientation.
        let dir = std::env::temp_dir().join("reetched-gpu-export-test");
        std::fs::create_dir_all(&dir).unwrap();
        let files = export_texture_set(
            &builtin_presets()[0],
            &ExportSettings::default(),
            &d,
            &dir,
            ("p", "m", "t"),
        )
        .unwrap();
        let base = image::open(&files[0]).unwrap().to_rgb8();
        let px = |x, y| base.get_pixel(x, y).0.map(i32::from);
        assert!(
            px(16, 16)
                .iter()
                .zip([255, 0, 0])
                .all(|(a, b)| (a - b).abs() < 40),
            "{:?}",
            px(16, 16)
        );
        assert!(px(48, 48).iter().all(|&c| c > 215), "{:?}", px(48, 48));
    }
}
