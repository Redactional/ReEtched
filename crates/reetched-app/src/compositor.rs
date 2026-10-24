//! Evaluates a texture set's layer stack on the GPU into "channel sets": per
//! channel group (see `CHANNEL_GROUPS`), four rgba16float textures (layouts in
//! `shaders/compositor.wgsl`). UDIM sets are composited one tile at a time.

use glam::{EulerRot, Mat3};
use reetched_doc::export::ChannelData;
use reetched_doc::{
    CHANNEL_GROUPS, Channel, GeneratorKind, MAX_MASK_EFFECTS, MaskBackground, MaskBlend,
    MaskEffectKind, MeshMap, Node, NodeId, NodeKind, Projection, ProjectionMode, ResourceId,
    Resources, TILE_BYTES, TILE_SIZE, TextureSet, TileMap,
};
use std::collections::HashMap;
use wgpu::util::DeviceExt;

pub const SET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Padding (in texels) grown around UV islands for display.
const DISPLAY_DILATION: u32 = 8;

/// Identifies a paint buffer: (layer, channel index in `Channel::ALL` or `MASK_SLOT`, UDIM tile).
pub type PaintKey = (NodeId, u8, u32);
pub const MASK_SLOT: u8 = 99;
/// Textures in a channel set.
const SET_TEXTURES: usize = 4;
/// Channel groups, each composited into its own channel set.
pub const GROUPS: usize = CHANNEL_GROUPS.len();
/// One texture set tile's composited channels; `None` for groups the set doesn't use.
pub type GroupSets = [Option<ChannelSet>; GROUPS];
const CHANNELS: usize = Channel::ALL.len();
/// Channels per group (and channel bitmaps per pass).
const SLOTS: usize = 7;
pub const PAINT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Vertex attributes the UV-space passes read (position, normal, uv, tangent, uv1).
pub const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4, 4 => Float32x2];

/// GPU copy of one paint buffer, kept in sync with its tiles.
pub struct PaintTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    resolution: u32,
    /// The tiles the texture currently holds.
    synced: TileMap,
}

/// Composited channels of one group of one texture set (tile).
pub struct ChannelSet {
    pub resolution: u32,
    textures: [wgpu::Texture; SET_TEXTURES],
    pub views: [wgpu::TextureView; SET_TEXTURES],
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
    warp: [[f32; 4]; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct MaskFx {
    head: [f32; 4],
    a: [f32; 4],
    b: [f32; 4],
    colors: [[f32; 4]; 4],
}

/// Mesh maps the compositor binds, in binding order (11..=16).
const BOUND_MAPS: [MeshMap; 6] = [
    MeshMap::Normal,
    MeshMap::Curvature,
    MeshMap::AmbientOcclusion,
    MeshMap::Position,
    MeshMap::Thickness,
    MeshMap::Id,
];

/// Texture views for one pass: 0–6 the group's channel bitmaps/paint, 7 mask
/// bitmap, 8 painted mask, 9–14 mesh maps.
type PassTextures = [Option<wgpu::TextureView>; 15];
const MASK_BITMAP: usize = SLOTS;
const MASK_PAINT: usize = SLOTS + 1;
const FIRST_MAP: usize = SLOTS + 2;

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    values: [[f32; 4]; SLOTS],
    blend: [[f32; 4]; SLOTS],
    mask: [f32; 4],
    proj: ProjUniform,
    mask_proj: ProjUniform,
    misc: [f32; 4],
    maps: [f32; 4],
    maps2: [f32; 4],
    fx: [MaskFx; MAX_MASK_EFFECTS],
    tile: [f32; 4],
    defaults: [[f32; 4]; 4],
}

/// The mesh triangles of one texture set (UDIM tile).
pub struct MeshPart<'a> {
    pub vertices: &'a wgpu::Buffer,
    pub indices: &'a wgpu::Buffer,
    pub range: std::ops::Range<u32>,
    /// Mesh size (bounding-box diagonal), for scaling procedural noise.
    pub scale: f32,
    /// UDIM tile these triangles are in.
    pub tile: u32,
}

impl MeshPart<'_> {
    pub fn uv_origin(&self) -> [f32; 2] {
        reetched_doc::udim_origin(self.tile)
    }
}

/// Render pipelines of one group layout (core or extra channels).
struct GroupPipelines {
    init: wgpu::RenderPipeline,
    fill: wgpu::RenderPipeline,
    folder: wgpu::RenderPipeline,
    paint: wgpu::RenderPipeline,
    dilate: wgpu::RenderPipeline,
}

struct Pipelines {
    core: GroupPipelines,
    ext: GroupPipelines,
    finalize: wgpu::RenderPipeline,
}

impl Pipelines {
    fn group(&self, g: usize) -> &GroupPipelines {
        if g == 0 { &self.core } else { &self.ext }
    }
}

