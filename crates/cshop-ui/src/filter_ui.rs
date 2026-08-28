//! The filter dialog: parameter controls and a live preview.
//!
//! Filters are far too slow to run on a full-resolution layer while a slider
//! moves — a 24 MP Radial Blur takes two seconds. The dialog therefore never
//! filters more than a viewport's worth of pixels, so a preview costs the same
//! whether the layer is 1 MP or 100 MP.
//!
//! How that viewport is filled depends on the filter, because a blur is judged
//! on detail the fit view has already thrown away:
//!
//! * A filter with bounded [`support`](Filter::support) — the blurs, sharpen,
//!   median, high pass — is previewed from a **crop of the full-resolution
//!   source**, taken with a margin so pixels at the edge of the crop still see
//!   real neighbours. Zoomed to 100% this is not an approximation at all: it
//!   is exactly what the applied filter will produce.
//! * The rest depend on the whole image — the distortions are anchored to the
//!   image extent, Average needs every pixel — so cropping would change the
//!   result. Those are rendered whole at fit scale, with
//!   [`Filter::scaled`] keeping the settings honest, and zoom only magnifies.
//!   The label says so rather than implying detail that is not there.

use crate::commands::Action;
use crate::theme::Palette;
use cshop_core::filters::{Filter, FilterContext};
use cshop_core::geom::IRect;
use cshop_core::pixels::PixelBuffer;
use cshop_core::resample::Resampling;

/// The preview viewport, in screen pixels.
const VIEW_W: u32 = 380;
const VIEW_H: u32 = 300;

/// Largest margin taken around the preview crop, in rendered pixels. Without
/// a cap a 400-pixel blur radius would pull in a full-image-sized window and
/// bring back exactly the freeze the crop exists to avoid.
const MAX_MARGIN: u32 = 150;

/// Zoom stops for the − and + buttons, as screen pixels per image pixel.
const ZOOM_STOPS: [f32; 10] =
    [0.0625, 0.125, 0.25, 0.333_333, 0.5, 0.666_667, 1.0, 2.0, 4.0, 8.0];

pub struct FilterDialog {
    pub filter: Filter,
    context: FilterContext,
    /// The affected region at full resolution. The preview is cut from this.
    source: PixelBuffer,
    /// Screen pixels per source pixel.
    zoom: f32,
    /// While set, the zoom follows the fit scale instead of a chosen stop.
    fit: bool,
    /// Centre of the view, in source pixels.
    centre: (f32, f32),
    /// The source rect the cached textures cover — the whole source for a
    /// whole-image filter, the visible crop otherwise.
    covered: IRect,
    preview: Option<egui::TextureHandle>,
    /// The same window without the filter, for the hold-to-compare.
    original: Option<egui::TextureHandle>,
    /// The settings and window the cache was built for.
    rendered: Option<(Filter, IRect, u32, u32)>,
    show_original: bool,
    /// Set while a press has moved, so panning does not also trigger compare.
    press_moved: bool,
}

impl FilterDialog {
    /// `source` is the affected region of the layer at full resolution.
    pub fn new(filter: Filter, source: PixelBuffer, context: FilterContext) -> Self {
        let centre = (source.width() as f32 / 2.0, source.height() as f32 / 2.0);
        let mut dialog = Self {
            filter,
            context,
            source,
            zoom: 1.0,
            fit: true,
            centre,
            covered: IRect::EMPTY,
            preview: None,
            original: None,
            rendered: None,
            show_original: false,
            press_moved: false,
        };
        dialog.zoom = dialog.fit_zoom();
        dialog
    }

    pub fn title(&self) -> String {
        self.filter.name().to_string()
    }

    /// Zoom at which the whole region fits the viewport. Never magnifies: a
    /// region smaller than the viewport is shown at 100%, not blown up.
    fn fit_zoom(&self) -> f32 {
        let w = VIEW_W as f32 / self.source.width().max(1) as f32;
        let h = VIEW_H as f32 / self.source.height().max(1) as f32;
        w.min(h).min(1.0)
    }

