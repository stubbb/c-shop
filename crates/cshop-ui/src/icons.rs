//! Vector icons, drawn rather than typed.
//!
//! The first attempt used Unicode glyphs, which failed: egui bundles a
//! proportional font plus a small emoji subset, and most of the symbols a
//! toolbar needs (a pipette, a lasso, a paint bucket) are in neither, so they
//! rendered as missing-glyph boxes. Drawing them means they are always present,
//! stay crisp at any scale, and take the interface's own colours.
//!
//! Every icon is authored in a 0..1 square and mapped onto the target rect, so
//! one definition serves every size.

use crate::theme::Palette;
use crate::tools::Tool;
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};

/// Map a unit-square coordinate onto `rect`, with a small inset so strokes do
/// not clip against the edge.
#[inline]
fn p(rect: Rect, x: f32, y: f32) -> Pos2 {
    let inset = rect.size() * 0.12;
    let inner = Rect::from_min_size(rect.min + inset, rect.size() - inset * 2.0);
    Pos2::new(inner.min.x + x * inner.width(), inner.min.y + y * inner.height())
}

fn stroke(rect: Rect, color: Color32) -> Stroke {
    // Scale line weight with the icon so it stays proportionate.
    Stroke::new((rect.width() * 0.075).clamp(1.0, 2.0), color)
}

fn line(painter: &Painter, rect: Rect, color: Color32, pts: &[(f32, f32)]) {
    let s = stroke(rect, color);
    let points: Vec<Pos2> = pts.iter().map(|&(x, y)| p(rect, x, y)).collect();
    painter.add(Shape::line(points, s));
}

fn polygon(painter: &Painter, rect: Rect, color: Color32, pts: &[(f32, f32)]) {
    let mut points: Vec<Pos2> = pts.iter().map(|&(x, y)| p(rect, x, y)).collect();
    if let Some(first) = points.first().copied() {
        points.push(first);
    }
    painter.add(Shape::line(points, stroke(rect, color)));
}

