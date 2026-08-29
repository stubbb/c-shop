//! Pixel selections.
//!
//! A selection is an 8-bit coverage mask the size of the document, so
//! antialiased edges and feathering are represented exactly rather than
//! approximated by a polygon. Every tool multiplies its own coverage by the
//! selection's, which is what makes "paint only inside the selection" fall out
//! of the existing compositing maths instead of needing a special case.
//!
//! **No selection is not the same as an empty selection.** A document with no
//! selection is entirely editable; a document whose selection is empty is
//! entirely protected. That distinction is why the document holds an
//! `Option<Selection>` rather than a mask that starts blank.

use crate::geom::{IRect, Vec2};
use crate::mask::MaskBuffer;

/// How a new selection combines with the existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}

impl SelectionMode {
    pub fn name(self) -> &'static str {
        match self {
            SelectionMode::Replace => "New selection",
            SelectionMode::Add => "Add to selection",
            SelectionMode::Subtract => "Subtract from selection",
            SelectionMode::Intersect => "Intersect with selection",
        }
    }

    /// The usual modifier convention: Shift adds, Alt subtracts, both
    /// intersect.
    pub fn from_modifiers(shift: bool, alt: bool) -> SelectionMode {
        match (shift, alt) {
            (true, true) => SelectionMode::Intersect,
            (true, false) => SelectionMode::Add,
            (false, true) => SelectionMode::Subtract,
            (false, false) => SelectionMode::Replace,
        }
    }
}

/// A coverage mask over the document, stored only where it is not zero.
///
/// The mask covers `window` rather than the whole document; everything outside
/// it reads as unselected. A marquee in one corner of a 10000x10000 document
/// therefore costs its own area rather than a hundred megabytes, and the
/// operations that walk the mask walk what is really there.
///
/// `window` is a rectangle rather than tiles because every selection this
/// editor makes is one connected region or a few near each other, and a
/// rectangle needs no indirection in the inner loop — coverage is read once
/// per pixel by the brush, and that path has to stay a bounds check and an
/// index.
#[derive(Debug, Clone)]
pub struct Selection {
    /// Coverage over `window`. Zero everywhere else.
    mask: MaskBuffer,
    /// Where `mask` sits in the document.
    window: IRect,
    /// The document's own size, which `window` is a part of.
    size: (u32, u32),
    /// Cached bounding box of non-zero coverage, kept in step with edits so
    /// tools can skip work outside it without rescanning. Always inside
    /// `window`.
    bounds: IRect,
    /// Cached marching-ants outlines in document coordinates.
    contours: Option<Vec<Vec<Vec2>>>,
    /// Outlines dropped from the cache because there were too many to draw.
    dropped_contours: usize,
}

impl Selection {
    /// Everything selected.
    pub fn all(width: u32, height: u32) -> Self {
        Self {
            mask: MaskBuffer::reveal_all(width, height),
            window: IRect::from_size(width, height),
            size: (width, height),
            bounds: IRect::from_size(width, height),
            contours: None,
            dropped_contours: 0,
        }
    }

    /// Nothing selected. Note this protects the whole document; to make it
    /// fully editable again, drop the selection instead.
    pub fn empty(width: u32, height: u32) -> Self {
        Self {
            mask: MaskBuffer::hide_all(0, 0),
            window: IRect::EMPTY,
            size: (width, height),
            bounds: IRect::EMPTY,
            contours: None,
            dropped_contours: 0,
        }
    }

    /// Adopt an existing document-sized coverage buffer.
    pub fn from_mask(mask: MaskBuffer) -> Self {
        let size = (mask.width(), mask.height());
        let bounds = mask.coverage_bounds();
        let window = IRect::from_size(size.0, size.1);
        let mut s = Self { mask, window, size, bounds, contours: None, dropped_contours: 0 };
        // A mask handed in whole is usually mostly empty; keep only the part
        // that matters, so adopting one is not a way to smuggle the old cost
        // back in.
        s.shrink_window_to_bounds();
        s
    }

    /// Adopt a buffer that already covers exactly `window`.
    pub fn from_window(mask: MaskBuffer, window: IRect, size: (u32, u32)) -> Selection {
        debug_assert_eq!(
            (mask.width(), mask.height()),
            (window.width(), window.height()),
            "the buffer must be the size of its window"
        );
        let mut s = Selection {
            mask,
            window,
            size,
            bounds: IRect::EMPTY,
            contours: None,
            dropped_contours: 0,
        };
        s.invalidate();
        s.shrink_window_to_bounds();
        s
    }

    /// Narrow the stored window to the selection's own extent.
    fn shrink_window_to_bounds(&mut self) {
        if self.bounds == self.window {
            return;
        }
        if self.bounds.is_empty() {
            self.mask = MaskBuffer::hide_all(0, 0);
            self.window = IRect::EMPTY;
            return;
        }
        let local = self.bounds.translate(-self.window.x0, -self.window.y0);
        self.mask = self.mask.copy_rect(local);
        self.window = self.bounds;
    }

    /// Grow the stored window so that `wanted` is inside it.
    ///
    /// Anything newly covered reads as zero, which is what it was.
    fn widen_window(&mut self, wanted: IRect) {
        let canvas = IRect::from_size(self.size.0, self.size.1);
        let wanted = wanted.intersect(&canvas);
        if wanted.is_empty() || self.window.contains_rect(&wanted) {
            return;
        }
        let next = if self.window.is_empty() { wanted } else { self.window.union(&wanted) };
        let mut grown = MaskBuffer::hide_all(next.width(), next.height());
        if !self.window.is_empty() {
            grown.paste(&self.mask, self.window.x0 - next.x0, self.window.y0 - next.y0);
        }
        self.mask = grown;
        self.window = next;
    }

    /// Make the stored window the whole document.
    ///
    /// For the callers that genuinely paint anywhere — Quick Mask — rather
    /// than growing the window a dab at a time.
    pub fn widen_to_document(&mut self) {
        self.widen_window(IRect::from_size(self.size.0, self.size.1));
    }

    /// The coverage as a document-sized buffer.
    ///
    /// Materialises what the window leaves implicit, so it costs the document.
    /// For saving a selection as a channel, writing it to a file, or handing it
    /// to something that must see every pixel — not for the drawing paths.
    pub fn to_mask(&self) -> MaskBuffer {
        let mut out = MaskBuffer::hide_all(self.size.0, self.size.1);
        if !self.window.is_empty() {
            out.paste(&self.mask, self.window.x0, self.window.y0);
        }
        out
    }

    /// Where the coverage is actually stored, and where that sits.
    pub fn window(&self) -> (&MaskBuffer, IRect) {
        (&self.mask, self.window)
    }