    /// The source rect currently visible, clamped inside the source.
    fn view_rect(&self) -> IRect {
        let sw = self.source.width() as i32;
        let sh = self.source.height() as i32;
        let w = ((VIEW_W as f32 / self.zoom).ceil() as i32).clamp(1, sw.max(1));
        let h = ((VIEW_H as f32 / self.zoom).ceil() as i32).clamp(1, sh.max(1));
        let x0 = (self.centre.0 - w as f32 / 2.0).round() as i32;
        let y0 = (self.centre.1 - h as f32 / 2.0).round() as i32;
        let x0 = x0.clamp(0, (sw - w).max(0));
        let y0 = y0.clamp(0, (sh - h).max(0));
        IRect::from_points(x0, y0, x0 + w, y0 + h)
    }

    /// True when the preview is showing real full-resolution detail rather
    /// than a downscaled stand-in.
    fn is_exact(&self) -> bool {
        self.filter.support().is_some() && self.zoom >= 1.0
    }

    /// Set the zoom from outside the dialog. Used by the screenshot flags.
    pub fn zoom_to(&mut self, zoom: f32) {
        self.set_zoom(zoom);
    }

    fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.clamp(ZOOM_STOPS[0], ZOOM_STOPS[ZOOM_STOPS.len() - 1]);
        self.fit = false;
    }

    /// Returns `true` when the dialog should close.
    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let p = Palette::DARK;
        let mut close = false;

        ui.horizontal_top(|ui| {
            // --- preview ---------------------------------------------------
            ui.vertical(|ui| {
                if self.fit {
                    self.zoom = self.fit_zoom();
                }
                let view = self.view_rect();
                self.refresh_preview(ui.ctx(), view);

                let viewport = egui::vec2(VIEW_W as f32, VIEW_H as f32);
                let (rect, response) =
                    ui.allocate_exact_size(viewport, egui::Sense::click_and_drag());
                ui.painter().rect_filled(rect, 2.0, p.canvas_backdrop);

                // Drag pans. A press that never moves is the hold-to-compare,
                // so the two gestures do not fight over the same button.
                if response.drag_started() {
                    self.press_moved = false;
                }
                let delta = response.drag_delta();
                if delta != egui::Vec2::ZERO {
                    self.press_moved = true;
                    self.centre.0 -= delta.x / self.zoom;
                    self.centre.1 -= delta.y / self.zoom;
                }
                self.show_original = response.is_pointer_button_down_on() && !self.press_moved;

                // Wheel over the preview zooms, which is the gesture everyone
                // reaches for first.
                if response.hovered() {
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    if scroll.abs() > 0.5 {
                        self.set_zoom(self.zoom * (scroll * 0.004).exp());
                    }
                }

                let texture =
                    if self.show_original { self.original.as_ref() } else { self.preview.as_ref() };
                if let Some(handle) = texture {
                    // The texture covers `self.covered`; draw the part of it
                    // the view is looking at, letterboxed when the source is
                    // smaller than the viewport.
                    let cw = self.covered.width().max(1) as f32;
                    let ch = self.covered.height().max(1) as f32;
                    let uv = egui::Rect::from_min_max(
                        egui::pos2(
                            (view.x0 - self.covered.x0) as f32 / cw,
                            (view.y0 - self.covered.y0) as f32 / ch,
                        ),
                        egui::pos2(
                            (view.x1 - self.covered.x0) as f32 / cw,
                            (view.y1 - self.covered.y0) as f32 / ch,
                        ),
                    );
                    let draw = egui::Rect::from_center_size(
                        rect.center(),
                        egui::vec2(
                            (view.width() as f32 * self.zoom).min(viewport.x),
                            (view.height() as f32 * self.zoom).min(viewport.y),
                        ),
                    );
                    ui.painter().image(handle.id(), draw, uv, egui::Color32::WHITE);
                }
                ui.painter().rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(1.0, p.separator),
                    egui::StrokeKind::Inside,
                );

                // --- zoom controls ---
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let stops = ZOOM_STOPS;
                    if ui.small_button("−").on_hover_text("Zoom out").clicked() {
                        let next = stops.iter().rev().find(|z| **z < self.zoom - 1e-4);
                        self.set_zoom(*next.unwrap_or(&stops[0]));
                    }
                    ui.label(
                        egui::RichText::new(format!("{:.0}%", self.zoom * 100.0))
                            .color(p.text)
                            .small(),
                    );
                    if ui.small_button("+").on_hover_text("Zoom in").clicked() {
                        let next = stops.iter().find(|z| **z > self.zoom + 1e-4);
                        self.set_zoom(*next.unwrap_or(&stops[stops.len() - 1]));
                    }
                    if ui.selectable_label(self.fit, "Fit").clicked() {
                        self.fit = true;
                        self.centre = (
                            self.source.width() as f32 / 2.0,
                            self.source.height() as f32 / 2.0,
                        );
                    }
                    if ui
                        .selectable_label(!self.fit && (self.zoom - 1.0).abs() < 1e-4, "100%")
                        .on_hover_text("Actual pixels — drag the preview to pan")
                        .clicked()
                    {
                        self.set_zoom(1.0);
                    }
                });

                ui.label(
                    egui::RichText::new(if self.show_original {
                        "Original".to_string()
                    } else if self.is_exact() {
                        "Preview at full resolution — press and hold to compare".to_string()
                    } else if self.filter.support().is_none() {
                        format!(
                            "{} works on the whole image, so the preview stays at {:.0}%",
                            self.filter.name(),
                            self.fit_zoom() * 100.0
                        )
                    } else {
                        "Preview — press and hold to compare, zoom in for detail".to_string()
                    })
                    .color(p.text_dim)
                    .small(),
                );
            });

            // A separator inside `horizontal_top` stretches to the tallest
            // column, which on a short dialog draws a rule through empty
            // space. Plain spacing avoids that.
            ui.add_space(16.0);

            // --- controls ---------------------------------------------------
            ui.vertical(|ui| {
                ui.set_min_width(280.0);
                ui.set_max_width(320.0);
                egui::ScrollArea::vertical()
                    .id_salt("filter-controls")
                    .max_height(400.0)
                    // Shrink to the controls' real height rather than
                    // reserving the maximum.
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_min_width(270.0);
                        filter_editor(ui, &mut self.filter);
                    });
            });
        });

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                actions.push(Action::ApplyFilter(Box::new(self.filter.clone())));
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
        close
    }

    /// Build the two textures for the current window, if anything changed.
    fn refresh_preview(&mut self, ctx: &egui::Context, view: IRect) {
        let Some(window) = self.render_window(view) else { return };
        if self.rendered.as_ref() == Some(&window.key) && self.preview.is_some() {
            self.covered = window.covered;
            return;
        }
        let RenderedWindow { filtered, original, covered, key } = window;
        set_texture(ctx, &mut self.preview, "filter-preview", &filtered);
        set_texture(ctx, &mut self.original, "filter-original", &original);
        self.covered = covered;
        self.rendered = Some(key);
    }

    /// Filter one window of the source. Pure, so the result can be checked
    /// against a full-resolution apply without an egui context.
    fn render_window(&self, view: IRect) -> Option<RenderedWindow> {
        let support = self.filter.support();
        // A whole-image filter has to see the whole image, so it is rendered
        // at fit scale and only magnified; everything else is cut from the
        // full-resolution source at the zoom being shown.
        let (covered, scale) = match support {
            Some(_) => (view, self.zoom.min(1.0)),
            None => (self.source.bounds(), self.fit_zoom()),
        };

        // Take the crop with a margin so pixels at its edge still see real
        // neighbours — without it a blur would fade into nothing at the
        // window border, which the real filter does not do. Capped so a huge
        // radius cannot turn a constant-cost preview into a full-image one.
        // The cap is on the *rendered* margin, which is what costs time.
        let max_margin = (MAX_MARGIN as f32 / scale.max(1e-3)) as i32;
        let margin = (support.unwrap_or(0) as i32).min(max_margin);
        let padded = covered.inflate(margin).intersect(&self.source.bounds());
        if padded.is_empty() {
            return None;
        }

        let render_w = ((padded.width() as f32 * scale).round() as u32).max(1);
        let render_h = ((padded.height() as f32 * scale).round() as u32).max(1);
        let crop = self.source.copy_rect(padded);
        let window = if render_w != crop.width() || render_h != crop.height() {
            cshop_core::resample::resize(&crop, render_w, render_h, Resampling::Bilinear)
        } else {
            crop
        };
        // Scale the settings to match, so the preview shows the same effect
        // the full-size apply will produce.
        let filtered = self.filter.scaled(scale).apply(&window, &self.context);

        // Cut the margin back off, in the rendered buffer's own coordinates.
        let trim = IRect::from_points(
            ((covered.x0 - padded.x0) as f32 * scale).round() as i32,
            ((covered.y0 - padded.y0) as f32 * scale).round() as i32,
            ((covered.x1 - padded.x0) as f32 * scale).round() as i32,
            ((covered.y1 - padded.y0) as f32 * scale).round() as i32,
        )
        .intersect(&filtered.bounds());
        let (shown, before) = if trim.is_empty() {
            (filtered, window)
        } else {
            (filtered.copy_rect(trim), window.copy_rect(trim))
        };

        Some(RenderedWindow {
            filtered: shown,
            original: before,
            covered,
            key: (self.filter.clone(), padded, render_w, render_h),
        })
    }
}

