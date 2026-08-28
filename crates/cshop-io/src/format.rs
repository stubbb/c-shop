//! Supported file formats and the mapping to filename extensions.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Bmp,
    Gif,
    Tiff,
    WebP,
    Tga,
    Ico,
    /// C-Shop's own layered format. Reading and writing land with the PSD work.
    Cshop,
    /// Layered PSD document. Reading and writing land in a later phase.
    Psd,
}

impl ImageFormat {
    /// Formats the encoder can currently write.
    pub const WRITABLE: &'static [ImageFormat] = &[
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Bmp,
        ImageFormat::Tiff,
        ImageFormat::Tga,
    ];

    /// Extensions the Open dialog offers, lowercase and without the dot.
    pub const OPENABLE_EXTENSIONS: &'static [&'static str] = &[
        "png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff", "webp", "tga", "ico",
    ];

    pub fn from_extension(ext: &str) -> Option<ImageFormat> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "png" => ImageFormat::Png,
            "jpg" | "jpeg" | "jpe" => ImageFormat::Jpeg,
            "bmp" => ImageFormat::Bmp,
            "gif" => ImageFormat::Gif,
            "tif" | "tiff" => ImageFormat::Tiff,
            "webp" => ImageFormat::WebP,
            "tga" => ImageFormat::Tga,
            "ico" => ImageFormat::Ico,
            "csd" => ImageFormat::Cshop,
            "psd" => ImageFormat::Psd,
            _ => return None,
        })
    }

    pub fn from_path(path: &Path) -> Option<ImageFormat> {
        Self::from_extension(&path.extension()?.to_string_lossy())
    }

    /// Whether an alpha channel survives a round trip through this format.
    pub fn supports_alpha(self) -> bool {
        !matches!(self, ImageFormat::Jpeg)
    }

    /// Whether the format keeps a layer stack rather than a flat image.
    pub fn is_layered(self) -> bool {
        matches!(self, ImageFormat::Cshop | ImageFormat::Psd)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Bmp => "BMP",
            ImageFormat::Gif => "GIF",
            ImageFormat::Tiff => "TIFF",
            ImageFormat::WebP => "WebP",
            ImageFormat::Tga => "Targa",
            ImageFormat::Ico => "Icon",
            ImageFormat::Cshop => "C-Shop Document",
            ImageFormat::Psd => "PSD Document",
        }
    }

    pub fn default_extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Bmp => "bmp",
            ImageFormat::Gif => "gif",
            ImageFormat::Tiff => "tif",
            ImageFormat::WebP => "webp",
            ImageFormat::Tga => "tga",
            ImageFormat::Ico => "ico",
            ImageFormat::Cshop => "csd",
            ImageFormat::Psd => "psd",
        }
    }

    pub(crate) fn to_image_crate(self) -> Option<image::ImageFormat> {
        Some(match self {
            ImageFormat::Png => image::ImageFormat::Png,
            ImageFormat::Jpeg => image::ImageFormat::Jpeg,
            ImageFormat::Bmp => image::ImageFormat::Bmp,
            ImageFormat::Gif => image::ImageFormat::Gif,
            ImageFormat::Tiff => image::ImageFormat::Tiff,
            ImageFormat::WebP => image::ImageFormat::WebP,
            ImageFormat::Tga => image::ImageFormat::Tga,
            ImageFormat::Ico => image::ImageFormat::Ico,
            ImageFormat::Cshop | ImageFormat::Psd => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_both_ways() {
        for f in ImageFormat::WRITABLE {
            assert_eq!(ImageFormat::from_extension(f.default_extension()), Some(*f));
        }
    }

    #[test]
    fn detection_is_case_insensitive() {
        assert_eq!(ImageFormat::from_extension("PNG"), Some(ImageFormat::Png));
        assert_eq!(ImageFormat::from_path(Path::new("/a/b/Photo.JPEG")), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::from_path(Path::new("/a/b/noext")), None);
    }

    #[test]
    fn jpeg_is_the_only_format_without_alpha() {
        for f in ImageFormat::WRITABLE {
            assert_eq!(f.supports_alpha(), *f != ImageFormat::Jpeg);
        }
    }

    #[test]
    fn layered_formats_have_no_flat_encoder() {
        assert!(ImageFormat::Psd.is_layered());
        assert!(ImageFormat::Psd.to_image_crate().is_none());
        assert!(ImageFormat::Cshop.to_image_crate().is_none());
    }

    #[test]
    fn every_openable_extension_resolves() {
        for e in ImageFormat::OPENABLE_EXTENSIONS {
            assert!(ImageFormat::from_extension(e).is_some(), "{e} is not mapped");
        }
    }
}
