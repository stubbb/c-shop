//! GPU device ownership.
//!
//! C-Shop creates the adapter and device itself rather than letting the UI
//! toolkit do it, because the compositor and egui must share one device: the
//! composited canvas is handed to egui as a texture, and cross-device textures
//! are not a thing.

use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no GPU adapter available: {0}")]
    NoAdapter(String),
    #[error("could not create GPU device: {0}")]
    NoDevice(String),
}

/// Handles shared by everything that touches the GPU. Cheap to clone.
#[derive(Clone)]
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    /// Format of the compositor's intermediate buffers. See
    /// [`GpuContext::work_format`] for why this is negotiated at runtime.
    work_format: wgpu::TextureFormat,
}

impl std::fmt::Debug for GpuContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuContext").field("adapter", &self.adapter.get_info().name).finish()
    }
}

impl GpuContext {
    /// Build a context from an existing instance, choosing an adapter that can
    /// present to `surface` when one is supplied.
    pub async fn new(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'static>>,
    ) -> Result<Self, GpuError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                // The compositor is fill-rate bound, so prefer the fastest GPU.
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: surface,
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::NoAdapter(e.to_string()))?;

        let info = adapter.get_info();
        log::info!("GPU: {} ({:?}, {:?})", info.name, info.device_type, info.backend);

        let limits = adapter.limits();

        // Rgba16Unorm would be the better fit — uniform 1/65535 precision
        // instead of Rgba16Float's ~1/2048 near white, which Color Burn and
        // Vivid Light amplify by dividing by (1 - backdrop) — but wgpu does not
        // allow it as a colour attachment, only as a storage texture. See
        // `docs/PLAN.md` on tiled f32 scratch for the intended fix.
        let work_format = crate::texture::WORK_FORMAT;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("cshop-device"),
                required_features: wgpu::Features::empty(),
                // Take whatever the adapter offers: max_texture_dimension_2d is
                // the ceiling on document size, and the defaults cap it at
                // 8192, which is too small for real photo work.
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::NoDevice(e.to_string()))?;

        // Without a handler, wgpu aborts the process on any uncaptured error.
        // Running out of VRAM on a large document is a foreseeable condition,
        // and losing the user's work over it is not acceptable — log it and let
        // the caller carry on with whatever it managed to allocate.
        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| match e {
            wgpu::Error::OutOfMemory { .. } => {
                log::error!(
                    "the GPU ran out of memory. The document may be too large \
                     for this hardware; try flattening or closing other documents."
                );
            }
            other => log::error!("GPU error: {other}"),
        }));

        Ok(Self {
            instance,
            adapter,
            device: Arc::new(device),
            queue: Arc::new(queue),
            work_format,
        })
    }

    /// A context with no surface, for tests and for command-line export.
    pub fn headless() -> Result<Self, GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        pollster::block_on(Self::new(instance, None))
    }

    /// Pixel format of the compositor's intermediate buffers.
    pub fn work_format(&self) -> wgpu::TextureFormat {
        self.work_format
    }

    /// Largest document dimension this GPU can hold in one texture.
    pub fn max_texture_dim(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// Rough budget for layer textures, in bytes.
    ///
    /// wgpu exposes no way to query real VRAM, so this is a conservative figure
    /// derived from the adapter class. It deliberately leaves room for the
    /// compositor's scratch buffers, the swapchain, and whatever else is
    /// sharing the GPU. Exceeding it is reported rather than attempted: a
    /// failed allocation would otherwise leave layers silently unrendered.
    pub fn texture_budget(&self) -> u64 {
        match self.adapter.get_info().device_type {
            wgpu::DeviceType::DiscreteGpu => 2 << 30,
            // Integrated GPUs share system memory, so the ceiling is softer but
            // the consequences of overshooting are worse.
            wgpu::DeviceType::IntegratedGpu => 1 << 30,
            _ => 512 << 20,
        }
    }

    pub fn adapter_name(&self) -> String {
        self.adapter.get_info().name
    }

    /// Block until the GPU has finished all submitted work. Used before a
    /// buffer read-back.
    pub fn wait(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }
}