/// One rendered preview window and the cache key it was built from.
struct RenderedWindow {
    filtered: PixelBuffer,
    original: PixelBuffer,
    /// The source rect `filtered` covers.
    covered: IRect,
    key: (Filter, IRect, u32, u32),
}

/// Update a texture in place where possible: this runs on every pan, and
/// allocating a fresh texture per frame makes the drag stutter.
fn set_texture(
    ctx: &egui::Context,
    slot: &mut Option<egui::TextureHandle>,
    name: &str,
    image: &PixelBuffer,
) {
    match slot {
        Some(handle) => handle.set(to_image(image), egui::TextureOptions::LINEAR),
        None => *slot = Some(upload(ctx, name, image)),
    }
}

fn to_image(image: &PixelBuffer) -> egui::ColorImage {
    let pixels: Vec<egui::Color32> = image
        .pixels()
        .iter()
        .map(|p| egui::Color32::from_rgba_unmultiplied(p.r, p.g, p.b, p.a))
        .collect();
    egui::ColorImage {
        size: [image.width() as usize, image.height() as usize],
        source_size: egui::vec2(image.width() as f32, image.height() as f32),
        pixels,
    }
}

fn upload(ctx: &egui::Context, name: &str, image: &PixelBuffer) -> egui::TextureHandle {
    ctx.load_texture(name, to_image(image), egui::TextureOptions::LINEAR)
}


