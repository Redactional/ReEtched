//! Headless GPU tests of the path tracer. Each skips (with a message) without an adapter.

use glam::{
    Mat4, Vec3,
    camera::rh::{proj::directx, view::look_at_mat4},
};
use half::f16;
use reetched_io::Mesh;
use reetched_trace::{FrameInputs, Materials, PathTracer};
use std::sync::{Mutex, MutexGuard};

/// GPU tests take turns: many devices at once stall some drivers.
fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("no GPU adapter, skipping");
        return None;
    };
    // Only GPUs the app runs on (it needs 23 textures per stage for its compositor);
    // CI's software OpenGL drivers fall short, and the tracer's output is wrong there.
    if adapter.limits().max_sampled_textures_per_shader_stage < 23 {
        eprintln!(
            "{} is below the app's minimum, skipping",
            adapter.get_info().name
        );
        return None;
    }
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .ok()
}

fn texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    w: u32,
    h: u32,
    texels: &[[f32; 4]],
) -> wgpu::Texture {
    let data: Vec<f16> = texels.iter().flatten().map(|&x| f16::from_f32(x)).collect();
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        tex.as_image_copy(),
        bytemuck::cast_slice(&data),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 8),
            rows_per_image: None,
        },
        tex.size(),
    );
    tex
}

/// An equirect environment from a function of the (environment frame) direction.
struct Env {
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
}

fn env(device: &wgpu::Device, queue: &wgpu::Queue, w: u32, f: impl Fn(Vec3) -> [f32; 3]) -> Env {
    let h = w / 2;
    let texels: Vec<[f32; 4]> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            let (phi, theta) = ((u - 0.5) * std::f32::consts::TAU, v * std::f32::consts::PI);
            let d = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            let [r, g, b] = f(d);
            [r, g, b, 1.0]
        })
        .collect();
    let tex = texture(device, queue, w, h, &texels);
    Env {
        view: tex.create_view(&Default::default()),
        sampler: device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        }),
    }
}

/// A uniform material for layer 0: base color, metallic, roughness, opacity.
fn material(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    base: [f32; 3],
    metal: f32,
    rough: f32,
    opacity: f32,
) -> Materials {
    let m = Materials::new(device, queue, 16, 1);
    let texels = [
        [base[0], base[1], base[2], metal],
        [0.0, 0.0, 1.0, rough],
        [0.0, 1.0, 1.0, opacity],
        [0.0; 4],
    ];
    let tex = texels.map(|t| texture(device, queue, 1, 1, &[t]));
    let views = tex.each_ref().map(|t| t.create_view(&Default::default()));
    let mut enc = device.create_command_encoder(&Default::default());
    m.update_layer(device, &mut enc, 0, views.each_ref());
    queue.submit([enc.finish()]);
    m
}

struct Shot<'a> {
    size: u32,
    view_proj: Mat4,
    eye: Vec3,
    env: &'a Env,
    materials: &'a Materials,
    hidden: &'a [bool],
    env_opacity: f32,
    frames: u32,
}

impl Shot<'_> {
    fn perspective(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
        directx::perspective(40f32.to_radians(), 1.0, 0.1, 100.0) * look_at_mat4(eye, target, up)
    }
}

/// Renders `frames` frames of 16 samples and reads the image back (row-major rgba).
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tracer: &mut PathTracer,
    s: &Shot,
) -> Vec<[f32; 4]> {
    for frame in 0..s.frames {
        let mut enc = device.create_command_encoder(&Default::default());
        tracer.render(
            device,
            &mut enc,
            &FrameInputs {
                width: s.size,
                height: s.size,
                inv_view_proj: s.view_proj.inverse(),
                eye: s.eye,
                aperture: 0.0,
                focus_distance: 1.0,
                env: &s.env.view,
                env_sampler: &s.env.sampler,
                env_rotation: 0.0,
                env_opacity: s.env_opacity,
                background_lod: 0.0,
                max_bounces: FrameInputs::DEFAULT_BOUNCES,
                samples_per_frame: 16,
                indirect_clamp: FrameInputs::DEFAULT_CLAMP,
                reset: frame == 0,
                frame,
                hidden: s.hidden,
                materials: s.materials,
            },
        );
        queue.submit([enc.finish()]);
    }
    read_output(device, queue, tracer, s.size)
}

