//! Window, swapchain and event loop.

use cshop_gpu::context::GpuContext;
use cshop_ui::commands::{Action, ResizeEdge, WindowCommand};
use cshop_ui::CShopApp;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Icon, ResizeDirection, Window, WindowId};

const INITIAL_SIZE: (u32, u32) = (1600, 980);

/// The taskbar and alt-tab icon.
///
/// Embedded rather than read from disk so a built binary carries its own
/// branding; a decode failure leaves the platform default rather than failing
/// to start.
fn load_icon() -> Option<Icon> {
    const LOGO: &[u8] = include_bytes!("../../../assets/logo.png");
    let image = match cshop_io::decode(LOGO, None) {
        Ok(image) => image,
        Err(e) => {
            log::warn!("could not decode the window icon: {e}");
            return None;
        }
    };
    // Window managers want a modest square; the source is larger than needed.
    let scaled = cshop_core::resample::resize(
        &image,
        64,
        64,
        cshop_core::resample::Resampling::Lanczos3,
    );
    Icon::from_rgba(scaled.as_bytes().to_vec(), scaled.width(), scaled.height())
        .inspect_err(|e| log::warn!("could not build the window icon: {e}"))
        .ok()
}

/// `started` is stamped at the top of `main`, so the reported figure covers
/// everything the process did, not just the parts this module owns.
pub fn run(files: Vec<String>, started: Instant) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new()?;
    // The editor is event-driven: with nothing happening it should use no CPU.
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut handler = Handler { state: None, files, started };
    event_loop.run_app(&mut handler)?;
    Ok(())
}

struct Handler {
    state: Option<State>,
    /// Paths named on the command line, opened once the window exists.
    files: Vec<String>,
    started: Instant,
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    gpu: GpuContext,

    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    renderer: egui_wgpu::Renderer,

    app: CShopApp,

    /// When the process began, and whether the first frame has been reported.
    /// Startup is only really over once something is on screen, so the figure
    /// is taken after the first frame is presented rather than after setup.
    started: Instant,
    reported_startup: bool,
}

impl ApplicationHandler for Handler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match State::new(event_loop, &self.files, self.started) {
            Ok(state) => self.state = Some(state),
            Err(e) => {
                log::error!("could not create the window: {e}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else { return };

        // Let egui see the event first; it reports whether it consumed it.
        let response = state.egui_state.on_window_event(&state.window, &event);
        if response.repaint {
            state.window.request_redraw();
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                state.resize(size.width, size.height);
                state.window.request_redraw();
            }

            WindowEvent::RedrawRequested => {
                state.render();
                if state.app.quit {
                    event_loop.exit();
                }
            }

            // Coming back from minimised or a workspace switch, the previous
            // frame's contents may be gone.
            WindowEvent::Occluded(false) | WindowEvent::Focused(true) => {
                state.window.request_redraw();
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Nothing to do: redraws are requested by egui when it needs them.
    }
}

impl State {
    fn new(
        event_loop: &ActiveEventLoop,
        files: &[String],
        started: Instant,
    ) -> Result<State, Box<dyn std::error::Error>> {
        let attrs = Window::default_attributes()
            .with_title("C-Shop")
            .with_inner_size(winit::dpi::LogicalSize::new(
                INITIAL_SIZE.0 as f64,
                INITIAL_SIZE.1 as f64,
            ))
            .with_min_inner_size(winit::dpi::LogicalSize::new(800.0, 600.0))
            // The application draws its own title bar so the window matches the
            // rest of the interface; see `chrome::title_bar`.
            .with_decorations(false)
            .with_window_icon(load_icon());
        let window = Arc::new(event_loop.create_window(attrs)?);

        // The instance is told about the display connection so the GLES path
        // works on Wayland; on Vulkan it is ignored but harmless.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(event_loop.owned_display_handle()),
        ));
        let surface = instance.create_surface(window.clone())?;
        let gpu = pollster::block_on(GpuContext::new(instance, Some(&surface)))?;

        let size = window.inner_size();
        let caps = surface.get_capabilities(&gpu.adapter);
        // egui expects to render into an sRGB target.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            // Vsync: an editor has no reason to render faster than the display.
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            // Auto resolves to sRGB for an 8-bit sRGB format, which is what
            // egui and our present pass both assume.
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&gpu.device, &surface_config);

