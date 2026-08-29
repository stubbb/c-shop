//! Copy, cut and paste, both inside the editor and with everything else on the
//! desktop.
//!
//! Two clipboards are kept, and the difference matters. The system one carries
//! plain RGBA and is what other programs see; the internal one keeps the same
//! pixels *and where they came from*, which is what Paste in Place needs and
//! what no system clipboard can express.
//!
//! Copying writes to both. Pasting reads the system clipboard, because
//! something else may have replaced it since; when what comes back is the same
//! image that went out, the internal copy is used instead so the origin is not
//! lost.

use cshop_core::color::Rgba8;
use cshop_core::pixels::PixelBuffer;

/// Pixels, and where on the canvas they were taken from.
#[derive(Clone)]
pub struct Clipping {
    pub pixels: PixelBuffer,
    pub origin: (i32, i32),
}

#[derive(Default)]
pub struct Clipboard {
    /// The last thing copied here, with its position.
    inner: Option<Clipping>,
    /// Opened lazily and then held: on X11 the process that owns a selection
    /// is the one that serves it, so dropping this would hand the clipboard
    /// back and lose whatever was copied.
    system: Option<arboard::Clipboard>,
    /// Set once opening the system clipboard has failed, so a headless or
    /// locked-down session does not retry on every copy.
    system_unavailable: bool,
}

impl Clipboard {
    /// A clipboard that never touches the system one.
    ///
    /// For a session with no desktop behind it, and for tests, which
    /// otherwise share one X selection between every thread and see each
    /// other's copies.
    pub fn detached() -> Clipboard {
        Clipboard { inner: None, system: None, system_unavailable: true }
    }

    /// The system clipboard, or `None` where there is not one.
    fn system(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.system.is_none() && !self.system_unavailable {
            match arboard::Clipboard::new() {
                Ok(c) => self.system = Some(c),
                Err(e) => {
                    log::warn!("no system clipboard: {e}");
                    self.system_unavailable = true;
                }
            }
        }
        self.system.as_mut()
    }

    /// True when there is something to paste without asking the system, which
    /// is what the menus grey themselves out on.
    pub fn has_content(&self) -> bool {
        self.inner.is_some()
    }

    /// Put an image on both clipboards.
    ///
    /// Failing to reach the system one is not an error: the copy still works
    /// inside the editor, which is what was asked for.
    pub fn set(&mut self, pixels: PixelBuffer, origin: (i32, i32)) {
        let data = arboard::ImageData {
            width: pixels.width() as usize,
            height: pixels.height() as usize,
            bytes: pixels.as_bytes().to_vec().into(),
        };
        self.inner = Some(Clipping { pixels, origin });
        if let Some(system) = self.system() {
            if let Err(e) = system.set_image(data) {
                log::warn!("could not put the image on the system clipboard: {e}");
            }
        }
    }

    /// What should be pasted, if anything.
    pub fn get(&mut self) -> Option<Clipping> {
        // Ask the system first: something else may have copied since.
        let outside = self.system().and_then(|s| s.get_image().ok()).and_then(from_system);

        match (outside, self.inner.clone()) {
            (Some(outside), Some(mine)) => {
                // The same image coming back is our own copy, so keep the
                // origin with it; anything else came from another program and
                // has no position of its own.
                if outside.pixels.width() == mine.pixels.width()
                    && outside.pixels.height() == mine.pixels.height()
                    && outside.pixels.as_bytes() == mine.pixels.as_bytes()
                {
                    Some(mine)
                } else {
                    Some(outside)
                }
            }
            (Some(outside), None) => Some(outside),
            // Nothing on the system clipboard, or no way to read it.
            (None, mine) => mine,
        }
    }
}

/// Turn what the system handed back into a clipping, refusing anything whose
/// dimensions do not match its bytes.
fn from_system(data: arboard::ImageData<'_>) -> Option<Clipping> {
    let (w, h) = (data.width as u32, data.height as u32);
    if w == 0 || h == 0 || w > 65_536 || h > 65_536 {
        return None;
    }
    let pixels = PixelBuffer::from_rgba_bytes(w, h, &data.bytes)?;
    // From elsewhere, so there is no canvas position to put it back to.
    Some(Clipping { pixels, origin: (0, 0) })
}

/// Lift a region out of a buffer, fading it by the selection's coverage so a
/// feathered edge is copied feathered rather than cut square.
pub fn extract(
    src: &PixelBuffer,
    src_origin: (i32, i32),
    rect: cshop_core::geom::IRect,
    coverage: impl Fn(i32, i32) -> f32,
) -> PixelBuffer {
    let mut out = PixelBuffer::new(rect.width(), rect.height());
    for y in 0..rect.height() as i32 {
        for x in 0..rect.width() as i32 {
            let (dx, dy) = (rect.x0 + x, rect.y0 + y);
            let c = coverage(dx, dy);
            if c <= 0.0 {
                continue;
            }
            let mut p = src.get(dx - src_origin.0, dy - src_origin.1);
            p.a = (p.a as f32 * c).round().clamp(0.0, 255.0) as u8;
            if p.a > 0 {
                out.set(x, y, p);
            } else {
                out.set(x, y, Rgba8::TRANSPARENT);
            }
        }
    }
    out
}
