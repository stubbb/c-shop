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
        let corners = [
            Vec2::new(rect.x0 as f32, rect.y0 as f32),
            Vec2::new(rect.x1 as f32, rect.y0 as f32),
            Vec2::new(rect.x1 as f32, rect.y1 as f32),
            Vec2::new(rect.x0 as f32, rect.y1 as f32),
        ];

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

    /// Which handle a document-space point grabs, given the current zoom.
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
        // Outside a corner but close to it means rotate.
        let outer = grab * 3.0;
        let near_corner = Handle::CORNERS
            .iter()
            .any(|h| self.handle_position(*h).distance(point) <= outer);
        if near_corner {
            return Some(Handle::Rotate);
        }
        if self.contains(point) {
            return Some(Handle::Body);
        }
        None
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
    /// `distort` (Ctrl) frees a corner from the rectangle; `constrain` (Shift)
    /// keeps proportions or snaps rotation; `from_centre` (Alt) anchors the
    /// pivot at the middle.
    pub fn drag_to(&mut self, at: Vec2, distort: bool, constrain: bool, from_centre: bool) {
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
                if constrain {
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

                // Scale along whichever axes this handle controls.
                let unit = h.unit_position();
                let scale_x = if unit.x == 0.5 { 1.0 } else { safe_ratio(after.x, before.x) };
                let scale_y = if unit.y == 0.5 { 1.0 } else { safe_ratio(after.y, before.y) };
                let (mut sx, mut sy) = (scale_x, scale_y);
                if constrain && unit.x != 0.5 && unit.y != 0.5 {
                    // Corner handles keep the aspect ratio under Shift.
                    let s = sx.abs().max(sy.abs());
                    sx = s * sx.signum();
                    sy = s * sy.signum();
                }

                let t = Transform::about(anchor, Transform::scale(sx, sy));
                for (i, c) in self.corners.iter_mut().enumerate() {
                    *c = t.apply(start[i]);
                }
            }
        }
    }

    pub fn end_drag(&mut self) {
        self.dragging = None;
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

    /// Reset to the untransformed rectangle.
    pub fn reset(&mut self) {
        let rect = self.source_rect;
        self.corners = [
            Vec2::new(rect.x0 as f32, rect.y0 as f32),
            Vec2::new(rect.x1 as f32, rect.y0 as f32),
            Vec2::new(rect.x1 as f32, rect.y1 as f32),
            Vec2::new(rect.x0 as f32, rect.y1 as f32),
        ];
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
    fn shift_keeps_a_corner_drag_proportional() {
        let mut t = transform_of(100, 100);
        t.begin_drag(Handle::BottomRight, Vec2::new(100.0, 100.0));
        t.drag_to(Vec2::new(200.0, 120.0), false, true, false);
        let (sw, sh) = t.scale_percent();
        assert!((sw - sh).abs() < 0.5, "aspect drifted: {sw} vs {sh}");
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
    fn hit_testing_prefers_handles_then_rotation_then_the_body() {
        let t = transform_of(100, 100);
        assert_eq!(t.hit(Vec2::new(0.0, 0.0), 1.0), Some(Handle::TopLeft));
        assert_eq!(t.hit(Vec2::new(50.0, 50.0), 1.0), Some(Handle::Body));
        // Just outside a corner is the rotation ring.
        assert_eq!(t.hit(Vec2::new(-15.0, -15.0), 1.0), Some(Handle::Rotate));
        assert_eq!(t.hit(Vec2::new(500.0, 500.0), 1.0), None);
    }

    #[test]
    fn the_grab_radius_scales_with_zoom() {
        let t = transform_of(100, 100);
        // At 10x zoom, a handle covers a tenth as many document pixels.
        assert_eq!(t.hit(Vec2::new(5.0, 0.0), 1.0), Some(Handle::TopLeft));
        assert_ne!(t.hit(Vec2::new(5.0, 0.0), 10.0), Some(Handle::TopLeft));
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