fn read_output(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tracer: &PathTracer,
    size: u32,
) -> Vec<[f32; 4]> {
    let tex = tracer.output().unwrap().texture();
    let row = (size * 8).div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * size) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        tex.size(),
    );
    queue.submit([enc.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let bytes = buf.slice(..).get_mapped_range().unwrap();
    (0..size)
        .flat_map(|y| {
            let line: &[f16] =
                bytemuck::cast_slice(&bytes[(y * row) as usize..(y * row + size * 8) as usize]);
            line.chunks(4)
                .map(|c| [0, 1, 2, 3].map(|i| c[i].to_f32()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Mean of a square block of pixels around (x, y).
fn mean(img: &[[f32; 4]], size: u32, x: u32, y: u32, r: u32) -> [f32; 4] {
    let mut sum = [0.0; 4];
    let mut n = 0.0;
    for py in y - r..=y + r {
        for px in x - r..=x + r {
            let p = img[(py * size + px) as usize];
            (0..4).for_each(|i| sum[i] += p[i]);
            n += 1.0;
        }
    }
    sum.map(|s| s / n)
}

fn sphere(device: &wgpu::Device) -> PathTracer {
    let mesh = Mesh::uv_sphere(1.0, 64, 32);
    let mut tracer = PathTracer::new(device);
    tracer
        .set_mesh(device, &mesh, &vec![0; mesh.triangle_count()])
        .unwrap();
    tracer
}

#[test]
fn furnace() {
    let _gpu = gpu_lock();
    let Some((device, queue)) = gpu() else { return };
    let mut tracer = sphere(&device);
    let white = env(&device, &queue, 64, |_| [1.0; 3]);
    let mat = material(&device, &queue, [1.0; 3], 0.0, 1.0, 1.0);
    let eye = Vec3::new(0.0, 0.0, 4.0);
    let shot = Shot {
        size: 32,
        view_proj: Shot::perspective(eye, Vec3::ZERO, Vec3::Y),
        eye,
        env: &white,
        materials: &mat,
        hidden: &[],
        env_opacity: 0.5,
        frames: 16,
    };
    let img = render(&device, &queue, &mut tracer, &shot);
    let c = mean(&img, 32, 16, 16, 3);
    eprintln!("furnace: center {c:?}, corner {:?}", img[0]);
    // A white, energy-conserving, convex surface under a uniform white sky reflects
    // exactly the sky's radiance. Single-scattering GGX loses a little at roughness 1.
    for v in &c[..3] {
        assert!((v - 1.0).abs() < 0.04, "furnace {c:?}");
    }
    assert!((c[3] - 1.0).abs() < 1e-3);
    // The sky: the environment itself, alpha = env_opacity.
    let sky = img[0];
    assert!(
        (sky[0] - 1.0).abs() < 0.01 && (sky[3] - 0.5).abs() < 0.01,
        "sky {sky:?}"
    );

    // Upper half white, lower half black, seen from above: the top of the sphere sees
    // the whole lit hemisphere, so it reads the same as in the furnace. This exercises
    // the CDF with empty regions (MIS of light and BSDF samples).
    let half = env(&device, &queue, 64, |d| {
        if d.y > 0.0 { [1.0; 3] } else { [0.0; 3] }
    });
    let eye = Vec3::new(0.0, 4.0, 0.0);
    let top = render(
        &device,
        &queue,
        &mut tracer,
        &Shot {
            view_proj: Shot::perspective(eye, Vec3::ZERO, Vec3::Z),
            eye,
            env: &half,
            ..shot
        },
    );
    let t = mean(&top, 32, 16, 16, 1);
    eprintln!("half sky: top {t:?}");
    for (a, b) in t[..3].iter().zip(&c) {
        assert!((a - b).abs() < 0.03, "half sky {t:?} vs furnace {c:?}");
    }
}

#[test]
fn sun_converges_with_importance_sampling() {
    let _gpu = gpu_lock();
    let Some((device, queue)) = gpu() else { return };
    let mut tracer = sphere(&device);
    let mat = material(&device, &queue, [1.0; 3], 0.0, 1.0, 1.0);
    // A dim sky and a small, very bright sun straight up.
    let sun = |d: Vec3| if d.y > 0.995 { [2000.0; 3] } else { [0.1; 3] };
    let size = 256;
    let e = env(&device, &queue, size, sun);
    // Irradiance at the top of the sphere, integrated over the image's texels.
    let (w, h) = (size as f32, (size / 2) as f32);
    let mut irradiance = 0.0;
    for y in 0..size / 2 {
        for x in 0..size {
            let (u, v) = ((x as f32 + 0.5) / w, (y as f32 + 0.5) / h);
            let (phi, theta) = ((u - 0.5) * std::f32::consts::TAU, v * std::f32::consts::PI);
            let d = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            let solid_angle = std::f32::consts::TAU / w * std::f32::consts::PI / h * theta.sin();
            irradiance += sun(d)[0] * d.y.max(0.0) * solid_angle;
        }
    }
    let eye = Vec3::new(0.0, 4.0, 0.0);
    let img = render(
        &device,
        &queue,
        &mut tracer,
        &Shot {
            size: 32,
            view_proj: Shot::perspective(eye, Vec3::ZERO, Vec3::Z),
            eye,
            env: &e,
            materials: &mat,
            hidden: &[],
            env_opacity: 1.0,
            frames: 8,
        },
    );
    let t = mean(&img, 32, 16, 16, 1);
    // Lambert-like: radiance = irradiance / pi (the dielectric layer keeps ~3% at
    // normal incidence for itself).
    let expected = irradiance / std::f32::consts::PI * 0.97;
    eprintln!("sun: top {t:?}, expected ~{expected}");
    assert!(
        (t[1] / expected - 1.0).abs() < 0.05,
        "sun {t:?} vs {expected}"
    );
}

#[test]
fn mirror_reflects_environment() {
    let _gpu = gpu_lock();
    let Some((device, queue)) = gpu() else { return };
    let mut tracer = sphere(&device);
    let two = env(&device, &queue, 128, |d| {
        if d.y > 0.0 {
            [2.0, 0.0, 0.0]
        } else {
            [0.0, 0.0, 2.0]
        }
    });
    let mat = material(&device, &queue, [1.0; 3], 1.0, 0.0, 1.0);
    let eye = Vec3::new(0.0, 0.0, 4.0);
    let img = render(
        &device,
        &queue,
        &mut tracer,
        &Shot {
            size: 32,
            view_proj: Shot::perspective(eye, Vec3::ZERO, Vec3::Y),
            eye,
            env: &two,
            materials: &mat,
            hidden: &[],
            env_opacity: 1.0,
            frames: 2,
        },
    );
    // Upper half of the sphere reflects the upper (red) sky, lower half the blue one.
    let up = mean(&img, 32, 16, 11, 1);
    let down = mean(&img, 32, 16, 21, 1);
    eprintln!("mirror: up {up:?}, down {down:?}");
    assert!((up[0] - 2.0).abs() < 0.1 && up[2] < 0.05, "upper {up:?}");
    assert!(
        (down[2] - 2.0).abs() < 0.1 && down[0] < 0.05,
        "lower {down:?}"
    );
}

#[test]
fn transparent_and_hidden_show_the_environment() {
    let _gpu = gpu_lock();
    let Some((device, queue)) = gpu() else { return };
    let mut tracer = sphere(&device);
    let sky = env(&device, &queue, 128, |d| {
        [0.5 + 0.5 * d.x, 0.5 + 0.5 * d.y, 1.0]
    });
    let clear = material(&device, &queue, [1.0, 0.0, 0.0], 0.0, 0.5, 0.0);
    let solid = material(&device, &queue, [1.0, 0.0, 0.0], 0.0, 0.5, 1.0);
    let eye = Vec3::new(0.0, 0.0, 4.0);
    let shot = Shot {
        size: 32,
        view_proj: Shot::perspective(eye, Vec3::ZERO, Vec3::Y),
        eye,
        env: &sky,
        materials: &clear,
        hidden: &[],
        env_opacity: 0.25,
        frames: 2,
    };
    let through = render(&device, &queue, &mut tracer, &shot);
    // The reference: an opaque sphere, but its layer hidden.
    let hidden = render(
        &device,
        &queue,
        &mut tracer,
        &Shot {
            materials: &solid,
            hidden: &[true],
            ..shot
        },
    );
    for (i, (a, b)) in through.iter().zip(&hidden).enumerate() {
        for c in 0..4 {
            assert!((a[c] - b[c]).abs() < 0.02, "pixel {i}: {a:?} vs {b:?}");
        }
        assert!((a[3] - 0.25).abs() < 1e-3, "pixel {i} alpha {a:?}");
    }
    // Center: the sky straight behind the sphere.
    let c = through[16 * 32 + 16];
    assert!(
        (c[0] - 0.5).abs() < 0.05 && (c[1] - 0.5).abs() < 0.05 && (c[2] - 1.0).abs() < 0.02,
        "{c:?}"
    );
}

#[test]
fn orthographic_rays() {
    let _gpu = gpu_lock();
    let Some((device, queue)) = gpu() else { return };
    let mut tracer = sphere(&device);
    let white = env(&device, &queue, 64, |_| [1.0; 3]);
    let mat = material(&device, &queue, [0.5; 3], 0.0, 0.5, 1.0);
    for distance in [3.0, 30.0] {
        let eye = Vec3::new(0.0, 0.0, distance);
        // 4 world units across 64 pixels: the unit sphere is 16 pixels in radius
        // however far away the camera is.
        let proj = directx::orthographic(-2.0, 2.0, -2.0, 2.0, distance - 2.0, distance + 2.0);
        let img = render(
            &device,
            &queue,
            &mut tracer,
            &Shot {
                size: 64,
                view_proj: proj * look_at_mat4(eye, Vec3::ZERO, Vec3::Y),
                eye,
                env: &white,
                materials: &mat,
                hidden: &[],
                env_opacity: 0.0,
                frames: 1,
            },
        );
        for y in 0..64 {
            for x in 0..64 {
                let r = ((x as f32 + 0.5 - 32.0).powi(2) + (y as f32 + 0.5 - 32.0).powi(2)).sqrt();
                let a = img[y * 64 + x][3];
                if r < 15.0 {
                    assert!(a > 0.999, "inside ({x}, {y}) at {distance}: {a}");
                } else if r > 17.0 {
                    assert!(a < 1e-3, "outside ({x}, {y}) at {distance}: {a}");
                }
            }
        }
    }
}

/// Frame times at 1080p, 1 sample per frame: `cargo test --release -p reetched-trace -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench() {
    let _gpu = gpu_lock();
    let Some((device, queue)) = gpu() else { return };
    // A lumpy sphere: ~400k triangles with real depth complexity and self shadowing.
    let mut mesh = Mesh::uv_sphere(1.0, 640, 320);
    for v in &mut mesh.vertices {
        let p = Vec3::from(v.position);
        let bump = 1.0 + 0.08 * ((p.x * 23.0).sin() * (p.y * 19.0).sin() * (p.z * 17.0).sin());
        v.position = (p * bump).into();
    }
    let mut tracer = PathTracer::new(&device);
    let took = tracer
        .set_mesh(&device, &mesh, &vec![0; mesh.triangle_count()])
        .unwrap();
    eprintln!(
        "BVH: {} triangles in {:.0} ms",
        mesh.triangle_count(),
        took.as_secs_f64() * 1e3
    );
    let big = Mesh::uv_sphere(1.0, 1000, 500);
    let mut t2 = PathTracer::new(&device);
    let took = t2
        .set_mesh(&device, &big, &vec![0; big.triangle_count()])
        .unwrap();
    eprintln!(
        "BVH: {} triangles in {:.0} ms",
        big.triangle_count(),
        took.as_secs_f64() * 1e3
    );

    let sky = env(&device, &queue, 2048, |d| {
        if d.dot(Vec3::new(0.5, 0.7, 0.5).normalize()) > 0.999 {
            [5000.0; 3]
        } else {
            [0.3, 0.4, 0.6]
        }
    });
    let mat = material(&device, &queue, [0.8, 0.6, 0.4], 0.3, 0.4, 1.0);
    let (w, h) = (1920u32, 1080u32);
    let eye = Vec3::new(0.0, 0.3, 2.6);
    let view_proj = directx::perspective(40f32.to_radians(), w as f32 / h as f32, 0.1, 100.0)
        * look_at_mat4(eye, Vec3::ZERO, Vec3::Y);
    for bounces in [2, 4, 6] {
        let mut times = Vec::new();
        for frame in 0..12 {
            let start = std::time::Instant::now();
            let mut enc = device.create_command_encoder(&Default::default());
            tracer.render(
                &device,
                &mut enc,
                &FrameInputs {
                    width: w,
                    height: h,
                    inv_view_proj: view_proj.inverse(),
                    eye,
                    aperture: 0.0,
                    focus_distance: 2.6,
                    env: &sky.view,
                    env_sampler: &sky.sampler,
                    env_rotation: 0.3,
                    env_opacity: 1.0,
                    background_lod: 0.0,
                    max_bounces: bounces,
                    samples_per_frame: 1,
                    indirect_clamp: FrameInputs::DEFAULT_CLAMP,
                    reset: frame == 0,
                    frame,
                    hidden: &[],
                    materials: &mat,
                },
            );
            queue.submit([enc.finish()]);
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            if frame >= 2 {
                times.push(start.elapsed().as_secs_f64() * 1e3);
            }
        }
        times.sort_by(f64::total_cmp);
        eprintln!(
            "1080p, 1 spp, {bounces} bounces: median {:.1} ms, max {:.1} ms",
            times[times.len() / 2],
            times[times.len() - 1]
        );
    }
}