    /// A selection whose extent is already known.
    ///
    /// The marquees draw into a region they chose, so scanning the whole mask
    /// afterwards to find out where they drew is work with a known answer —
    /// and on a large canvas it is the most expensive part of making a
    /// selection at all. Passing a wrong rect here would leave tools skipping
    /// parts of the selection, so it is only for callers that computed the
    /// region they filled.
    fn from_mask_bounded(mask: MaskBuffer, bounds: IRect) -> Self {
        debug_assert_eq!(
            bounds,
            mask.coverage_bounds(),
            "a bounded selection must agree with what is actually in its mask"
        );
        let size = (mask.width(), mask.height());
        let window = IRect::from_size(size.0, size.1);
        let mut s = Self { mask, window, size, bounds, contours: None, dropped_contours: 0 };
        s.shrink_window_to_bounds();
        s
    }

    pub fn width(&self) -> u32 {
        self.size.0
    }

    pub fn height(&self) -> u32 {
        self.size.1
    }

    /// Bounding box of everything selected.
    pub fn bounds(&self) -> IRect {
        self.bounds
    }

    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }

    /// `true` when every pixel is fully selected, in which case tools can skip
    /// the per-pixel multiply entirely.
    pub fn is_everything(&self) -> bool {
        self.bounds == IRect::from_size(self.width(), self.height())
            && self.window == self.bounds
            && self.mask.as_bytes().iter().all(|&v| v == 255)
    }

    /// Coverage at a document pixel. Outside the stored window it is zero,
    /// which is what "not selected" is.
    #[inline]
    pub fn coverage(&self, x: i32, y: i32) -> u8 {
        if x < self.window.x0 || x >= self.window.x1 || y < self.window.y0 || y >= self.window.y1 {
            return 0;
        }
        self.mask.get(x - self.window.x0, y - self.window.y0)
    }

    /// Recompute the cached bounds and drop the cached outlines. Call after any
    /// direct mutation of the mask.
    pub fn invalidate(&mut self) {
        let local = self.mask.coverage_bounds();
        self.bounds = if local.is_empty() {
            IRect::EMPTY
        } else {
            local.translate(self.window.x0, self.window.y0)
        };
        self.contours = None;
        self.dropped_contours = 0;
        // Deliberately does not narrow the window. Quick Mask paints straight
        // into the mask and then calls this, and a window trimmed to what is
        // currently covered would throw away the next dab that landed outside
        // it. Narrowing happens where the extent is known to be final.
    }

    /// Direct access for tools that paint into the selection, such as Quick
    /// Mask.
    ///
    /// The window must already cover wherever the caller intends to paint —
    /// call [`Selection::widen_to_document`] first — and the caller must
    /// follow up with [`Selection::invalidate`].
    pub fn mask_mut(&mut self) -> &mut MaskBuffer {
        &mut self.mask
    }

    /// Where the stored window sits in the document, for callers holding
    /// [`Selection::mask_mut`].
    pub fn window_origin(&self) -> (i32, i32) {
        (self.window.x0, self.window.y0)
    }

    /// Bytes of coverage actually held, which is the window rather than the
    /// document.
    pub fn memory_bytes(&self) -> u64 {
        self.mask.as_bytes().len() as u64
    }

    // -----------------------------------------------------------------------
    // Construction from shapes
    // -----------------------------------------------------------------------

    /// Rectangular selection, with antialiased edges when the rectangle has a
    /// fractional position or size.
    ///
    /// Without antialiasing the rectangle snaps to the pixel grid rather than
    /// testing pixel centres. Centre testing puts an edge that lands exactly on
    /// a centre — a half-pixel drag — on whichever side the comparison happens
    /// to favour, which shows up as a marquee that is one pixel wider in some
    /// drags than others.
    pub fn from_rect(width: u32, height: u32, rect: Rectf, antialias: bool) -> Self {
        let rect = if antialias { rect } else { rect.snap_to_pixels() };
        let mut mask = MaskBuffer::hide_all(width, height);
        let scan = rect.pixel_bounds().intersect(&IRect::from_size(width, height));
        let mut filled = IRect::EMPTY;
        for y in scan.y0..scan.y1 {
            for x in scan.x0..scan.x1 {
                // Coverage is the area of the pixel square inside the rect.
                let ox = overlap(x as f32, x as f32 + 1.0, rect.x0, rect.x1);
                let oy = overlap(y as f32, y as f32 + 1.0, rect.y0, rect.y1);
                let cov = ox * oy;
                if cov > 0.0 {
                    let v = (cov.min(1.0) * 255.0 + 0.5) as u8;
                    if v != 0 {
                        mask.set(x, y, v);
                        filled = filled.union(&IRect::new(x, y, x + 1, y + 1));
                    }
                }
            }
        }
        Self::from_mask_bounded(mask, filled)
    }

    /// Elliptical selection inscribed in `rect`.
    pub fn from_ellipse(width: u32, height: u32, rect: Rectf, antialias: bool) -> Self {
        let rect = if antialias { rect } else { rect.snap_to_pixels() };
        let mut mask = MaskBuffer::hide_all(width, height);
        let scan = rect.pixel_bounds().intersect(&IRect::from_size(width, height));
        let cx = (rect.x0 + rect.x1) * 0.5;
        let cy = (rect.y0 + rect.y1) * 0.5;
        let rx = (rect.x1 - rect.x0) * 0.5;
        let ry = (rect.y1 - rect.y0) * 0.5;
        if rx <= 0.0 || ry <= 0.0 {
            return Self::from_mask(mask);
        }

        // Supersampling rather than an analytic area: exact ellipse-square
        // overlap has no closed form, and 4x4 samples are visually clean.
        const SS: i32 = 4;
        let mut filled = IRect::EMPTY;
        for y in scan.y0..scan.y1 {
            for x in scan.x0..scan.x1 {
                let cov = if antialias {
                    let mut hits = 0;
                    for sy in 0..SS {
                        for sx in 0..SS {
                            let px = x as f32 + (sx as f32 + 0.5) / SS as f32;
                            let py = y as f32 + (sy as f32 + 0.5) / SS as f32;
                            let dx = (px - cx) / rx;
                            let dy = (py - cy) / ry;
                            if dx * dx + dy * dy <= 1.0 {
                                hits += 1;
                            }
                        }
                    }
                    hits as f32 / (SS * SS) as f32
                } else {
                    let dx = (x as f32 + 0.5 - cx) / rx;
                    let dy = (y as f32 + 0.5 - cy) / ry;
                    if dx * dx + dy * dy <= 1.0 { 1.0 } else { 0.0 }
                };
                if cov > 0.0 {
                    let v = (cov * 255.0 + 0.5) as u8;
                    if v != 0 {
                        mask.set(x, y, v);
                        filled = filled.union(&IRect::new(x, y, x + 1, y + 1));
                    }
                }
            }
        }
        Self::from_mask_bounded(mask, filled)
    }

    /// Polygon selection, filled with the non-zero winding rule.
    ///
    /// Used by both lasso tools: the freehand lasso simply supplies many more
    /// points.
    pub fn from_polygon(width: u32, height: u32, points: &[Vec2], antialias: bool) -> Self {
        let mut mask = MaskBuffer::hide_all(width, height);
        if points.len() < 3 {
            return Self::from_mask(mask);
        }

        let mut bounds = Rectf::EMPTY;
        for p in points {
            bounds = bounds.include(p.x, p.y);
        }
        let scan = bounds.pixel_bounds().intersect(&IRect::from_size(width, height));
        if scan.is_empty() {
            return Self::from_mask(mask);
        }

        // Sample rows at sub-pixel offsets and accumulate, which antialiases
        // vertically as well as horizontally.
        let sub = if antialias { 4 } else { 1 };
        let mut accum = vec![0u16; scan.width() as usize];
        let mut crossings: Vec<f32> = Vec::new();

        for y in scan.y0..scan.y1 {
            accum.iter_mut().for_each(|v| *v = 0);
            for s in 0..sub {
                let sy = y as f32 + (s as f32 + 0.5) / sub as f32;
                crossings.clear();
                // Gather x positions where the polygon crosses this scanline.
                for i in 0..points.len() {
                    let a = points[i];
                    let b = points[(i + 1) % points.len()];
                    if (a.y <= sy && b.y > sy) || (b.y <= sy && a.y > sy) {
                        let t = (sy - a.y) / (b.y - a.y);
                        crossings.push(a.x + t * (b.x - a.x));
                    }
                }
                if crossings.len() < 2 {
                    continue;
                }
                crossings.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

                for pair in crossings.chunks_exact(2) {
                    let (x0, x1) = (pair[0], pair[1]);
                    // Only the span the pair covers, not the whole row: a lasso
                    // is mostly empty space, and walking the full width per
                    // crossing made a 24 MP selection take over a tenth of a
                    // second.
                    let from = (x0.floor() as i32).max(scan.x0);
                    let to = (x1.ceil() as i32 + 1).min(scan.x1);
                    for x in from..to {
                        let cov = if antialias {
                            overlap(x as f32, x as f32 + 1.0, x0, x1)
                        } else {
                            let cx = x as f32 + 0.5;
                            if cx >= x0 && cx < x1 { 1.0 } else { 0.0 }
                        };
                        if cov > 0.0 {
                            let idx = (x - scan.x0) as usize;
                            accum[idx] = (accum[idx] + (cov * 255.0) as u16).min(255 * sub as u16);
                        }
                    }
                }
            }
            for (i, &v) in accum.iter().enumerate() {
                let cov = (v / sub as u16).min(255) as u8;
                if cov > 0 {
                    mask.set(scan.x0 + i as i32, y, cov);
                }
            }
        }
        Self::from_mask(mask)
    }

    // -----------------------------------------------------------------------
    // Boolean combination
    // -----------------------------------------------------------------------

    /// Combine `other` into this selection.
    ///
    /// Coverage is treated as a fuzzy set: add is a union that keeps the
    /// stronger coverage, subtract scales down by the complement, intersect
    /// multiplies. That keeps antialiased edges smooth through repeated
    /// operations instead of hardening them.
    pub fn combine(&mut self, other: &Selection, mode: SelectionMode) {
        debug_assert_eq!(self.width(), other.width());
        debug_assert_eq!(self.height(), other.height());

        // Only where one of them has coverage can the answer differ from what
        // is already there. Add can only gain inside the other's bounds;
        // Subtract and Intersect can only lose inside our own, since both
        // shrink. Walking the whole canvas instead cost 57 ms per click on a
        // large document, for a change covering a fraction of it.
        let region = match mode {
            SelectionMode::Replace => IRect::EMPTY,
            SelectionMode::Add => other.bounds,
            SelectionMode::Subtract | SelectionMode::Intersect => {
                self.bounds.intersect(&match mode {
                    // Intersect zeroes everything outside the other's bounds,
                    // so that part has to be walked as well.
                    SelectionMode::Subtract => other.bounds,
                    _ => self.bounds,
                })
            }
        };

        match mode {
            SelectionMode::Replace => {
                self.mask = other.mask.clone();
                self.window = other.window;
                self.bounds = other.bounds;
            }
            SelectionMode::Intersect => {
                // The result cannot reach outside either, so the window can
                // only shrink — narrow it first and the walk is over the
                // overlap alone.
                let keep = self.bounds.intersect(&other.bounds);
                if keep.is_empty() {
                    *self = Selection::empty(self.size.0, self.size.1);
                    return;
                }
                let mut kept = MaskBuffer::hide_all(keep.width(), keep.height());
                for y in keep.y0..keep.y1 {
                    for x in keep.x0..keep.x1 {
                        let a = self.coverage(x, y) as u16;
                        let b = other.coverage(x, y) as u16;
                        kept.set(x - keep.x0, y - keep.y0, ((a * b) / 255) as u8);
                    }
                }
                self.mask = kept;
                self.window = keep;
                self.invalidate();
                self.shrink_window_to_bounds();
                return;
            }
            SelectionMode::Add => {
                // The result can reach as far as either, so make room first.
                self.widen_window(self.bounds.union(&other.bounds));
                for y in region.y0..region.y1 {
                    for x in region.x0..region.x1 {
                        let v = self.coverage(x, y).max(other.coverage(x, y));
                        self.mask.set(x - self.window.x0, y - self.window.y0, v);
                    }
                }
                self.bounds = self.bounds.union(&other.bounds);
            }
            SelectionMode::Subtract => {
                for y in region.y0..region.y1 {
                    for x in region.x0..region.x1 {
                        let a = self.coverage(x, y) as u16;
                        let b = other.coverage(x, y) as u16;
                        self.mask.set(
                            x - self.window.x0,
                            y - self.window.y0,
                            ((a * (255 - b)) / 255) as u8,
                        );
                    }
                }
                self.invalidate();
                self.shrink_window_to_bounds();
                return;
            }
        }
        self.contours = None;
        self.dropped_contours = 0;
    }

    pub fn invert(&mut self) {
        self.widen_to_document();
        self.mask.invert();
        self.invalidate();
    }

    // -----------------------------------------------------------------------
    // Modify
    // -----------------------------------------------------------------------

    /// Blur the edge over `radius` pixels.
    ///
    /// Three box blurs approximate a Gaussian closely enough that the
    /// difference is invisible, and run in time independent of the radius.
    pub fn feather(&mut self, radius: f32) {
        if radius <= 0.0 {
            return;
        }
        let (w, h) = (self.width(), self.height());
        // A feather radius of R should put the transition roughly within R of
        // the edge, which is a Gaussian of sigma R/2.
        let sigma = radius / 2.0;
        // Box *radius* for three passes to approximate that Gaussian. The
        // standard formula gives a box width, so halve it — using the width as
        // a radius blurs four times too far and washes out the interior.
        let box_r = (((4.0 * sigma * sigma + 1.0).sqrt() - 1.0) / 2.0).round().max(1.0) as u32;
        let _ = (w, h);

        // Three passes of a box of this radius, so that is how far a set pixel
        // can spread; one more for the rounding.
        self.within_reach(3 * box_r + 1, |mask, w, h| {
            let mut buf: Vec<u8> = mask.as_bytes().to_vec();
            let mut tmp = vec![0u8; buf.len()];
            for _ in 0..3 {
                box_blur_h(&buf, &mut tmp, w, h, box_r);
                box_blur_v(&tmp, &mut buf, w, h, box_r);
            }
            *mask = MaskBuffer::from_bytes(w, h, buf).expect("blur preserved the length");
        });
    }

    /// Grow the selection by `pixels`.
    pub fn expand(&mut self, pixels: u32) {
        self.morph(pixels as f32, true);
    }

    /// Shrink the selection by `pixels`.
    pub fn contract(&mut self, pixels: u32) {
        self.morph(pixels as f32, false);
    }

    /// Replace the selection with a band of `pixels` width straddling its edge.
    pub fn border(&mut self, pixels: u32) {
        if pixels == 0 {
            return;
        }
        let half = pixels as f32 / 2.0;
        // The band straddles the edge by half its width either way.
        self.within_reach(pixels + 1, |mask, w, h| {
            let inside = distance_field(mask, w, h, true);
            let outside = distance_field(mask, w, h, false);
            let bytes = mask.as_bytes_mut();
            for i in 0..bytes.len() {
                // Signed distance from the edge, negative inside.
                let d = if bytes[i] >= 128 { -inside[i] } else { outside[i] };
                // Antialias the band edges over one pixel.
                let cov = (1.0 - (d.abs() - half + 0.5).clamp(0.0, 1.0)).clamp(0.0, 1.0);
                bytes[i] = (cov * 255.0 + 0.5) as u8;
            }
        });
    }

    /// Round off corners and remove stray specks, the way Select > Modify >
    /// Smooth does.
    pub fn smooth(&mut self, radius: u32) {
        if radius == 0 {
            return;
        }
        // Expand then contract by the same amount closes notches; the
        // subsequent feather softens what is left.
        self.expand(radius);
        self.contract(radius);
        self.feather(radius as f32 * 0.5);
    }

    /// Run an edit over only the part of the mask it could possibly change.
    ///
    /// Every one of these operations reaches a bounded distance beyond the
    /// selection's own edge — a feather by its blur, a morph by the distance
    /// it moves the edge — and everything past that is zero before and zero
    /// after. Running them over the whole mask instead made them cost the
    /// canvas rather than the selection: feathering a corner of a 10000x10000
    /// document by three pixels took 1.2 seconds.
    ///
    /// The algorithms themselves are unchanged and run on a copy of the
    /// region. That is safe because the region is a rectangle containing the
    /// whole selection plus `reach`, so every distance and every blur tap that
    /// could reach a pixel inside it is also inside it.
    fn within_reach(&mut self, reach: u32, op: impl FnOnce(&mut MaskBuffer, u32, u32)) {
        let canvas = IRect::from_size(self.width(), self.height());
        if self.bounds.is_empty() {
            return;
        }
        let region = self.bounds.inflate(reach as i32).intersect(&canvas);
        if region.is_empty() {
            return;
        }

        // The operation may push coverage outward, so the stored window has to
        // reach as far as the region does before it runs.
        self.widen_window(region);
        let local = region.translate(-self.window.x0, -self.window.y0);

        if local == self.mask.bounds() {
            // The window is exactly the region; run in place.
            let (w, h) = (self.mask.width(), self.mask.height());
            op(&mut self.mask, w, h);
        } else {
            let mut sub = self.mask.copy_rect(local);
            op(&mut sub, local.width(), local.height());
            self.mask.paste(&sub, local.x0, local.y0);
        }
        // Only the region changed, and outside the old bounds was zero, so the
        // new extent is whatever is now inside it.
        let found = self.mask.coverage_bounds_within(local);
        self.bounds =
            if found.is_empty() { IRect::EMPTY } else { found.translate(self.window.x0, self.window.y0) };
        self.contours = None;
        self.dropped_contours = 0;
        self.shrink_window_to_bounds();
    }

    /// Shared implementation of expand and contract via a distance field.
    fn morph(&mut self, distance: f32, grow: bool) {
        if distance <= 0.0 {
            return;
        }
        // The edge moves by `distance`, so nothing further out than that can
        // change; one more pixel for the antialiased ring.
        self.within_reach(distance.ceil() as u32 + 1, |mask, w, h| {
        // Growing measures how far each outside pixel is from the selection;
        // shrinking measures how far each inside pixel is from the outside.
        let field = distance_field(mask, w, h, !grow);
        let bytes = mask.as_bytes_mut();
        for i in 0..bytes.len() {
            let was_in = bytes[i] >= 128;
            // Coverage of the moved edge. The `+ 1.0` matters: the nearest ring
            // of pixels is at distance 1, so "expand by n" must fully include
            // distance n, and "contract by n" must fully exclude it. With a
            // half-pixel offset instead, distance n lands exactly on 128 —
            // which the >=128 test reads as inside — so contract would remove
            // one ring fewer than expand added, and repeated round trips would
            // creep outward.
            let cov = (distance + 1.0 - field[i]).clamp(0.0, 1.0);

            if grow {
                if was_in {
                    bytes[i] = 255;
                } else {
                    bytes[i] = (cov * 255.0 + 0.5) as u8;
                }
            } else if was_in {
                bytes[i] = ((1.0 - cov) * 255.0 + 0.5) as u8;
            } else {
                // Already outside, and shrinking can only push the edge
                // further in, so any leftover partial coverage from an earlier
                // antialiased edge goes too. Leaving it behind would stop the
                // selection's bounds ever shrinking back.
                bytes[i] = 0;
            }
        }
        });
    }

    // -----------------------------------------------------------------------
    // Marching ants
    // -----------------------------------------------------------------------

    /// Outlines of the selection in document coordinates, as closed loops.
    ///
    /// Traced along pixel edges, so the result is the hard-edged outline
    /// editors draw rather than a smoothed contour. Cached until the
    /// selection changes, because tracing costs a scan of the bounding box.
    pub fn contours(&mut self) -> &[Vec<Vec2>] {
        if self.contours.is_none() {
            let local = self.bounds.translate(-self.window.x0, -self.window.y0);
            let (mut loops, dropped) = trace_outline(&self.mask, local);
            // `trace_outline` works in the buffer's coordinates; the caller
            // draws in the document's.
            let (ox, oy) = (self.window.x0 as f32, self.window.y0 as f32);
            for line in &mut loops {
                for p in line.iter_mut() {
                    p.x += ox;
                    p.y += oy;
                }
            }
            self.contours = Some(loops);
            self.dropped_contours = dropped;
        }
        self.contours.as_deref().unwrap_or(&[])
    }

    /// How many outlines were left out of [`Selection::contours`] because the
    /// selection had more islands than can be drawn at frame rate.
    ///
    /// The selection itself is unaffected — only its outline is simplified.
    pub fn dropped_contours(&self) -> usize {
        self.dropped_contours
    }

    /// Store the selection compactly: only the covered region is kept, so a
    /// small selection in a large document costs almost nothing to snapshot
    /// for undo.
    pub fn compress(&self) -> CompressedSelection {
        CompressedSelection {
            width: self.width(),
            height: self.height(),
            bounds: self.bounds,
            data: if self.bounds.is_empty() {
                Vec::new()
            } else {
                self.mask
                    .copy_rect(self.bounds.translate(-self.window.x0, -self.window.y0))
                    .as_bytes()
                    .to_vec()
            },
        }
    }
}

