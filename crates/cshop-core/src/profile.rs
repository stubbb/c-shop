//! Colour profiles: what the numbers in a pixel actually mean.
//!
//! A pixel of `(220, 175, 143)` is not a colour. It is a colour *once you know
//! which red, green and blue it is counting in* — and a file that does not say
//! is guessed at. The guess is nearly always sRGB and nearly always right,
//! which is why an editor can go a long way without ever mentioning profiles.
//! It stops being right the moment a picture comes from a camera in Adobe RGB,
//! or has to leave for a press in CMYK, and then the difference is not subtle.
//!
//! So a document carries a profile, every import is read as whatever it says
//! it is, and every export says what it is. The two operations that matter are
//! worth keeping straight, because one of them changes pixels and the other
//! changes their meaning:
//!
//! * **Assign** — keep the numbers, change what they mean. The picture looks
//!   different. This is the repair for a file that arrived mislabelled.
//! * **Convert** — keep the appearance, change the numbers. This is what
//!   moving between spaces means, and what export does.
//!
//! The transforms themselves are [`moxcms`]'s work. What is here is the part
//! that belongs to the editor: which profile a document is in, how to name it
//! for someone reading a menu, and the handful of conversions the rest of the
//! program actually asks for.

use crate::color::Rgba8;
use std::sync::{Arc, OnceLock};

pub use moxcms::RenderingIntent;

/// The device space a profile describes.
///
/// Only the three the editor can do something with are named; everything else
/// is [`Space::Other`], which is enough to refuse politely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    Rgb,
    Cmyk,
    Gray,
    Other,
}

