//! The dark theme.
//!
//! The palette follows the neutral dark greys that editing tools settled on:
//! dark enough that the image is the brightest thing on screen. Every colour
//! lives here rather than being
//! sprinkled through the panels, so the look can be retuned in one place and a
//! light theme can be added later without hunting for literals.

use egui::{Color32, CornerRadius, Stroke, Visuals};

/// Named colours for the whole interface.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Behind panels and the menu bar.
    pub chrome: Color32,
    /// The custom title bar, a shade darker than the rest of the chrome so the
    /// window still reads as having a top edge.
    pub titlebar: Color32,
    /// The brand teal, from the logo.
    pub brand: Color32,
    /// Outline around the frameless window.
    pub window_edge: Color32,
    /// Panel bodies.
    pub panel: Color32,
    /// Panel title bars and the tool options bar.
    pub header: Color32,
    /// The area around the document, darker than everything else so the canvas
    /// reads as the brightest thing on screen.
    pub canvas_backdrop: Color32,
    /// Resting fill for buttons and fields.
    pub widget: Color32,
    pub widget_hover: Color32,
    pub widget_active: Color32,
    /// The selection blue.
    pub accent: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub separator: Color32,
    /// Row highlight for the selected layer or history state.
    pub row_selected: Color32,
    pub row_hover: Color32,
    /// The two checkerboard greys drawn behind transparent pixels.
    pub checker_light: Color32,
    pub checker_dark: Color32,
    /// Thin outline drawn around the document.
    pub canvas_border: Color32,
}

impl Palette {
    pub const DARK: Palette = Palette {
        chrome: Color32::from_rgb(0x39, 0x39, 0x39),
        titlebar: Color32::from_rgb(0x2a, 0x2a, 0x2a),
        brand: Color32::from_rgb(0x00, 0xf5, 0xf5),
        window_edge: Color32::from_rgb(0x14, 0x14, 0x14),
        panel: Color32::from_rgb(0x32, 0x32, 0x32),
        header: Color32::from_rgb(0x3d, 0x3d, 0x3d),
        canvas_backdrop: Color32::from_rgb(0x1e, 0x1e, 0x1e),
        widget: Color32::from_rgb(0x45, 0x45, 0x45),
        widget_hover: Color32::from_rgb(0x56, 0x56, 0x56),
        widget_active: Color32::from_rgb(0x6a, 0x6a, 0x6a),
        accent: Color32::from_rgb(0x14, 0x73, 0xe6),
        text: Color32::from_rgb(0xe6, 0xe6, 0xe6),
        text_dim: Color32::from_rgb(0x9a, 0x9a, 0x9a),
        separator: Color32::from_rgb(0x25, 0x25, 0x25),
        row_selected: Color32::from_rgb(0x4a, 0x4a, 0x4a),
        row_hover: Color32::from_rgb(0x3e, 0x3e, 0x3e),
        checker_light: Color32::from_rgb(0xcc, 0xcc, 0xcc),
        checker_dark: Color32::from_rgb(0x99, 0x99, 0x99),
        canvas_border: Color32::from_rgb(0x0a, 0x0a, 0x0a),
    };
}

/// Install the theme on an egui context. Call once at startup.
pub fn apply(ctx: &egui::Context) {
    let p = Palette::DARK;
    let mut visuals = Visuals::dark();

    visuals.panel_fill = p.panel;
    visuals.window_fill = p.panel;
    visuals.extreme_bg_color = p.canvas_backdrop;
    visuals.faint_bg_color = p.header;
    visuals.window_stroke = Stroke::new(1.0, p.separator);
    visuals.selection.bg_fill = p.accent;
    visuals.selection.stroke = Stroke::new(1.0, p.text);
    visuals.hyperlink_color = p.accent;

    // The chrome is nearly square; rounded corners read as "web app".
    let radius = CornerRadius::same(2);
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = radius;
    }

    visuals.widgets.noninteractive.bg_fill = p.panel;
    visuals.widgets.noninteractive.weak_bg_fill = p.panel;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.separator);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);

    visuals.widgets.inactive.bg_fill = p.widget;
    visuals.widgets.inactive.weak_bg_fill = p.widget;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);

    visuals.widgets.hovered.bg_fill = p.widget_hover;
    visuals.widgets.hovered.weak_bg_fill = p.widget_hover;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.separator);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);

    visuals.widgets.active.bg_fill = p.widget_active;
    visuals.widgets.active.weak_bg_fill = p.widget_active;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);

    visuals.widgets.open.bg_fill = p.widget_active;
    visuals.widgets.open.weak_bg_fill = p.widget_active;

    // No drop shadows on menus: they belong to a different visual language.
    visuals.popup_shadow = egui::epaint::Shadow::NONE;
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.window_corner_radius = radius;
    visuals.menu_corner_radius = radius;

    // The application is dark-only, so both theme slots get the same visuals
    // and the OS preference cannot lighten half the interface.
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);

    use egui::{FontFamily::Proportional, FontId, TextStyle};
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 3.0);
        style.spacing.menu_margin = egui::Margin::symmetric(2, 4);
        style.spacing.slider_width = 120.0;
        style.spacing.indent = 14.0;
        style.spacing.interact_size.y = 20.0;
        // A little wider than it needs to be: the window's resize border
        // claims the outermost few pixels, and this keeps the bar grabbable.
        style.spacing.scroll.bar_width = 13.0;

        // A denser type scale than egui's default, so the panels hold as much
        // information as a compact editor's do.
        style.text_styles = [
            (TextStyle::Small, FontId::new(10.0, Proportional)),
            (TextStyle::Body, FontId::new(12.0, Proportional)),
            (TextStyle::Button, FontId::new(12.0, Proportional)),
            (TextStyle::Heading, FontId::new(14.0, Proportional)),
            (TextStyle::Monospace, FontId::new(11.0, egui::FontFamily::Monospace)),
        ]
        .into();
    });
}

/// Frame for the menu bar and the tool options bar.
pub fn bar_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(Palette::DARK.chrome)
        .inner_margin(egui::Margin::symmetric(6, 3))
}
