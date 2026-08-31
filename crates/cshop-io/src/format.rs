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
    /// C-Shop's own layered format: the whole document, still editable.
    Cshop,
    /// Layered PSD document.
    Psd,
    /// Vector: shape layers keep their geometry, and everything else goes out
    /// as a picture embedded in it.
    Svg,
    /// A page. Written, not read — see [`crate::pdf`].
    Pdf,
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

    /// Formats the Save dialog offers.
    ///
    /// Wider than [`ImageFormat::WRITABLE`]: the layered formats keep the
    /// document rather than a flat image, so they go out through
    /// [`crate::save_document`] instead of the encoder.
    pub const SAVEABLE: &'static [ImageFormat] = &[
        ImageFormat::Cshop,
        ImageFormat::Psd,
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Bmp,
        ImageFormat::Tiff,
        ImageFormat::Tga,
        ImageFormat::Gif,
        ImageFormat::Svg,
        ImageFormat::Pdf,
    ];

    /// Extensions the Open dialog offers, lowercase and without the dot.
    pub const OPENABLE_EXTENSIONS: &'static [&'static str] = &[
        "cshop", "csd", "psd", "png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff", "webp",
        "tga", "ico", "svg", "apng", "dng",
    ];

    pub fn from_extension(ext: &str) -> Option<ImageFormat> {
        Some(match ext.to_ascii_lowercase().as_str() {
            // An .apng is a PNG whose animation chunks a still reader ignores.
            "png" | "apng" => ImageFormat::Png,
            "jpg" | "jpeg" | "jpe" => ImageFormat::Jpeg,
            "bmp" => ImageFormat::Bmp,
            "gif" => ImageFormat::Gif,
            "tif" | "tiff" => ImageFormat::Tiff,
            "webp" => ImageFormat::WebP,
            "tga" => ImageFormat::Tga,
            "ico" => ImageFormat::Ico,
            "cshop" | "csd" => ImageFormat::Cshop,
            "psd" => ImageFormat::Psd,
            "svg" => ImageFormat::Svg,
            "pdf" => ImageFormat::Pdf,
            // A DNG is a TIFF as far as the extension table is concerned; the
            // reader decides from the tags whether it is raw.
            "dng" => ImageFormat::Tiff,
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
        matches!(self, ImageFormat::Cshop | ImageFormat::Psd | ImageFormat::Svg)
    }

    /// Whether the format keeps geometry rather than pixels.
    pub fn is_vector(self) -> bool {
        matches!(self, ImageFormat::Svg)
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
            ImageFormat::Svg => "SVG Drawing",
            ImageFormat::Pdf => "PDF Page",
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
            ImageFormat::Cshop => "cshop",
            ImageFormat::Psd => "psd",
            ImageFormat::Svg => "svg",
            ImageFormat::Pdf => "pdf",
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
            ImageFormat::Cshop | ImageFormat::Psd | ImageFormat::Svg | ImageFormat::Pdf => {
                return None
            }
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