impl Space {
    /// How many samples one pixel takes in this space, alpha aside.
    pub fn channels(self) -> usize {
        match self {
            Space::Rgb => 3,
            Space::Cmyk => 4,
            Space::Gray => 1,
            Space::Other => 0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Space::Rgb => "RGB",
            Space::Cmyk => "CMYK",
            Space::Gray => "Grey",
            Space::Other => "unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// The bytes are not an ICC profile, or not one that can be read.
    Unreadable(String),
    /// Readable, but describing a space the editor has no path for.
    Unsupported(Space),
    /// The transform itself could not be built or run.
    Transform(String),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::Unreadable(why) => write!(f, "not a readable ICC profile: {why}"),
            ProfileError::Unsupported(s) => {
                write!(f, "{} profiles are not supported", s.name())
            }
            ProfileError::Transform(why) => write!(f, "the colour transform failed: {why}"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// An ICC profile, with the bytes it came from.
///
/// The bytes are kept rather than re-encoded on demand, so a profile that
/// arrives in a file leaves in one unchanged — including the parts this
/// program does not understand. A profile is shared by [`Arc`], because a
/// document, its layers and every transform built from it all want the same
/// one and some of them are several hundred kilobytes.
#[derive(Clone)]
pub struct Profile {
    bytes: Arc<Vec<u8>>,
    parsed: Arc<moxcms::ColorProfile>,
    name: String,
}

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Profile")
            .field("name", &self.name)
            .field("space", &self.space())
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// Two profiles are the same profile if they are the same bytes. Comparing
/// what was parsed out of them would call two different files equal whenever
/// this reader happened to ignore the parts that differ.
impl PartialEq for Profile {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Profile {
    /// The profile assumed for anything that does not say otherwise, and the
    /// one a new document starts in.
    pub fn srgb() -> Profile {
        static SRGB: OnceLock<Profile> = OnceLock::new();
        SRGB.get_or_init(|| {
            let parsed = moxcms::ColorProfile::new_srgb();
            // Encoding can only fail on a profile this one built itself, so a
            // failure here is a bug rather than a condition to report.
            let bytes = parsed.encode().unwrap_or_default();
            Profile { bytes: Arc::new(bytes), parsed: Arc::new(parsed), name: "sRGB".into() }
        })
        .clone()
    }

    /// Read a profile from the bytes an image file carried.
    pub fn parse(bytes: &[u8]) -> Result<Profile, ProfileError> {
        let parsed = moxcms::ColorProfile::new_from_slice(bytes)
            .map_err(|e| ProfileError::Unreadable(e.to_string()))?;
        let name = describe(&parsed);
        Ok(Profile { bytes: Arc::new(bytes.to_vec()), parsed: Arc::new(parsed), name })
    }

    pub fn load(path: &std::path::Path) -> Result<Profile, ProfileError> {
        let bytes = std::fs::read(path)
            .map_err(|e| ProfileError::Unreadable(format!("{}: {e}", path.display())))?;
        let mut p = Profile::parse(&bytes)?;
        if p.name.is_empty() {
            if let Some(stem) = path.file_stem() {
                p.name = stem.to_string_lossy().into_owned();
            }
        }
        Ok(p)
    }

    /// What to put in a menu. Never empty.
    pub fn name(&self) -> &str {
        if self.name.is_empty() { "Untitled profile" } else { &self.name }
    }

    pub fn space(&self) -> Space {
        match self.parsed.color_space {
            moxcms::DataColorSpace::Rgb => Space::Rgb,
            moxcms::DataColorSpace::Cmyk => Space::Cmyk,
            moxcms::DataColorSpace::Gray => Space::Gray,
            _ => Space::Other,
        }
    }

    /// The bytes to embed in an exported file.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// True for the profile a document gets when nothing said otherwise.
    pub fn is_srgb(&self) -> bool {
        self.bytes == Profile::srgb().bytes
    }
}

/// The best name the profile offers, preferring the readable field over the
/// ASCII one that older profiles pad with junk.
fn describe(p: &moxcms::ColorProfile) -> String {
    use moxcms::ProfileText;
    let text = match p.description.as_ref() {
        Some(t) => t,
        None => return String::new(),
    };
    let raw = match text {
        ProfileText::PlainString(s) => s.clone(),
        ProfileText::Localizable(all) => all
            .iter()
            .find(|s| s.language.eq_ignore_ascii_case("en"))
            .or_else(|| all.first())
            .map(|s| s.value.clone())
            .unwrap_or_default(),
        ProfileText::Description(d) => {
            if !d.unicode_string.is_empty() {
                d.unicode_string.clone()
            } else {
                d.ascii_string.clone()
            }
        }
    };
    raw.trim_end_matches('\0').trim().to_string()
}

// --- conversions -----------------------------------------------------------
//
// Four of them, which is all the editor asks for. Each takes a whole image at
// once, because a transform costs more to build than to run and building one
// per pixel would be the slowest possible way to do this.

impl Profile {
    fn options(intent: RenderingIntent) -> moxcms::TransformOptions {
        moxcms::TransformOptions {
            rendering_intent: intent,
            ..Default::default()
        }
    }

    /// Re-encode pixels from this profile into `dst`, keeping their appearance.
    ///
    /// Alpha rides through untouched: it is coverage, not colour, and has no
    /// business in a colour transform.
    pub fn convert_rgba8(
        &self,
        dst: &Profile,
        pixels: &mut [Rgba8],
        intent: RenderingIntent,
    ) -> Result<(), ProfileError> {
        if self.space() != Space::Rgb || dst.space() != Space::Rgb {
            return Err(ProfileError::Unsupported(if self.space() == Space::Rgb {
                dst.space()
            } else {
                self.space()
            }));
        }
        if self == dst {
            return Ok(());
        }
        let t = self
            .parsed
            .create_transform_8bit(
                moxcms::Layout::Rgba,
                &dst.parsed,
                moxcms::Layout::Rgba,
                Self::options(intent),
            )
            .map_err(|e| ProfileError::Transform(e.to_string()))?;
        let flat: &mut [u8] = bytemuck::cast_slice_mut(pixels);
        // The transform cannot read and write the same slice, so it gets a
        // copy of the input to read from.
        let src = flat.to_vec();
        t.transform(&src, flat).map_err(|e| ProfileError::Transform(e.to_string()))
    }

    /// Ink samples in this profile's space to pixels in `dst`.
    ///
    /// CMYK carries no alpha, so the result is opaque.
    pub fn inks_to_rgba8(
        &self,
        dst: &Profile,
        inks: &[u8],
        intent: RenderingIntent,
    ) -> Result<Vec<Rgba8>, ProfileError> {
        let n = self.space().channels();
        if self.space() != Space::Cmyk || dst.space() != Space::Rgb {
            return Err(ProfileError::Unsupported(self.space()));
        }
        if inks.len() % n != 0 {
            return Err(ProfileError::Transform(format!(
                "{} ink samples is not a whole number of {n}-ink pixels",
                inks.len()
            )));
        }
        let count = inks.len() / n;
        let t = self
            .parsed
            .create_transform_8bit(
                // Four inks with no alpha: moxcms counts channels, and four of
                // them is what its `Rgba` layout means for a CMYK profile.
                moxcms::Layout::Rgba,
                &dst.parsed,
                moxcms::Layout::Rgb,
                Self::options(intent),
            )
            .map_err(|e| ProfileError::Transform(e.to_string()))?;
        let mut rgb = vec![0u8; count * 3];
        t.transform(inks, &mut rgb).map_err(|e| ProfileError::Transform(e.to_string()))?;
        Ok(rgb.chunks_exact(3).map(|c| Rgba8::new(c[0], c[1], c[2], 255)).collect())
    }

    /// Pixels in this profile to ink samples in `dst`.
    ///
    /// Transparency has no meaning on paper, so anything short of opaque is
    /// composited over white first — the colour of the stock, as near as this
    /// can know it.
    pub fn rgba8_to_inks(
        &self,
        dst: &Profile,
        pixels: &[Rgba8],
        intent: RenderingIntent,
    ) -> Result<Vec<u8>, ProfileError> {
        if self.space() != Space::Rgb || dst.space() != Space::Cmyk {
            return Err(ProfileError::Unsupported(dst.space()));
        }
        let mut rgb = Vec::with_capacity(pixels.len() * 3);
        for p in pixels {
            let over = |c: u8| -> u8 {
                let a = p.a as u32;
                ((c as u32 * a + 255 * (255 - a) + 127) / 255).min(255) as u8
            };
            rgb.extend_from_slice(&[over(p.r), over(p.g), over(p.b)]);
        }
        let t = self
            .parsed
            .create_transform_8bit(
                moxcms::Layout::Rgb,
                &dst.parsed,
                moxcms::Layout::Rgba,
                Self::options(intent),
            )
            .map_err(|e| ProfileError::Transform(e.to_string()))?;
        let mut inks = vec![0u8; pixels.len() * 4];
        t.transform(&rgb, &mut inks).map_err(|e| ProfileError::Transform(e.to_string()))?;
        Ok(inks)
    }

    /// Grey samples in this profile to pixels in `dst`.
    pub fn grey_to_rgba8(
        &self,
        dst: &Profile,
        grey: &[u8],
        intent: RenderingIntent,
    ) -> Result<Vec<Rgba8>, ProfileError> {
        if self.space() != Space::Gray || dst.space() != Space::Rgb {
            return Err(ProfileError::Unsupported(self.space()));
        }
        let t = self
            .parsed
            .create_transform_8bit(
                moxcms::Layout::Gray,
                &dst.parsed,
                moxcms::Layout::Rgb,
                Self::options(intent),
            )
            .map_err(|e| ProfileError::Transform(e.to_string()))?;
        let mut rgb = vec![0u8; grey.len() * 3];
        t.transform(grey, &mut rgb).map_err(|e| ProfileError::Transform(e.to_string()))?;
        Ok(rgb.chunks_exact(3).map(|c| Rgba8::new(c[0], c[1], c[2], 255)).collect())
    }
}

/// The name to show for a rendering intent, and the one a script writes.
pub fn intent_name(i: RenderingIntent) -> &'static str {
    match i {
        RenderingIntent::Perceptual => "perceptual",
        RenderingIntent::RelativeColorimetric => "relative",
        RenderingIntent::Saturation => "saturation",
        RenderingIntent::AbsoluteColorimetric => "absolute",
    }
}

pub fn intent_from_name(s: &str) -> Option<RenderingIntent> {
    Some(match s.to_ascii_lowercase().as_str() {
        "perceptual" => RenderingIntent::Perceptual,
        "relative" | "relative-colorimetric" => RenderingIntent::RelativeColorimetric,
        "saturation" => RenderingIntent::Saturation,
        "absolute" | "absolute-colorimetric" => RenderingIntent::AbsoluteColorimetric,
        _ => return None,
    })
}
