//! C-Shop — a native, GPU-accelerated layered image editor.
//!
//! This binary owns the window, the swapchain and the egui integration. It
//! deliberately does not use a framework such as `eframe`: the compositor and
//! egui have to share one `wgpu::Device` so the composited canvas can be handed
//! to egui as a texture, and the render loop needs to stay under our control so
//! it can idle when nothing is changing.

mod mcp;
mod screenshot;
mod script;
mod window;

const USAGE: &str = "\
C-Shop — a native, GPU-accelerated layered image editor

USAGE:
    cshop [FILES]...                open the given images
    cshop --script PATH             run a script headlessly and exit
    cshop --run 'SCRIPT'            run a script given inline
    cshop --serve [ADDR]            serve the editor over MCP and stay up
    cshop --screenshot PATH [FILES] render one frame offscreen and exit
    cshop --help                    show this message

SERVE OPTIONS:
    --serve [ADDR]                  default 127.0.0.1:7333
    --workspace DIR                 the only directory scripts may touch
                                    (default: the working directory)
    --token SECRET                  require `Authorization: Bearer SECRET`;
                                    mandatory when ADDR is not loopback
    --allow-origin ORIGIN           permit a browser origin besides localhost

SCREENSHOT OPTIONS:
    --size WxH                      output size (default 1600x980)
    --demo                          build a sample layered document first
    --demo-selection                build a document with an active selection
    --demo-profile                  open the colour-profile window
    --right-click X,Y               right-click there, to capture a context menu
";

/// Read what `--serve` was given.
///
/// Accepts a full `host:port`, a bare port, or nothing at all — a caller
/// writing `--serve 8080` means a port, and refusing it on a technicality
/// would be pedantry.
fn parse_addr(given: &str) -> Result<std::net::SocketAddr, String> {
    use std::net::{SocketAddr, ToSocketAddrs};
    if given.is_empty() {
        return Ok(SocketAddr::from(([127, 0, 0, 1], 7333)));
    }
    if let Ok(port) = given.parse::<u16>() {
        return Ok(SocketAddr::from(([127, 0, 0, 1], port)));
    }
    given
        .to_socket_addrs()
        .map_err(|e| format!("could not read {given:?} as an address: {e}"))?
        .next()
        .ok_or_else(|| format!("{given:?} resolved to no address"))
}

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
    let mut demo_segment = false;
    let mut demo_profile = false;
    let mut clicks: Vec<(f32, f32, egui::PointerButton)> = Vec::new();
    let mut script: Option<String> = None;
    let mut script_inline: Option<String> = None;
    let mut report_json = false;
    let mut serve: Option<String> = None;
    let mut workspace: Option<String> = None;
    let mut token: Option<String> = None;
    let mut allow_origins: Vec<String> = Vec::new();
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
            // The scripted pathway: intake, draw, analyse, return.
            "--script" => {
                i += 1;
                script = args.get(i).cloned();
            }
            "--run" => {
                i += 1;
                script_inline = args.get(i).cloned();
            }
            "--json" => report_json = true,
            "--serve" => {
                // The address is optional, so only take the next argument if
                // it is not itself a flag.
                serve = Some(match args.get(i + 1) {
                    Some(next) if !next.starts_with('-') => {
                        i += 1;
                        next.clone()
                    }
                    _ => String::new(),
                });
            }
            "--workspace" => {
                i += 1;
                workspace = args.get(i).cloned();
            }
            "--token" => {
                i += 1;
                token = args.get(i).cloned();
            }
            "--allow-origin" => {
                i += 1;
                if let Some(origin) = args.get(i) {
                    allow_origins.push(origin.clone());
                }
            }
            "--demo" => demo = true,
            "--demo-segment" => demo_segment = true,
            "--demo-profile" => demo_profile = true,
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

    // Serving keeps the process alive; nothing after this runs.
    if let Some(addr) = serve {
        let config = mcp::server::Config {
            addr: match parse_addr(&addr) {
                Ok(addr) => addr,
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            },
            workspace: workspace
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
            token,
            allow_origins,
        };
        if let Err(e) = mcp::server::serve(config) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }

    // A script runs headlessly and exits: no window, no event loop.
    if script.is_some() || script_inline.is_some() {
        let (source, base) = match &script {
            Some(path) => {
                let path = std::path::PathBuf::from(path);
                let base = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                match std::fs::read_to_string(&path) {
                    Ok(s) => (s, base),
                    Err(e) => {
                        eprintln!("could not read {}: {e}", path.display());
                        std::process::exit(1);
                    }
                }
            }
            None => (
                script_inline.clone().unwrap_or_default(),
                std::env::current_dir().unwrap_or_default(),
            ),
        };
        match script::run(&source, &base) {
            Ok(report) => {
                print!("{}", if report_json { report.to_json() } else { report.summary() });
                // A failed step is a failed run, so a caller can branch on it.
                std::process::exit(if report.ok { 0 } else { 2 });
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }

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
                if demo_profile {
                    app.dispatch(cshop_ui::commands::Action::ShowColorProfile);
                }
                if demo_segment {
                    // Opened and shown mid-run, so the screenshot catches the
                    // spinner rather than a window that has already answered.
                    app.dispatch(cshop_ui::commands::Action::ShowSegment);
                    if let cshop_ui::dialogs::Dialog::Segment(d) = &mut app.dialog {
                        d.busy = true;
                        d.status = "Segmenting…".into();
                        d.found = vec![
                            cshop_ui::vision::Found {
                                class: "dog".into(),
                                score: 0.90,
                                box_: [10.0, 10.0, 100.0, 100.0],
                            },
                            cshop_ui::vision::Found {
                                class: "bench".into(),
                                score: 0.55,
                                box_: [0.0, 40.0, 120.0, 110.0],
                            },
                        ];
                        d.feather = 2.0;
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