/// A selection snapshot for the undo stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedSelection {
    width: u32,
    height: u32,
    bounds: IRect,
    data: Vec<u8>,
}

impl CompressedSelection {
    /// Rebuild the selection.
    ///
    /// The stored rectangle becomes the window directly rather than being
    /// painted into a document-sized mask that is then trimmed back to it —
    /// which on a large canvas was the most expensive step of an undo.
    pub fn restore(&self) -> Selection {
        if self.bounds.is_empty() {
            return Selection::empty(self.width, self.height);
        }
        let mask = MaskBuffer::from_bytes(
            self.bounds.width(),
            self.bounds.height(),
            self.data.clone(),
        )
        .expect("a compressed selection carries exactly its bounds");
        Selection::from_window(mask, self.bounds, (self.width, self.height))
    }

    /// Bytes held, for the memory readout.
    pub fn memory_bytes(&self) -> u64 {
        self.data.len() as u64
    }
}

/// Floating-point rectangle used while a marquee is being dragged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rectf {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rectf {
    pub const EMPTY: Rectf = Rectf { x0: f32::MAX, y0: f32::MAX, x1: f32::MIN, y1: f32::MIN };

    /// Normalised rectangle spanning two corners, in either order.
    pub fn from_points(a: Vec2, b: Vec2) -> Rectf {
        Rectf {
            x0: a.x.min(b.x),
            y0: a.y.min(b.y),
            x1: a.x.max(b.x),
            y1: a.y.max(b.y),
        }
    }

    pub fn include(self, x: f32, y: f32) -> Rectf {
        Rectf {
            x0: self.x0.min(x),
            y0: self.y0.min(y),
            x1: self.x1.max(x),
            y1: self.y1.max(y),
        }
    }

    pub fn width(&self) -> f32 {
        (self.x1 - self.x0).max(0.0)
    }

    pub fn height(&self) -> f32 {
        (self.y1 - self.y0).max(0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    /// Integer pixels the rectangle can touch.
    pub fn pixel_bounds(&self) -> IRect {
        if self.is_empty() {
            return IRect::EMPTY;
        }
        IRect::new(
            self.x0.floor() as i32,
            self.y0.floor() as i32,
            self.x1.ceil() as i32,
            self.y1.ceil() as i32,
        )
    }

    /// Force a square (or a circle for the ellipse tool), anchoring at `from`.
    pub fn constrain_square(from: Vec2, to: Vec2) -> Rectf {
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        let size = dx.abs().max(dy.abs());
        let sx = if dx < 0.0 { -size } else { size };
        let sy = if dy < 0.0 { -size } else { size };
        Rectf::from_points(from, Vec2::new(from.x + sx, from.y + sy))
    }

    /// Round outward to whole pixels, for tools drawing without antialiasing.
    pub fn snap_to_pixels(self) -> Rectf {
        Rectf {
            x0: self.x0.round(),
            y0: self.y0.round(),
            x1: self.x1.round(),
            y1: self.y1.round(),
        }
    }

    /// Grow outward from `centre` instead of from a corner.
    pub fn from_center(centre: Vec2, corner: Vec2) -> Rectf {
        let dx = (corner.x - centre.x).abs();
        let dy = (corner.y - centre.y).abs();
        Rectf {
            x0: centre.x - dx,
            y0: centre.y - dy,
            x1: centre.x + dx,
            y1: centre.y + dy,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Length of the overlap between two 1D intervals.
#[inline]
fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).clamp(0.0, 1.0)
}

fn box_blur_h(src: &[u8], dst: &mut [u8], w: u32, h: u32, radius: u32) {
    use rayon::prelude::*;
    let w = w as usize;
    let r = radius as i32;
    let _ = h;
    // Rows are independent and contiguous, so this parallelises for free.
    dst.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let row = y * w;
        // Running sum, so cost is independent of the radius.
        let mut sum: u32 = 0;
        for x in -r..=r {
            sum += src[row + x.clamp(0, w as i32 - 1) as usize] as u32;
        }
        let n = (2 * r + 1) as u32;
        for (x, slot) in out.iter_mut().enumerate() {
            *slot = (sum / n) as u8;
            let drop = (x as i32 - r).clamp(0, w as i32 - 1) as usize;
            let add = (x as i32 + r + 1).clamp(0, w as i32 - 1) as usize;
            sum = sum + src[row + add] as u32 - src[row + drop] as u32;
        }
    });
}

