//! Free Transform and Crop.
//!
//! Free Transform keeps a quadrilateral of four corners rather than a
//! scale/rotate/skew triple. Dragging any corner anywhere is then always
//! meaningful, and one projective matrix reproduces the result — scale,
//! rotate, skew, distort and perspective all fall out of the same model
//! instead of each needing its own mode.

use cshop_core::geom::{IRect, Vec2};
use cshop_core::layer::{LayerId, LayerMask};
use cshop_core::pixels::PixelBuffer;
use cshop_core::resample::Resampling;
use cshop_core::transform::{Handle, Transform};

/// How far, in screen points, a click can be from a handle and still grab it.
pub const HANDLE_GRAB: f32 = 9.0;
/// Largest edge of the live preview proxy, in pixels.
///
/// Transforming a full-resolution layer every frame would stall the drag, so
/// the preview resamples a small copy and the real pixels are resampled once,
/// on commit.
const PROXY_MAX: u32 = 640;

/// A Free Transform in progress.
pub struct ActiveTransform {
    pub layer: LayerId,
    /// The layer's pixels and position when the transform began.
    pub source: PixelBuffer,
    pub source_offset: (i32, i32),
    pub source_mask: Option<LayerMask>,
    /// Document-space rect of the untransformed layer.
    pub source_rect: IRect,
    /// Current corner positions, top-left then clockwise.
    pub corners: [Vec2; 4],
    /// Corners as they were when the current drag started.
    start_corners: [Vec2; 4],
    pub dragging: Option<Handle>,
    drag_origin: Vec2,
    pub filter: Resampling,
    /// Downscaled copy for the live preview, and its scale relative to source.
    pub proxy: PixelBuffer,
    /// Set once the user has actually moved something.
    pub modified: bool,
    /// The corners as they stood before each completed gesture, oldest first,
    /// and the gestures stepped back from. A transform has an undo of its own:
    /// while it is live the document's history is out of reach, since undoing
    /// underneath the preview would change the very layer it was made from.
    undo_stack: Vec<[Vec2; 4]>,
    redo_stack: Vec<[Vec2; 4]>,
}

