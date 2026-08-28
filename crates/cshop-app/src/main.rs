//! C-Shop — a native, GPU-accelerated layered image editor.
//!
//! This binary owns the window, the swapchain and the egui integration. It
//! deliberately does not use a framework such as `eframe`: the compositor and
//! egui have to share one `wgpu::Device` so the composited canvas can be handed
//! to egui as a texture, and the render loop needs to stay under our control so
//! it can idle when nothing is changing.

mod screenshot;
mod window;

const USAGE: &str = "\
C-Shop — a native, GPU-accelerated layered image editor

USAGE:
    cshop [FILES]...                open the given images
    cshop --screenshot PATH [FILES] render one frame offscreen and exit
    cshop --help                    show this message

SCREENSHOT OPTIONS:
    --size WxH                      output size (default 1600x980)
    --demo                          build a sample layered document first
    --demo-selection                build a document with an active selection
    --right-click X,Y               right-click there, to capture a context menu
";

fn main() {
    // Stamped before anything else, so the startup figure covers the whole
    // process rather than only the parts after the logger exists.
    let started = std::time::Instant::now();

    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("cshop=info,warn"),
    )
    .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return;
    }

    let mut files = Vec::new();
    let mut shot: Option<String> = None;
    let mut size = (1600u32, 980u32);
    let mut demo = false;
    let mut demo_selection = false;
    let mut demo_quick_mask = false;
    let mut demo_adjust = false;
    let mut demo_transform = false;
    let mut demo_filter = false;
    let mut demo_blur = false;
    let mut demo_text = false;
    let mut demo_shapes = false;
    let mut demo_fx = false;
    let mut demo_fx_dialog = false;
    let mut clicks: Vec<(f32, f32, egui::PointerButton)> = Vec::new();
    let mut drag: Option<(f32, f32, f32, f32)> = None;
    let mut demo_curves = false;
    let mut demo_tools = false;
    let mut demo_tool: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--screenshot" => {
                i += 1;
                shot = args.get(i).cloned();
            }
            "--size" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    if let Some((w, h)) = v.split_once('x') {
                        if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
                            size = (w, h);
                        }
                    }
                }
            }
            "--demo" => demo = true,
            "--demo-selection" => demo_selection = true,
            "--demo-quickmask" => demo_quick_mask = true,
            "--demo-adjust" => demo_adjust = true,
            "--demo-transform" => demo_transform = true,
            "--demo-filter" => demo_filter = true,
            // A local filter zoomed to 100%, where the preview shows real detail.
            "--demo-blur" => demo_blur = true,
            "--demo-text" => demo_text = true,
            "--demo-shapes" => demo_shapes = true,
            "--demo-fx" => demo_fx = true,
            // The effects demo with the Layer Style dialog open on top.
            "--demo-fx-dialog" => {
                demo_fx = true;
                demo_fx_dialog = true;
            }
            "--demo-curves" => demo_curves = true,
            "--demo-tools" => demo_tools = true,
            // Picks which tool is active for the --demo-tools shot, so the
            // options bar of any one tool can be inspected on its own.
            "--demo-tool" => {
                i += 1;
                demo_tools = true;
                demo_tool = args.get(i).cloned();
            }
            // Repeatable, so a menu can be opened and then walked into.
            "--drag" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    let n: Vec<f32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
                    if let [x0, y0, x1, y1] = n[..] {
                        drag = Some((x0, y0, x1, y1));
                    }
                }
            }
            arg @ ("--right-click" | "--click") => {
                let secondary = arg == "--right-click";
                i += 1;
                if let Some(v) = args.get(i) {
                    if let Some((x, y)) = v.split_once(',') {
                        if let (Ok(x), Ok(y)) = (x.trim().parse(), y.trim().parse()) {
                            clicks.push((
                                x,
                                y,
                                if secondary {
                                    egui::PointerButton::Secondary
                                } else {
                                    egui::PointerButton::Primary
                                },
                            ));
                        }
                    }
                }
            }
            other if other.starts_with('-') => eprintln!("ignoring unknown option {other}"),
            other => files.push(other.to_string()),
        }
        i += 1;
    }

    // The font scan takes a moment; start it now so picking the Type tool
    // does not stall on it.
    cshop_core::font::FontDb::warm_up();

    let result = match shot {
        Some(path) => screenshot::capture(
            std::path::Path::new(&path),
            size,
            &files,
            // Enough frames for a click to land and for egui's fade-ins to
            // finish.
            30,
            |app| {
                if demo {
                    screenshot::build_demo(app);
                }
                if demo_selection {
                    screenshot::build_selection_demo(app);
                    app.tool = cshop_ui::tools::Tool::EllipticalMarquee;
                    app.selection_feather = 2.0;
                }
                if demo_adjust {
                    screenshot::build_adjustment_demo(app);
                }
                if demo_transform {
                    screenshot::build_selection_demo(app);
                    app.dispatch(cshop_ui::commands::Action::Deselect);
                    // Select an unlocked layer, then start a transform and drag
                    // a corner so the box and its preview are on screen.
                    let id = app.doc().and_then(|d| d.doc.tree.root().get(2).copied());
                    if let Some(id) = id {
                        app.dispatch(cshop_ui::commands::Action::SelectLayer(id));
                    }
                    app.dispatch(cshop_ui::commands::Action::BeginTransform);
                    if let Some(t) = app.transform.as_mut() {
                        use cshop_core::geom::Vec2;
                        use cshop_core::transform::Handle;
                        t.begin_drag(Handle::TopRight, Vec2::new(360.0, 40.0));
                        t.drag_to(Vec2::new(470.0, 120.0), true, false, false);
                        t.end_drag();
                    }
                    app.tool = cshop_ui::tools::Tool::Move;
                }
                if demo_blur {
                    screenshot::build_adjustment_demo(app);
                    app.dispatch(cshop_ui::commands::Action::FlattenImage);
                    app.dispatch(cshop_ui::commands::Action::ShowFilterDialog(Box::new(
                        cshop_core::filters::Filter::GaussianBlur { radius: 4.0 },
                    )));
                    if let cshop_ui::dialogs::Dialog::Filter(d) = &mut app.dialog {
                        d.zoom_to(1.0);
                    }
                }
                if demo_filter {
                    screenshot::build_adjustment_demo(app);
                    // Flatten to one raster layer, then open a filter dialog on
                    // it so the preview has something photographic to show.
                    app.dispatch(cshop_ui::commands::Action::FlattenImage);
                    app.dispatch(cshop_ui::commands::Action::ShowFilterDialog(Box::new(
                        cshop_core::filters::Filter::RadialBlur {
                            amount: 0.35,
                            spin: false,
                            centre: (0.72, 0.28),
                        },
                    )));
                }
                if demo_curves {
                    screenshot::build_adjustment_demo(app);
                    app.dispatch(cshop_ui::commands::Action::FlattenImage);
                    app.dispatch(cshop_ui::commands::Action::ShowAdjustmentDialog(Box::new(
                        cshop_core::adjust::Adjustment::Curves { curves: Default::default() },
                    )));
                }
                if demo_fx {
                    screenshot::build_effects_demo(app);
                    if demo_fx_dialog {
                        app.dispatch(cshop_ui::commands::Action::ShowLayerStyle);
                    }
                }
                if demo_shapes {
                    screenshot::build_shape_demo(app);
                }
                if demo_text {
                    screenshot::build_text_demo(app);
                }
                if demo_tools {
                    screenshot::build_tools_demo(app);
                    if let Some(name) = &demo_tool {
                        match cshop_ui::tools::Tool::from_name(name) {
                            Some(t) => app.tool = t,
                            None => eprintln!("unknown tool {name}"),
                        }
                    }
                }
                if demo_quick_mask {
                    screenshot::build_selection_demo(app);
                    app.dispatch(cshop_ui::commands::Action::ToggleQuickMask);
                    app.tool = cshop_ui::tools::Tool::Brush;
                }
            },
            clicks,
            drag,
        ),
        None => window::run(files, started),
    };

    if let Err(e) = result {
        eprintln!("C-Shop failed: {e}");
        std::process::exit(1);
    }
}