/// Rows per band of the vertical blur.
///
/// Each band re-seeds its running sums, which costs `2r+1` rows of reading, so
/// bands want to be tall enough for that to disappear and numerous enough to
/// keep every core busy.
const BLUR_BAND: usize = 64;

fn box_blur_v(src: &[u8], dst: &mut [u8], w: u32, h: u32, radius: u32) {
    use rayon::prelude::*;
    let (w, h) = (w as usize, h as usize);
    let r = radius as i32;
    let n = (2 * r + 1) as u32;
    if w == 0 || h == 0 {
        return;
    }

    // Bands of rows rather than a column at a time. The obvious way to write a
    // vertical blur — walk down each column keeping a running sum — steps a
    // whole row through memory on every iteration, so on a large mask nearly
    // every read is a cache miss; it was two thirds of the cost of a bucket
    // fill. Here each band keeps one running sum per column and walks *down*
    // it row by row, so every read and write is sequential, and the bands are
    // independent so they spread across cores.
    dst.par_chunks_mut(w * BLUR_BAND).enumerate().for_each(|(band, out)| {
        let y0 = (band * BLUR_BAND) as i32;
        let rows = out.len() / w;

        // Seed the window as it stands just above this band.
        let mut sums = vec![0u32; w];
        for dy in -r..=r {
            let y = (y0 + dy).clamp(0, h as i32 - 1) as usize;
            let row = &src[y * w..][..w];
            for (acc, &v) in sums.iter_mut().zip(row) {
                *acc += v as u32;
            }
        }

        for i in 0..rows {
            let y = y0 + i as i32;
            let slot = &mut out[i * w..][..w];
            for (o, acc) in slot.iter_mut().zip(sums.iter()) {
                *o = (*acc / n) as u8;
            }
            // Slide the window down one row.
            let leaving = (y - r).clamp(0, h as i32 - 1) as usize;
            let entering = (y + r + 1).clamp(0, h as i32 - 1) as usize;
            let (a, b) = (&src[leaving * w..][..w], &src[entering * w..][..w]);
            for ((acc, &gone), &came) in sums.iter_mut().zip(a).zip(b) {
                *acc = *acc + came as u32 - gone as u32;
            }
        }
    });
}