impl ActiveTransform {
    pub fn begin(
        layer: LayerId,
        source: PixelBuffer,
        source_offset: (i32, i32),
        source_mask: Option<LayerMask>,
    ) -> Self {
        let rect =
            IRect::at(source_offset.0, source_offset.1, source.width(), source.height());
        let corners = corners_of(rect);

        // Build the preview proxy once, here, rather than per frame.
        let scale = (PROXY_MAX as f32 / source.width().max(source.height()).max(1) as f32).min(1.0);
        let proxy = if scale < 1.0 {
            cshop_core::resample::resize(
                &source,
                ((source.width() as f32 * scale) as u32).max(1),
                ((source.height() as f32 * scale) as u32).max(1),
                Resampling::Bilinear,
            )
        } else {
            source.clone()
        };

        Self {
            layer,
            source,
            source_offset,
            source_mask,
            source_rect: rect,
            corners,
            start_corners: corners,
            dragging: None,
            drag_origin: Vec2::ZERO,
            filter: Resampling::Bicubic,
            proxy,
            modified: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Centre of the current quad, used as the rotation and scale pivot.
    pub fn centre(&self) -> Vec2 {
        let sum = self.corners.iter().fold(Vec2::ZERO, |a, b| a + *b);
        sum * 0.25
    }

    /// Position of a handle on the current quad.
    pub fn handle_position(&self, handle: Handle) -> Vec2 {
        let c = &self.corners;
        match handle {
            Handle::TopLeft => c[0],
            Handle::TopRight => c[1],
            Handle::BottomRight => c[2],
            Handle::BottomLeft => c[3],
            Handle::Top => c[0].lerp(c[1], 0.5),
            Handle::Right => c[1].lerp(c[2], 0.5),
            Handle::Bottom => c[3].lerp(c[2], 0.5),
            Handle::Left => c[0].lerp(c[3], 0.5),
            Handle::Body | Handle::Rotate => self.centre(),
        }
    }

    /// What a press at a document-space point takes hold of, given the zoom.
    ///
    /// A handle when the point is on one; otherwise the body, to move it, when
    /// the point is inside the box; and otherwise rotation, anywhere outside.
    /// The whole outside, not a ring round the corners: a ring's inner edge
    /// reached well into a small layer, so clicking the picture turned it
    /// instead of moving it.
    pub fn hit(&self, point: Vec2, zoom: f32) -> Handle {
        let grab = HANDLE_GRAB / zoom.max(0.01);
        let nearest = Handle::ALL
            .iter()
            .map(|h| (*h, self.handle_position(*h).distance(point)))
            .filter(|(_, d)| *d <= grab)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((h, _)) = nearest {
            return h;
        }
        if self.contains(point) {
            Handle::Body
        } else {
            Handle::Rotate
        }
    }

    /// Whether a point lies inside the quad, by winding.
    pub fn contains(&self, p: Vec2) -> bool {
        let mut inside = false;
        for i in 0..4 {
            let a = self.corners[i];
            let b = self.corners[(i + 1) % 4];
            if (a.y > p.y) != (b.y > p.y) {
                let t = (p.y - a.y) / (b.y - a.y);
                if p.x < a.x + t * (b.x - a.x) {
                    inside = !inside;
                }
            }
        }
        inside
    }

    pub fn begin_drag(&mut self, handle: Handle, at: Vec2) {
        self.dragging = Some(handle);
        self.drag_origin = at;
        self.start_corners = self.corners;
    }

    /// Update the quad for a drag to `at`.
    ///
    /// `distort` (Ctrl) frees a corner from the rectangle; `shift` lets a
    /// corner's two axes scale apart, or snaps a rotation to fifteen degrees;
    /// `from_centre` (Alt) anchors the pivot at the middle.
    pub fn drag_to(&mut self, at: Vec2, distort: bool, shift: bool, from_centre: bool) {
        let Some(handle) = self.dragging else { return };
        let delta = at - self.drag_origin;
        let start = self.start_corners;
        self.modified = true;

        match handle {
            Handle::Body => {
                for (i, c) in self.corners.iter_mut().enumerate() {
                    *c = start[i] + delta;
                }
            }

            Handle::Rotate => {
                let pivot = quad_centre(&start);
                let from = (self.drag_origin - pivot).angle();
                let to = (at - pivot).angle();
                let mut angle = to - from;
                if shift {
                    // Snap to 15-degree steps.
                    let step = std::f32::consts::PI / 12.0;
                    angle = (angle / step).round() * step;
                }
                let t = Transform::about(pivot, Transform::rotate(angle));
                for (i, c) in self.corners.iter_mut().enumerate() {
                    *c = t.apply(start[i]);
                }
            }

            // Corner drags either move that corner alone (distort) or scale the
            // whole box about its opposite.
            h if h.corner_index().is_some() && distort => {
                let i = h.corner_index().expect("checked");
                self.corners = start;
                self.corners[i] = at;
            }

            h => {
                let anchor = if from_centre {
                    quad_centre(&start)
                } else {
                    handle_on(&start, h.opposite())
                };
                let before = handle_on(&start, h) - anchor;
                let after = at - anchor;
                let unit = h.unit_position();
                let corner = unit.x != 0.5 && unit.y != 0.5;

                // The scale happens along the box's own axes — its top edge
                // and its left edge — rather than the document's, so a box
                // that has been turned stays a rectangle when a handle is
                // pulled. The basis is those two edges; its inverse takes a
                // document vector into box coordinates, where the scale is a
                // plain diagonal.
                let basis = Transform {
                    m: [
                        [start[1].x - start[0].x, start[3].x - start[0].x, 0.0],
                        [start[1].y - start[0].y, start[3].y - start[0].y, 0.0],
                        [0.0, 0.0, 1.0],
                    ],
                };
                let (to_box, from_box) = match basis.invert() {
                    Some(inverse) => (inverse, basis),
                    // A collapsed box has no axes of its own.
                    None => (Transform::IDENTITY, Transform::IDENTITY),
                };

                let scale = if corner && !shift {
                    // A corner scales both axes alike, following the pointer's
                    // projection onto the line from the anchor through the
                    // corner: the corner stays on that diagonal and the box
                    // keeps its proportions. Shift lets the axes go their own
                    // ways.
                    let along = before.x * before.x + before.y * before.y;
                    let s = if along < 1e-8 {
                        1.0
                    } else {
                        (after.x * before.x + after.y * before.y) / along
                    };
                    Transform::scale(s, s)
                } else {
                    // An edge scales its one axis; a corner under Shift scales
                    // each axis by its own ratio.
                    let b = to_box.apply(before);
                    let a = to_box.apply(after);
                    let sx = if unit.x == 0.5 { 1.0 } else { safe_ratio(a.x, b.x) };
                    let sy = if unit.y == 0.5 { 1.0 } else { safe_ratio(a.y, b.y) };
                    Transform::scale(sx, sy)
                };

                let t = Transform::about(anchor, to_box.then(scale).then(from_box));
                for (i, c) in self.corners.iter_mut().enumerate() {
                    *c = t.apply(start[i]);
                }
            }
        }
    }

    /// Finish the gesture, remembering what it changed so it can be stepped
    /// back with [`ActiveTransform::undo`].
    pub fn end_drag(&mut self) {
        if self.dragging.take().is_some() && self.corners != self.start_corners {
            self.undo_stack.push(self.start_corners);
            self.redo_stack.clear();
        }
        self.refresh_modified();
    }

    /// Step back to before the last gesture. `false` when there is none left,
    /// which is the moment the caller should say so rather than reach for the
    /// document's history.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo_stack.pop() else { return false };
        self.redo_stack.push(self.corners);
        self.corners = previous;
        self.dragging = None;
        self.refresh_modified();
        true
    }

    /// Step forward again through a gesture that was stepped back.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else { return false };
        self.undo_stack.push(self.corners);
        self.corners = next;
        self.dragging = None;
        self.refresh_modified();
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Whether the box differs from the untransformed rectangle, kept in
    /// `modified` so a gesture that ends where it began records nothing.
    fn refresh_modified(&mut self) {
        self.modified = self.corners != corners_of(self.source_rect);
    }

    /// The matrix taking the original layer to its current quad.
    pub fn matrix(&self) -> Option<Transform> {
        Transform::from_quad(self.source_rect, self.corners)
    }

    /// Apply the transform to the real pixels. `clip` bounds the result.
    pub fn render(&self, clip: IRect) -> Option<(PixelBuffer, (i32, i32))> {
        let matrix = self.matrix()?;
        // A generous margin beyond the canvas keeps content that a later move
        // could bring back into view.
        let bounds = clip.inflate(clip.width().max(clip.height()) as i32);
        cshop_core::resample::transform(
            &self.source,
            self.source_offset,
            matrix,
            self.filter,
            Some(bounds),
        )
    }

    /// Reset to the untransformed rectangle. A gesture like any other, so it
    /// can be stepped back from.
    pub fn reset(&mut self) {
        let rest = corners_of(self.source_rect);
        if self.corners != rest {
            self.undo_stack.push(self.corners);
            self.redo_stack.clear();
        }
        self.corners = rest;
        self.dragging = None;
        self.modified = false;
    }

    /// Width and height of the current quad's edges, for the options bar.
    pub fn scale_percent(&self) -> (f32, f32) {
        let w = self.corners[0].distance(self.corners[1]);
        let h = self.corners[0].distance(self.corners[3]);
        let sw = self.source_rect.width().max(1) as f32;
        let sh = self.source_rect.height().max(1) as f32;
        (w / sw * 100.0, h / sh * 100.0)
    }

    /// Current rotation in degrees, taken from the top edge.
    pub fn rotation_degrees(&self) -> f32 {
        let edge = self.corners[1] - self.corners[0];
        edge.angle().to_degrees()
    }
}

fn quad_centre(corners: &[Vec2; 4]) -> Vec2 {
    corners.iter().fold(Vec2::ZERO, |a, b| a + *b) * 0.25
}

/// A rectangle's corners, top-left then clockwise.
fn corners_of(rect: IRect) -> [Vec2; 4] {
    [
        Vec2::new(rect.x0 as f32, rect.y0 as f32),
        Vec2::new(rect.x1 as f32, rect.y0 as f32),
        Vec2::new(rect.x1 as f32, rect.y1 as f32),
        Vec2::new(rect.x0 as f32, rect.y1 as f32),
    ]
}

fn handle_on(corners: &[Vec2; 4], handle: Handle) -> Vec2 {
    match handle {
        Handle::TopLeft => corners[0],
        Handle::TopRight => corners[1],
        Handle::BottomRight => corners[2],
        Handle::BottomLeft => corners[3],
        Handle::Top => corners[0].lerp(corners[1], 0.5),
        Handle::Right => corners[1].lerp(corners[2], 0.5),
        Handle::Bottom => corners[3].lerp(corners[2], 0.5),
        Handle::Left => corners[0].lerp(corners[3], 0.5),
        Handle::Body | Handle::Rotate => quad_centre(corners),
    }
}

/// Ratio guarded against a zero-width start, which would otherwise send the
/// quad to infinity the moment a handle is dragged onto its own anchor.
fn safe_ratio(after: f32, before: f32) -> f32 {
    if before.abs() < 1e-4 {
        1.0
    } else {
        after / before
    }
}

/// A crop rectangle being dragged out.
pub struct ActiveCrop {
    pub rect: IRect,
    pub dragging: Option<Handle>,
    start_rect: IRect,
    drag_origin: Vec2,
    /// `None` for a free crop, otherwise width divided by height.
    pub aspect: Option<f32>,
    /// Four corners, top-left then clockwise, when the crop is a quadrilateral
    /// rather than a rectangle — put them on the corners of something that is
    /// rectangular in the world and cropping straightens it.
    pub corners: Option<[Vec2; 4]>,
    /// Which corner the pointer took hold of.
    pub dragging_corner: Option<usize>,
}

impl ActiveCrop {
    pub fn new(rect: IRect) -> Self {
        Self {
            rect,
            dragging: None,
            start_rect: rect,
            drag_origin: Vec2::ZERO,
            aspect: None,
            corners: None,
            dragging_corner: None,
        }
    }

