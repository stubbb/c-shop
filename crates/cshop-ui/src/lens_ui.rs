//! The Lens Correction window.
//!
//! # Why the preview is honest
//!
//! Every control here is defined in units of the frame — a radius where the
//! corner is 1, a fraction of the width, an angle — so none of them has a size
//! in pixels. That is what lets the preview run on a picture scaled down to
//! 720p and still be exactly what the full-resolution pass will produce, only
//! smaller. The blur filters cannot do this and have to scale their radii to
//! match; there is nothing to scale here.
//!
//! So the preview is quick on any size of photograph — a 60 megapixel frame
//! previews at the same speed as a 2 megapixel one — and the full pass, which
//! is not quick, happens once, when the answer has been agreed.

use crate::commands::Action;
use crate::theme::Palette;
use cshop_core::filters::plane::Plane;
use cshop_core::geom::IRect;
use cshop_core::layer::LayerId;
use cshop_core::lens::{apply, largest_opaque_rect, Lens};
use cshop_core::pixels::PixelBuffer;
use cshop_core::resample::Resampling;

/// The preview is scaled to fit inside this. 720p is enough to judge whether a
/// line is straight, which is the only question this window asks.
const PREVIEW_W: u32 = 1280;
const PREVIEW_H: u32 = 720;

/// How wide the preview is drawn in the window.
const SHOWN_W: f32 = 420.0;

pub struct LensDialog {
    pub lens: Lens,
    /// Cut the empty edges away afterwards.
    pub autocrop: bool,
    /// The layer being corrected.
    pub layer: LayerId,
    /// The full-resolution size, so the crop can be reported in real pixels.
    pub full: (u32, u32),
    /// The source at preview size, in the form the corrections take.
    small: Plane,
    /// Scale from full resolution down to the preview.
    scale: f32,
    texture: Option<egui::TextureHandle>,
    /// What the cached texture was built from.
    rendered: Option<(Lens, u32)>,
    /// The crop the current settings would take, in preview pixels.
    crop: Option<IRect>,
    /// Set while the full-resolution pass runs, counting rows done and rows
    /// wanted.
    pub applying: Option<cshop_core::progress::Progress>,
}

impl LensDialog {
    pub fn new(layer: LayerId, source: &PixelBuffer) -> LensDialog {
        let (w, h) = (source.width().max(1), source.height().max(1));
        let scale = (PREVIEW_W as f32 / w as f32)
            .min(PREVIEW_H as f32 / h as f32)
            .min(1.0);
        let small = if scale < 1.0 {
            let (pw, ph) = (
                ((w as f32 * scale).round() as u32).max(1),
                ((h as f32 * scale).round() as u32).max(1),
            );
            cshop_core::resample::resize(source, pw, ph, Resampling::Bilinear)
        } else {
            source.clone()
        };
        LensDialog {
            lens: Lens::default(),
            autocrop: false,
            layer,
            full: (w, h),
            small: Plane::from_pixels(&small),
            scale,
            texture: None,
            rendered: None,
            crop: None,
            applying: None,
        }
    }

