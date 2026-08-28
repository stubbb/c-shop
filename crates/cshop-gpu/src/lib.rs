//! # cshop-gpu
//!
//! GPU compositing for C-Shop: the device, the layer texture cache, and the
//! ping-pong compositor that turns a layer tree into an image.
//!
//! The compositing maths here is a mirror of `cshop_core::blend`; the tests in
//! [`compositor`] check the two against each other so the shader cannot drift.

pub mod compositor;
pub mod context;
pub mod layers;
pub mod readback;
pub mod texture;

pub use compositor::Compositor;
pub use context::{GpuContext, GpuError};
pub use layers::LayerTextures;
pub use texture::{GpuTexture, DISPLAY_FORMAT, WORK_FORMAT};