pub struct Compositor {
    params_layout: wgpu::BindGroupLayout,
    set_layout: wgpu::BindGroupLayout,
    pipelines: Pipelines,
    sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    /// Decoded bitmaps, keyed by (resource, how it's decoded).
    bitmaps: HashMap<(ResourceId, Decode), Option<wgpu::TextureView>>,
    /// The OCIO config bitmaps can be read through.
    ocio: Option<crate::color_ui::Ocio>,
    /// Scratch sets, reused between passes and frames.
    pool: Vec<ChannelSet>,
    /// 1×1 set bound where a pass reads nothing (a pass can't read its own target).
    dummy: ChannelSet,
    /// 1×1 sets holding each group's empty-stack values, for groups a set doesn't use.
    defaults: [ChannelSet; GROUPS],
    /// Paint layers' and masks' painted texels on the GPU.
    paint: HashMap<PaintKey, PaintTexture>,
    /// Rendered procedural materials by render key: one texture per channel.
    procedural: HashMap<String, [Option<wgpu::TextureView>; CHANNELS]>,
    /// Resolution and mesh maps of the tile being composited (for render keys).
    proc_ctx: (
        u32,
        std::collections::BTreeMap<reetched_doc::MeshMap, ResourceId>,
    ),
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
                // The vertex stage reads the UDIM tile offset.
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
        entries.extend((2..2 + 15).map(|b| tex(b, true)));
        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compositor params"),
            entries: &entries,
        });
        let set_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("channel set"),
            entries: &[tex(0, false), tex(1, false), tex(2, false), tex(3, false)],
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
            attributes: &VERTEX_ATTRIBUTES,
        };
        let targets: [Option<wgpu::ColorTargetState>; SET_TEXTURES] =
            std::array::from_fn(|_| Some(SET_FORMAT.into()));
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
        let group = |suffix: &str| GroupPipelines {
            init: pipeline("vs_uv", &format!("fs_init{suffix}"), true),
            fill: pipeline("vs_uv", &format!("fs_fill{suffix}"), true),
            folder: pipeline("vs_uv", &format!("fs_folder{suffix}"), true),
            paint: pipeline("vs_uv", &format!("fs_paint{suffix}"), true),
            dilate: pipeline("vs_full", &format!("fs_dilate{suffix}"), false),
        };
        let pipelines = Pipelines {
            core: group(""),
            ext: group("_ex"),
            finalize: pipeline("vs_full", "fs_finalize", false),
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
        let defaults: [ChannelSet; GROUPS] =
            std::array::from_fn(|_| make_set(device, &set_layout, 1));
        let mut enc = device.create_command_encoder(&Default::default());
        for (g, set) in defaults.iter().enumerate() {
            drop(begin(&mut enc, set, g, true));
        }
        queue.submit([enc.finish()]);
        Self {
            params_layout,
            set_layout,
            pipelines,
            sampler,
            white,
            bitmaps: HashMap::new(),
            ocio: None,
            pool: Vec::new(),
            dummy,
            defaults,
            paint: HashMap::new(),
            procedural: HashMap::new(),
            proc_ctx: Default::default(),
        }
    }

    /// Views of group `g`'s channel set: `set`'s, or the empty-stack defaults.
    pub fn group_views<'a>(&'a self, sets: &'a GroupSets, g: usize) -> &'a [wgpu::TextureView; 4] {
        &sets[g].as_ref().unwrap_or(&self.defaults[g]).views
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

    /// Re-evaluates `set` on `mesh` (one UDIM tile) into `out` (created or
    /// resized as needed). Returns warnings, e.g. bitmaps that couldn't be decoded.
    pub fn composite(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &MeshPart,
        set: &TextureSet,
        resources: &Resources,
        out: &mut GroupSets,
    ) -> Vec<String> {
        let tile = mesh.tile;
        let res = set.tile_resolution(tile);
        let mesh_maps = set.mesh_maps_for(tile).clone();
        self.proc_ctx = (res, mesh_maps.clone());
        let mut warnings = Vec::new();
        let mut enc = device.create_command_encoder(&Default::default());

        // Baked mesh maps, shared by every pass of this tile.
        let mut maps: [Option<wgpu::TextureView>; 6] = Default::default();
        for (slot, kind) in BOUND_MAPS.into_iter().enumerate() {
            if let Some(id) = mesh_maps.get(&kind) {
                maps[slot] = self.bitmap(device, queue, resources, id, Decode::Raw, &mut warnings);
            }
        }
        let flag = |i: usize| if maps[i].is_some() { 1.0 } else { 0.0 };
        let map_flags = (
            [flag(0), flag(1), flag(2), flag(3)],
            [flag(4), flag(5), 0.0, 0.0],
        );
        let [u0, v0] = mesh.uv_origin();
        let with_maps = |g: usize, mut p: Params, mut t: PassTextures| {
            (p.maps, p.maps2) = map_flags;
            p.misc[2] = mesh.scale;
            p.tile = [u0, v0, 0.0, 0.0];
            p.defaults = group_values(g, true);
            for (slot, view) in maps.iter().enumerate() {
                t[FIRST_MAP + slot] = view.clone();
            }
            (p, t)
        };

        for (g, out) in out.iter_mut().enumerate() {
            if !set.uses_group(g) {
                if let Some(old) = out.take() {
                    self.pool.push(old);
                }
                continue;
            }
            if out.as_ref().is_none_or(|o| o.resolution != res) {
                *out = Some(self.new_set(device, res));
            }
            let start = self.take(device, res);
            let (init, init_tex) = with_maps(g, Params::default(), Default::default());
            let default_params = self.params(device, &init, &init_tex);
            self.mesh_pass(
                &mut enc,
                &self.pipelines.group(g).init,
                &default_params,
                &self.dummy,
                &self.dummy,
                &start,
                mesh,
                g,
            );
            let result = self.eval(
                device,
                queue,
                &mut enc,
                mesh,
                g,
                &set.stack,
                start,
                resources,
                &mut warnings,
                &with_maps,
            );

            // Core: height → normal. Then grow the islands a few texels, ending in `out`.
            let mut cur = if g == 0 {
                let finalize = Params {
                    misc: [res as f32 / 256.0, 0.0, 0.0, 0.0],
                    ..Default::default()
                };
                let finalize = self.params(device, &finalize, &Default::default());
                let cur = self.take(device, res);
                self.full_pass(
                    &mut enc,
                    &self.pipelines.finalize,
                    &finalize,
                    &result,
                    &cur,
                    g,
                );
                self.pool.push(result);
                cur
            } else {
                result
            };
            for i in 0..DISPLAY_DILATION {
                let last = i + 1 == DISPLAY_DILATION;
                if last {
                    self.full_pass(
                        &mut enc,
                        &self.pipelines.group(g).dilate,
                        &default_params,
                        &cur,
                        out.as_ref().unwrap(),
                        g,
                    );
                } else {
                    let next = self.take(device, res);
                    let dilate = &self.pipelines.group(g).dilate;
                    self.full_pass(&mut enc, dilate, &default_params, &cur, &next, g);
                    self.pool.push(std::mem::replace(&mut cur, next));
                }
            }
            self.pool.push(cur);
        }
        // Different-sized scratch sets are dead weight now.
        self.pool.retain(|s| s.resolution == res);
        queue.submit([enc.finish()]);
        warnings
    }

    /// Composites `nodes` (top first) of group `g` on top of `cur`; returns the result.
    #[allow(clippy::too_many_arguments)]
    fn eval(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        mesh: &MeshPart,
        g: usize,
        nodes: &[Node],
        mut cur: ChannelSet,
        resources: &Resources,
        warnings: &mut Vec<String>,
        with_maps: &dyn Fn(usize, Params, PassTextures) -> (Params, PassTextures),
    ) -> ChannelSet {
        let res = cur.resolution;
        for node in nodes
            .iter()
            .rev()
            .filter(|n| n.visible && n.on_tile(mesh.tile))
        {
            let (params, bitmaps) =
                self.node_params(device, queue, node, g, mesh.tile, res, resources, warnings);
            let (params, bitmaps) = with_maps(g, params, bitmaps);
            let group = self.params(device, &params, &bitmaps);
            let dst = self.take(device, res);
            match &node.kind {
                NodeKind::Fill(_) => {
                    let pl = &self.pipelines.group(g).fill;
                    self.mesh_pass(enc, pl, &group, &cur, &self.dummy, &dst, mesh, g);
                }
                NodeKind::Paint(_) => {
                    let pl = &self.pipelines.group(g).paint;
                    self.mesh_pass(enc, pl, &group, &cur, &self.dummy, &dst, mesh, g);
                }
                NodeKind::Folder { children } => {
                    let start = self.take(device, res);
                    copy_set(enc, &cur, &start);
                    let inner = self.eval(
                        device, queue, enc, mesh, g, children, start, resources, warnings,
                        with_maps,
                    );
                    let pl = &self.pipelines.group(g).folder;
                    self.mesh_pass(enc, pl, &group, &cur, &inner, &dst, mesh, g);
                    self.pool.push(inner);
                }
            }
            self.pool.push(std::mem::replace(&mut cur, dst));
        }
        cur
    }

    #[allow(clippy::too_many_arguments)]
    fn node_params(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        node: &Node,
        g: usize,
        tile: u32,
        res: u32,
        resources: &Resources,
        warnings: &mut Vec<String>,
    ) -> (Params, PassTextures) {
        let mut p = Params::default();
        let mut bitmaps: PassTextures = Default::default();
        let channels = CHANNEL_GROUPS[g];
        for (i, &c) in channels.iter().enumerate() {
            let b = node.blend(c);
            p.blend[i] = [b.mode.index() as f32, b.opacity, 0.0, 0.0];
        }
        if let NodeKind::Fill(fill) = &node.kind {
            for (i, &c) in channels.iter().enumerate() {
                let Some(src) = fill.channels.get(&c) else {
                    continue;
                };
                let [x, y, z] = src.value;
                p.values[i] = [x, y, z, if src.enabled { 1.0 } else { 0.0 }];
                if let Some(id) = &src.bitmap {
                    let decode = match &src.ocio {
                        Some(name) if self.ocio.is_some() => Decode::Ocio(name.clone()),
                        _ if src.color_space.is_srgb(c) => Decode::Srgb,
                        _ => Decode::Raw,
                    };
                    bitmaps[i] = self.bitmap(device, queue, resources, id, decode, warnings);
                    p.blend[i][2] = if bitmaps[i].is_some() { 1.0 } else { 0.0 };
                }
            }
            p.proj = proj_uniform(&fill.projection);
            if let Some(proc) = &fill.procedural
                && let Some(views) = self
                    .procedural
                    .get(&proc.render_key(self.proc_ctx.0, &self.proc_ctx.1))
            {
                for (i, &c) in channels.iter().enumerate() {
                    if let Some(v) = &views[c.index()] {
                        bitmaps[i] = Some(v.clone());
                        p.blend[i][2] = 1.0;
                    }
                }
            }
        }
        if let NodeKind::Paint(paint) = &node.kind {
            for (i, &c) in channels.iter().enumerate() {
                if let Some(map) = paint.map(tile, c).filter(|m| !m.tiles.is_empty()) {
                    bitmaps[i] = Some(self.paint_view(
                        device,
                        queue,
                        (node.id, c.index() as u8, tile),
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
                bitmaps[MASK_BITMAP] =
                    self.bitmap(device, queue, resources, id, Decode::Raw, warnings);
                p.mask[2] = if bitmaps[MASK_BITMAP].is_some() {
                    1.0
                } else {
                    0.0
                };
            }
            p.mask_proj = proj_uniform(&mask.projection);
            for (fx, e) in
                p.fx.iter_mut()
                    .zip(mask.effects.iter().filter(|e| e.enabled))
            {
                *fx = mask_fx(e);
            }
            if let Some(map) = mask.paint_for(tile).filter(|m| !m.tiles.is_empty()) {
                bitmaps[MASK_PAINT] = Some(self.paint_view(
                    device,
                    queue,
                    (node.id, MASK_SLOT, tile),
                    map,
                    res,
                    resources,
                ));
                p.misc[1] = 1.0;
            }
        }
        (p, bitmaps)
    }

    /// Sets the OCIO config bitmaps with an OCIO color space are read through.
    pub fn set_ocio(&mut self, ocio: Option<crate::color_ui::Ocio>) {
        self.ocio = ocio;
        self.bitmaps
            .retain(|(_, d), _| !matches!(d, Decode::Ocio(_)));
    }

    pub fn has_procedural(&self, key: &str) -> bool {
        self.procedural.contains_key(key)
    }

    /// Stores a procedural material's rendered channels (RGBA8; color channels are sRGB).
    pub fn set_procedural(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: String,
        outputs: &[(Channel, image::RgbaImage)],
    ) {
        let mut views: [Option<wgpu::TextureView>; CHANNELS] = Default::default();
        for (c, img) in outputs {
            views[c.index()] = Some(upload_rgba8(device, queue, img, c.is_color()));
        }
        self.procedural.insert(key, views);
    }

    /// Drops rendered materials no layer uses any more.
    pub fn retain_procedural(&mut self, keep: &std::collections::HashSet<String>) {
        self.procedural.retain(|k, _| keep.contains(k));
    }

    /// A color image resource, decoded from sRGB.
    pub fn bitmap_view_srgb(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &Resources,
        id: &ResourceId,
    ) -> Option<wgpu::TextureView> {
        self.bitmap(device, queue, resources, id, Decode::Srgb, &mut Vec::new())
    }

    /// A linear (non-sRGB) image resource as a texture, e.g. a brush alpha.
    pub fn bitmap_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &Resources,
        id: &ResourceId,
    ) -> Option<wgpu::TextureView> {
        self.bitmap(device, queue, resources, id, Decode::Raw, &mut Vec::new())
    }

    fn bitmap(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &Resources,
        id: &ResourceId,
        decode: Decode,
        warnings: &mut Vec<String>,
    ) -> Option<wgpu::TextureView> {
        let ocio = self.ocio.as_ref();
        self.bitmaps
            .entry((id.clone(), decode.clone()))
            .or_insert_with(|| {
                let bytes = resources.files.get(id)?;
                let img = match crate::images::decode(bytes) {
                    Ok(img) => img,
                    Err(e) => {
                        warnings.push(format!("could not decode an image: {e:#}"));
                        return None;
                    }
                };
                match (&decode, ocio) {
                    (Decode::Ocio(space), Some(ocio)) => {
                        let mut px = img.to_rgba32f();
                        if let Err(e) = ocio.to_working(space, &mut px) {
                            warnings.push(format!("OCIO conversion from {space} failed: {e:#}"));
                        }
                        Some(upload_rgba32f(device, queue, &px))
                    }
                    (d, _) => Some(upload_rgba8(
                        device,
                        queue,
                        &img.to_rgba8(),
                        !matches!(d, Decode::Raw),
                    )),
                }
            })
            .clone()
    }

    fn params(
        &self,
        device: &wgpu::Device,
        params: &Params,
        bitmaps: &PassTextures,
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
        g: usize,
    ) {
        let mut pass = begin(enc, dst, g, false);
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
        g: usize,
    ) {
        let mut pass = begin(enc, dst, g, false);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, params, &[]);
        pass.set_bind_group(1, &src.read, &[]);
        pass.set_bind_group(2, &src.read, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Copies a tile's channel sets back to the CPU for export.
    pub fn read_back(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sets: &GroupSets,
    ) -> anyhow::Result<ChannelData> {
        let core = sets[0]
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("nothing composited yet"))?;
        let res = core.resolution;
        let planes = read_planes(device, queue, core)?;
        let (t0, t1, t2, t3) = (&planes[0], &planes[1], &planes[2], &planes[3]);
        let mut data = ChannelData {
            width: res,
            height: res,
            base_color: t0.iter().map(|p| [p[0], p[1], p[2]]).collect(),
            metallic: t0.iter().map(|p| p[3]).collect(),
            roughness: t1.iter().map(|p| p[3]).collect(),
            normal: t1.iter().map(|p| [p[0], p[1], p[2]]).collect(),
            height_map: t2.iter().map(|p| p[0]).collect(),
            emissive: t3.iter().map(|p| [p[0], p[1], p[2]]).collect(),
            opacity: t2.iter().map(|p| p[3]).collect(),
            coverage: t2.iter().map(|p| p[1] > 0.5).collect(),
            ambient_occlusion: Vec::new(),
            extra: Default::default(),
        };
        for (g, set) in sets.iter().enumerate().skip(1) {
            let Some(set) = set else { continue };
            let pl = read_planes(device, queue, set)?;
            let group = CHANNEL_GROUPS[g];
            let rgb = |k: usize| pl[k].iter().map(|p| [p[0], p[1], p[2]]).collect();
            let scalar = |k: usize, c: usize| pl[k].iter().map(|p| [p[c]; 3]).collect();
            data.extra.insert(group[0], rgb(0));
            data.extra.insert(group[1], rgb(1));
            data.extra.insert(group[2], scalar(0, 3));
            data.extra.insert(group[3], scalar(1, 3));
            data.extra.insert(group[4], scalar(2, 0));
            data.extra.insert(group[5], scalar(2, 1));
            data.extra.insert(group[6], scalar(2, 2));
        }
        Ok(data)
    }
}

/// A channel set's four textures as RGBA f32 rows (row 0 = top).
fn read_planes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    set: &ChannelSet,
) -> anyhow::Result<Vec<Vec<[f32; 4]>>> {
    let res = set.resolution;
    let row = res * 8; // rgba16float
    let padded_row =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffers: Vec<wgpu::Buffer> = (0..SET_TEXTURES)
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
    let mut planes: Vec<Vec<[f32; 4]>> = Vec::with_capacity(SET_TEXTURES);
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
    Ok(planes)
}

/// Group `g`'s empty-stack values packed like its textures (see compositor.wgsl);
/// `inside`: within the UV islands (coverage and filled set).
fn group_values(g: usize, inside: bool) -> [[f32; 4]; 4] {
    let e = |slot: usize| CHANNEL_GROUPS[g][slot].empty_value();
    let k = if inside { 1.0 } else { 0.0 };
    if g == 0 {
        let (base, metal, rough, normal) = (e(0), e(1)[0], e(2)[0], e(3));
        [
            [base[0], base[1], base[2], metal],
            [normal[0], normal[1], normal[2], rough],
            [e(4)[0], k, k, e(6)[0]],
            [e(5)[0], e(5)[1], e(5)[2], 1.0],
        ]
    } else {
        let (a, b) = (e(0), e(1));
        [
            [a[0], a[1], a[2], e(2)[0]],
            [b[0], b[1], b[2], e(3)[0]],
            [e(4)[0], e(5)[0], e(6)[0], 0.0],
            [k, k, 0.0, 0.0],
        ]
    }
}

/// A pass writing `dst`, cleared to group `g`'s empty values outside the islands
/// (`inside`: as inside, for the 1×1 default sets).
fn begin<'a>(
    enc: &'a mut wgpu::CommandEncoder,
    dst: &'a ChannelSet,
    g: usize,
    inside: bool,
) -> wgpu::RenderPass<'a> {
    let clears = group_values(g, inside).map(|[r, gr, b, a]| wgpu::Color {
        r: r as f64,
        g: gr as f64,
        b: b as f64,
        a: a as f64,
    });
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
        ProjectionMode::Spherical => 3.0,
        ProjectionMode::Cylindrical => 4.0,
        ProjectionMode::Warp => 5.0,
        ProjectionMode::UvSet2 => 6.0,
    };
    let [px, py, pz] = p.position;
    let mirror = p
        .mirror
        .iter()
        .enumerate()
        .map(|(i, &m)| if m { 1 << i } else { 0 })
        .sum::<u32>() as f32;
    let w = &p.warp;
    ProjUniform {
        a: [
            mode,
            p.rotation_deg.to_radians(),
            1.0 + p.hardness * 15.0,
            1.0 / p.size.max(1e-4),
        ],
        so: [p.scale[0], p.scale[1], p.offset[0], p.offset[1]],
        origin: [px, py, pz, mirror],
        r0: row(0),
        r1: row(1),
        r2: row(2),
        warp: std::array::from_fn(|k| [w[2 * k][0], w[2 * k][1], w[2 * k + 1][0], w[2 * k + 1][1]]),
    }
}