    pub fn title(&self) -> &'static str {
        "Lens Correction"
    }

    pub fn is_working(&self) -> bool {
        self.applying.is_some()
    }

    /// How far the full-resolution pass has got, 0 to 1.
    pub fn progress(&self) -> f32 {
        self.applying.as_ref().and_then(|p| p.fraction()).unwrap_or(0.0)
    }

    /// The crop the settings would take, in full-resolution pixels.
    ///
    /// Worked out on the preview and scaled up, so it is an estimate — the
    /// real one is measured on the full-resolution result, where a curved edge
    /// falls where it falls rather than where a smaller copy of it fell.
    pub fn crop_estimate(&self) -> Option<IRect> {
        let r = self.crop?;
        let s = 1.0 / self.scale;
        Some(IRect::new(
            (r.x0 as f32 * s).round() as i32,
            (r.y0 as f32 * s).round() as i32,
            (r.x1 as f32 * s).round() as i32,
            (r.y1 as f32 * s).round() as i32,
        ))
    }

    /// Re-run the preview if anything it depends on has moved.
    fn refresh(&mut self, ui: &egui::Ui) {
        let want = (self.lens, self.autocrop as u32);
        if self.rendered == Some(want) && self.texture.is_some() {
            return;
        }
        let out = apply(&self.small, self.lens, &cshop_core::progress::Progress::ignored());
        self.crop = self.autocrop.then(|| largest_opaque_rect(&out)).filter(|r| !r.is_empty());

        let pixels = out.to_pixels();
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [pixels.width() as usize, pixels.height() as usize],
            pixels.as_bytes(),
        );
        self.texture = Some(ui.ctx().load_texture("lens-preview", image, Default::default()));
        self.rendered = Some(want);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let p = Palette::DARK;
        let mut close = false;
        let working = self.is_working();

        self.refresh(ui);

        // --- preview ------------------------------------------------------
        if let Some(tex) = &self.texture {
            let size = tex.size_vec2();
            let shown = egui::vec2(SHOWN_W, SHOWN_W * size.y / size.x);
            let (rect, _) = ui.allocate_exact_size(shown, egui::Sense::hover());
            ui.painter().image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            // What autocrop would keep, drawn over what it would throw away.
            if let Some(c) = self.crop {
                let sx = rect.width() / size.x;
                let sy = rect.height() / size.y;
                let keep = egui::Rect::from_min_max(
                    rect.min + egui::vec2(c.x0 as f32 * sx, c.y0 as f32 * sy),
                    rect.min + egui::vec2(c.x1 as f32 * sx, c.y1 as f32 * sy),
                );
                ui.painter().rect_stroke(
                    keep,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(0x60, 0xc0, 0xff)),
                    egui::StrokeKind::Inside,
                );
            }
        }

        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!(
                "Preview at {}x{} of {}x{}",
                self.small.width, self.small.height, self.full.0, self.full.1
            ))
            .color(p.text_dim)
            .small(),
        );

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        // --- the corrections ----------------------------------------------
        ui.add_enabled_ui(!working, |ui| {
            egui::Grid::new("lens-controls").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("Distortion");
                ui.add(
                    egui::Slider::new(&mut self.lens.distortion, -0.6..=0.6)
                        .fixed_decimals(3)
                        .custom_formatter(|v, _| distortion_label(v as f32)),
                )
                .on_hover_text(
                    "Bend straight lines back. Toward pincushion corrects the bulge of \
                     a wide-angle lens; toward barrel corrects the pinch of a long one.",
                );
                ui.end_row();

                ui.label("Rotation");
                ui.add(
                    egui::Slider::new(&mut self.lens.rotation, -45.0..=45.0).suffix("°"),
                )
                .on_hover_text("Level the horizon.");
                ui.end_row();

                ui.label("Perspective ↕");
                ui.add(egui::Slider::new(&mut self.lens.perspective_v, -0.5..=0.5))
                    .on_hover_text("Keystone: for a camera tilted up or down.");
                ui.end_row();

                ui.label("Perspective ↔");
                ui.add(egui::Slider::new(&mut self.lens.perspective_h, -0.5..=0.5))
                    .on_hover_text("Keystone: for a camera turned left or right.");
                ui.end_row();

                ui.label("Scale");
                ui.add(egui::Slider::new(&mut self.lens.scale, 0.5..=2.0).fixed_decimals(2))
                    .on_hover_text(
                        "Push the empty edges out of frame by hand, instead of cropping \
                         them away.",
                    );
                ui.end_row();

                ui.label("Vignette");
                ui.add(
                    egui::Slider::new(&mut self.lens.vignette, -1.0..=1.0)
                        .custom_formatter(|v, _| vignette_label(v as f32)),
                )
                .on_hover_text("Darken the corners, or lift them.");
                ui.end_row();

                ui.label("Midpoint");
                ui.add(
                    egui::Slider::new(&mut self.lens.vignette_midpoint, 0.0..=0.95)
                        .fixed_decimals(2),
                )
                .on_hover_text("How far out the vignette starts. Everything inside is left alone.");
                ui.end_row();
            });

            ui.add_space(6.0);
            let moves = self.lens.moves_pixels();
            ui.add_enabled_ui(moves, |ui| {
                ui.checkbox(&mut self.autocrop, "Crop away the empty edges")
                    .on_hover_text(
                        "Keep the largest rectangle that has no transparency in it. \
                         Outlined on the preview.",
                    );
            });
            if !moves {
                // Nothing has moved, so there are no empty edges to crop. Left
                // visible but disabled rather than hidden, so the window does
                // not change shape as the sliders move.
                self.autocrop = false;
            }
            if let Some(c) = self.crop_estimate() {
                ui.label(
                    egui::RichText::new(format!(
                        "would keep about {}x{} of {}x{}",
                        c.width(),
                        c.height(),
                        self.full.0,
                        self.full.1
                    ))
                    .color(p.text_dim)
                    .small(),
                );
            }
        });

        // --- applying ------------------------------------------------------
        ui.add_space(8.0);
        if working {
            ui.add(egui::ProgressBar::new(self.progress()).show_percentage().desired_height(14.0));
            ui.label(
                egui::RichText::new(format!(
                    "Correcting {}x{}…",
                    self.full.0, self.full.1
                ))
                .color(p.text_dim)
                .small(),
            );
        }

        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ready = !working && !self.lens.is_identity();
            if ui.add_enabled(ready, egui::Button::new("Apply")).clicked() {
                actions.push(Action::ApplyLens);
            }
            if ui.add_enabled(!working, egui::Button::new("Reset")).clicked() {
                self.lens = Lens::default();
                self.autocrop = false;
            }
            if ui.add_enabled(!working, egui::Button::new("Cancel")).clicked() {
                close = true;
            }
        });
        close
    }
}

/// Name the ends of the distortion slider, because a number alone does not say
/// which way it bends.
fn distortion_label(v: f32) -> String {
    if v.abs() < 0.005 {
        "none".to_string()
    } else if v > 0.0 {
        format!("pincushion {v:.3}")
    } else {
        format!("barrel {:.3}", -v)
    }
}

fn vignette_label(v: f32) -> String {
    if v.abs() < 0.005 {
        "none".to_string()
    } else if v > 0.0 {
        format!("lift {v:.2}")
    } else {
        format!("darken {:.2}", -v)
    }
}