/// Exact Euclidean distance from every pixel to the nearest pixel of the
/// opposite class.
///
/// `from_outside` chooses the reference set: `true` measures distance to the
/// nearest *unselected* pixel (used when shrinking), `false` to the nearest
/// *selected* pixel (used when growing).
///
/// Uses Felzenszwalb and Huttenlocher's two-pass parabola algorithm, which is
/// linear in the pixel count and exact — a chamfer approximation would make
/// expand and contract visibly lopsided on diagonals.
pub(crate) fn distance_field(mask: &MaskBuffer, w: u32, h: u32, from_outside: bool) -> Vec<f32> {
    const INF: f32 = 1e20;
    let (wi, hi) = (w as usize, h as usize);
    let bytes = mask.as_bytes();

    let mut f = vec![0.0f32; wi * hi];
    for i in 0..wi * hi {
        let selected = bytes[i] >= 128;
        // Seed pixels are at distance zero; everything else starts at infinity.
        let is_seed = if from_outside { !selected } else { selected };
        f[i] = if is_seed { 0.0 } else { INF };
    }

    let mut column = vec![0.0f32; hi.max(wi)];
    let mut d = vec![0.0f32; hi.max(wi)];
    let mut v = vec![0usize; hi.max(wi)];
    let mut z = vec![0.0f32; hi.max(wi) + 1];

    // Transform along rows, then along columns. Rows are contiguous and
    // independent; columns are strided, so only the row pass parallelises.
    {
        use rayon::prelude::*;
        f.par_chunks_mut(wi).for_each(|row| {
            let mut d = vec![0.0f32; wi];
            let mut v = vec![0usize; wi];
            let mut z = vec![0.0f32; wi + 1];
            edt_1d(row, &mut d, &mut v, &mut z);
            row.copy_from_slice(&d);
        });
    }
    for x in 0..wi {
        for y in 0..hi {
            column[y] = f[y * wi + x];
        }
        edt_1d(&column[..hi], &mut d[..hi], &mut v[..hi], &mut z[..hi + 1]);
        for y in 0..hi {
            f[y * wi + x] = d[y];
        }
    }

    // The transform yields squared distances.
    for value in &mut f {
        *value = value.max(0.0).sqrt();
    }
    f
}