/// How a bitmap's values become linear texture values.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Decode {
    Raw,
    Srgb,
    /// Through the OCIO config, from this color space to the working space.
    Ocio(String),
}

/// Uploads a float image (as rgba16f) with a box-filtered mip chain.
fn upload_rgba32f(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    img: &image::Rgba32FImage,
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
    let data: Vec<half::f16> = mips
        .iter()
        .flat_map(|m| m.as_raw().iter().map(|&v| half::f16::from_f32(v)))
        .collect();
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("bitmap (OCIO)"),
                size: wgpu::Extent3d {
                    width: img.width(),
                    height: img.height(),
                    depth_or_array_layers: 1,
                },
                mip_level_count: mips.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::MipMajor,
            bytemuck::cast_slice(&data),
        )
        .create_view(&Default::default())
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
            label: Some(["set t0", "set t1", "set t2", "set t3"][i]),
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
    let views: [wgpu::TextureView; SET_TEXTURES] =
        std::array::from_fn(|i: usize| textures[i].create_view(&Default::default()));
    let read = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: set_layout,
        entries: &[0, 1, 2, 3].map(|i| wgpu::BindGroupEntry {
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

fn mask_fx(e: &reetched_doc::MaskEffect) -> MaskFx {
    let blend = MaskBlend::ALL
        .iter()
        .position(|&b| b == e.blend)
        .unwrap_or(0) as f32;
    let mut fx = MaskFx::default();
    match &e.kind {
        MaskEffectKind::Generator(g) => {
            let kind = GeneratorKind::ALL
                .iter()
                .position(|&k| k == g.kind)
                .unwrap_or(0) as f32;
            fx.head = [1.0, blend, e.opacity, 0.0];
            fx.a = [g.level, g.contrast, g.noise, g.noise_scale];
            fx.b = [kind, g.axis as f32, if g.invert { 1.0 } else { 0.0 }, 0.0];
        }
        MaskEffectKind::Levels {
            in_min,
            in_max,
            gamma,
            out_min,
            out_max,
        } => {
            fx.head = [2.0, blend, e.opacity, 0.0];
            fx.a = [*in_min, *in_max, *gamma, *out_min];
            fx.b = [*out_max, 0.0, 0.0, 0.0];
        }
        MaskEffectKind::ColorSelection { colors, tolerance } => {
            fx.head = [3.0, blend, e.opacity, 0.0];
            fx.b = [0.0, 0.0, 0.0, *tolerance];
            for (dst, c) in fx.colors.iter_mut().zip(colors) {
                *dst = [c[0], c[1], c[2], 1.0];
            }
        }
    }
    fx
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
        let tiles_per_side = res.div_ceil(TILE_SIZE);
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
                // Textures under 256 px use the top-left part of their one tile.
                let (w, h) = (
                    TILE_SIZE.min(res - tx * TILE_SIZE),
                    TILE_SIZE.min(res - ty * TILE_SIZE),
                );
                if w == TILE_SIZE && h == TILE_SIZE {
                    write(tx * TILE_SIZE, ty * TILE_SIZE, w, h, &bytes);
                } else {
                    let rows: Vec<u8> = (0..h as usize)
                        .flat_map(|y| {
                            let at = y * TILE_SIZE as usize * 8;
                            bytes[at..at + w as usize * 8].iter().copied()
                        })
                        .collect();
                    write(tx * TILE_SIZE, ty * TILE_SIZE, w, h, &rows);
                }
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
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .ok()
    }

    /// The UV quad, with positions `pos(uv)` along X and Y.
    fn quad_at(device: &wgpu::Device, pos: fn(f32) -> f32) -> (wgpu::Buffer, wgpu::Buffer) {
        let v = |u: f32, w: f32| Vertex {
            position: [pos(u), pos(w), 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [u, w],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv1: [w, u],
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
        run_at(stack, resources, |u| u)
    }

    fn run_at(stack: Vec<Node>, resources: &Resources, pos: fn(f32) -> f32) -> Option<ChannelData> {
        let mut set = TextureSet::new("t".into(), stack);
        set.resolution = RES;
        run_set(&set, resources, pos)
    }

    fn run_set(
        set: &TextureSet,
        resources: &Resources,
        pos: fn(f32) -> f32,
    ) -> Option<ChannelData> {
        let _gpu = crate::gpu_test_lock();
        let (device, queue) = gpu()?;
        let (vb, ib) = quad_at(&device, pos);
        let mesh = MeshPart {
            vertices: &vb,
            indices: &ib,
            range: 0..6,
            scale: 1.0,
            tile: reetched_doc::FIRST_UDIM,
        };
        let mut comp = Compositor::new(&device, &queue);
        let mut out = GroupSets::default();
        let warnings = comp.composite(&device, &queue, &mesh, set, resources, &mut out);
        assert!(warnings.is_empty(), "{warnings:?}");
        Some(comp.read_back(&device, &queue, &out).unwrap())
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
            None,
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

    #[test]
    fn planar_projection_mirrors_across_x() {
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
        let layer = |mirror: bool| {
            let mut doc = Document::new(None, &[]);
            let mut fill = Fill::new([1.0; 3], 0.0, 0.5);
            fill.channels.get_mut(&Channel::BaseColor).unwrap().bitmap = Some(id.clone());
            fill.projection.mode = ProjectionMode::Planar;
            fill.projection.size = 2.0; // the quad spans -1..1
            fill.projection.mirror = [mirror, false, false];
            doc.new_fill("img".into(), fill)
        };
        let to_pm1 = |u: f32| u * 2.0 - 1.0;
        let Some(plain) = run_at(vec![layer(false)], &resources, to_pm1) else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let mirrored = run_at(vec![layer(true)], &resources, to_pm1).unwrap();
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.06);
        // Left half, top: red normally, green (the right half's color) when mirrored.
        assert!(near(plain.base_color[at(&plain, 16, 16)], [1.0, 0.0, 0.0]));
        let m = mirrored.base_color[at(&mirrored, 16, 16)];
        assert!(near(m, [0.0, 1.0, 0.0]), "{m:?}");
        // The right half is unchanged.
        assert!(near(
            mirrored.base_color[at(&mirrored, 48, 16)],
            [0.0, 1.0, 0.0]
        ));
    }

    #[test]
    fn emissive_and_opacity_composite() {
        let mut doc = Document::new(None, &[]);
        let mut fill = Fill::new([1.0; 3], 0.0, 0.5);
        for (c, v) in [
            (Channel::Emissive, [1.0, 0.5, 0.0]),
            (Channel::Opacity, [0.3; 3]),
        ] {
            let src = fill.channels.get_mut(&c).unwrap();
            src.enabled = true;
            src.value = v;
        }
        let layer = doc.new_fill("glow".into(), fill);
        let Some(d) = run(vec![layer], &Resources::default()) else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let i = at(&d, 32, 32);
        assert!(close(d.emissive[i], [1.0, 0.5, 0.0]), "{:?}", d.emissive[i]);
        assert!((d.opacity[i] - 0.3).abs() < 1e-3, "{}", d.opacity[i]);
        // An empty stack is opaque and not emissive.
        let d = run(vec![], &Resources::default()).unwrap();
        let i = at(&d, 32, 32);
        assert_eq!((d.emissive[i], d.opacity[i]), ([0.0; 3], 1.0));
    }

    #[test]
    fn bitmap_color_space_controls_decoding() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([128, 128, 128, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut resources = Resources::default();
        let id = resources.add(png);
        let base = |cs: reetched_doc::ColorSpace| {
            let mut doc = Document::new(None, &[]);
            let mut fill = Fill::new([1.0; 3], 0.0, 0.5);
            let src = fill.channels.get_mut(&Channel::BaseColor).unwrap();
            src.bitmap = Some(id.clone());
            src.color_space = cs;
            let d = run(vec![doc.new_fill("img".into(), fill)], &resources)?;
            Some(d.base_color[at(&d, 32, 32)][0])
        };
        let Some(srgb) = base(reetched_doc::ColorSpace::Auto) else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let raw = base(reetched_doc::ColorSpace::Linear).unwrap();
        assert!(
            (srgb - 0.216).abs() < 0.01,
            "sRGB 128 → linear 0.216: {srgb}"
        );
        assert!((raw - 0.502).abs() < 0.01, "raw 128 → 0.502: {raw}");
    }

    /// A 2×2 PNG: top row red, green; bottom row blue, white.
    fn quadrants(resources: &mut Resources) -> reetched_doc::ResourceId {
        let mut img = image::RgbaImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
        img.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        img.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        resources.add(png)
    }

    #[test]
    fn extra_channels_composite_in_their_groups() {
        let mut doc = Document::new(None, &[]);
        let mut fill = Fill::new([1.0; 3], 0.0, 0.5);
        for (c, v) in [
            (Channel::SpecularLevel, [0.8; 3]),
            (Channel::CoatColor, [1.0, 0.0, 0.0]),
            (Channel::Sheen, [0.6; 3]),
        ] {
            let src = fill.channels.get_mut(&c).unwrap();
            src.enabled = true;
            src.value = v;
        }
        let mut set = TextureSet::new("t".into(), vec![doc.new_fill("rich".into(), fill)]);
        set.resolution = RES;
        set.extra_channels = vec![
            Channel::SpecularLevel,
            Channel::CoatColor,
            Channel::Sheen,
            Channel::Coat,
        ];
        let Some(d) = run_set(&set, &Resources::default(), |u| u) else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let i = at(&d, 32, 32);
        let get = |c: Channel| d.extra.get(&c).unwrap_or_else(|| panic!("{c:?} read back"))[i];
        assert!((get(Channel::SpecularLevel)[0] - 0.8).abs() < 0.01);
        assert!(close(get(Channel::CoatColor), [1.0, 0.0, 0.0]));
        assert!((get(Channel::Sheen)[0] - 0.6).abs() < 0.01);
        // Untouched channels of a composited group keep their empty value.
        assert_eq!(get(Channel::Coat)[0], 0.0);
        assert!(close(get(Channel::SpecularColor), [1.0; 3]));
        // Group 2 is composited for Coat/CoatColor/Sheen, group 1 for SpecularLevel.
        assert!(d.extra.contains_key(&Channel::Transmission));
        // The core channels are unaffected.
        assert!(close(d.base_color[i], [1.0; 3]));
    }

    #[test]
    fn warp_and_second_uv_set_projections() {
        let mut resources = Resources::default();
        let id = quadrants(&mut resources);
        let layer = |mode: ProjectionMode, warp: Option<[[f32; 2]; 16]>| {
            let mut doc = Document::new(None, &[]);
            let mut fill = Fill::new([1.0; 3], 0.0, 0.5);
            fill.channels.get_mut(&Channel::BaseColor).unwrap().bitmap = Some(id.clone());
            fill.projection.mode = mode;
            if let Some(w) = warp {
                fill.projection.warp = w;
            }
            doc.new_fill("img".into(), fill)
        };
        let Some(plain) = run(vec![layer(ProjectionMode::Uv, None)], &resources) else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.06);
        // The identity lattice changes nothing.
        let warped = run(vec![layer(ProjectionMode::Warp, None)], &resources).unwrap();
        for (x, y) in [(16, 16), (48, 16), (16, 48), (48, 48)] {
            assert!(
                near(
                    warped.base_color[at(&warped, x, y)],
                    plain.base_color[at(&plain, x, y)]
                ),
                "({x}, {y})"
            );
        }
        // Squeeze the lattice into the left half: the whole image lands there, so
        // the left quarter shows what was the image's left half (red at the top).
        let mut squeezed = reetched_doc::warp_identity();
        for p in &mut squeezed {
            p[0] *= 0.5;
        }
        let s = run(
            vec![layer(ProjectionMode::Warp, Some(squeezed))],
            &resources,
        )
        .unwrap();
        assert!(near(s.base_color[at(&s, 8, 16)], [1.0, 0.0, 0.0]));
        assert!(
            near(s.base_color[at(&s, 24, 16)], [0.0, 1.0, 0.0]),
            "{:?}",
            s.base_color[at(&s, 24, 16)]
        );
        // The test quad's second UV set is the first one transposed: top-left
        // samples what is bottom-right in UV set 1.
        let t = run(vec![layer(ProjectionMode::UvSet2, None)], &resources).unwrap();
        assert!(near(plain.base_color[at(&plain, 16, 16)], [1.0, 0.0, 0.0]));
        assert!(
            near(t.base_color[at(&t, 16, 16)], [1.0; 3]),
            "{:?}",
            t.base_color[at(&t, 16, 16)]
        );
    }

    #[test]
    fn udim_tiles_composite_their_own_paint() {
        let _gpu = crate::gpu_test_lock();
        let Some((device, queue)) = gpu() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        // The unit quad moved to tile 1002 (u in 1..2).
        let v = |u: f32, w: f32| Vertex {
            position: [u, w, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [u + 1.0, w],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv1: [0.0; 2],
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
        let vb = buf(bytemuck::cast_slice(&vertices), wgpu::BufferUsages::VERTEX);
        let ib = buf(bytemuck::cast_slice(&indices), wgpu::BufferUsages::INDEX);
        let mesh = MeshPart {
            vertices: &vb,
            indices: &ib,
            range: 0..6,
            scale: 1.0,
            tile: 1002,
        };
        // Paint on tile 1002 only: one red tile at a smaller tile resolution.
        let mut resources = Resources::default();
        let mut doc = Document::new(None, &["t".to_string()]);
        let mut set = doc.texture_sets[0].clone();
        set.resolution = 128;
        set.udim = vec![
            reetched_doc::UdimTile {
                number: 1001,
                name: String::new(),
                resolution: None,
            },
            reetched_doc::UdimTile {
                number: 1002,
                name: "Head".into(),
                resolution: Some(RES),
            },
        ];
        let red = TileMap::from_fn(RES, &mut resources, |_, _| [1.0, 0.0, 0.0, 1.0]);
        let NodeKind::Paint(paint) = &mut set.stack[0].kind else {
            panic!("paint layer")
        };
        paint.set_map(1002, Channel::BaseColor, red);
        let mut comp = Compositor::new(&device, &queue);
        let mut out = GroupSets::default();
        let warnings = comp.composite(&device, &queue, &mesh, &set, &resources, &mut out);
        assert!(warnings.is_empty(), "{warnings:?}");
        let d = comp.read_back(&device, &queue, &out).unwrap();
        assert_eq!(d.width, RES, "the tile's own resolution");
        let i = at(&d, 32, 32);
        assert!(d.coverage[i], "the triangles land inside the tile");
        assert!(
            close(d.base_color[i], [1.0, 0.0, 0.0]),
            "{:?}",
            d.base_color[i]
        );
        // A layer masked to tile 1001 doesn't show here.
        set.stack[0].udim_mask = vec![1001];
        let mut out = GroupSets::default();
        comp.composite(&device, &queue, &mesh, &set, &resources, &mut out);
        let d = comp.read_back(&device, &queue, &out).unwrap();
        assert!(close(d.base_color[at(&d, 32, 32)], [0.214; 3]));
        let _ = doc.new_paint("x".into());
    }
}
