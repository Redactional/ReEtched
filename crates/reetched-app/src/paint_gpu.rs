//! Brush strokes on the GPU (see `shaders/paint.wgsl`): dabs accumulate into a
//! stroke mask that is blended into the target paint textures; when the stroke
//! ends, the touched tiles are read back into the document. A stroke can cross
//! UDIM tiles: each tile gets its own mask and snapshots once the brush reaches it.

use crate::compositor::{Compositor, MeshPart, PAINT_FORMAT, PaintKey, VERTEX_ATTRIBUTES};
use glam::{Mat4, Vec3};
use reetched_doc::{NodeId, Resources, TILE_BYTES, TILE_SIZE, TextureSet, TileMap};
use std::collections::BTreeMap;
use wgpu::util::DeviceExt;

pub const MAX_DABS: usize = 64;
const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

/// One brush stamp (layout matches `Dab` in paint.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Dab {
    pub center_radius: [f32; 4],
    pub right_flow: [f32; 4],
    pub up_angle: [f32; 4],
    pub view_flags: [f32; 4],
    /// xy: UV center, z: UV radius, w: ribbon arc length (brush radii along the path)
    pub uv: [f32; 4],
}

/// `Dab::view_flags.w` bits.
pub const DAB_DEPTH_TEST: u32 = 1;
pub const DAB_UV_SPACE: u32 = 2;
/// Ribbon: the alpha runs along the path instead of being stamped.
pub const DAB_RIBBON: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SplatUniform {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    params: [f32; 4],
    dabs: [Dab; MAX_DABS],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ApplyUniform {
    color: [f32; 4],
    params: [f32; 4],
    /// x: 1 = copy from an offset (clone, smudge), 2 = projected; yz: offset in texels.
    extra: [f32; 4],
}

/// What a stroke does to the paint it covers.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum StrokeMode {
    Paint,
    Erase,
    /// Copies paint from `offset` texels away, as it was when the stroke began.
    Clone {
        offset: [f32; 2],
    },
    /// Drags paint along with the brush.
    Smudge,
    /// Paints the Projection tool's image into targets marked `projected`.
    Project,
}

/// Where the Projection tool's image sits over the 3D view.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ProjectUniform {
    pub view_proj: [[f32; 4]; 4],
    /// xy: image center (NDC), zw: half size (NDC)
    pub place: [f32; 4],
    /// x: cos, y: sin of the rotation, z: 1 = depth test
    pub rot: [f32; 4],
}

/// The view the dabs were placed from (for the occlusion test).
pub struct Camera {
    pub view_proj: Mat4,
    pub eye: Vec3,
}

pub struct BrushShape {
    /// 0 = soft, 1 = hard edge.
    pub hardness: f32,
    /// Texels facing away more than this are skipped; `None` paints backfaces too.
    pub backface_cos: Option<f32>,
    pub alpha: Option<wgpu::TextureView>,
}

/// A paint buffer a stroke writes into (in every UDIM tile it reaches).
pub struct StrokeTarget {
    /// (layer, channel index or `MASK_SLOT`)
    pub key: (NodeId, u8),
    /// Brush value for this buffer (linear color, normal, or scalar in x).
    pub value: [f32; 3],
    pub is_normal: bool,
    /// Projection tool: this buffer takes the projected image's colors.
    pub projected: bool,
    /// The buffer's tiles per UDIM tile when the stroke started (missing = empty).
    pub old: BTreeMap<u32, TileMap>,
}

impl StrokeTarget {
    fn paint_key(&self, tile: u32) -> PaintKey {
        (self.key.0, self.key.1, tile)
    }
}

/// The part of a stroke on one UDIM tile.
struct TileStroke {
    res: u32,
    mask: wgpu::Texture,
    mask_view: wgpu::TextureView,
    /// Smudge: this frame's dabs only.
    frame_mask: Option<wgpu::TextureView>,
    /// Each target's paint ("before"), index-aligned with `Stroke::targets`: from
    /// when the brush reached the tile, or for smudge from the start of the frame.
    before: Vec<(wgpu::Texture, wgpu::TextureView)>,
    /// Smudge: motion not yet applied because it was under a texel.
    carry: [f32; 2],
    /// Projection tool: the image as seen on each texel.
    projected: Option<wgpu::TextureView>,
}