        let egui_ctx = egui::Context::default();
        cshop_ui::theme::apply(&egui_ctx);

        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        let renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            format,
            egui_wgpu::RendererOptions::default(),
        );

        let mut app = CShopApp::new(gpu.clone());
        for path in files {
            app.push(Action::OpenPath(std::path::PathBuf::from(path)));
        }

        log::info!(
            "C-Shop ready — {} on {:?}, surface {format:?}",
            gpu.adapter_name(),
            gpu.adapter.get_info().backend
        );

        // Ask for the first frame explicitly. With ControlFlow::Wait nothing
        // else will, and on some platforms no initial RedrawRequested arrives,
        // which leaves the window blank until the pointer happens to move.
        window.request_redraw();

        Ok(State {
            window,
            surface,
            surface_config,
            gpu,
            egui_ctx,
            egui_state,
            renderer,
            app,
            started,
            reported_startup: false,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.gpu.device, &self.surface_config);
    }

    /// Carry out whatever the interface asked the window to do.
    fn run_window_commands(&mut self) {
        for command in std::mem::take(&mut self.app.window_commands) {
            let result = match command {
                WindowCommand::StartDrag => self.window.drag_window(),
                WindowCommand::StartResize(edge) => {
                    self.window.drag_resize_window(match edge {
                        ResizeEdge::North => ResizeDirection::North,
                        ResizeEdge::South => ResizeDirection::South,
                        ResizeEdge::East => ResizeDirection::East,
                        ResizeEdge::West => ResizeDirection::West,
                        ResizeEdge::NorthEast => ResizeDirection::NorthEast,
                        ResizeEdge::NorthWest => ResizeDirection::NorthWest,
                        ResizeEdge::SouthEast => ResizeDirection::SouthEast,
                        ResizeEdge::SouthWest => ResizeDirection::SouthWest,
                    })
                }
                WindowCommand::Minimize => {
                    self.window.set_minimized(true);
                    Ok(())
                }
                WindowCommand::ToggleMaximize => {
                    self.window.set_maximized(!self.window.is_maximized());
                    Ok(())
                }
                WindowCommand::Close => {
                    self.app.quit = true;
                    Ok(())
                }
            };
            // Interactive move and resize are not supported on every backend;
            // failing to start one should not take the application down.
            if let Err(e) = result {
                log::warn!("the window manager refused {command:?}: {e}");
            }
        }
    }

    fn render(&mut self) {
        let raw_input = self.egui_state.take_egui_input(&self.window);

        // The title bar shows a maximise or restore icon depending on this.
        self.app.is_maximized = self.window.is_maximized();

        let app = &mut self.app;
        let renderer = &mut self.renderer;
        let output = self.egui_ctx.run_ui(raw_input, |ui| {
            app.update(ui, renderer);
        });

        self.egui_state
            .handle_platform_output(&self.window, output.platform_output);
        // Acted on after the frame, so a drag begins from a settled state.
        self.run_window_commands();

        let pixels_per_point = output.pixels_per_point;
        let primitives = self.egui_ctx.tessellate(output.shapes, pixels_per_point);

        use wgpu::CurrentSurfaceTexture as Acquired;
        let frame = match self.surface.get_current_texture() {
            Acquired::Success(f) => f,
            // Still usable, but the swapchain no longer matches the window, so
            // draw this frame and reconfigure for the next one.
            Acquired::Suboptimal(f) => {
                self.surface.configure(&self.gpu.device, &self.surface_config);
                f
            }
            // A resize can invalidate the swapchain between configure and
            // acquire; reconfiguring and skipping one frame is the fix.
            Acquired::Outdated | Acquired::Lost => {
                self.surface.configure(&self.gpu.device, &self.surface_config);
                self.window.request_redraw();
                return;
            }
            // Minimised or hidden: there is nothing to present, and asking for
            // another redraw here would spin the CPU.
            Acquired::Occluded | Acquired::Timeout => return,
            other => {
                log::warn!("dropped a frame: {other:?}");
                return;
            }
        };

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("egui") });

        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point,
        };

        // One texture can carry several deltas in a single frame.
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
            }
        }
        self.renderer.update_buffers(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &primitives,
            &screen,
        );

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.11,
                            g: 0.11,
                            b: 0.11,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer.render(&mut pass.forget_lifetime(), &primitives, &screen);
        }

        self.gpu.queue.submit(Some(encoder.finish()));
        self.gpu.queue.present(frame);

        if !self.reported_startup {
            self.reported_startup = true;
            log::info!("startup: {:.1?} to the first frame", self.started.elapsed());
        }

        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }

        // egui tells us when it next needs to draw: immediately while something
        // is animating, never when the interface is at rest.
        if let Some(after) = output.viewport_output.get(&egui::ViewportId::ROOT) {
            if after.repaint_delay.is_zero() {
                self.window.request_redraw();
            }
        }
    }
}
