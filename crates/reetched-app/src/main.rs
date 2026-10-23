//! ReEtched entry point: window, GPU setup, and the frame loop.
//! Usage: `reetched [mesh-file]`

// Release builds are a normal GUI app on Windows (no console window).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod env;
mod panels;
mod preview;
mod theme;
mod viewport;

use anyhow::{Context, Result};
use egui_winit::winit::{
    self,
    application::ApplicationHandler,
    event::{StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(
        // The Vulkan loader warns about third-party overlay layers (OBS, Medal…); not our problem.
        "warn,wgpu_hal::vulkan::instance=error,reetched_app=info,reetched_io=info",
    ))
    .init();
    let event_loop = EventLoop::new()?;
    let mut runner = Runner {
        startup_mesh: std::env::args_os().nth(1).map(PathBuf::from),
        gfx: None,
        error: None,
    };
    event_loop.run_app(&mut runner)?;
    runner.error.map_or(Ok(()), Err)
}

struct Runner {
    startup_mesh: Option<PathBuf>,
    gfx: Option<Gfx>,
    error: Option<anyhow::Error>,
}

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    /// Texture uploads egui asked for; kept across frames we couldn't present.
    pending_textures: egui::TexturesDelta,
    app: app::App,
}

impl ApplicationHandler for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("ReEtched")
            .with_inner_size(winit::dpi::LogicalSize::new(1600.0, 900.0))
            .with_maximized(true);
        let result = event_loop
            .create_window(attrs)
            .context("failed to create window")
            .and_then(|w| pollster::block_on(Gfx::new(Arc::new(w), self.startup_mesh.take())));
        match result {
            Ok(gfx) => self.gfx = Some(gfx),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if let (StartCause::ResumeTimeReached { .. }, Some(g)) = (cause, &self.gfx) {
            g.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(g) = &mut self.gfx else { return };
        let response = g.egui_state.on_window_event(&g.window, &event);
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => g.resize(size.width, size.height),
            WindowEvent::RedrawRequested => {
                let repaint_after = g.redraw();
                if g.app.wants_quit() {
                    event_loop.exit();
                } else if repaint_after.is_zero() {
                    g.window.request_redraw();
                } else if repaint_after < Duration::from_secs(60) {
                    event_loop
                        .set_control_flow(ControlFlow::WaitUntil(Instant::now() + repaint_after));
                } else {
                    event_loop.set_control_flow(ControlFlow::Wait);
                }
            }
            _ => {}
        }
        if response.repaint {
            g.window.request_redraw();
        }
    }
}

impl Gfx {
    async fn new(window: Arc<Window>, startup_mesh: Option<PathBuf>) -> Result<Self> {
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())),
        );
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .context("no compatible GPU found")?;
        let info = adapter.get_info();
        log::info!("GPU: {} ({:?})", info.name, info.backend);

        let wireframe = adapter
            .features()
            .contains(wgpu::Features::POLYGON_MODE_LINE);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("reetched"),
                required_features: if wireframe {
                    wgpu::Features::POLYGON_MODE_LINE
                } else {
                    wgpu::Features::empty()
                },
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await?;

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("surface is not supported by this GPU")?;
        // egui wants a non-sRGB swapchain; it does its own gamma.
        let caps = surface.get_capabilities(&adapter);
        config.format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(config.format);
        surface.configure(&device, &config);

        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let egui_state = egui_winit::State::new(
            ctx,
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let mut egui_renderer = egui_wgpu::Renderer::new(
            &device,
            config.format,
            egui_wgpu::RendererOptions::default(),
        );
        let app = app::App::new(
            &device,
            &queue,
            &mut egui_renderer,
            &info,
            wireframe,
            startup_mesh,
        );
        Ok(Self {
            window,
            surface,
            config,
            device,
            queue,
            egui_state,
            egui_renderer,
            pending_textures: Default::default(),
            app,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
            self.window.request_redraw();
        }
    }

    /// Runs one UI frame and presents it. Returns how long egui wants to wait before the next one.
    fn redraw(&mut self) -> Duration {
        let input = self.egui_state.take_egui_input(&self.window);
        let ctx = self.egui_state.egui_ctx().clone();
        let app = &mut self.app;
        let output = ctx.run_ui(input, |ui| app.ui(ui));
        self.egui_state
            .handle_platform_output(&self.window, output.platform_output);
        self.pending_textures.append(output.textures_delta);
        let repaint_after = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::MAX, |v| v.repaint_delay);

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Duration::ZERO;
            }
            _ => return repaint_after,
        };
        let target = frame.texture.create_view(&Default::default());
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: output.pixels_per_point,
        };

        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.app
            .render_viewport(&mut self.egui_renderer, &mut encoder);
        for (id, deltas) in self.pending_textures.set.drain() {
            for delta in &deltas {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, id, delta);
            }
        }
        let extra = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &jobs,
            &screen,
        );
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.egui_renderer.render(&mut pass, &jobs, &screen);
        }
        self.queue
            .submit(extra.into_iter().chain([encoder.finish()]));
        self.window.pre_present_notify();
        self.queue.present(frame);
        for id in self.pending_textures.free.drain() {
            self.egui_renderer.free_texture(&id);
        }
        repaint_after
    }
}