struct Stroke {
    mode: StrokeMode,
    opacity: f32,
    targets: Vec<StrokeTarget>,
    /// Texture size per UDIM tile.
    resolution: BTreeMap<u32, u32>,
    /// Fallback size for tiles not listed.
    default_res: u32,
    tiles: BTreeMap<u32, TileStroke>,
    /// Projection tool: the image and where it sits.
    projection: Option<(wgpu::TextureView, ProjectUniform)>,
}

pub struct PaintGpu {
    splat: wgpu::RenderPipeline,
    /// Ribbons: the same splat, but texels keep the strongest dab (no build-up).
    ribbon: wgpu::RenderPipeline,
    /// Fills whole triangles into the stroke mask (Polygon Fill).
    fill: wgpu::RenderPipeline,
    /// Projects the Projection tool's image into texture space.
    project: wgpu::RenderPipeline,
    apply: wgpu::RenderPipeline,
    splat_layout: wgpu::BindGroupLayout,
    apply_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Bound when there's no 3D view depth (2D-view painting).
    no_depth: wgpu::TextureView,
    white: wgpu::TextureView,
    stroke: Option<Stroke>,
}

impl PaintGpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        use wgpu::{BindingType as B, ShaderStages as S};
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        };
        let uniform = B::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let tex = |filterable| B::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let splat_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splat"),
            entries: &[
                entry(0, S::FRAGMENT, uniform),
                entry(
                    1,
                    S::FRAGMENT,
                    B::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(2, S::FRAGMENT, tex(true)),
                entry(
                    3,
                    S::FRAGMENT,
                    B::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                entry(4, S::VERTEX, uniform),
            ],
        });
        let apply_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("apply"),
            entries: &[
                entry(0, S::FRAGMENT, uniform),
                entry(1, S::FRAGMENT, tex(false)),
                entry(2, S::FRAGMENT, tex(false)),
                entry(3, S::FRAGMENT, tex(false)),
            ],
        });
        let module = device.create_shader_module(wgpu::include_wgsl!("shaders/paint.wgsl"));
        let pipeline = |layout: &wgpu::BindGroupLayout,
                        vs: &str,
                        fs: &str,
                        buffers: &[Option<wgpu::VertexBufferLayout>],
                        target: wgpu::ColorTargetState| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(fs),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(target)],
                }),
                multiview_mask: None,
                cache: crate::gpu_cache::get(),
            })
        };
        let accumulate = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::OneMinusDst,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let strongest = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Max,
        };
        let vertex_layout = [Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<reetched_io::Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &VERTEX_ATTRIBUTES[..4],
        })];
        let mask_target = |blend| wgpu::ColorTargetState {
            format: MASK_FORMAT,
            blend: Some(wgpu::BlendState {
                color: blend,
                alpha: blend,
            }),
            write_mask: wgpu::ColorWrites::ALL,
        };
        let splat = pipeline(
            &splat_layout,
            "vs_uv",
            "fs_splat",
            &vertex_layout,
            mask_target(accumulate),
        );
        let ribbon = pipeline(
            &splat_layout,
            "vs_uv",
            "fs_splat",
            &vertex_layout,
            mask_target(strongest),
        );
        let fill = pipeline(
            &splat_layout,
            "vs_uv",
            "fs_fill",
            &vertex_layout,
            mask_target(accumulate),
        );
        let project = pipeline(
            &splat_layout,
            "vs_uv",
            "fs_project",
            &vertex_layout,
            PAINT_FORMAT.into(),
        );
        let apply = pipeline(
            &apply_layout,
            "vs_full",
            "fs_apply",
            &[],
            PAINT_FORMAT.into(),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("brush alpha"),
            address_mode_u: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let no_depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("no depth"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let white = device
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
            .create_view(&Default::default());
        Self {
            splat,
            ribbon,
            fill,
            project,
            apply,
            splat_layout,
            apply_layout,
            sampler,
            no_depth,
            white,
            stroke: None,
        }
    }

    /// Starts a stroke on `targets` in texture set `set`. Nothing is touched on the
    /// GPU until the brush reaches a tile.
    pub fn begin(
        &mut self,
        targets: Vec<StrokeTarget>,
        set: &TextureSet,
        mode: StrokeMode,
        opacity: f32,
    ) {
        let mut resolution: BTreeMap<u32, u32> = set
            .udim
            .iter()
            .map(|t| (t.number, set.tile_resolution(t.number)))
            .collect();
        resolution
            .entry(reetched_doc::FIRST_UDIM)
            .or_insert(set.resolution);
        self.stroke = Some(Stroke {
            mode,
            opacity,
            targets,
            resolution,
            default_res: set.resolution,
            tiles: BTreeMap::new(),
            projection: None,
        });
    }

    /// A stroke is in progress (its paint textures hold texels not yet read back).
    pub fn active(&self) -> bool {
        self.stroke.is_some()
    }

    /// Projection tool: the image this stroke paints and where it sits on screen.
    pub fn set_projection(&mut self, image: wgpu::TextureView, u: ProjectUniform) {
        if let Some(s) = &mut self.stroke {
            s.projection = Some((image, u));
        }
    }

    /// The splat bind group for tile `mesh` with uniform `contents`.
    fn splat_group(
        &self,
        device: &wgpu::Device,
        contents: &[u8],
        depth: Option<&wgpu::TextureView>,
        alpha: Option<&wgpu::TextureView>,
        mesh: &MeshPart,
    ) -> wgpu::BindGroup {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("splat"),
            contents,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let [u, v] = mesh.uv_origin();
        let tile = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tile"),
            contents: bytemuck::bytes_of(&[u, v, 0.0f32, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.splat_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(depth.unwrap_or(&self.no_depth)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(alpha.unwrap_or(&self.white)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: tile.as_entire_binding(),
                },
            ],
        })
    }

    /// Sets up the stroke on tile `mesh.tile` the first time the brush reaches it:
    /// syncs and snapshots each target's paint, clears the masks, projects the image.
    fn ensure_tile(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        comp: &mut Compositor,
        mesh: &MeshPart,
        depth: Option<&wgpu::TextureView>,
        resources: &Resources,
    ) {
        let tile = mesh.tile;
        let Some(stroke) = &self.stroke else { return };
        if stroke.tiles.contains_key(&tile) {
            return;
        }
        let res = stroke
            .resolution
            .get(&tile)
            .copied()
            .unwrap_or(stroke.default_res);
        let mut enc = device.create_command_encoder(&Default::default());
        let empty = TileMap::new(res);
        let before = stroke
            .targets
            .iter()
            .map(|t| {
                let key = t.paint_key(tile);
                let old = t.old.get(&tile).unwrap_or(&empty);
                comp.paint_view(device, queue, key, old, res, resources);
                let src = &comp.paint_texture(key).expect("just created").texture;
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("stroke before"),
                    size: src.size(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: PAINT_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                enc.copy_texture_to_texture(
                    src.as_image_copy(),
                    texture.as_image_copy(),
                    src.size(),
                );
                let view = texture.create_view(&Default::default());
                (texture, view)
            })
            .collect();
        let new_mask = || {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("stroke mask"),
                size: wgpu::Extent3d {
                    width: res,
                    height: res,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: MASK_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let mask = new_mask();
        let mask_view = mask.create_view(&Default::default());
        let frame_mask = (stroke.mode == StrokeMode::Smudge)
            .then(|| new_mask().create_view(&Default::default()));
        drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear stroke mask"),
            color_attachments: &[Some(attachment(
                &mask_view,
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            ))],
            ..Default::default()
        }));
        // Projection tool: the image as it lands on this tile's texels.
        let projected = stroke.projection.as_ref().map(|(image, u)| {
            let view = device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("projected image"),
                    size: wgpu::Extent3d {
                        width: res,
                        height: res,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: PAINT_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            let mut u = *u;
            u.rot[2] = if depth.is_some() { 1.0 } else { 0.0 };
            let group = self.splat_group(device, bytemuck::bytes_of(&u), depth, Some(image), mesh);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("project image"),
                color_attachments: &[Some(attachment(
                    &view,
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                ))],
                ..Default::default()
            });
            pass.set_pipeline(&self.project);
            pass.set_bind_group(0, &group, &[]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(mesh.range.clone(), 0, 0..1);
            drop(pass);
            view
        });
        queue.submit([enc.finish()]);
        let ts = TileStroke {
            res,
            mask,
            mask_view,
            frame_mask,
            before,
            carry: [0.0; 2],
            projected,
        };
        if let Some(stroke) = &mut self.stroke {
            stroke.tiles.insert(tile, ts);
        }
    }

    /// Stamps `dabs` on tile `mesh` and updates its paint textures. `motion`: smudge
    /// only, how far the brush moved this frame, in texels.
    #[allow(clippy::too_many_arguments)]
    pub fn dabs(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        comp: &mut Compositor,
        mesh: &MeshPart,
        camera: &Camera,
        depth: Option<&wgpu::TextureView>,
        shape: &BrushShape,
        dabs: &[Dab],
        motion: [f32; 2],
        resources: &Resources,
    ) {
        if dabs.is_empty() || self.stroke.is_none() {
            return;
        }
        self.ensure_tile(device, queue, comp, mesh, depth, resources);
        let mut enc = device.create_command_encoder(&Default::default());
        // Smudge: drag from the paint as it is now, by whole texels.
        let mut offset = [0.0; 2];
        if let Some(stroke) = &mut self.stroke
            && let Some(ts) = stroke.tiles.get_mut(&mesh.tile)
            && ts.frame_mask.is_some()
        {
            for (k, c) in ts.carry.iter_mut().enumerate() {
                *c += motion[k];
                offset[k] = -c.round();
                *c += offset[k];
            }
        }
        let Some(stroke) = &self.stroke else { return };
        let Some(ts) = stroke.tiles.get(&mesh.tile) else {
            return;
        };
        if let Some(frame) = &ts.frame_mask {
            for (target, (before, _)) in stroke.targets.iter().zip(&ts.before) {
                if let Some(dst) = comp.paint_texture(target.paint_key(mesh.tile)) {
                    enc.copy_texture_to_texture(
                        dst.texture.as_image_copy(),
                        before.as_image_copy(),
                        before.size(),
                    );
                }
            }
            drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear frame mask"),
                color_attachments: &[Some(attachment(
                    frame,
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                ))],
                ..Default::default()
            }));
        }
        let ribbon = dabs
            .first()
            .is_some_and(|d| d.view_flags[3] as u32 & DAB_RIBBON != 0);
        let groups: Vec<wgpu::BindGroup> = dabs
            .chunks(MAX_DABS)
            .map(|chunk| {
                let mut u: SplatUniform = bytemuck::Zeroable::zeroed();
                u.view_proj = camera.view_proj.to_cols_array_2d();
                u.inv_view_proj = camera.view_proj.inverse().to_cols_array_2d();
                u.eye = camera.eye.extend(1.0).into();
                u.params = [
                    chunk.len() as f32,
                    shape.hardness,
                    shape.backface_cos.unwrap_or(-2.0),
                    if shape.alpha.is_some() { 1.0 } else { 0.0 },
                ];
                for (dst, src) in u.dabs.iter_mut().zip(chunk) {
                    *dst = *src;
                    if depth.is_none() {
                        // No depth to test against: skip the occlusion test.
                        let flags = dst.view_flags[3] as u32 & !DAB_DEPTH_TEST;
                        dst.view_flags[3] = flags as f32;
                    }
                }
                self.splat_group(
                    device,
                    bytemuck::bytes_of(&u),
                    depth,
                    shape.alpha.as_ref(),
                    mesh,
                )
            })
            .collect();
        for mask in std::iter::once(&ts.mask_view).chain(ts.frame_mask.as_ref()) {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("splat"),
                color_attachments: &[Some(attachment(mask, wgpu::LoadOp::Load))],
                ..Default::default()
            });
            pass.set_pipeline(if ribbon { &self.ribbon } else { &self.splat });
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            for group in &groups {
                pass.set_bind_group(0, group, &[]);
                pass.draw_indexed(mesh.range.clone(), 0, 0..1);
            }
        }
        self.apply(device, &mut enc, comp, mesh.tile, offset);
        queue.submit([enc.finish()]);
    }

    /// Fills whole triangles (`indices`: three vertex indices each) of tile `mesh`
    /// with the stroke's value, at the stroke's opacity.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_triangles(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        comp: &mut Compositor,
        mesh: &MeshPart,
        indices: &[u32],
        depth: Option<&wgpu::TextureView>,
        resources: &Resources,
    ) {
        if indices.is_empty() || self.stroke.is_none() {
            return;
        }
        self.ensure_tile(device, queue, comp, mesh, depth, resources);
        let Some(ts) = self.stroke.as_ref().and_then(|s| s.tiles.get(&mesh.tile)) else {
            return;
        };
        let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("polygon fill"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let zero: SplatUniform = bytemuck::Zeroable::zeroed();
        let group = self.splat_group(device, bytemuck::bytes_of(&zero), None, None, mesh);
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fill triangles"),
                color_attachments: &[Some(attachment(&ts.mask_view, wgpu::LoadOp::Load))],
                ..Default::default()
            });
            pass.set_pipeline(&self.fill);
            pass.set_bind_group(0, &group, &[]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
        }
        self.apply(device, &mut enc, comp, mesh.tile, [0.0; 2]);
        queue.submit([enc.finish()]);
    }

    /// Blends the stroke into every target of `tile`: before + mask → paint texture.
    /// `offset`: smudge's copy offset this frame (texels).
    fn apply(
        &self,
        device: &wgpu::Device,
        enc: &mut wgpu::CommandEncoder,
        comp: &Compositor,
        tile: u32,
        offset: [f32; 2],
    ) {
        let Some(stroke) = &self.stroke else { return };
        let Some(ts) = stroke.tiles.get(&tile) else {
            return;
        };
        let (copy, offset, mask) = match stroke.mode {
            StrokeMode::Clone { offset } => (1.0, offset, &ts.mask_view),
            StrokeMode::Smudge => (1.0, offset, ts.frame_mask.as_ref().unwrap_or(&ts.mask_view)),
            _ => (0.0, [0.0; 2], &ts.mask_view),
        };
        for (target, (_, before)) in stroke.targets.iter().zip(&ts.before) {
            let Some(dst) = comp.paint_texture(target.paint_key(tile)) else {
                continue;
            };
            let [r, g, b] = target.value;
            let project =
                stroke.mode == StrokeMode::Project && target.projected && ts.projected.is_some();
            let u = ApplyUniform {
                color: [r, g, b, if target.is_normal { 1.0 } else { 0.0 }],
                params: [
                    stroke.opacity,
                    if stroke.mode == StrokeMode::Erase {
                        1.0
                    } else {
                        0.0
                    },
                    0.0,
                    0.0,
                ],
                extra: [if project { 2.0 } else { copy }, offset[0], offset[1], 0.0],
            };
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("apply"),
                contents: bytemuck::bytes_of(&u),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.apply_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(before),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(mask),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(
                            ts.projected.as_ref().unwrap_or(&self.white),
                        ),
                    },
                ],
            });
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("apply stroke"),
                color_attachments: &[Some(attachment(&dst.view, wgpu::LoadOp::Load))],
                ..Default::default()
            });
            pass.set_pipeline(&self.apply);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Ends the stroke: reads the touched tiles back and returns each target's new
    /// tiles, per UDIM tile the stroke reached.
    pub fn end(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        comp: &mut Compositor,
        resources: &mut Resources,
    ) -> anyhow::Result<Vec<(PaintKey, TileMap)>> {
        let Some(stroke) = self.stroke.take() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for (&tile, ts) in &stroke.tiles {
            let res = ts.res;
            let per_side = res.div_ceil(TILE_SIZE);
            // Which paint tiles did the stroke touch?
            let mask_bytes = read_region(device, queue, &ts.mask, 0, 0, res, res, 2)?;
            let row = (res * 2) as usize;
            let mut touched = Vec::new();
            for ty in 0..per_side {
                for tx in 0..per_side {
                    let (w, h) = (
                        TILE_SIZE.min(res - tx * TILE_SIZE),
                        TILE_SIZE.min(res - ty * TILE_SIZE),
                    );
                    let hit = (0..h).any(|y| {
                        let start =
                            (ty * TILE_SIZE + y) as usize * row + (tx * TILE_SIZE * 2) as usize;
                        mask_bytes[start..start + (w * 2) as usize]
                            .iter()
                            .any(|&b| b != 0)
                    });
                    if hit {
                        touched.push((tx, ty));
                    }
                }
            }
            for target in &stroke.targets {
                let key = target.paint_key(tile);
                let mut map = target
                    .old
                    .get(&tile)
                    .cloned()
                    .unwrap_or_else(|| TileMap::new(res));
                map.resolution = res;
                let Some(pt) = comp.paint_texture(key) else {
                    continue;
                };
                for &(tx, ty) in &touched {
                    let (w, h) = (
                        TILE_SIZE.min(res - tx * TILE_SIZE),
                        TILE_SIZE.min(res - ty * TILE_SIZE),
                    );
                    let region = read_region(
                        device,
                        queue,
                        &pt.texture,
                        tx * TILE_SIZE,
                        ty * TILE_SIZE,
                        w,
                        h,
                        8,
                    )?;
                    // Pad partial tiles (texture sets under 256 px) to a full tile.
                    let mut bytes = vec![0u8; TILE_BYTES];
                    for y in 0..h as usize {
                        let src = &region[y * w as usize * 8..(y + 1) * w as usize * 8];
                        let at = y * TILE_SIZE as usize * 8;
                        bytes[at..at + src.len()].copy_from_slice(src);
                    }
                    let key = TileMap::key(tx, ty);
                    // Alpha is the 4th half-float of every texel; ±0 means untouched.
                    let empty = bytes
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .all(|t| u16::from_le_bytes([t[6], t[7]]) & 0x7fff == 0);
                    if empty {
                        map.tiles.remove(&key);
                    } else {
                        map.tiles.insert(key, resources.add_tile(bytes));
                    }
                }
                comp.mark_synced(key, map.clone());
                out.push((key, map));
            }
        }
        Ok(out)
    }
}

