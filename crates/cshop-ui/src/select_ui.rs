//! The two windows that select by looking at the picture: Colour Range, which
//! finds a colour everywhere it appears, and Refine Edge, which fits a
//! boundary already found to the one that is actually there.
//!
//! Both preview as a matte — white selected, black not — rather than as
//! marching ants, because both produce partial coverage and ants can only
//! draw a line. Where an edge is half selected, a line has to choose, and the
//! choosing is exactly the information worth seeing.

use crate::commands::Action;
use crate::theme::Palette;
use cshop_core::color::Rgba8;
use cshop_core::color_range::{ColorRange, Pick};
use cshop_core::mask::MaskBuffer;
use cshop_core::pixels::PixelBuffer;
use cshop_core::refine::RefineEdge;

/// The longest side a preview is rendered at. A matte is looked at for its
/// shape, and a shape reads at this size.
const PREVIEW: u32 = 360;

/// Shared by both windows: a downscaled copy of what is being selected from,
/// and the texture of whatever matte the settings currently produce.
struct Matte {
    /// The picture, small.
    source: PixelBuffer,
    texture: Option<egui::TextureHandle>,
    /// The settings the texture was built for.
    rendered: Option<u64>,
}

impl Matte {
    fn new(source: &PixelBuffer) -> Matte {
        let (w, h) = (source.width().max(1), source.height().max(1));
        // Reduced only for looking at. Both windows apply their settings to the
        // full-size picture when the OK is pressed, so the preview never has to
        // map a click back.
        let scale = (PREVIEW as f32 / w.max(h) as f32).min(1.0);
        let small = if scale < 1.0 {
            source.downscale(
                ((w as f32 * scale).round() as u32).max(1),
                ((h as f32 * scale).round() as u32).max(1),
            )
        } else {
            source.clone()
        };
        Matte { source: small, texture: None, rendered: None }
    }

    /// Draw the matte, rebuilding the texture only when the settings moved.
    fn show(&mut self, ui: &mut egui::Ui, key: u64, matte: impl FnOnce(&PixelBuffer) -> MaskBuffer) {
        if self.rendered != Some(key) || self.texture.is_none() {
            let m = matte(&self.source);
            let (w, h) = (m.width() as usize, m.height() as usize);
            let mut rgba = Vec::with_capacity(w * h * 4);
            for y in 0..h as i32 {
                for x in 0..w as i32 {
                    let v = m.get(x, y);
                    rgba.extend_from_slice(&[v, v, v, 255]);
                }
            }
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            self.texture = Some(ui.ctx().load_texture("matte", image, Default::default()));
            self.rendered = Some(key);
        }
        if let Some(t) = &self.texture {
            ui.add(egui::Image::new((t.id(), t.size_vec2())).maintain_aspect_ratio(true));
        }
    }
}

/// Quantised settings, so a texture is rebuilt when they move and not when a
/// float wobbles in the last bit.
fn key(parts: &[f32]) -> u64 {
    let mut k = 0xcbf2_9ce4_8422_2325u64;
    for p in parts {
        k ^= (p * 4096.0) as i64 as u64;
        k = k.wrapping_mul(0x100_0000_01b3);
    }
    k
}

// ---------------------------------------------------------------------------
// Colour Range
// ---------------------------------------------------------------------------

pub struct ColorRangeDialog {
    pub range: ColorRange,
    /// The full-size picture the selection will be made from.
    pub source: PixelBuffer,
    preview: Matte,
    /// True while the next canvas click should sample a colour.
    pub picking: bool,
    /// Add to the sampled colours rather than replacing them.
    pub adding: bool,
}

impl ColorRangeDialog {
    pub fn new(source: PixelBuffer) -> ColorRangeDialog {
        let preview = Matte::new(&source);
        ColorRangeDialog {
            range: ColorRange::default(),
            source,
            preview,
            picking: true,
            adding: false,
        }
    }