/// Parameter controls for one filter.
pub fn filter_editor(ui: &mut egui::Ui, filter: &mut Filter) -> bool {
    let mut changed = false;
    match filter {
        Filter::GaussianBlur { radius } | Filter::BoxBlur { radius } => {
            changed |= slider(ui, "Radius", radius, 0.0..=250.0, " px");
        }
        Filter::MotionBlur { angle, distance } => {
            changed |= slider(ui, "Angle", angle, -180.0..=180.0, "°");
            changed |= slider(ui, "Distance", distance, 1.0..=500.0, " px");
        }
        Filter::RadialBlur { amount, spin, centre } => {
            changed |= slider(ui, "Amount", amount, 0.0..=1.0, "");
            ui.horizontal(|ui| {
                changed |= ui.radio_value(spin, true, "Spin").changed();
                changed |= ui.radio_value(spin, false, "Zoom").changed();
            });
            changed |= slider(ui, "Centre X", &mut centre.0, 0.0..=1.0, "");
            changed |= slider(ui, "Centre Y", &mut centre.1, 0.0..=1.0, "");
        }
        Filter::SurfaceBlur { radius, threshold } => {
            changed |= slider(ui, "Radius", radius, 1.0..=60.0, " px");
            changed |= slider(ui, "Threshold", threshold, 0.001..=0.5, "");
        }
        Filter::AverageBlur | Filter::FindEdges | Filter::Solarize => {
            ui.label(
                egui::RichText::new("This filter has no settings.")
                    .color(Palette::DARK.text_dim),
            );
        }
        Filter::Sharpen { amount } => {
            changed |= slider(ui, "Amount", amount, 0.0..=4.0, "");
        }
        Filter::UnsharpMask { amount, radius, threshold } => {
            changed |= slider(ui, "Amount", amount, 0.0..=5.0, "");
            changed |= slider(ui, "Radius", radius, 0.1..=100.0, " px");
            changed |= slider(ui, "Threshold", threshold, 0.0..=0.5, "");
        }
        Filter::AddNoise { amount, monochromatic, gaussian, seed } => {
            changed |= slider(ui, "Amount", amount, 0.0..=1.0, "");
            changed |= ui.checkbox(monochromatic, "Monochromatic").changed();
            changed |= ui.checkbox(gaussian, "Gaussian distribution").changed();
            changed |= reseed(ui, seed);
        }
        Filter::Median { radius } | Filter::Maximum { radius } | Filter::Minimum { radius } => {
            changed |= int_slider(ui, "Radius", radius, 0..=20);
        }
        Filter::DustAndScratches { radius, threshold } => {
            changed |= int_slider(ui, "Radius", radius, 1..=16);
            changed |= slider(ui, "Threshold", threshold, 0.0..=0.5, "");
        }
        Filter::Twirl { angle } => {
            changed |= slider(ui, "Angle", angle, -720.0..=720.0, "°");
        }
        Filter::Pinch { amount } | Filter::Spherize { amount } => {
            changed |= slider(ui, "Amount", amount, -1.0..=1.0, "");
        }
        Filter::Wave { amplitude, wavelength, vertical } => {
            changed |= slider(ui, "Amplitude", amplitude, 0.0..=200.0, " px");
            changed |= slider(ui, "Wavelength", wavelength, 2.0..=500.0, " px");
            changed |= ui.checkbox(vertical, "Vertical").changed();
        }
        Filter::PolarCoordinates { to_polar } => {
            ui.horizontal(|ui| {
                changed |= ui.radio_value(to_polar, true, "Rectangular to polar").changed();
                changed |= ui.radio_value(to_polar, false, "Polar to rectangular").changed();
            });
        }
        Filter::Mosaic { size } => {
            changed |= int_slider(ui, "Cell size", size, 2..=200);
        }
        Filter::Crystallize { size, seed } => {
            changed |= int_slider(ui, "Cell size", size, 2..=200);
            changed |= reseed(ui, seed);
        }
        Filter::Fragment { distance } => {
            let mut v = *distance as u32;
            if int_slider(ui, "Distance", &mut v, 1..=40) {
                *distance = v as i32;
                changed = true;
            }
        }
        Filter::Clouds { scale, seed, difference } => {
            changed |= slider(ui, "Scale", scale, 4.0..=400.0, " px");
            changed |= reseed(ui, seed);
            ui.label(
                egui::RichText::new(if *difference {
                    "Blended into the layer with a difference blend."
                } else {
                    "Uses the foreground and background colours."
                })
                .color(Palette::DARK.text_dim)
                .small(),
            );
        }
        Filter::Fibers { strength, length, seed } => {
            changed |= slider(ui, "Variance", strength, 0.0..=1.0, "");
            changed |= slider(ui, "Strength", length, 1.0..=64.0, "");
            changed |= reseed(ui, seed);
        }
        Filter::Emboss { angle, height, amount } => {
            changed |= slider(ui, "Angle", angle, -180.0..=180.0, "°");
            changed |= slider(ui, "Height", height, 1.0..=20.0, " px");
            changed |= slider(ui, "Amount", amount, 0.0..=8.0, "");
        }
        Filter::Diffuse { amount, seed } => {
            changed |= int_slider(ui, "Amount", amount, 1..=20);
            changed |= reseed(ui, seed);
        }
        Filter::HighPass { radius } => {
            changed |= slider(ui, "Radius", radius, 0.1..=120.0, " px");
        }
        Filter::Offset { dx, dy, wrap } => {
            changed |= int_signed(ui, "Horizontal", dx);
            changed |= int_signed(ui, "Vertical", dy);
            changed |= ui.checkbox(wrap, "Wrap around").changed();
        }
        Filter::Custom { kernel, divisor, offset } => {
            ui.label(egui::RichText::new("5 x 5 kernel").small().strong());
            egui::Grid::new("custom-kernel").spacing([2.0, 2.0]).show(ui, |ui| {
                for row in 0..5 {
                    for col in 0..5 {
                        let v = &mut kernel[row * 5 + col];
                        changed |= ui
                            .add(
                                egui::DragValue::new(v)
                                    .speed(0.1)
                                    .range(-999.0..=999.0)
                                    .max_decimals(2),
                            )
                            .changed();
                    }
                    ui.end_row();
                }
            });
            ui.add_space(4.0);
            changed |= slider(ui, "Divisor", divisor, -100.0..=100.0, "");
            changed |= slider(ui, "Offset", offset, -1.0..=1.0, "");
            if ui.button("Auto divisor").clicked() {
                // The sum keeps the image's overall brightness unchanged.
                let sum: f32 = kernel.iter().sum();
                *divisor = if sum.abs() < 1e-4 { 1.0 } else { sum };
                changed = true;
            }
        }
    }
    changed
}

