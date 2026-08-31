//! # cshop-ui
//!
//! The C-Shop interface: a dark, panelled shell built on egui, drawing
//! the composited canvas that [`cshop_gpu`] produces.
//!
//! Every user operation is expressed as an [`commands::Action`] and applied in
//! one place, which keeps mutation out of egui's drawing closures.

pub mod adjust_ui;
pub mod app;
pub mod canvas;
pub mod chrome;
pub mod color_picker;
pub mod clipboard;
pub mod commands;
pub mod context_menus;
pub mod denoise_ui;
pub mod dialogs;
pub mod doc_view;
pub mod filter_ui;
pub mod icons;
pub mod input_harness;
pub mod layer_style;
pub mod lens_ui;
pub mod panels;
pub mod properties;
pub mod theme;
pub mod settings;
pub mod shortcuts;
pub mod text_tool;
pub mod tools;
pub mod profile_ui;
pub mod relight_ui;
pub mod rulers;
pub mod segment_ui;
pub mod separate_ui;
pub mod vision;
pub mod transform_tool;
pub mod upscale_ui;

pub use app::CShopApp;
pub use commands::Action;

use cshop_core::document::Document;
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::compositor::Compositor;
use cshop_gpu::context::GpuContext;
use cshop_gpu::layers::LayerTextures;
use cshop_gpu::texture::GpuTexture;

/// Human-readable byte count for the status bar and dialogs.
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    match bytes {
        b if b >= GB => format!("{:.1} GB", b as f64 / GB as f64),
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.0} KB", b as f64 / KB as f64),
        b => format!("{b} B"),
    }
}

/// Composite a document to a flat image, off to the side of any cached state.
///
/// Used by Merge Down and Flatten, which need a one-off composite of a modified
/// copy of the layer stack rather than of the document on screen.
pub fn render_document(
    gpu: &GpuContext,
    compositor: &mut Compositor,
    doc: &Document,
) -> PixelBuffer {
    let target =
        GpuTexture::render_target(gpu, "flatten", doc.width, doc.height, gpu.work_format());
    let mut cache = LayerTextures::new();
    cache.sync(gpu, doc, &cshop_core::document::Dirty::NONE);
    compositor.composite(gpu, doc, &cache, &target, doc.bounds());
    cshop_gpu::readback::read_as_pixels(gpu, &target, doc.bounds())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_formatting_picks_sensible_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