/// One-dimensional squared distance transform: the lower envelope of parabolas
/// rooted at each sample.
fn edt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    const INF: f32 = 1e20;
    let mut k = 0usize;
    v[0] = 0;
    z[0] = -INF;
    z[1] = INF;

    for q in 1..n {
        // Find where the parabola from q overtakes the current lowest one.
        let mut s;
        loop {
            let vk = v[k] as f32;
            let qf = q as f32;
            s = ((f[q] + qf * qf) - (f[v[k]] + vk * vk)) / (2.0 * qf - 2.0 * vk);
            if s <= z[k] && k > 0 {
                k -= 1;
            } else {
                break;
            }
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = INF;
    }

    let mut k = 0usize;
    for (q, out) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let dq = q as f32 - v[k] as f32;
        *out = dq * dq + f[v[k]];
    }
}

/// Most outlines to return from a single trace.
///
/// A magic wand over noisy photographic content can produce tens of thousands
/// of single-pixel islands; drawing every one as an animated dashed polyline
/// would cost far more per frame than compositing the image. The largest
/// outlines are kept, which is what the eye actually reads as the selection
/// boundary, and the count of the rest is reported.
const MAX_CONTOURS: usize = 1200;

/// Trace the selection boundary as closed loops along pixel edges.
///
/// Returns the outlines plus how many were dropped by [`MAX_CONTOURS`].
fn trace_outline(mask: &MaskBuffer, bounds: IRect) -> (Vec<Vec<Vec2>>, usize) {
    if bounds.is_empty() {
        return (Vec::new(), 0);
    }
    // One pixel of margin so edges on the boundary of the region are found.
    let scan = bounds.inflate(1);
    let inside = |x: i32, y: i32| mask.get(x, y) >= 128;

    // Directed unit edges, oriented so the selection is on the left. That
    // orientation is what lets the walk below always pick a consistent path.
    let mut edges: Vec<((i32, i32), (i32, i32))> = Vec::new();
    for y in scan.y0..scan.y1 {
        for x in scan.x0..scan.x1 {
            if !inside(x, y) {
                continue;
            }
            if !inside(x, y - 1) {
                edges.push(((x, y), (x + 1, y)));
            }
            if !inside(x + 1, y) {
                edges.push(((x + 1, y), (x + 1, y + 1)));
            }
            if !inside(x, y + 1) {
                edges.push(((x + 1, y + 1), (x, y + 1)));
            }
            if !inside(x - 1, y) {
                edges.push(((x, y + 1), (x, y)));
            }
        }
    }
    if edges.is_empty() {
        return (Vec::new(), 0);
    }

    // Index edges by their start point so the walk can follow them in order.
    let mut by_start: ahash::AHashMap<(i32, i32), Vec<usize>> = ahash::AHashMap::new();
    for (i, e) in edges.iter().enumerate() {
        by_start.entry(e.0).or_default().push(i);
    }

    let mut used = vec![false; edges.len()];
    let mut loops = Vec::new();

    for start in 0..edges.len() {
        if used[start] {
            continue;
        }
        let mut path: Vec<Vec2> = Vec::new();
        let mut current = start;
        let origin = edges[start].0;

        loop {
            used[current] = true;
            let (from, to) = edges[current];
            path.push(Vec2::new(from.0 as f32, from.1 as f32));

            if to == origin {
                break;
            }
            let Some(candidates) = by_start.get(&to) else { break };
            match candidates.iter().copied().find(|&i| !used[i]) {
                Some(next) => current = next,
                // An open end, which happens where several regions touch at a
                // corner; the loop is closed by the next pass.
                None => break,
            }
        }

        if path.len() >= 4 {
            loops.push(simplify_collinear(path));
        }
    }

    let total = loops.len();
    if total > MAX_CONTOURS {
        // Longest first, so what survives is the boundary a viewer would
        // recognise rather than an arbitrary subset.
        loops.sort_by(|a, b| perimeter(b).total_cmp(&perimeter(a)));
        loops.truncate(MAX_CONTOURS);
    }
    (loops, total.saturating_sub(MAX_CONTOURS))
}