fn slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::Slider::new(value, range).suffix(suffix).max_decimals(2)).changed()
        })
        .inner
    })
    .inner
}

fn int_slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::Slider::new(value, range)).changed()
        })
        .inner
    })
    .inner
}

fn int_signed(ui: &mut egui::Ui, label: &str, value: &mut i32) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::DragValue::new(value).range(-4000..=4000).suffix(" px")).changed()
        })
        .inner
    })
    .inner
}

/// A button that picks a new random seed, for the filters that use one.
fn reseed(ui: &mut egui::Ui, seed: &mut u64) -> bool {
    ui.horizontal(|ui| {
        ui.label("Seed");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Randomise").clicked() {
                // Any change is enough; the sequence itself is deterministic.
                *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                true
            } else {
                false
            }
        })
        .inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detail at every scale, so a downscaled preview and a full-resolution
    /// one cannot be mistaken for each other.
    fn detailed(w: u32, h: u32) -> PixelBuffer {
        let mut px = PixelBuffer::new(w, h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let checker = if (x / 3 + y / 3) % 2 == 0 { 220 } else { 40 };
                let ramp = (x * 255 / w.max(1) as i32) as u8;
                px.set(x, y, cshop_core::color::Rgba8::new(checker, ramp, 255 - ramp, 255));
            }
        }
        px
    }

    fn dialog(filter: Filter, source: PixelBuffer) -> FilterDialog {
        FilterDialog::new(filter, source, FilterContext::default())
    }

    #[test]
    fn a_region_smaller_than_the_viewport_is_shown_at_actual_size() {
        let d = dialog(Filter::GaussianBlur { radius: 2.0 }, detailed(64, 48));
        assert_eq!(d.fit_zoom(), 1.0, "a small region should not be blown up to fit");
    }

    #[test]
    fn fit_shows_the_whole_region_and_zooming_in_shows_less_of_it() {
        let d = dialog(Filter::GaussianBlur { radius: 2.0 }, detailed(2000, 1500));
        let fit = d.view_rect();
        assert_eq!(fit, IRect::from_points(0, 0, 2000, 1500), "fit should show everything");

        let mut d = d;
        d.set_zoom(1.0);
        let close = d.view_rect();
        assert_eq!(close.width(), VIEW_W, "at 100% one source pixel is one screen pixel");
        assert_eq!(close.height(), VIEW_H);
        assert!(close.width() < fit.width());
    }

    #[test]
    fn panning_stays_inside_the_source() {
        let mut d = dialog(Filter::GaussianBlur { radius: 2.0 }, detailed(2000, 1500));
        d.set_zoom(1.0);
        d.centre = (-9999.0, -9999.0);
        let r = d.view_rect();
        assert_eq!((r.x0, r.y0), (0, 0), "panning past the corner should clamp");
        d.centre = (9999.0, 9999.0);
        let r = d.view_rect();
        assert_eq!((r.x1, r.y1), (2000, 1500), "panning past the far corner should clamp");
    }

    /// The point of the whole design: zoomed to 100%, the window a local
    /// filter previews is not an approximation of the applied result, it is
    /// the applied result. The margin around the crop is what makes this
    /// true at the window's edges.
    #[test]
    fn the_zoomed_preview_matches_a_full_resolution_apply() {
        let source = detailed(900, 700);
        for filter in [
            Filter::GaussianBlur { radius: 6.0 },
            Filter::BoxBlur { radius: 5.0 },
            Filter::UnsharpMask { amount: 1.4, radius: 3.0, threshold: 0.0 },
            Filter::Maximum { radius: 4 },
            Filter::Median { radius: 3 },
        ] {
            let reference = filter.apply(&source, &FilterContext::default());

            let mut d = dialog(filter.clone(), source.clone());
            d.set_zoom(1.0);
            d.centre = (500.0, 400.0);
            let view = d.view_rect();
            let window = d.render_window(view).expect("a local filter renders a window");
            assert_eq!(window.covered, view, "a local filter covers exactly the view");

            let want = reference.copy_rect(view);
            assert_eq!(window.filtered.width(), want.width());
            assert_eq!(window.filtered.height(), want.height());
            let worst = window
                .filtered
                .pixels()
                .iter()
                .zip(want.pixels())
                .map(|(a, b)| {
                    let d = |x: u8, y: u8| x.abs_diff(y) as u32;
                    d(a.r, b.r).max(d(a.g, b.g)).max(d(a.b, b.b)).max(d(a.a, b.a))
                })
                .max()
                .unwrap_or(0);
            assert!(worst <= 1, "{} preview differs by {worst} levels", filter.name());
        }
    }

    /// A whole-image filter must keep seeing the whole image; cropping would
    /// move the centre of a Radial Blur or reflow a distortion.
    #[test]
    fn a_whole_image_filter_still_renders_the_whole_region() {
        let source = detailed(1200, 900);
        let mut d = dialog(Filter::Twirl { angle: 60.0 }, source.clone());
        d.set_zoom(4.0);
        let window = d.render_window(d.view_rect()).expect("renders");
        assert_eq!(window.covered, source.bounds(), "the whole region must be filtered");
        assert!(!d.is_exact(), "and the label must not claim full resolution");
    }

    /// A huge radius must not turn a constant-cost preview into a full-image
    /// one — that is the freeze the proxy exists to avoid.
    #[test]
    fn a_huge_radius_does_not_blow_up_the_rendered_window() {
        let source = detailed(4000, 3000);
        let mut d = dialog(Filter::GaussianBlur { radius: 400.0 }, source);
        d.set_zoom(1.0);
        let window = d.render_window(d.view_rect()).expect("renders");
        let (w, h) = (window.key.2, window.key.3);
        assert!(
            w <= VIEW_W + 2 * MAX_MARGIN && h <= VIEW_H + 2 * MAX_MARGIN,
            "rendered {w}x{h}, which is more than the capped margin should allow"
        );
    }
}