    /// Whether this crop straightens as well as cuts.
    pub fn is_perspective(&self) -> bool {
        self.corners.is_some()
    }

    /// Switch between the two, seeding the corners from the rectangle so the
    /// handles do not jump when the mode changes.
    pub fn set_perspective(&mut self, on: bool) {
        self.corners = on.then(|| {
            let (x0, y0) = (self.rect.x0 as f32, self.rect.y0 as f32);
            let (x1, y1) = (self.rect.x1 as f32, self.rect.y1 as f32);
            [
                Vec2::new(x0, y0),
                Vec2::new(x1, y0),
                Vec2::new(x1, y1),
                Vec2::new(x0, y1),
            ]
        });
        self.dragging = None;
    }

    /// Which corner is nearest, when this is a quadrilateral.
    pub fn hit_corner(&self, point: Vec2, zoom: f32) -> Option<usize> {
        let corners = self.corners?;
        let grab = HANDLE_GRAB / zoom.max(0.01);
        corners
            .iter()
            .enumerate()
            .map(|(i, c)| (i, c.distance(point)))
            .filter(|(_, d)| *d <= grab)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    pub fn handle_position(&self, handle: Handle) -> Vec2 {
        let unit = handle.unit_position();
        Vec2::new(
            self.rect.x0 as f32 + unit.x * self.rect.width() as f32,
            self.rect.y0 as f32 + unit.y * self.rect.height() as f32,
        )
    }

    pub fn hit(&self, point: Vec2, zoom: f32) -> Option<Handle> {
        let grab = HANDLE_GRAB / zoom.max(0.01);
        let nearest = Handle::ALL
            .iter()
            .map(|h| (*h, self.handle_position(*h).distance(point)))
            .filter(|(_, d)| *d <= grab)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((h, _)) = nearest {
            return Some(h);
        }
        if self.rect.contains(point.x as i32, point.y as i32) {
            return Some(Handle::Body);
        }
        None
    }

    pub fn begin_drag(&mut self, handle: Handle, at: Vec2) {
        self.dragging = Some(handle);
        self.start_rect = self.rect;
        self.drag_origin = at;
    }

    pub fn drag_to(&mut self, at: Vec2, bounds: IRect) {
        let Some(handle) = self.dragging else { return };
        let d = at - self.drag_origin;
        let (dx, dy) = (d.x.round() as i32, d.y.round() as i32);
        let s = self.start_rect;

        let mut rect = match handle {
            Handle::Body => s.translate(dx, dy),
            Handle::TopLeft => IRect::new(s.x0 + dx, s.y0 + dy, s.x1, s.y1),
            Handle::Top => IRect::new(s.x0, s.y0 + dy, s.x1, s.y1),
            Handle::TopRight => IRect::new(s.x0, s.y0 + dy, s.x1 + dx, s.y1),
            Handle::Right => IRect::new(s.x0, s.y0, s.x1 + dx, s.y1),
            Handle::BottomRight => IRect::new(s.x0, s.y0, s.x1 + dx, s.y1 + dy),
            Handle::Bottom => IRect::new(s.x0, s.y0, s.x1, s.y1 + dy),
            Handle::BottomLeft => IRect::new(s.x0 + dx, s.y0, s.x1, s.y1 + dy),
            Handle::Left => IRect::new(s.x0 + dx, s.y0, s.x1, s.y1),
            Handle::Rotate => s,
        };

        // Dragging an edge past its opposite flips the rectangle rather than
        // inverting it.
        if rect.x1 < rect.x0 {
            std::mem::swap(&mut rect.x0, &mut rect.x1);
        }
        if rect.y1 < rect.y0 {
            std::mem::swap(&mut rect.y0, &mut rect.y1);
        }

        if let Some(aspect) = self.aspect {
            if handle != Handle::Body && aspect > 0.0 {
                let h = (rect.width() as f32 / aspect).round() as i32;
                rect.y1 = rect.y0 + h.max(1);
            }
        }

        self.rect = rect.intersect(&bounds);
        if self.rect.is_empty() {
            self.rect = IRect::at(s.x0, s.y0, 1, 1).intersect(&bounds);
        }
    }

    pub fn end_drag(&mut self) {
        self.dragging = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cshop_core::color::Rgba8;

    fn transform_of(w: u32, h: u32) -> ActiveTransform {
        ActiveTransform::begin(
            LayerId(1),
            PixelBuffer::filled(w, h, Rgba8::WHITE),
            (0, 0),
            None,
        )
    }

    #[test]
    fn a_fresh_transform_is_the_source_rectangle() {
        let t = transform_of(100, 50);
        assert_eq!(t.corners[0], Vec2::new(0.0, 0.0));
        assert_eq!(t.corners[2], Vec2::new(100.0, 50.0));
        assert!(t.matrix().unwrap().is_identity());
        assert!(!t.modified);
        let (sw, sh) = t.scale_percent();
        assert!((sw - 100.0).abs() < 0.1 && (sh - 100.0).abs() < 0.1);
    }

    #[test]
    fn dragging_the_body_moves_every_corner() {
        let mut t = transform_of(100, 50);
        t.begin_drag(Handle::Body, Vec2::new(50.0, 25.0));
        t.drag_to(Vec2::new(70.0, 35.0), false, false, false);
        assert_eq!(t.corners[0], Vec2::new(20.0, 10.0));
        assert_eq!(t.corners[2], Vec2::new(120.0, 60.0));
        assert!(t.modified);
    }

    #[test]
    fn dragging_a_corner_scales_about_the_opposite_one() {
        let mut t = transform_of(100, 100);
        t.begin_drag(Handle::BottomRight, Vec2::new(100.0, 100.0));
        t.drag_to(Vec2::new(200.0, 200.0), false, false, false);
        assert_eq!(t.corners[0], Vec2::new(0.0, 0.0), "the anchor stays put");
        assert!((t.corners[2].x - 200.0).abs() < 0.01);
        let (sw, _) = t.scale_percent();
        assert!((sw - 200.0).abs() < 1.0, "should read as 200%, got {sw}");
    }

    #[test]
    fn an_edge_handle_scales_one_axis_only() {
        let mut t = transform_of(100, 100);
        t.begin_drag(Handle::Right, Vec2::new(100.0, 50.0));
        t.drag_to(Vec2::new(150.0, 90.0), false, false, false);
        assert!((t.corners[1].x - 150.0).abs() < 0.01, "width follows the drag");
        assert!((t.corners[2].y - 100.0).abs() < 0.01, "height is untouched");
    }

    #[test]
    fn a_corner_drag_scales_both_dimensions_alike() {
        let mut t = transform_of(100, 100);
        t.begin_drag(Handle::BottomRight, Vec2::new(100.0, 100.0));
        t.drag_to(Vec2::new(200.0, 120.0), false, false, false);
        let (sw, sh) = t.scale_percent();
        assert!((sw - sh).abs() < 0.5, "aspect drifted: {sw} vs {sh}");
        // The corner follows the pointer's projection onto the diagonal:
        // (200, 120) · (100, 100) over |(100, 100)|² is 1.6.
        assert!((sw - 160.0).abs() < 0.5, "got {sw}%");
        assert_eq!(t.corners[0], Vec2::new(0.0, 0.0), "the anchor stays put");
    }

    #[test]
    fn shift_lets_a_corner_drag_scale_the_axes_apart() {
        let mut t = transform_of(100, 100);
        t.begin_drag(Handle::BottomRight, Vec2::new(100.0, 100.0));
        t.drag_to(Vec2::new(200.0, 120.0), false, true, false);
        let (sw, sh) = t.scale_percent();
        assert!((sw - 200.0).abs() < 0.5 && (sh - 120.0).abs() < 0.5, "got {sw} x {sh}");
    }

    /// Two edges of a box are perpendicular when their dot product is zero.
    fn right_angled(t: &ActiveTransform) -> bool {
        let u = t.corners[1] - t.corners[0];
        let v = t.corners[3] - t.corners[0];
        (u.x * v.x + u.y * v.y).abs() < 1e-2 * u.length() * v.length()
    }

    #[test]
    fn a_turned_box_scales_along_its_own_edges() {
        let mut t = transform_of(100, 100);
        // A quarter turn, then some: the box's axes no longer line up with
        // the document's.
        t.begin_drag(Handle::Rotate, Vec2::new(150.0, 50.0));
        t.drag_to(Vec2::new(120.0, 120.0), false, false, false);
        t.end_drag();
        let height_before = t.corners[0].distance(t.corners[3]);
        let right = t.handle_position(Handle::Right);
        let left = t.handle_position(Handle::Left);
        // Pull the right edge straight out along the box's own width.
        let outward = right - left;
        t.begin_drag(Handle::Right, right);
        t.drag_to(right + outward * 0.5, false, false, false);
        t.end_drag();
        assert!(right_angled(&t), "an edge drag skewed the turned box: {:?}", t.corners);
        let height_after = t.corners[0].distance(t.corners[3]);
        assert!((height_after - height_before).abs() < 0.1, "the other axis moved");
        let (sw, _) = t.scale_percent();
        assert!((sw - 150.0).abs() < 1.0, "the width should read 150%, got {sw}");

        // And a corner, which used to skew the turned box worst of all.
        let br = t.handle_position(Handle::BottomRight);
        let tl = t.handle_position(Handle::TopLeft);
        t.begin_drag(Handle::BottomRight, br);
        t.drag_to(tl + (br - tl) * 2.0, false, false, false);
        t.end_drag();
        assert!(right_angled(&t), "a corner drag skewed the turned box: {:?}", t.corners);
        let (sw, sh) = t.scale_percent();
        assert!((sw - 300.0).abs() < 1.0 && (sh - 200.0).abs() < 1.0, "got {sw} x {sh}");
    }

    #[test]
    fn a_transform_steps_back_through_its_own_gestures() {
        let mut t = transform_of(100, 100);
        assert!(!t.can_undo() && !t.can_redo());

        t.begin_drag(Handle::Body, Vec2::new(50.0, 50.0));
        t.drag_to(Vec2::new(60.0, 50.0), false, false, false);
        t.end_drag();
        t.begin_drag(Handle::BottomRight, Vec2::new(110.0, 100.0));
        t.drag_to(Vec2::new(210.0, 200.0), false, false, false);
        t.end_drag();
        assert!(t.can_undo());

        assert!(t.undo(), "the scale steps back");
        assert_eq!(t.corners[0], Vec2::new(10.0, 0.0));
        assert_eq!(t.corners[2], Vec2::new(110.0, 100.0));
        assert!(t.modified && t.can_redo());

        assert!(t.undo(), "then the move");
        assert!(t.matrix().unwrap().is_identity());
        assert!(!t.modified, "back at the start, there is nothing to apply");
        assert!(!t.undo(), "and nothing further to step back to");

        assert!(t.redo());
        assert_eq!(t.corners[0], Vec2::new(10.0, 0.0));
        assert!(t.modified);

        // A new gesture forgets what was stepped back from.
        t.begin_drag(Handle::Body, Vec2::ZERO);
        t.drag_to(Vec2::new(0.0, 5.0), false, false, false);
        t.end_drag();
        assert!(!t.can_redo());
    }

    #[test]
    fn a_gesture_that_ends_where_it_began_records_nothing() {
        let mut t = transform_of(100, 100);
        t.begin_drag(Handle::Body, Vec2::new(50.0, 50.0));
        t.drag_to(Vec2::new(70.0, 50.0), false, false, false);
        t.drag_to(Vec2::new(50.0, 50.0), false, false, false);
        t.end_drag();
        assert!(!t.can_undo());
        assert!(!t.modified);
    }

    #[test]
    fn reset_is_a_gesture_that_can_be_stepped_back() {
        let mut t = transform_of(100, 100);
        t.begin_drag(Handle::Body, Vec2::ZERO);
        t.drag_to(Vec2::new(30.0, 0.0), false, false, false);
        t.end_drag();
        t.reset();
        assert!(t.matrix().unwrap().is_identity());
        assert!(t.undo(), "reset can be stepped back");
        assert_eq!(t.corners[0], Vec2::new(30.0, 0.0));
    }

    #[test]
    fn alt_scales_from_the_centre() {
        let mut t = transform_of(100, 100);
        t.begin_drag(Handle::BottomRight, Vec2::new(100.0, 100.0));
        t.drag_to(Vec2::new(150.0, 150.0), false, false, true);
        let centre = t.centre();
        assert!(
            (centre.x - 50.0).abs() < 0.01 && (centre.y - 50.0).abs() < 0.01,
            "the centre should not move, got {centre:?}"
        );
        assert!(t.corners[0].x < 0.0, "the far corner should move outward too");
    }

    #[test]
    fn ctrl_distorts_a_single_corner() {
        let mut t = transform_of(100, 100);
        t.begin_drag(Handle::TopRight, Vec2::new(100.0, 0.0));
        t.drag_to(Vec2::new(140.0, 30.0), true, false, false);
        assert_eq!(t.corners[1], Vec2::new(140.0, 30.0), "only the dragged corner moves");
        assert_eq!(t.corners[0], Vec2::new(0.0, 0.0));
        assert_eq!(t.corners[2], Vec2::new(100.0, 100.0));
        // The result is a genuine perspective transform.
        assert!(t.matrix().is_some());
    }

    #[test]
    fn rotation_turns_the_quad_about_its_centre() {
        let mut t = transform_of(100, 100);
        t.begin_drag(Handle::Rotate, Vec2::new(150.0, 50.0));
        // A quarter turn about the centre.
        t.drag_to(Vec2::new(50.0, 150.0), false, false, false);
        let centre = t.centre();
        assert!((centre.x - 50.0).abs() < 0.1 && (centre.y - 50.0).abs() < 0.1);
        assert!((t.rotation_degrees().abs() - 90.0).abs() < 1.0, "got {}", t.rotation_degrees());
    }

    #[test]
    fn dragging_a_handle_onto_its_anchor_does_not_explode() {
        let mut t = transform_of(100, 100);
        t.begin_drag(Handle::BottomRight, Vec2::new(100.0, 100.0));
        // Straight onto the opposite corner, which is a zero-size box.
        t.drag_to(Vec2::new(0.0, 0.0), false, false, false);
        for c in t.corners {
            assert!(c.x.is_finite() && c.y.is_finite(), "corner went non-finite: {c:?}");
        }
    }

    #[test]
    fn hit_testing_is_handles_then_inside_to_move_then_outside_to_rotate() {
        let t = transform_of(100, 100);
        assert_eq!(t.hit(Vec2::new(0.0, 0.0), 1.0), Handle::TopLeft);
        assert_eq!(t.hit(Vec2::new(50.0, 50.0), 1.0), Handle::Body);
        // Inside but near a corner is still the body: on a small layer the
        // old rotation ring reached most of the picture.
        assert_eq!(t.hit(Vec2::new(14.0, 14.0), 1.0), Handle::Body);
        // Anywhere outside rotates, near a corner or far from one.
        assert_eq!(t.hit(Vec2::new(-15.0, -15.0), 1.0), Handle::Rotate);
        assert_eq!(t.hit(Vec2::new(150.0, 50.0), 1.0), Handle::Rotate);
        assert_eq!(t.hit(Vec2::new(500.0, 500.0), 1.0), Handle::Rotate);
    }

    #[test]
    fn the_grab_radius_scales_with_zoom() {
        let t = transform_of(100, 100);
        // At 10x zoom, a handle covers a tenth as many document pixels.
        assert_eq!(t.hit(Vec2::new(5.0, 0.0), 1.0), Handle::TopLeft);
        assert_ne!(t.hit(Vec2::new(5.0, 0.0), 10.0), Handle::TopLeft);
    }

    #[test]
    fn reset_returns_to_the_source_rectangle() {
        let mut t = transform_of(80, 40);
        t.begin_drag(Handle::Body, Vec2::ZERO);
        t.drag_to(Vec2::new(33.0, 21.0), false, false, false);
        t.end_drag();
        t.reset();
        assert!(t.matrix().unwrap().is_identity());
        assert!(!t.modified);
    }

    #[test]
    fn rendering_produces_transformed_pixels() {
        let mut t = transform_of(40, 40);
        t.begin_drag(Handle::BottomRight, Vec2::new(40.0, 40.0));
        t.drag_to(Vec2::new(80.0, 80.0), false, false, false);
        let (px, offset) = t.render(IRect::new(0, 0, 200, 200)).expect("renders");
        assert_eq!(offset, (0, 0));
        assert_eq!((px.width(), px.height()), (80, 80));
        assert_eq!(px.get(40, 40).a, 255);
    }

    // --- crop --------------------------------------------------------------

    #[test]
    fn a_crop_handle_moves_one_edge() {
        let mut c = ActiveCrop::new(IRect::new(10, 10, 90, 90));
        c.begin_drag(Handle::Left, Vec2::new(10.0, 50.0));
        c.drag_to(Vec2::new(30.0, 50.0), IRect::new(0, 0, 100, 100));
        assert_eq!(c.rect, IRect::new(30, 10, 90, 90));
    }

    #[test]
    fn a_crop_stays_inside_the_canvas() {
        let mut c = ActiveCrop::new(IRect::new(10, 10, 90, 90));
        c.begin_drag(Handle::Body, Vec2::new(50.0, 50.0));
        c.drag_to(Vec2::new(500.0, 500.0), IRect::new(0, 0, 100, 100));
        assert!(c.rect.x1 <= 100 && c.rect.y1 <= 100, "escaped the canvas: {:?}", c.rect);
        assert!(!c.rect.is_empty());
    }

    #[test]
    fn dragging_a_crop_edge_past_its_opposite_flips_it() {
        let mut c = ActiveCrop::new(IRect::new(40, 40, 60, 60));
        c.begin_drag(Handle::Right, Vec2::new(60.0, 50.0));
        c.drag_to(Vec2::new(10.0, 50.0), IRect::new(0, 0, 100, 100));
        assert!(c.rect.x0 < c.rect.x1, "rect inverted: {:?}", c.rect);
        assert!(!c.rect.is_empty());
    }

    #[test]
    fn a_locked_aspect_ratio_is_maintained() {
        let mut c = ActiveCrop::new(IRect::new(0, 0, 100, 100));
        c.aspect = Some(2.0);
        c.begin_drag(Handle::Right, Vec2::new(100.0, 50.0));
        c.drag_to(Vec2::new(200.0, 50.0), IRect::new(0, 0, 400, 400));
        let ratio = c.rect.width() as f32 / c.rect.height() as f32;
        assert!((ratio - 2.0).abs() < 0.1, "ratio drifted to {ratio}");
    }
}