    /// A colour taken off the canvas.
    pub fn sample(&mut self, c: Rgba8) {
        match &mut self.range.pick {
            Pick::Sampled(v) if self.adding => v.push(c),
            Pick::Sampled(v) => {
                v.clear();
                v.push(c);
            }
            other => *other = Pick::Sampled(vec![c]),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let p = Palette::DARK;
        let mut close = false;

        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_min_width(240.0);
                ui.label("Select:");
                let picks = [
                    ("Sampled Colours", Pick::Sampled(Vec::new())),
                    ("Shadows", Pick::Shadows),
                    ("Midtones", Pick::Midtones),
                    ("Highlights", Pick::Highlights),
                    ("Reds", Pick::Hue { centre: 0.0 }),
                    ("Greens", Pick::Hue { centre: 1.0 / 3.0 }),
                    ("Blues", Pick::Hue { centre: 2.0 / 3.0 }),
                ];
                for (name, pick) in picks {
                    let on = std::mem::discriminant(&self.range.pick)
                        == std::mem::discriminant(&pick)
                        && match (&self.range.pick, &pick) {
                            (Pick::Hue { centre: a }, Pick::Hue { centre: b }) => {
                                (a - b).abs() < 1e-4
                            }
                            _ => true,
                        };
                    if ui.selectable_label(on, name).clicked() {
                        // Keeping the colours already sampled when the mode
                        // comes back to them: switching to Shadows to look and
                        // back again should not lose the work.
                        if !matches!(pick, Pick::Sampled(_)) {
                            self.range.pick = pick;
                        } else if !matches!(self.range.pick, Pick::Sampled(_)) {
                            self.range.pick = Pick::Sampled(Vec::new());
                        }
                    }
                }

                ui.add_space(8.0);
                if let Pick::Sampled(colours) = &self.range.pick {
                    ui.horizontal(|ui| {
                        ui.label(match colours.len() {
                            0 => "Click the picture to sample".to_string(),
                            1 => "1 colour".to_string(),
                            n => format!("{n} colours"),
                        });
                    });
                    ui.checkbox(&mut self.adding, "Add to the sample");
                    ui.add_space(4.0);
                }

                ui.label("Fuzziness:");
                ui.add(
                    egui::Slider::new(&mut self.range.fuzziness, 0.0..=1.0)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                ui.checkbox(&mut self.range.invert, "Invert");
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Selected in white. Partly-selected edges come out grey, \
                         and stay that way.",
                    )
                    .color(p.text_dim)
                    .small(),
                );
            });

            ui.separator();
            let range = self.range.clone();
            let colours = match &range.pick {
                Pick::Sampled(v) => v.len() as f32,
                _ => 0.0,
            };
            let hue = match &range.pick {
                Pick::Hue { centre } => *centre,
                _ => -1.0,
            };
            let k = key(&[
                range.fuzziness,
                range.invert as u8 as f32,
                colours,
                hue,
                match &range.pick {
                    Pick::Sampled(v) => v.last().map_or(0.0, |c| {
                        c.r as f32 + c.g as f32 * 256.0 + c.b as f32 * 65536.0
                    }),
                    Pick::Shadows => 1.0,
                    Pick::Midtones => 2.0,
                    Pick::Highlights => 3.0,
                    Pick::Hue { .. } => 4.0,
                },
            ]);
            self.preview.show(ui, k, |src| range.matte(src));
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ready = !matches!(&self.range.pick, Pick::Sampled(v) if v.is_empty());
            if ui.add_enabled(ready, egui::Button::new("OK")).clicked() {
                actions.push(Action::ApplyColorRange(Box::new(self.range.clone())));
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
        close
    }
}

// ---------------------------------------------------------------------------
// Refine Edge
// ---------------------------------------------------------------------------

pub struct RefineEdgeDialog {
    pub settings: RefineEdge,
    source: PixelBuffer,
    /// The selection as it stands, at the preview's size.
    small_mask: MaskBuffer,
    preview: Matte,
}

impl RefineEdgeDialog {
    pub fn new(source: PixelBuffer, mask: &MaskBuffer) -> RefineEdgeDialog {
        let preview = Matte::new(&source);
        let (w, h) = (preview.source.width(), preview.source.height());
        // The mask has to be reduced alongside the picture, or the fit would be
        // done against a boundary in the wrong place.
        let mut small = MaskBuffer::hide_all(w, h);
        let sx = mask.width() as f32 / w as f32;
        let sy = mask.height() as f32 / h as f32;
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                small.set(x, y, mask.get((x as f32 * sx) as i32, (y as f32 * sy) as i32));
            }
        }
        RefineEdgeDialog {
            settings: RefineEdge { radius: 4.0, ..Default::default() },
            source,
            small_mask: small,
            preview,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let p = Palette::DARK;
        let mut close = false;

        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_min_width(240.0);
                let s = &mut self.settings;
                ui.label("Radius:");
                ui.add(egui::Slider::new(&mut s.radius, 0.0..=40.0).suffix(" px"));
                ui.label(
                    egui::RichText::new(
                        "How far to look for the edge. It has to reach it: an edge \
                         further out than the radius is not found.",
                    )
                    .color(p.text_dim)
                    .small(),
                );
                ui.add_space(6.0);
                ui.label("Smooth:");
                ui.add(egui::Slider::new(&mut s.smooth, 0.0..=20.0));
                ui.label("Feather:");
                ui.add(egui::Slider::new(&mut s.feather, 0.0..=20.0));
                ui.label("Contrast:");
                ui.add(
                    egui::Slider::new(&mut s.contrast, 0.0..=1.0)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                ui.label("Shift edge:");
                ui.add(
                    egui::Slider::new(&mut s.shift, -1.0..=1.0)
                        .custom_formatter(|v, _| format!("{:+.0}%", v * 100.0)),
                );
            });

            ui.separator();
            let s = self.settings;
            let mask = &self.small_mask;
            let k = key(&[s.radius, s.smooth, s.feather, s.contrast, s.shift]);
            self.preview.show(ui, k, |src| s.apply(mask, src));
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                actions.push(Action::ApplyRefineEdge(Box::new(self.settings)));
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
            ui.label(
                egui::RichText::new(format!(
                    "{}x{}",
                    self.source.width(),
                    self.source.height()
                ))
                .color(p.text_dim)
                .small(),
            );
        });
        close
    }
}