fn filled(painter: &Painter, rect: Rect, color: Color32, pts: &[(f32, f32)]) {
    let points: Vec<Pos2> = pts.iter().map(|&(x, y)| p(rect, x, y)).collect();
    painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// A rectangle drawn as a marching-ants dashed outline, for the marquee tools.
fn dashed_rect(painter: &Painter, rect: Rect, color: Color32, r: (f32, f32, f32, f32)) {
    let (x0, y0, x1, y1) = r;
    let s = stroke(rect, color);
    let dash = rect.width() * 0.11;
    for (a, b) in [
        ((x0, y0), (x1, y0)),
        ((x1, y0), (x1, y1)),
        ((x1, y1), (x0, y1)),
        ((x0, y1), (x0, y0)),
    ] {
        painter.add(Shape::dashed_line(
            &[p(rect, a.0, a.1), p(rect, b.0, b.1)],
            s,
            dash,
            dash,
        ));
    }
}

fn dashed_ellipse(painter: &Painter, rect: Rect, color: Color32) {
    let s = stroke(rect, color);
    let dash = rect.width() * 0.10;
    let centre = p(rect, 0.5, 0.5);
    let (rx, ry) = (rect.width() * 0.36, rect.height() * 0.30);
    let mut pts = Vec::with_capacity(33);
    for i in 0..=32 {
        let t = i as f32 / 32.0 * std::f32::consts::TAU;
        pts.push(Pos2::new(centre.x + rx * t.cos(), centre.y + ry * t.sin()));
    }
    painter.add(Shape::dashed_line(&pts, s, dash, dash));
}

/// Draw the icon for `tool` inside `rect`.
pub fn tool(painter: &Painter, rect: Rect, tool: Tool, color: Color32) {
    match tool {
        // A pointer arrow, the usual Move icon.
        Tool::Move => {
            filled(
                painter,
                rect,
                color,
                &[(0.22, 0.06), (0.22, 0.82), (0.42, 0.63), (0.55, 0.95), (0.70, 0.87), (0.57, 0.57), (0.82, 0.55)],
            );
        }

        Tool::RectangularMarquee => dashed_rect(painter, rect, color, (0.05, 0.15, 0.95, 0.85)),
        Tool::EllipticalMarquee => dashed_ellipse(painter, rect, color),

        // An open loop with a trailing tail.
        Tool::Lasso => {
            let centre = p(rect, 0.5, 0.42);
            let (rx, ry) = (rect.width() * 0.32, rect.height() * 0.26);
            let mut pts = Vec::new();
            for i in 0..=28 {
                let t = 0.45 + i as f32 / 28.0 * (std::f32::consts::TAU * 0.92);
                pts.push(Pos2::new(centre.x + rx * t.cos(), centre.y + ry * t.sin()));
            }
            painter.add(Shape::line(pts, stroke(rect, color)));
            line(painter, rect, color, &[(0.62, 0.68), (0.55, 0.95)]);
        }

        Tool::PolygonalLasso => {
            polygon(painter, rect, color, &[(0.10, 0.30), (0.50, 0.06), (0.92, 0.38), (0.70, 0.90), (0.24, 0.78)]);
        }

        // A wand with a sparkle at the tip.
        Tool::MagicWand => {
            line(painter, rect, color, &[(0.10, 0.92), (0.66, 0.34)]);
            line(painter, rect, color, &[(0.78, 0.22), (0.78, 0.04)]);
            line(painter, rect, color, &[(0.78, 0.22), (0.96, 0.22)]);
            line(painter, rect, color, &[(0.66, 0.10), (0.90, 0.34)]);
        }

        // Two overlapping right angles.
        Tool::Crop => {
            line(painter, rect, color, &[(0.24, 0.00), (0.24, 0.76), (1.00, 0.76)]);
            line(painter, rect, color, &[(0.00, 0.24), (0.76, 0.24), (0.76, 1.00)]);
        }

        // A pipette: barrel plus tip.
        Tool::Eyedropper => {
            line(painter, rect, color, &[(0.95, 0.05), (0.55, 0.45)]);
            polygon(painter, rect, color, &[(0.62, 0.24), (0.86, 0.48), (0.72, 0.62), (0.48, 0.38)]);
            filled(painter, rect, color, &[(0.44, 0.44), (0.58, 0.58), (0.20, 0.92), (0.08, 0.96), (0.12, 0.82)]);
        }

        // A round brush with a ferrule.
        Tool::Brush => {
            line(painter, rect, color, &[(0.92, 0.08), (0.46, 0.54)]);
            polygon(painter, rect, color, &[(0.36, 0.44), (0.58, 0.66), (0.40, 0.84), (0.18, 0.62)]);
            filled(painter, rect, color, &[(0.18, 0.62), (0.40, 0.84), (0.16, 0.96), (0.06, 0.86)]);
        }

        // Same body, but a sharp point.
        Tool::Pencil => {
            polygon(painter, rect, color, &[(0.70, 0.02), (0.98, 0.30), (0.36, 0.92), (0.06, 0.98), (0.12, 0.66)]);
            line(painter, rect, color, &[(0.12, 0.66), (0.36, 0.92)]);
        }

        // A rubber stamp: handle, collar and pad.
        Tool::CloneStamp => {
            line(painter, rect, color, &[(0.5, 0.04), (0.5, 0.26)]);
            polygon(painter, rect, color, &[(0.30, 0.26), (0.70, 0.26), (0.74, 0.46), (0.26, 0.46)]);
            let collar = Rect::from_min_max(p(rect, 0.16, 0.50), p(rect, 0.84, 0.62));
            painter.rect_filled(collar, 1.0, color);
            line(painter, rect, color, &[(0.10, 0.94), (0.90, 0.94)]);
        }

        // The darkroom tools these are named after. Dodge is the paddle — a
        // disc of card on a wire, held between the enlarger and the paper.
        Tool::Dodge => {
            let centre = p(rect, 0.62, 0.32);
            let r = rect.width() * 0.20;
            painter.circle_stroke(centre, r, stroke(rect, color));
            line(painter, rect, color, &[(0.48, 0.46), (0.10, 0.92)]);
        }

        // Burn is the cupped hands: two arcs with the light coming through
        // the gap between them.
        Tool::Burn => {
            for (from, flip) in [(0.30f32, 1.0f32), (0.70, -1.0)] {
                let mut pts = Vec::new();
                for i in 0..=16 {
                    let t = std::f32::consts::PI * (i as f32 / 16.0) * flip;
                    pts.push(p(rect, from + 0.26 * flip * (1.0 - t.cos()) * 0.5, 0.5 - 0.42 * t.sin()));
                }
                painter.add(Shape::line(pts, stroke(rect, color)));
            }
            line(painter, rect, color, &[(0.10, 0.94), (0.90, 0.94)]);
        }

        // A sponge: a rounded body with holes in it.
        Tool::Sponge => {
            polygon(
                painter,
                rect,
                color,
                &[(0.14, 0.34), (0.34, 0.14), (0.72, 0.16), (0.90, 0.40), (0.82, 0.82), (0.30, 0.88)],
            );
            for (x, y, r) in [(0.36f32, 0.40f32, 0.07f32), (0.62, 0.36, 0.05), (0.50, 0.64, 0.06)] {
                painter.circle_filled(p(rect, x, y), rect.width() * r, color);
            }
        }

        // A plaster: a rectangle at an angle with a pad in the middle.
        Tool::HealingBrush => {
            polygon(
                painter,
                rect,
                color,
                &[(0.06, 0.36), (0.36, 0.06), (0.94, 0.64), (0.64, 0.94)],
            );
            line(painter, rect, color, &[(0.30, 0.60), (0.60, 0.30)]);
            line(painter, rect, color, &[(0.40, 0.70), (0.70, 0.40)]);
        }

        // The same idea with a spot marked on it: no source to set.
        Tool::SpotHealing => {
            polygon(
                painter,
                rect,
                color,
                &[(0.06, 0.36), (0.36, 0.06), (0.94, 0.64), (0.64, 0.94)],
            );
            painter.circle_filled(p(rect, 0.50, 0.50), rect.width() * 0.11, color);
        }

        // An arrow curving back on itself: paint the way it was.
        Tool::HistoryBrush => {
            let centre = p(rect, 0.50, 0.54);
            let r = rect.width() * 0.30;
            let mut pts = Vec::new();
            for i in 0..=24 {
                let t = std::f32::consts::TAU * (0.10 + i as f32 / 24.0 * 0.80);
                pts.push(Pos2::new(centre.x + r * t.sin(), centre.y - r * t.cos()));
            }
            painter.add(Shape::line(pts, stroke(rect, color)));
            polygon(painter, rect, color, &[(0.30, 0.06), (0.30, 0.34), (0.06, 0.20)]);
        }

        // A water drop, for softening.
        Tool::Blur => {
            let mut pts = vec![(0.50f32, 0.06f32)];
            for i in 0..=20 {
                let t = std::f32::consts::PI * (-0.5 + i as f32 / 20.0 * 2.0);
                pts.push((0.50 + 0.30 * t.sin(), 0.60 - 0.32 * t.cos()));
            }
            polygon(painter, rect, color, &pts);
        }

        // A cone: the point that sharpening puts back.
        Tool::Sharpen => {
            polygon(painter, rect, color, &[(0.50, 0.06), (0.76, 0.74), (0.24, 0.74)]);
            line(painter, rect, color, &[(0.14, 0.92), (0.86, 0.92)]);
        }

        // A fingertip drawing a line away from itself.
        Tool::Smudge => {
            polygon(
                painter,
                rect,
                color,
                &[(0.40, 0.10), (0.62, 0.20), (0.66, 0.52), (0.42, 0.62), (0.30, 0.40)],
            );
            line(painter, rect, color, &[(0.30, 0.72), (0.86, 0.88)]);
        }

        // A slanted block with a baseline.
        Tool::Eraser => {
            polygon(painter, rect, color, &[(0.44, 0.06), (0.96, 0.42), (0.58, 0.76), (0.06, 0.40)]);
            line(painter, rect, color, &[(0.06, 0.94), (0.94, 0.94)]);
        }

        // A tipped bucket with a drop.
        Tool::PaintBucket => {
            polygon(painter, rect, color, &[(0.14, 0.34), (0.62, 0.06), (0.94, 0.52), (0.46, 0.82)]);
            line(painter, rect, color, &[(0.30, 0.24), (0.18, 0.10)]);
            filled(painter, rect, color, &[(0.86, 0.70), (0.98, 0.86), (0.86, 0.98), (0.76, 0.86)]);
        }

        // A square with a light-to-dark ramp.
        Tool::Gradient => {
            let r = Rect::from_min_max(p(rect, 0.02, 0.18), p(rect, 0.98, 0.82));
            let steps = 7;
            for i in 0..steps {
                let t = i as f32 / (steps - 1) as f32;
                let band = Rect::from_min_max(
                    Pos2::new(r.min.x + r.width() * (i as f32 / steps as f32), r.min.y),
                    Pos2::new(r.min.x + r.width() * ((i + 1) as f32 / steps as f32), r.max.y),
                );
                painter.rect_filled(band, 0.0, color.gamma_multiply(0.15 + 0.85 * t));
            }
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, color), StrokeKind::Inside);
        }

        // A serif "T", drawn so it does not depend on the font.
        Tool::Text => {
            line(painter, rect, color, &[(0.12, 0.10), (0.88, 0.10)]);
            line(painter, rect, color, &[(0.50, 0.10), (0.50, 0.90)]);
            line(painter, rect, color, &[(0.30, 0.90), (0.70, 0.90)]);
        }

        Tool::Shape => {
            let r = Rect::from_min_max(p(rect, 0.06, 0.20), p(rect, 0.94, 0.80));
            painter.rect_filled(r, 1.0, color);
        }

        // The hollow arrow every vector editor uses for editing points.
        Tool::DirectSelect => {
            let pts = [(0.30, 0.10), (0.72, 0.52), (0.52, 0.54), (0.62, 0.86), (0.46, 0.90), (0.38, 0.60), (0.24, 0.72)];
            let path: Vec<_> = pts.iter().map(|(x, y)| p(rect, *x, *y)).collect();
            painter.add(egui::Shape::closed_line(path, egui::Stroke::new(1.4, color)));
        }

        // A nib: a wedge running to a point, split down the middle.
        Tool::Pen => {
            filled(
                painter,
                rect,
                color,
                &[(0.50, 0.06), (0.78, 0.44), (0.62, 0.78), (0.38, 0.78), (0.22, 0.44)],
            );
            painter.line_segment(
                [p(rect, 0.50, 0.30), p(rect, 0.50, 0.94)],
                egui::Stroke::new(1.5, color),
            );
        }

        // A mitten: palm plus four fingers.
        Tool::Hand => {
            filled(
                painter,
                rect,
                color,
                &[(0.20, 0.52), (0.20, 0.34), (0.34, 0.34), (0.34, 0.16), (0.48, 0.16), (0.48, 0.10), (0.62, 0.10), (0.62, 0.20), (0.76, 0.20), (0.80, 0.62), (0.62, 0.96), (0.30, 0.94)],
            );
        }

        // A loupe.
        Tool::Zoom => {
            let centre = p(rect, 0.42, 0.42);
            painter.circle_stroke(centre, rect.width() * 0.26, stroke(rect, color));
            line(painter, rect, color, &[(0.66, 0.66), (0.96, 0.96)]);
        }
    }
}