fn attachment(
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load,
            store: wgpu::StoreOp::Store,
        },
    }
}

/// Copies a texture region to the CPU, rows tightly packed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn read_region(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    bytes_per_texel: u32,
) -> anyhow::Result<Vec<u8>> {
    let row = w * bytes_per_texel;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("paint readback"),
        size: (padded * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely())?;
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|e| anyhow::anyhow!("could not read GPU data: {e:?}"))?;
    let mut out = Vec::with_capacity((row * h) as usize);
    for r in 0..h as usize {
        let start = r * padded as usize;
        out.extend_from_slice(&mapped[start..start + row as usize]);
    }
    Ok(out)
}

/// Strokes on a real GPU against a quad covering the UV tile (z = 0, facing +z).
/// Skipped when no adapter is available.
#[cfg(test)]
mod tests {
    use super::*;
    use reetched_doc::{Channel, Document, NodeKind, TextureSet};
    use reetched_io::Vertex;

    const RES: u32 = 256;

    struct Gpu {
        device: wgpu::Device,
        queue: wgpu::Queue,
        vb: wgpu::Buffer,
        ib: wgpu::Buffer,
        _turn: std::sync::MutexGuard<'static, ()>,
    }

    fn gpu() -> Option<Gpu> {
        let turn = crate::gpu_test_lock();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .ok()?;
        let v = |u: f32, w: f32| Vertex {
            position: [u, w, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [u, w],
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
        Some(Gpu {
            vb: buf(bytemuck::cast_slice(&vertices), wgpu::BufferUsages::VERTEX),
            ib: buf(bytemuck::cast_slice(&indices), wgpu::BufferUsages::INDEX),
            device,
            queue,
            _turn: turn,
        })
    }

    fn uv_dab(u: f32, v: f32, radius: f32) -> Dab {
        Dab {
            right_flow: [0.0, 0.0, 0.0, 1.0],
            view_flags: [0.0, 0.0, 0.0, DAB_UV_SPACE as f32],
            uv: [u, v, radius, 0.0],
            ..Default::default()
        }
    }

    fn world_dab(center: [f32; 3], radius: f32, view_dir: [f32; 3]) -> Dab {
        let [x, y, z] = center;
        let [a, b, c] = view_dir;
        Dab {
            center_radius: [x, y, z, radius],
            right_flow: [1.0, 0.0, 0.0, 1.0],
            up_angle: [0.0, 1.0, 0.0, 0.0],
            view_flags: [a, b, c, 0.0],
            uv: [0.0; 4],
        }
    }

    /// One stroke of `dabs` on paint buffer `old`; returns the new tiles.
    fn stroke(
        g: &Gpu,
        comp: &mut Compositor,
        res: &mut Resources,
        old: TileMap,
        erase: bool,
        backface: Option<f32>,
        dabs: &[Dab],
    ) -> TileMap {
        let mode = if erase {
            StrokeMode::Erase
        } else {
            StrokeMode::Paint
        };
        run(g, comp, res, old, mode, backface, &[(dabs, [0.0; 2])], None)
    }

    /// A stroke of several frames (dabs, smudge motion), optionally ending
    /// with a polygon fill of `fill` (vertex indices).
    #[allow(clippy::too_many_arguments)]
    fn run(
        g: &Gpu,
        comp: &mut Compositor,
        res: &mut Resources,
        old: TileMap,
        mode: StrokeMode,
        backface: Option<f32>,
        frames: &[(&[Dab], [f32; 2])],
        fill: Option<&[u32]>,
    ) -> TileMap {
        let mut pg = PaintGpu::new(&g.device, &g.queue);
        let target = StrokeTarget {
            key: (7, 0),
            value: [1.0, 0.0, 0.0],
            is_normal: false,
            projected: false,
            old: BTreeMap::from([(reetched_doc::FIRST_UDIM, old)]),
        };
        let mut set = TextureSet::new("quad".into(), Vec::new());
        set.resolution = RES;
        pg.begin(vec![target], &set, mode, 1.0);
        let mesh = MeshPart {
            vertices: &g.vb,
            indices: &g.ib,
            range: 0..6,
            scale: 1.0,
            tile: reetched_doc::FIRST_UDIM,
        };
        let camera = Camera {
            view_proj: Mat4::IDENTITY,
            eye: Vec3::Z,
        };
        let shape = BrushShape {
            hardness: 1.0,
            backface_cos: backface,
            alpha: None,
        };
        for (dabs, motion) in frames {
            pg.dabs(
                &g.device, &g.queue, comp, &mesh, &camera, None, &shape, dabs, *motion, res,
            );
        }
        if let Some(indices) = fill {
            pg.fill_triangles(&g.device, &g.queue, comp, &mesh, indices, None, res);
        }
        let mut out = pg.end(&g.device, &g.queue, comp, res).unwrap();
        assert_eq!(out.len(), 1);
        out.remove(0).1
    }

    #[test]
    fn paint_composite_and_erase() {
        let Some(g) = gpu() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let mut comp = Compositor::new(&g.device, &g.queue);
        let mut resources = Resources::default();
        let painted = stroke(
            &g,
            &mut comp,
            &mut resources,
            TileMap::new(RES),
            false,
            None,
            &[uv_dab(0.5, 0.5, 0.25)],
        );
        assert_eq!(
            painted.tiles.len(),
            1,
            "one 256² tile covers the whole texture"
        );

        // Composite a stack with that paint layer.
        let mut doc = Document::new(None, &["quad".to_string()]);
        let set = &mut doc.texture_sets[0];
        set.resolution = RES;
        let NodeKind::Paint(paint) = &mut set.stack[0].kind else {
            panic!("new sets start with a paint layer")
        };
        paint.channels.insert(Channel::BaseColor, painted.clone());
        let id = set.stack[0].id;
        let set: TextureSet = set.clone();
        // The compositor caches paint by node id; this layer's texture is key (id, 0).
        let mesh = MeshPart {
            vertices: &g.vb,
            indices: &g.ib,
            range: 0..6,
            scale: 1.0,
            tile: reetched_doc::FIRST_UDIM,
        };
        let mut fresh = Compositor::new(&g.device, &g.queue);
        let mut out = crate::compositor::GroupSets::default();
        assert!(
            fresh
                .composite(&g.device, &g.queue, &mesh, &set, &resources, &mut out)
                .is_empty()
        );
        let d = fresh.read_back(&g.device, &g.queue, &out).unwrap();
        let px = |x: u32, y: u32| d.base_color[(y * RES + x) as usize];
        assert!(
            (px(128, 128)[0] - 1.0).abs() < 0.01 && px(128, 128)[1] < 0.01,
            "center is red: {:?}",
            px(128, 128)
        );
        assert!(
            (px(5, 5)[0] - 0.214).abs() < 0.01,
            "corner untouched: {:?}",
            px(5, 5)
        );
        let _ = id;

        // Erasing the same spot leaves nothing, so the tile is dropped.
        let erased = stroke(
            &g,
            &mut comp,
            &mut resources,
            painted,
            true,
            None,
            &[uv_dab(0.5, 0.5, 0.3)],
        );
        assert!(erased.tiles.is_empty(), "{erased:?}");
    }

    #[test]
    fn world_dabs_respect_backface_culling() {
        let Some(g) = gpu() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let mut comp = Compositor::new(&g.device, &g.queue);
        let mut resources = Resources::default();
        // Looking at the front (down -z): paints.
        let front = stroke(
            &g,
            &mut comp,
            &mut resources,
            TileMap::new(RES),
            false,
            Some(0.0),
            &[world_dab([0.5, 0.5, 0.0], 0.2, [0.0, 0.0, -1.0])],
        );
        assert_eq!(front.tiles.len(), 1);
        // Looking from behind with culling on: nothing.
        let mut comp = Compositor::new(&g.device, &g.queue);
        let back = stroke(
            &g,
            &mut comp,
            &mut resources,
            TileMap::new(RES),
            false,
            Some(0.0),
            &[world_dab([0.5, 0.5, 0.0], 0.2, [0.0, 0.0, 1.0])],
        );
        assert!(back.tiles.is_empty());
    }

    /// Texel at UV (u, v) (v up) of a paint buffer.
    fn texel(map: &TileMap, res: &Resources, u: f32, v: f32) -> [f32; 4] {
        let x = (u * RES as f32) as u32;
        let y = ((1.0 - v) * RES as f32) as u32;
        map.texel(res, x.min(RES - 1), y.min(RES - 1))
    }

    #[test]
    fn polygon_fill_covers_whole_triangles() {
        let Some(g) = gpu() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let mut comp = Compositor::new(&g.device, &g.queue);
        let mut r = Resources::default();
        // Triangle 0 of the quad: (0,0), (1,0), (1,1), the half below the diagonal.
        let map = run(
            &g,
            &mut comp,
            &mut r,
            TileMap::new(RES),
            StrokeMode::Paint,
            None,
            &[],
            Some(&[0, 1, 2]),
        );
        let inside = texel(&map, &r, 0.75, 0.25);
        assert!(
            inside[3] > 0.99 && (inside[0] - 1.0).abs() < 1e-3,
            "{inside:?}"
        );
        assert_eq!(texel(&map, &r, 0.25, 0.75)[3], 0.0);
    }

    #[test]
    fn clone_copies_from_the_offset_and_smudge_drags_paint() {
        let Some(g) = gpu() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let mut comp = Compositor::new(&g.device, &g.queue);
        let mut r = Resources::default();
        let spot = stroke(
            &g,
            &mut comp,
            &mut r,
            TileMap::new(RES),
            false,
            None,
            &[uv_dab(0.25, 0.5, 0.1)],
        );
        assert_eq!(texel(&spot, &r, 0.75, 0.5)[3], 0.0);

        // Clone at u = 0.75 from u = 0.25: half the texture to the left.
        let mut comp2 = Compositor::new(&g.device, &g.queue);
        let cloned = run(
            &g,
            &mut comp2,
            &mut r,
            spot.clone(),
            StrokeMode::Clone {
                offset: [-0.5 * RES as f32, 0.0],
            },
            None,
            &[(&[uv_dab(0.75, 0.5, 0.1)], [0.0; 2])],
            None,
        );
        let c = texel(&cloned, &r, 0.75, 0.5);
        assert!(c[3] > 0.99 && (c[0] - 1.0).abs() < 1e-3, "cloned: {c:?}");
        assert!(texel(&cloned, &r, 0.25, 0.5)[3] > 0.99, "source kept");

        // Smudge from the spot towards the right drags paint into empty texels.
        let mut comp3 = Compositor::new(&g.device, &g.queue);
        let step = 0.06;
        let frames: Vec<(Vec<Dab>, [f32; 2])> = (0..4)
            .map(|k| {
                let u = 0.3 + step * k as f32;
                let motion = if k == 0 { 0.0 } else { step * RES as f32 };
                (vec![uv_dab(u, 0.5, 0.06)], [motion, 0.0])
            })
            .collect();
        let frames: Vec<(&[Dab], [f32; 2])> =
            frames.iter().map(|(d, m)| (d.as_slice(), *m)).collect();
        assert_eq!(texel(&spot, &r, 0.42, 0.5)[3], 0.0, "empty before");
        let smudged = run(
            &g,
            &mut comp3,
            &mut r,
            spot,
            StrokeMode::Smudge,
            None,
            &frames,
            None,
        );
        let s = texel(&smudged, &r, 0.42, 0.5);
        assert!(s[3] > 0.3, "paint dragged right: {s:?}");
    }
}