fn perimeter(points: &[Vec2]) -> f32 {
    let mut total = 0.0;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        total += a.distance(b);
    }
    total
}

/// Drop points that lie on a straight run, which cuts a typical rectangular
/// outline from thousands of unit segments to four.
fn simplify_collinear(points: Vec<Vec2>) -> Vec<Vec2> {
    if points.len() < 3 {
        return points;
    }
    let mut out: Vec<Vec2> = Vec::with_capacity(points.len());
    for i in 0..points.len() {
        let prev = points[(i + points.len() - 1) % points.len()];
        let cur = points[i];
        let next = points[(i + 1) % points.len()];
        let d1 = (cur.x - prev.x, cur.y - prev.y);
        let d2 = (next.x - cur.x, next.y - cur.y);
        // Keep only the corners.
        if d1.0 * d2.1 - d1.1 * d2.0 != 0.0 {
            out.push(cur);
        }
    }
    if out.len() < 3 {
        points
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rectf {
        Rectf { x0, y0, x1, y1 }
    }

    #[test]
    fn a_rectangle_selects_exactly_its_pixels() {
        let s = Selection::from_rect(16, 16, rect(4.0, 4.0, 12.0, 12.0), true);
        assert_eq!(s.coverage(4, 4), 255);
        assert_eq!(s.coverage(11, 11), 255);
        assert_eq!(s.coverage(3, 4), 0);
        assert_eq!(s.coverage(12, 12), 0);
        assert_eq!(s.bounds(), IRect::new(4, 4, 12, 12));
    }

    #[test]
    fn fractional_edges_are_antialiased() {
        let s = Selection::from_rect(16, 16, rect(4.5, 4.0, 12.0, 12.0), true);
        // Half a pixel of coverage on the left edge.
        assert_eq!(s.coverage(4, 6), 128);
        assert_eq!(s.coverage(5, 6), 255);

        // Without antialiasing the same edge snaps to whole pixels; 4.5 rounds
        // away, so the pixel is either fully in or fully out, never partial.
        let hard = Selection::from_rect(16, 16, rect(4.5, 4.0, 12.0, 12.0), false);
        assert!(matches!(hard.coverage(4, 6), 0 | 255));
        assert_eq!(hard.coverage(5, 6), 255);
        assert_eq!(hard.coverage(7, 6), 255);
    }

    #[test]
    fn an_ellipse_is_round_and_centred() {
        let s = Selection::from_ellipse(32, 32, rect(4.0, 4.0, 28.0, 28.0), true);
        assert_eq!(s.coverage(16, 16), 255, "the centre is inside");
        assert_eq!(s.coverage(5, 5), 0, "the corner is outside");
        // The extremes of both axes are inside.
        assert!(s.coverage(16, 5) > 128);
        assert!(s.coverage(5, 16) > 128);
        assert_eq!(s.bounds(), IRect::new(4, 4, 28, 28));
    }

    #[test]
    fn a_polygon_fills_its_interior() {
        let tri = [Vec2::new(2.0, 2.0), Vec2::new(30.0, 2.0), Vec2::new(2.0, 30.0)];
        let s = Selection::from_polygon(32, 32, &tri, false);
        assert!(s.coverage(4, 4) > 200, "just inside the hypotenuse");
        assert_eq!(s.coverage(28, 28), 0, "beyond the hypotenuse");
        assert_eq!(s.coverage(1, 1), 0);
    }

    #[test]
    fn a_degenerate_polygon_selects_nothing() {
        let s = Selection::from_polygon(16, 16, &[Vec2::new(1.0, 1.0), Vec2::new(5.0, 5.0)], true);
        assert!(s.is_empty());
    }

    #[test]
    fn boolean_modes_combine_as_expected() {
        let left = Selection::from_rect(16, 16, rect(0.0, 0.0, 8.0, 16.0), false);
        let right = Selection::from_rect(16, 16, rect(6.0, 0.0, 14.0, 16.0), false);

        let mut add = left.clone();
        add.combine(&right, SelectionMode::Add);
        assert_eq!(add.bounds(), IRect::new(0, 0, 14, 16));
        assert_eq!(add.coverage(7, 8), 255);

        let mut sub = left.clone();
        sub.combine(&right, SelectionMode::Subtract);
        assert_eq!(sub.coverage(7, 8), 0, "the overlap is removed");
        assert_eq!(sub.coverage(2, 8), 255);
        assert_eq!(sub.bounds(), IRect::new(0, 0, 6, 16));

        let mut and = left.clone();
        and.combine(&right, SelectionMode::Intersect);
        assert_eq!(and.bounds(), IRect::new(6, 0, 8, 16), "only the overlap survives");

        let mut replace = left.clone();
        replace.combine(&right, SelectionMode::Replace);
        assert_eq!(replace.bounds(), right.bounds());
    }

    #[test]
    fn subtracting_keeps_partial_coverage_partial() {
        // Antialiased edges must stay soft through boolean operations.
        let mut a = Selection::from_rect(16, 16, rect(0.0, 0.0, 16.0, 16.0), false);
        let mut half = Selection::empty(16, 16);
        half.widen_to_document();
        half.mask_mut().fill(128);
        half.invalidate();
        a.combine(&half, SelectionMode::Subtract);
        assert_eq!(a.coverage(8, 8), 127, "half subtracted from full leaves half");
    }

    #[test]
    fn invert_swaps_inside_and_outside() {
        let mut s = Selection::from_rect(16, 16, rect(4.0, 4.0, 12.0, 12.0), false);
        s.invert();
        assert_eq!(s.coverage(8, 8), 0);
        assert_eq!(s.coverage(0, 0), 255);
        assert_eq!(s.bounds(), IRect::new(0, 0, 16, 16));
    }

    #[test]
    fn select_all_and_empty_are_opposites() {
        let all = Selection::all(8, 8);
        assert!(all.is_everything());
        assert!(!all.is_empty());

        let none = Selection::empty(8, 8);
        assert!(none.is_empty());
        assert!(!none.is_everything());
    }

    #[test]
    fn feathering_softens_the_edge_without_moving_the_centre() {
        let mut s = Selection::from_rect(64, 64, rect(16.0, 16.0, 48.0, 48.0), false);
        s.feather(6.0);
        assert!(s.coverage(32, 32) > 240, "the middle stays selected");
        let edge = s.coverage(16, 32);
        assert!(edge > 20 && edge < 235, "the edge should be partial, got {edge}");
        // Feathering spreads coverage outside the original rectangle.
        assert!(s.coverage(13, 32) > 0);
    }

    #[test]
    fn expand_grows_by_the_requested_distance() {
        let mut s = Selection::from_rect(64, 64, rect(20.0, 20.0, 44.0, 44.0), false);
        s.expand(5);
        assert_eq!(s.coverage(32, 16), 255, "5 px above the old edge is now inside");
        assert_eq!(s.coverage(32, 13), 0, "7 px above is still outside");
        assert_eq!(s.coverage(32, 32), 255);
    }

    #[test]
    fn contract_shrinks_by_the_requested_distance() {
        let mut s = Selection::from_rect(64, 64, rect(20.0, 20.0, 44.0, 44.0), false);
        s.contract(5);
        assert_eq!(s.coverage(32, 32), 255, "the middle survives");
        assert_eq!(s.coverage(32, 22), 0, "2 px inside the old edge is now out");
        assert_eq!(s.coverage(32, 26), 255, "6 px inside is still in");
    }

    #[test]
    fn expand_is_isotropic() {
        // A chamfer distance would make diagonals grow further than axes.
        let mut s = Selection::from_rect(80, 80, rect(38.0, 38.0, 42.0, 42.0), false);
        s.expand(20);
        // 20 px straight up from the centre.
        assert_eq!(s.coverage(40, 20), 255);
        // The same Euclidean distance diagonally must be outside the square.
        assert_eq!(s.coverage(58, 58), 0, "diagonal growth should not exceed the radius");
    }

    #[test]
    fn expand_then_contract_returns_to_the_original_bounds() {
        // Expanding leaves an antialiased fringe; contracting has to clear it,
        // or the bounds creep outward with every round trip.
        let original = IRect::new(20, 20, 44, 44);
        let mut s = Selection::from_rect(80, 80, rect(20.0, 20.0, 44.0, 44.0), false);
        for _ in 0..3 {
            s.expand(6);
            s.contract(6);
        }
        let b = s.bounds();
        assert!(
            (b.x0 - original.x0).abs() <= 1
                && (b.y0 - original.y0).abs() <= 1
                && (b.x1 - original.x1).abs() <= 1
                && (b.y1 - original.y1).abs() <= 1,
            "bounds drifted to {b:?} from {original:?}"
        );
    }

    #[test]
    fn contracting_past_the_size_empties_the_selection() {
        let mut s = Selection::from_rect(64, 64, rect(30.0, 30.0, 34.0, 34.0), false);
        s.contract(20);
        assert!(s.is_empty());
    }

    #[test]
    fn border_produces_a_band_around_the_edge() {
        let mut s = Selection::from_rect(64, 64, rect(20.0, 20.0, 44.0, 44.0), false);
        s.border(4);
        assert_eq!(s.coverage(32, 32), 0, "the interior is no longer selected");
        assert!(s.coverage(32, 20) > 200, "the edge itself is selected");
        assert_eq!(s.coverage(32, 10), 0, "well outside is not");
    }

    #[test]
    fn a_rectangle_traces_to_four_corners() {
        let mut s = Selection::from_rect(32, 32, rect(8.0, 8.0, 24.0, 24.0), false);
        let contours = s.contours();
        assert_eq!(contours.len(), 1, "one closed loop");
        assert_eq!(contours[0].len(), 4, "collinear runs should collapse to corners");

        let xs: Vec<f32> = contours[0].iter().map(|p| p.x).collect();
        let ys: Vec<f32> = contours[0].iter().map(|p| p.y).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), 8.0);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 24.0);
        assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), 8.0);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 24.0);
    }

    #[test]
    fn two_separate_regions_trace_as_two_loops() {
        let mut s = Selection::from_rect(64, 32, rect(4.0, 4.0, 12.0, 12.0), false);
        let other = Selection::from_rect(64, 32, rect(40.0, 4.0, 52.0, 12.0), false);
        s.combine(&other, SelectionMode::Add);
        assert_eq!(s.contours().len(), 2);
    }

    #[test]
    fn a_selection_with_a_hole_traces_both_boundaries() {
        let mut s = Selection::from_rect(64, 64, rect(8.0, 8.0, 56.0, 56.0), false);
        let hole = Selection::from_rect(64, 64, rect(24.0, 24.0, 40.0, 40.0), false);
        s.combine(&hole, SelectionMode::Subtract);
        assert_eq!(s.contours().len(), 2, "outer edge and the hole");
    }

    #[test]
    fn an_empty_selection_has_no_outline() {
        let mut s = Selection::empty(16, 16);
        assert!(s.contours().is_empty());
    }

    #[test]
    fn compression_round_trips_exactly() {
        let mut s = Selection::from_ellipse(64, 64, rect(10.0, 10.0, 50.0, 40.0), true);
        s.feather(3.0);
        let restored = s.compress().restore();
        assert_eq!(restored.to_mask().as_bytes(), s.to_mask().as_bytes());
        assert_eq!(restored.bounds(), s.bounds());
    }

    #[test]
    fn compressing_a_small_selection_is_cheap() {
        let s = Selection::from_rect(2000, 2000, rect(10.0, 10.0, 20.0, 20.0), false);
        // A naive snapshot would be 4 MB; only the covered region is stored.
        assert!(s.compress().memory_bytes() <= 100, "a 10x10 selection should cost ~100 bytes");
    }

    #[test]
    fn an_empty_selection_compresses_to_nothing() {
        let s = Selection::empty(1000, 1000);
        assert_eq!(s.compress().memory_bytes(), 0);
        assert!(s.compress().restore().is_empty());
    }

    #[test]
    fn modifiers_map_to_boolean_modes() {
        assert_eq!(SelectionMode::from_modifiers(false, false), SelectionMode::Replace);
        assert_eq!(SelectionMode::from_modifiers(true, false), SelectionMode::Add);
        assert_eq!(SelectionMode::from_modifiers(false, true), SelectionMode::Subtract);
        assert_eq!(SelectionMode::from_modifiers(true, true), SelectionMode::Intersect);
    }

    #[test]
    fn dragging_backwards_still_gives_a_normalised_rectangle() {
        let r = Rectf::from_points(Vec2::new(30.0, 40.0), Vec2::new(10.0, 20.0));
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (10.0, 20.0, 30.0, 40.0));
    }

    #[test]
    fn shift_constrains_to_a_square_in_every_direction() {
        for (dx, dy) in [(20.0, 10.0), (-20.0, 10.0), (10.0, -20.0), (-10.0, -20.0)] {
            let from = Vec2::new(50.0, 50.0);
            let r = Rectf::constrain_square(from, Vec2::new(50.0 + dx, 50.0 + dy));
            assert!((r.width() - r.height()).abs() < 1e-5, "not square for ({dx}, {dy})");
        }
    }

    #[test]
    fn alt_grows_the_marquee_from_its_centre() {
        let r = Rectf::from_center(Vec2::new(50.0, 50.0), Vec2::new(60.0, 55.0));
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (40.0, 45.0, 60.0, 55.0));
    }
}