/// Icons used outside the toolbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Eye,
    EyeOff,
    LockTransparency,
    LockPixels,
    LockPosition,
    Lock,
    Plus,
    Trash,
    Duplicate,
    MergeDown,
    Folder,
    ChevronDown,
    ChevronRight,
    Swap,
    Reset,
    Undo,
    Redo,
    Clip,
    Mask,
    Close,
    Home,
    Up,
    File,
    /// Boolean selection modes, drawn as two overlapping squares showing which
    /// parts survive, which is the visual language an options bar wants.
    SelectNew,
    SelectAdd,
    SelectSubtract,
    SelectIntersect,
    QuickMask,
    MaskLink,
    /// Collapse the whole stack into one layer.
    Flatten,
}

/// Draw a general interface icon inside `rect`.
pub fn icon(painter: &Painter, rect: Rect, which: Icon, color: Color32) {
    match which {
        // An eye: two arcs and a pupil.
        Icon::Eye | Icon::EyeOff => {
            let s = stroke(rect, color);
            let mut top = Vec::new();
            let mut bottom = Vec::new();
            for i in 0..=16 {
                let t = i as f32 / 16.0;
                let x = 0.04 + t * 0.92;
                let bulge = (t * std::f32::consts::PI).sin() * 0.30;
                top.push(p(rect, x, 0.5 - bulge));
                bottom.push(p(rect, x, 0.5 + bulge));
            }
            painter.add(Shape::line(top, s));
            painter.add(Shape::line(bottom, s));
            if which == Icon::Eye {
                painter.circle_filled(p(rect, 0.5, 0.5), rect.width() * 0.11, color);
            } else {
                // A slash marks the layer as hidden.
                line(painter, rect, color, &[(0.10, 0.90), (0.90, 0.10)]);
            }
        }

        // A padlock; the variants differ by what sits in the body.
        Icon::Lock | Icon::LockTransparency | Icon::LockPixels | Icon::LockPosition => {
            let body = Rect::from_min_max(p(rect, 0.16, 0.44), p(rect, 0.84, 0.94));
            painter.rect_stroke(body, 1.0, stroke(rect, color), StrokeKind::Inside);
            let mut shackle = Vec::new();
            for i in 0..=12 {
                let t = i as f32 / 12.0;
                let a = std::f32::consts::PI * (1.0 + t);
                shackle.push(p(rect, 0.5 + 0.22 * a.cos(), 0.42 + 0.24 * a.sin()));
            }
            painter.add(Shape::line(shackle, stroke(rect, color)));

            match which {
                // Checkerboard: locks transparency.
                Icon::LockTransparency => {
                    let q = body.shrink(body.width() * 0.22);
                    let h = q.size() / 2.0;
                    painter.rect_filled(Rect::from_min_size(q.min, h), 0.0, color);
                    painter.rect_filled(Rect::from_min_size(q.center(), h), 0.0, color);
                }
                // A dot: locks pixels.
                Icon::LockPixels => {
                    painter.circle_filled(body.center(), body.width() * 0.16, color);
                }
                // A cross: locks position.
                Icon::LockPosition => {
                    let s = Stroke::new(1.0, color);
                    let c = body.center();
                    let r = body.width() * 0.22;
                    painter.line_segment([c - Vec2::new(r, 0.0), c + Vec2::new(r, 0.0)], s);
                    painter.line_segment([c - Vec2::new(0.0, r), c + Vec2::new(0.0, r)], s);
                }
                _ => {}
            }
        }

        Icon::Plus => {
            line(painter, rect, color, &[(0.5, 0.08), (0.5, 0.92)]);
            line(painter, rect, color, &[(0.08, 0.5), (0.92, 0.5)]);
        }

        Icon::Close => {
            line(painter, rect, color, &[(0.15, 0.15), (0.85, 0.85)]);
            line(painter, rect, color, &[(0.85, 0.15), (0.15, 0.85)]);
        }

        // A bin with a lid.
        Icon::Trash => {
            line(painter, rect, color, &[(0.08, 0.20), (0.92, 0.20)]);
            line(painter, rect, color, &[(0.38, 0.20), (0.42, 0.08), (0.58, 0.08), (0.62, 0.20)]);
            polygon(painter, rect, color, &[(0.18, 0.26), (0.82, 0.26), (0.74, 0.94), (0.26, 0.94)]);
        }

        // Two offset sheets.
        Icon::Duplicate => {
            let back = Rect::from_min_max(p(rect, 0.06, 0.06), p(rect, 0.66, 0.66));
            let front = Rect::from_min_max(p(rect, 0.34, 0.34), p(rect, 0.94, 0.94));
            painter.rect_stroke(back, 1.0, stroke(rect, color), StrokeKind::Inside);
            painter.rect_stroke(front, 1.0, stroke(rect, color), StrokeKind::Inside);
        }

        // An arrow onto a baseline.
        Icon::MergeDown => {
            line(painter, rect, color, &[(0.5, 0.06), (0.5, 0.62)]);
            line(painter, rect, color, &[(0.26, 0.40), (0.5, 0.66), (0.74, 0.40)]);
            line(painter, rect, color, &[(0.10, 0.90), (0.90, 0.90)]);
        }

        Icon::Folder => {
            polygon(
                painter,
                rect,
                color,
                &[(0.06, 0.86), (0.06, 0.20), (0.40, 0.20), (0.50, 0.32), (0.94, 0.32), (0.94, 0.86)],
            );
        }

        Icon::File => {
            polygon(painter, rect, color, &[(0.18, 0.06), (0.64, 0.06), (0.86, 0.30), (0.86, 0.94), (0.18, 0.94)]);
            line(painter, rect, color, &[(0.64, 0.06), (0.64, 0.30), (0.86, 0.30)]);
        }

        Icon::ChevronDown => line(painter, rect, color, &[(0.22, 0.36), (0.5, 0.66), (0.78, 0.36)]),
        Icon::ChevronRight => line(painter, rect, color, &[(0.36, 0.22), (0.66, 0.5), (0.36, 0.78)]),

        // Two arrows curving past each other.
        Icon::Swap => {
            line(painter, rect, color, &[(0.10, 0.32), (0.90, 0.32)]);
            line(painter, rect, color, &[(0.72, 0.14), (0.90, 0.32), (0.72, 0.50)]);
            line(painter, rect, color, &[(0.90, 0.72), (0.10, 0.72)]);
            line(painter, rect, color, &[(0.28, 0.54), (0.10, 0.72), (0.28, 0.90)]);
        }

        // The default black-over-white swatch pair.
        Icon::Reset => {
            let a = Rect::from_min_max(p(rect, 0.04, 0.04), p(rect, 0.62, 0.62));
            let b = Rect::from_min_max(p(rect, 0.38, 0.38), p(rect, 0.96, 0.96));
            painter.rect_filled(b, 0.0, Color32::WHITE);
            painter.rect_stroke(b, 0.0, Stroke::new(1.0, color), StrokeKind::Inside);
            painter.rect_filled(a, 0.0, Color32::BLACK);
            painter.rect_stroke(a, 0.0, Stroke::new(1.0, color), StrokeKind::Inside);
        }

        Icon::Undo | Icon::Redo => {
            let flip = which == Icon::Redo;
            let fx = |x: f32| if flip { 1.0 - x } else { x };
            let mut arc = Vec::new();
            for i in 0..=16 {
                let t = i as f32 / 16.0;
                let a = std::f32::consts::PI * (0.15 + t * 0.9);
                arc.push(p(rect, fx(0.5 + 0.38 * a.cos()), 0.62 - 0.34 * a.sin()));
            }
            painter.add(Shape::line(arc, stroke(rect, color)));
            line(painter, rect, color, &[(fx(0.10), 0.30), (fx(0.12), 0.62), (fx(0.42), 0.58)]);
        }

        // The right-angled arrow that marks a clipped layer.
        Icon::Clip => line(painter, rect, color, &[(0.24, 0.14), (0.24, 0.72), (0.80, 0.72)]),

        // A square, half filled: the layer-mask badge.
        Icon::Mask => {
            let r = Rect::from_min_max(p(rect, 0.10, 0.16), p(rect, 0.90, 0.84));
            painter.rect_stroke(r, 0.0, stroke(rect, color), StrokeKind::Inside);
            let half = Rect::from_min_max(r.min, Pos2::new(r.center().x, r.max.y));
            painter.rect_filled(half, 0.0, color);
        }

        Icon::Home => {
            line(painter, rect, color, &[(0.06, 0.50), (0.50, 0.10), (0.94, 0.50)]);
            polygon(painter, rect, color, &[(0.18, 0.46), (0.82, 0.46), (0.82, 0.92), (0.18, 0.92)]);
        }

        Icon::Up => {
            line(painter, rect, color, &[(0.5, 0.90), (0.5, 0.14)]);
            line(painter, rect, color, &[(0.22, 0.42), (0.5, 0.12), (0.78, 0.42)]);
        }

        Icon::SelectNew | Icon::SelectAdd | Icon::SelectSubtract | Icon::SelectIntersect => {
            let a = Rect::from_min_max(p(rect, 0.02, 0.10), p(rect, 0.62, 0.70));
            let b = Rect::from_min_max(p(rect, 0.38, 0.30), p(rect, 0.98, 0.90));
            let faint = color.gamma_multiply(0.45);
            let thin = Stroke::new(1.0, color);

            match which {
                // Only the new shape survives.
                Icon::SelectNew => {
                    painter.rect_stroke(a, 0.0, Stroke::new(1.0, faint), StrokeKind::Inside);
                    painter.rect_filled(b, 0.0, color);
                }
                // Both shapes.
                Icon::SelectAdd => {
                    painter.rect_filled(a, 0.0, color);
                    painter.rect_filled(b, 0.0, color);
                }
                // The first shape minus the second.
                Icon::SelectSubtract => {
                    painter.rect_filled(a, 0.0, color);
                    painter.rect_filled(b, 0.0, Palette::DARK.chrome);
                    painter.rect_stroke(b, 0.0, thin, StrokeKind::Inside);
                }
                // Only where they overlap.
                _ => {
                    painter.rect_stroke(a, 0.0, Stroke::new(1.0, faint), StrokeKind::Inside);
                    painter.rect_stroke(b, 0.0, Stroke::new(1.0, faint), StrokeKind::Inside);
                    painter.rect_filled(a.intersect(b), 0.0, color);
                }
            }
        }

        // A circle half covered by a mask, the conventional Quick Mask
        // button.
        Icon::QuickMask => {
            let r = Rect::from_min_max(p(rect, 0.02, 0.14), p(rect, 0.98, 0.86));
            painter.rect_stroke(r, 1.0, stroke(rect, color), StrokeKind::Inside);
            let left = Rect::from_min_max(r.min, Pos2::new(r.center().x, r.max.y));
            painter.rect_filled(left, 0.0, color.gamma_multiply(0.4));
            painter.circle_stroke(r.center(), r.height() * 0.30, Stroke::new(1.0, color));
        }

        // Several layers collapsing into one.
        Icon::Flatten => {
            line(painter, rect, color, &[(0.14, 0.10), (0.86, 0.10)]);
            line(painter, rect, color, &[(0.14, 0.32), (0.86, 0.32)]);
            // A short arrow showing where they go.
            line(painter, rect, color, &[(0.5, 0.42), (0.5, 0.62)]);
            line(painter, rect, color, &[(0.34, 0.50), (0.5, 0.66), (0.66, 0.50)]);
            let bar = Rect::from_min_max(p(rect, 0.08, 0.78), p(rect, 0.92, 0.96));
            painter.rect_filled(bar, 1.0, color);
        }

        // The chain that links a mask to its layer.
        Icon::MaskLink => {
            line(painter, rect, color, &[(0.30, 0.30), (0.70, 0.70)]);
            line(painter, rect, color, &[(0.12, 0.46), (0.30, 0.28), (0.46, 0.12)]);
            line(painter, rect, color, &[(0.54, 0.88), (0.70, 0.72), (0.88, 0.54)]);
        }
    }
}

/// A square button that draws a vector icon, used across the panels.
pub fn icon_button(ui: &mut egui::Ui, which: Icon, size: f32, hover: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(size), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    if response.hovered() {
        ui.painter().rect_filled(rect, 2.0, visuals.weak_bg_fill);
    }
    icon(&ui.painter_at(rect), rect.shrink(size * 0.2), which, visuals.fg_stroke.color);
    response.on_hover_text(hover)
}
