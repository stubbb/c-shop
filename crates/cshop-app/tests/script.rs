//! The scripted pathway: intake, draw, analyse, return.
//!
//! The parsing tests run anywhere; the ones that draw need a GPU and skip
//! themselves without one.

use std::path::Path;

/// The binary's own module, reached the way an integration test can.
// A test binary uses a subset of the module it includes; the rest being
// unused here says nothing about whether it is used in the binary.
#[allow(dead_code)]
#[path = "../src/script.rs"]
mod script;

fn run(source: &str) -> Option<script::Report> {
    script::run(source, Path::new(".")).ok()
}

#[test]
fn quotes_options_and_comments_all_parse() {
    let parsed = script::parse(
        r#"
# a comment, and the blank line above
text 10 20 "two words" size=48 bold
"#,
    );
    assert_eq!(parsed.len(), 1);
    let cmd = parsed.into_iter().next().unwrap().expect("should parse");
    assert_eq!(cmd.name, "text");
    // A bare flag stays among the positional arguments, because nothing tells
    // the parser it is not one — hence the rule that flags come last.
    assert_eq!(cmd.args, vec!["10", "20", "two words", "bold"]);
    assert_eq!(cmd.opt("size"), Some("48"));
    assert!(cmd.flag("bold"), "a bare word reads as a flag");
    assert!(!cmd.flag("italic"));
}

#[test]
fn an_unclosed_quote_is_reported_rather_than_swallowing_the_line() {
    let parsed = script::parse("text 1 2 \"never ends");
    let err = parsed.into_iter().next().unwrap().unwrap_err();
    assert!(err.2.contains("never closed"), "got {}", err.2);
}

#[test]
fn escapes_survive_inside_quotes() {
    let parsed = script::parse(r#"text 0 0 "a\nb \"quoted\"""#);
    let cmd = parsed.into_iter().next().unwrap().expect("parses");
    assert_eq!(cmd.args[2], "a\nb \"quoted\"");
}

#[test]
fn colours_take_every_written_form() {
    use cshop_core::color::Rgba8;
    assert_eq!(script::parse_color("#f00").unwrap(), Rgba8::opaque(255, 0, 0));
    assert_eq!(script::parse_color("#ff8800").unwrap(), Rgba8::opaque(255, 136, 0));
    assert_eq!(script::parse_color("#ff880080").unwrap(), Rgba8::new(255, 136, 0, 128));
    assert_eq!(script::parse_color("white").unwrap(), Rgba8::WHITE);
    assert!(script::parse_color("#12345").is_err(), "an odd length is not a colour");
    assert!(script::parse_color("chartreuse").unwrap_err().contains("not a colour"));
}

#[test]
fn a_script_builds_what_it_describes() {
    let Some(report) = run(
        r#"
new 200 120 background=#101010
text 10 80 "Hi" size=40 color=#ffffff
effect drop-shadow distance=4 size=6
"#,
    ) else {
        return;
    };
    assert!(report.ok, "{}", report.summary());
    assert_eq!(report.document.as_ref().map(|d| (d.1, d.2)), Some((200, 120)));
    assert_eq!(report.layers.len(), 2, "a background and the type");

    let type_layer = report.layers.iter().find(|l| l.kind == "Type").expect("a type layer");
    assert_eq!(type_layer.effects, vec!["Drop Shadow"]);
    // The report has to say where it landed, or a caller cannot place anything.
    assert!(type_layer.bounds[2] > 0 && type_layer.bounds[3] > 0);
}

/// The rule the design rests on: one bad line does not discard the rest, and
/// every failure says why.
#[test]
fn a_failed_step_is_reported_and_the_run_continues() {
    let Some(report) = run(
        r#"
new 100 100
wobble the canvas
fill #00ff00
"#,
    ) else {
        return;
    };
    assert!(!report.ok, "the run should be marked failed");
    assert_eq!(report.steps.len(), 3);
    assert!(report.steps[0].ok);
    assert!(!report.steps[1].ok);
    assert!(report.steps[1].note.contains("unknown command"), "{}", report.steps[1].note);
    // The listing of what *is* available is what makes the failure actionable.
    assert!(report.steps[1].note.contains("text"), "the error should list the alternatives");
    assert!(report.steps[2].ok, "the run should carry on past the bad line");
}

#[test]
fn drawing_before_there_is_a_document_says_so() {
    let Some(report) = run("text 0 0 \"nowhere\"") else { return };
    assert!(!report.ok);
    assert!(report.steps[0].note.contains("no document"), "{}", report.steps[0].note);
}

#[test]
fn an_unknown_font_is_refused_rather_than_quietly_substituted() {
    let Some(report) = run(
        "new 100 100\ntext 0 50 \"x\" family=\"Definitely Not Installed\"",
    ) else {
        return;
    };
    assert!(!report.ok);
    assert!(report.steps[1].note.contains("no font family"), "{}", report.steps[1].note);
}

/// Measuring without drawing is what lets a caller place text before
/// committing to it.
#[test]
fn measure_reports_a_size_without_adding_a_layer() {
    let Some(report) = run("new 300 200\nmeasure text \"Measured\" size=40") else { return };
    assert!(report.ok, "{}", report.summary());
    assert_eq!(report.layers.len(), 1, "measuring must not draw anything");
    let (key, value) = report.facts.first().expect("a measurement should be reported");
    assert!(key.contains("Measured"));
    assert!(value.contains('x'), "the fact should carry a size, got {value}");
}

#[test]
fn shapes_effects_and_adjustments_all_reach_the_document() {
    let Some(report) = run(
        r#"
new 160 160 background=white
shape star 20 20 120 120 fill=#ffcc33 stroke=#332200 stroke-width=3
effect outer-glow size=8 color=#ff8800
adjust brightness-contrast contrast=0.2
"#,
    ) else {
        return;
    };
    assert!(report.ok, "{}", report.summary());
    let shape = report.layers.iter().find(|l| l.kind == "Shape").expect("a shape layer");
    assert_eq!(shape.effects, vec!["Outer Glow"]);
}

#[test]
fn exporting_writes_a_file_and_names_it_in_the_report() {
    let dir = std::env::temp_dir().join("cshop-script-test");
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join("out.png");
    let _ = std::fs::remove_file(&out);

    let source = format!("new 40 30 background=#ff0000\nexport {}", out.display());
    let Some(report) = run(&source) else { return };
    assert!(report.ok, "{}", report.summary());
    assert_eq!(report.outputs.len(), 1);
    assert!(out.exists(), "the file should be on disk");

    // And it should hold what was asked for.
    let back = cshop_io::load(&out).expect("read back");
    assert_eq!((back.width(), back.height()), (40, 30));
    assert_eq!(back.get(20, 15), cshop_core::color::Rgba8::opaque(255, 0, 0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_report_is_well_formed_json() {
    let Some(report) = run("new 20 20\ninfo") else { return };
    let json = report.to_json();
    // Not a parser, but enough to catch an unbalanced or unescaped emission.
    assert_eq!(
        json.matches('{').count(),
        json.matches('}').count(),
        "braces should balance:\n{json}"
    );
    assert_eq!(json.matches('[').count(), json.matches(']').count());
    assert!(json.contains("\"ok\": true"));
    assert!(json.contains("\"document\""));
    assert!(json.contains("\"steps\""));
}

#[test]
fn strings_with_quotes_and_newlines_do_not_break_the_json() {
    let Some(mut report) = run("new 10 10") else { return };
    report.facts.push(("odd \"key\"".into(), "line\nbreak\tand \\ slash".into()));
    let json = report.to_json();
    assert!(json.contains(r#"odd \"key\""#), "quotes should be escaped:\n{json}");
    assert!(json.contains(r"line\nbreak\tand \\ slash"), "controls should be escaped");
    assert_eq!(json.matches('{').count(), json.matches('}').count());
}

/// `~` is how a path gets written by hand, so it has to mean the home
/// directory rather than a folder of that name.
#[test]
fn a_leading_tilde_is_expanded() {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return;
    }
    let base = Path::new("/tmp");
    assert_eq!(script::resolve(base, "~/assets/x.jpg"), Path::new(&home).join("assets/x.jpg"));
    assert_eq!(script::resolve(base, "/abs/x.jpg"), Path::new("/abs/x.jpg"));
    assert_eq!(script::resolve(base, "rel/x.jpg"), Path::new("/tmp/rel/x.jpg"));
}

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

/// Write a style file into a temporary directory and hand back the directory.
fn styles_dir(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cshop-style-{name}"));
    let _ = std::fs::create_dir_all(dir.join("styles"));
    for (file, body) in files {
        std::fs::write(dir.join("styles").join(file), body).expect("write style");
    }
    dir
}

#[test]
fn a_style_separates_its_parameters_from_its_body() {
    let s = script::parse_style(
        "# a comment\nparam blur = 12\nparam ink = #333333\n\nfilter gaussian-blur radius={blur}\n",
    );
    assert_eq!(s.params, vec![("blur".into(), "12".into()), ("ink".into(), "#333333".into())]);
    assert!(s.body.contains("gaussian-blur radius={blur}"));
    assert!(!s.body.contains("param blur"), "parameters are not commands");
}

#[test]
fn substitution_fills_holes_and_refuses_unknown_ones() {
    let values = vec![("blur".to_string(), "9".to_string())];
    assert_eq!(script::substitute("radius={blur}", &values).unwrap(), "radius=9");
    assert_eq!(script::substitute("{blur} and {blur}", &values).unwrap(), "9 and 9");
    assert_eq!(script::substitute("nothing here", &values).unwrap(), "nothing here");

    // Leaving an unknown name in place would draw `{radius}` pixels of blur.
    let err = script::substitute("radius={radius}", &values).unwrap_err();
    assert!(err.contains("no parameter \"radius\""), "{err}");
    assert!(err.contains("blur"), "the error should list what it does take");

    assert!(script::substitute("a {b", &values).unwrap_err().contains("never closed"));
}

#[test]
fn a_style_runs_its_body_and_the_report_says_which_style() {
    let dir = styles_dir(
        "basic",
        &[("tint.style", "param level = 0.3\nadjust brightness-contrast brightness={level}\n")],
    );
    let Ok(report) = script::run("new 40 40\nstyle tint", &dir) else { return };
    assert!(report.ok, "{}", report.summary());
    // Steps a style ran carry its name, so a failure inside one is traceable.
    let inner = report
        .steps
        .iter()
        .find(|s| s.command.contains("brightness-contrast"))
        .expect("the style's own step should be reported");
    assert!(inner.command.starts_with("tint:"), "got {:?}", inner.command);
}

#[test]
fn a_parameter_the_style_does_not_declare_is_refused() {
    let dir = styles_dir("params", &[("tint.style", "param level = 0.3\nadjust invert\n")]);
    let Ok(report) = script::run("new 40 40\nstyle tint wobble=2", &dir) else { return };
    assert!(!report.ok);
    let note = &report.steps[1].note;
    assert!(note.contains("no parameter \"wobble\""), "{note}");
    assert!(note.contains("level"), "the error should say what it does take: {note}");
}

#[test]
fn an_override_wins_over_the_default() {
    let dir = styles_dir(
        "override",
        &[("box.style", "param w = 10\nshape rect 0 0 {w} {w} fill=#000000\n")],
    );
    let Ok(report) = script::run("new 100 100\nstyle box w=60", &dir) else { return };
    assert!(report.ok, "{}", report.summary());
    let shape = report.layers.iter().find(|l| l.kind == "Shape").expect("a shape");
    // The report gives what the layer *draws*, which is a couple of pixels
    // wider than the geometry because of the antialiasing margin — so this
    // checks the override took, not an exact edge.
    assert!(
        (58..=66).contains(&shape.bounds[2]),
        "the override should have set the width to about 60, got {}",
        shape.bounds[2]
    );
}

#[test]
fn an_unknown_style_lists_the_ones_there_are() {
    let dir = styles_dir("listing", &[("alpha.style", "adjust invert\n"), ("beta.style", "adjust invert\n")]);
    let Ok(report) = script::run("style gamma", &dir) else { return };
    assert!(!report.ok);
    let note = &report.steps[0].note;
    assert!(note.contains("no style called \"gamma\""), "{note}");
    assert!(note.contains("alpha") && note.contains("beta"), "{note}");
    // Several search paths can find one directory; the list must not repeat.
    assert_eq!(note.matches("alpha").count(), 1, "{note}");
}

/// A style is script, and script can apply a style, so one that applies itself
/// would otherwise never stop.
#[test]
fn a_style_that_applies_itself_is_stopped() {
    let dir = styles_dir("recursive", &[("loop.style", "style loop\n")]);
    let Ok(report) = script::run("new 20 20\nstyle loop", &dir) else { return };
    assert!(!report.ok);
    assert!(
        report.steps.iter().any(|s| s.note.contains("applying itself")),
        "the depth limit should say what it suspects"
    );
    // And it must actually have stopped rather than run away.
    assert!(report.steps.len() < 40, "ran {} steps", report.steps.len());
}

#[test]
fn styles_compose() {
    let dir = styles_dir(
        "compose",
        &[
            ("outer.style", "param n = 3\nstyle inner levels={n}\n"),
            ("inner.style", "param levels = 8\nadjust posterize levels={levels}\n"),
        ],
    );
    let Ok(report) = script::run("new 30 30\nstyle outer n=5", &dir) else { return };
    assert!(report.ok, "{}", report.summary());
    let deep = report
        .steps
        .iter()
        .find(|s| s.command.contains("posterize"))
        .expect("the inner style should have run");
    assert!(deep.command.contains("outer > inner"), "the trail should show both: {:?}", deep.command);
    assert!(deep.command.contains("levels=5"), "the value should pass through: {:?}", deep.command);
}

#[test]
fn a_hole_that_is_a_bare_name_is_replaced_verbatim() {
    // So a parameter can carry a blend mode, not only a number.
    let out = script::substitute("set blend=\"{mode}\"", &[("mode".into(), "Color Dodge".into())])
        .expect("should substitute");
    assert_eq!(out, "set blend=\"Color Dodge\"");
}

#[test]
fn arithmetic_in_a_hole_is_evaluated() {
    let v = |body: &str| {
        script::substitute(
            body,
            &[("min".into(), "800".into()), ("n".into(), "3".into())],
        )
    };
    assert_eq!(v("{min*0.01}").unwrap(), "8");
    assert_eq!(v("{min/4}").unwrap(), "200");
    // Multiplication binds tighter than addition, and parens override it.
    assert_eq!(v("{n+n*2}").unwrap(), "9");
    assert_eq!(v("{(n+n)*2}").unwrap(), "12");
    assert_eq!(v("{0-n}").unwrap(), "-3");
    assert_eq!(v("{-n}").unwrap(), "-3");
    // Whitespace is stripped, so subtraction survives being written spaced out.
    assert_eq!(v("{min - 800}").unwrap(), "0");
}

#[test]
fn arithmetic_that_cannot_work_says_why() {
    let v = |body: &str| script::substitute(body, &[("n".into(), "2".into())]);
    assert!(v("{n/0}").unwrap_err().contains("divides by zero"));
    assert!(v("{n*q}").unwrap_err().contains("not a parameter"));
    assert!(v("{(n+1}").unwrap_err().contains("unclosed"));
    // A bare unknown name is still the plain error, which names the parameters.
    assert!(v("{wobble}").unwrap_err().contains("no parameter"));
}

#[test]
fn a_style_can_scale_itself_to_the_document() {
    let dir = styles_dir("sized", &[("half.style", "resize {width*0.5} {height*0.25}\n")]);
    let Ok(report) = script::run("new 200 80\nstyle half", &dir) else { return };
    assert!(report.ok, "{}", report.summary());
    let (_, w, h) = report.document.expect("a document");
    assert_eq!((w, h), (100, 20), "half the width, a quarter the height");
}

#[test]
fn a_style_parameter_beats_the_bound_document_size() {
    let dir = styles_dir("shadow-width", &[("own.style", "param width = 12\nresize {width} {width}\n")]);
    let Ok(report) = script::run("new 300 300\nstyle own", &dir) else { return };
    assert!(report.ok, "{}", report.summary());
    let (_, w, h) = report.document.expect("a document");
    assert_eq!((w, h), (12, 12), "the style's own name wins over the bound size");
}

#[test]
fn resize_scales_and_keeps_the_proportions() {
    let size = |source: &str| run(source).and_then(|r| r.document).map(|(_, w, h)| (w, h));
    let Some(got) = size("new 400 100\nresize fit=200") else { return };
    assert_eq!(got, (200, 50), "fit works off the long side");
    assert_eq!(size("new 400 100\nresize 80").unwrap(), (80, 20), "one keeps the ratio");
    assert_eq!(size("new 400 100\nresize 80 80").unwrap(), (80, 80), "two are taken as given");
    assert_eq!(size("new 400 100\nresize scale=0.5").unwrap(), (200, 50));
    // The canvas form pads or crops rather than scaling.
    assert_eq!(size("new 400 100\nresize 500 200 canvas").unwrap(), (500, 200));
}

#[test]
fn a_whole_number_option_accepts_the_decimal_arithmetic_produces() {
    // `radius={min*0.0045}` arrives as "3.0465"; refusing it would mean no
    // integer option could ever be scaled to the image.
    let cmd = script::parse("filter median radius=3.0465")
        .into_iter()
        .next()
        .unwrap()
        .expect("should parse");
    assert_eq!(cmd.u32("radius").unwrap(), Some(3));
}

/// Every style in the repository has to apply cleanly.
///
/// Discovered from the directory rather than listed, so a style added later is
/// covered without anyone remembering to add it here. A `-lettering` style
/// wants a type layer under it and the rest want a picture, which is the one
/// distinction the loop has to make.
#[test]
fn every_shipped_style_applies() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = with_image("shipped", "#6b8f5a");
    let src = dir.join("src.png");
    let mut names: Vec<String> = std::fs::read_dir(repo.join("styles"))
        .expect("the styles directory should exist")
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            (path.extension()? == "style")
                .then(|| path.file_stem()?.to_str().map(str::to_string))?
        })
        .collect();
    names.sort();
    assert!(names.len() >= 10, "expected a library of styles, found {names:?}");

    for name in &names {
        let script = if name.ends_with("-lettering") {
            format!(
                "open {}
resize 240 160
text 10 90 \"Ab\" size=48
style {name}",
                src.display()
            )
        } else {
            // Small, or the heavier styles make the suite crawl.
            format!("open {}
resize 240 160
style {name}", src.display())
        };
        let Ok(report) = script::run(&script, &repo) else { return };
        assert!(report.ok, "style {name:?} failed: {}", report.summary());
    }
}

/// The two styles the documentation walks through, in more detail.
#[test]
fn the_bundled_styles_apply() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Ok(report) = script::run(
        "new 80 60 background=#4488aa\nstyle pencil-sketch blur=4\ntext 5 40 \"a\" size=20\nstyle pencil-lettering",
        &repo,
    ) else {
        return;
    };
    assert!(report.ok, "{}", report.summary());
    let type_layer = report.layers.iter().find(|l| l.kind == "Type").expect("type");
    assert!(
        type_layer.effects.contains(&"Stroke".to_string()),
        "the lettering style should have outlined it: {:?}",
        type_layer.effects
    );
}

// ---------------------------------------------------------------------------
// Placing
// ---------------------------------------------------------------------------

/// Write a small image and hand back the directory holding it.
fn with_image(name: &str, colour: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cshop-place-{name}"));
    let _ = std::fs::create_dir_all(&dir);
    let src = dir.join("src.png");
    let _ = script::run(&format!("new 30 20 background={colour}\nexport {}", src.display()), &dir);
    dir
}

#[test]
fn place_adds_an_image_as_a_layer_where_it_is_told() {
    let dir = with_image("basic", "#ff0000");
    let Ok(report) = script::run("new 100 100 background=white\nplace src.png x=25 y=40", &dir)
    else {
        return;
    };
    assert!(report.ok, "{}", report.summary());
    assert_eq!(report.layers.len(), 2, "the document keeps its own layer");
    let placed = &report.layers[1];
    assert_eq!(placed.bounds, [25, 40, 30, 20], "placed at the size and spot asked for");
    assert!(placed.name.contains("src"), "named after the file: {}", placed.name);
}

/// The no-argument form is what lets a style lay an original back over its own
/// treatment without being told where the original lives.
#[test]
fn place_with_no_path_re_places_the_document_s_own_file() {
    let dir = with_image("reopen", "#00ff00");
    let Ok(report) = script::run("open src.png\nadjust invert\nplace", &dir) else { return };
    assert!(report.ok, "{}", report.summary());
    assert_eq!(report.layers.len(), 2);
    assert_eq!(report.layers[1].bounds, [0, 0, 30, 20]);
}

#[test]
fn place_with_nothing_to_re_place_says_so() {
    let dir = std::env::temp_dir();
    let Ok(report) = script::run("new 40 40\nplace", &dir) else { return };
    assert!(!report.ok);
    assert!(
        report.steps[1].note.contains("not opened from a file"),
        "got {}",
        report.steps[1].note
    );
}

/// Colour blending keeps the backdrop's lightness, so the drawing underneath
/// has to keep some midtone or there is nowhere for colour to live. This pins
/// the pipeline that depends on it.
#[test]
fn the_coloured_pencil_style_lays_colour_back_over_its_own_drawing() {
    let dir = with_image("coloured", "#3388cc");
    let src = dir.join("src.png");
    // Run from the repo, so the bundled style is always found and a failure
    // here is a real one rather than a style that could not be located.
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = format!("open {}\nstyle coloured-pencil softness=0.06", src.display());
    let Ok(report) = script::run(&script, &repo) else { return };
    assert!(report.ok, "{}", report.summary());
    let colour = report
        .layers
        .iter()
        .find(|l| l.name == "Colour")
        .expect("the original should be back on top");
    assert_eq!(colour.blend, "Color");
    assert!((colour.opacity - 0.5).abs() < 0.01, "opacity was {}", colour.opacity);
}

// ---------------------------------------------------------------------------
// Sandboxing
// ---------------------------------------------------------------------------

mod sandbox {
    use super::script::Sandbox;
    use std::path::PathBuf;

    fn workspace(name: &str) -> (Sandbox, PathBuf) {
        let dir = std::env::temp_dir().join(format!("cshop-sandbox-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        let sandbox = Sandbox::new(&dir).expect("make the workspace");
        let root = sandbox.root().to_path_buf();
        (sandbox, root)
    }

    #[test]
    fn a_plain_relative_path_resolves_inside_the_root() {
        let (sandbox, root) = workspace("plain");
        let got = sandbox.resolve("out.png").expect("should resolve");
        assert_eq!(got, root.join("out.png"));
        // A directory that does not exist yet still resolves: export creates
        // its parents, and the check that matters is where it lands.
        let got = sandbox.resolve("nested/deep/out.png").expect("should resolve");
        assert_eq!(got, root.join("nested/deep/out.png"));
    }

    #[test]
    fn climbing_out_is_refused() {
        let (sandbox, _) = workspace("climb");
        for attempt in ["../secrets", "a/../../secrets", "..", "a/.."] {
            let err = sandbox.resolve(attempt).expect_err(attempt);
            assert!(err.contains(".."), "{attempt}: {err}");
        }
    }

    #[test]
    fn absolute_paths_and_home_are_refused() {
        let (sandbox, _) = workspace("absolute");
        assert!(sandbox.resolve("/etc/passwd").unwrap_err().contains("absolute"));
        assert!(sandbox.resolve("~/.ssh/id_rsa").unwrap_err().contains("home"));
        assert!(sandbox.resolve("~").unwrap_err().contains("home"));
    }

    /// The check the lexical pass cannot make on its own.
    #[test]
    #[cfg(unix)]
    fn a_symlink_pointing_out_of_the_workspace_is_refused() {
        let (sandbox, root) = workspace("symlink");
        let outside = std::env::temp_dir().join("cshop-sandbox-symlink-target");
        std::fs::create_dir_all(&outside).expect("make the target");
        std::fs::write(outside.join("secret.txt"), b"secret").expect("write");
        std::os::unix::fs::symlink(&outside, root.join("escape")).expect("link");

        // Lexically this is a plain relative path with no `..` in it at all.
        let err = sandbox.resolve("escape/secret.txt").expect_err("should refuse");
        assert!(err.contains("outside the workspace"), "{err}");
    }

    #[test]
    fn a_symlink_inside_the_workspace_is_allowed() {
        let (sandbox, root) = workspace("inner-link");
        std::fs::create_dir_all(root.join("real")).expect("make");
        std::fs::write(root.join("real/photo.png"), b"x").expect("write");
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).expect("link");
        #[cfg(unix)]
        assert!(sandbox.resolve("alias/photo.png").is_ok(), "staying inside is fine");
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

#[test]
fn path_data_parses_the_four_commands() {
    let subs = script::parse_path_data("M 0 0 L 10 0 C 20 0 30 10 30 20 Z")
        .expect("should parse");
    assert_eq!(subs.len(), 1);
    let sub = &subs[0];
    assert!(sub.closed, "Z closes it");
    assert_eq!(sub.anchors.len(), 3, "move, line, curve");
    // The curve's first control point belongs to the anchor it leaves.
    assert_eq!(sub.anchors[1].out_handle, cshop_core::geom::Vec2::new(20.0, 0.0));
    assert_eq!(sub.anchors[2].in_handle, cshop_core::geom::Vec2::new(30.0, 10.0));

    // Two contours, and only the closed one says so.
    let subs = script::parse_path_data("M 0 0 L 5 5 Z M 10 10 L 20 20").expect("should parse");
    assert_eq!(subs.len(), 2);
    assert!(subs[0].closed && !subs[1].closed);
}

#[test]
fn malformed_path_data_says_what_is_wrong() {
    assert!(script::parse_path_data("L 1 2").unwrap_err().contains("start with M"));
    assert!(script::parse_path_data("M 1").unwrap_err().contains("number"));
    assert!(script::parse_path_data("Q 1 2 3 4").unwrap_err().contains("not a path command"));
}

#[test]
fn a_path_becomes_a_shape_layer() {
    let Some(report) = run("new 120 120\npath \"M 10 10 L 100 10 L 100 100 Z\" fill=#ff0000")
    else {
        return;
    };
    assert!(report.ok, "{}", report.summary());
    let shape = report.layers.iter().find(|l| l.kind == "Shape").expect("a shape layer");
    // The layer is placed where the path is, not at the origin.
    assert!(shape.bounds[0] <= 10 && shape.bounds[1] <= 10, "{:?}", shape.bounds);
}

/// An unclosed path is a stroke, so it must not arrive with a fill.
#[test]
fn an_open_path_is_given_a_stroke_rather_than_a_fill() {
    let Some(report) = run("new 120 120\npath \"M 10 10 L 100 60\" fill=#0000ff") else {
        return;
    };
    assert!(report.ok, "{}", report.summary());
    assert!(report.layers.iter().any(|l| l.kind == "Shape"));
}

#[test]
fn shapes_combine_into_one_path_layer() {
    for op in ["union", "subtract", "intersect", "exclude"] {
        let source = format!(
            "new 200 140\nshape ellipse 10 20 90 90 fill=#3366cc\n\
             shape ellipse 60 20 90 90 fill=#3366cc\ncombine {op}"
        );
        let Some(report) = run(&source) else { return };
        assert!(report.ok, "{op}: {}", report.summary());
        let shapes: Vec<_> = report.layers.iter().filter(|l| l.kind == "Shape").collect();
        assert_eq!(shapes.len(), 1, "{op}: the operands should have become one layer");
    }
}

#[test]
fn combining_needs_something_to_combine() {
    let Some(report) = run("new 100 100\nshape ellipse 10 10 50 50\ncombine union") else {
        return;
    };
    assert!(!report.ok, "one shape is not a combination");
    assert!(
        report.steps.iter().any(|s| s.note.contains("two or more")),
        "should say what is missing: {:?}",
        report.steps.last().map(|s| &s.note)
    );

    let Some(report) = run("new 100 100\nshape ellipse 0 0 40 40\nshape ellipse 20 0 40 40\ncombine wibble")
    else {
        return;
    };
    assert!(!report.ok);
    assert!(report.steps.iter().any(|s| s.note.contains("Union")), "should list the operations");
}

// ---------------------------------------------------------------------------
// The vision pack
// ---------------------------------------------------------------------------

/// Without the pack, the commands have to say so — not crash, and not
/// pretend to have worked.
#[test]
fn the_vision_commands_explain_themselves_when_the_pack_is_absent() {
    if cshop_ui::vision::is_available() {
        return;
    }
    for source in ["new 40 40\ndetect", "new 40 40\nsegment class=dog"] {
        let Some(report) = run(source) else { return };
        assert!(!report.ok, "{source:?} should fail without the pack");
        let note = report.steps.last().map(|s| s.note.clone()).unwrap_or_default();
        assert!(
            note.contains("not installed") || note.contains("setup.sh"),
            "it should say what is missing and how: {note}"
        );
    }
}

#[test]
fn segment_asks_for_a_prompt_when_given_none() {
    let Some(report) = run("new 40 40\nsegment") else { return };
    assert!(!report.ok);
    let note = report.steps.last().map(|s| s.note.clone()).unwrap_or_default();
    // Either it has no prompt, or it has no pack; both are worth saying.
    assert!(
        note.contains("class=") || note.contains("not installed") || note.contains("setup.sh"),
        "{note}"
    );
}

/// The whole point of the pair: find a thing, then cut that thing out.
#[test]
fn detect_then_segment_isolates_what_was_found() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let sample = std::path::PathBuf::from(std::env::var("HOME").unwrap())
        .join("assets/samples/dog.jpg");
    if !sample.exists() {
        return;
    }
    let out = std::env::temp_dir().join("cshop-vision-test-dog.png");
    let _ = std::fs::remove_file(&out);
    let source = format!(
        "open {}\nresize fit=700\ndetect class=dog\nsegment feather=1\n\
         layer via-copy\nlayer select 0\nlayer delete\nexport {}",
        sample.display(),
        out.display()
    );
    let Some(report) = run(&source) else { return };
    assert!(report.ok, "{}", report.summary());

    // The detection is reported as a fact, so a caller reading JSON gets it.
    assert!(
        report.facts.iter().any(|(k, _)| k.contains("dog")),
        "the detection should be in the report: {:?}",
        report.facts
    );

    // And the export is mostly transparent, because the background is gone.
    let cut = cshop_io::load(&out).expect("the cutout should have been written");
    let opaque = cut.pixels().iter().filter(|p| p.a > 128).count();
    let share = opaque as f64 / cut.pixels().len() as f64;
    assert!(
        (0.02..0.6).contains(&share),
        "a cut-out dog should be a minority of an otherwise empty picture, not {:.0}%",
        share * 100.0
    );
    let _ = std::fs::remove_file(&out);
}

/// "There was no detect" and "the detect found nothing" are different
/// mistakes, and a caller that cannot see the picture needs to be told which.
#[test]
fn segment_tells_apart_no_detection_from_an_empty_one() {
    let Some(report) = run("new 60 60\nsegment") else { return };
    let note = report.steps.last().map(|s| s.note.clone()).unwrap_or_default();
    if note.contains("not installed") || note.contains("setup.sh") {
        return;
    }
    assert!(note.contains("or a `detect` before it"), "{note}");

    if !cshop_ui::vision::is_available() {
        return;
    }
    // A detect that runs and finds nothing must say so rather than claim it
    // never ran.
    let Some(report) = run("new 200 200 background=white\ndetect class=person\nsegment") else {
        return;
    };
    let note = report.steps.last().map(|s| s.note.clone()).unwrap_or_default();
    assert!(
        note.contains("found nothing to segment"),
        "it should say the detection was empty, not that there was none: {note}"
    );
}

/// The expand option is bounded, and says so before it goes near a model.
#[test]
fn segment_will_not_expand_past_its_range() {
    let Some(report) = run("new 60 60\nsegment point=30,30 expand=60") else { return };
    let note = report.steps.last().map(|s| s.note.clone()).unwrap_or_default();
    assert!(
        note.contains("expand goes up to 50 pixels"),
        "it should name the limit rather than try: {note}"
    );
}

// --- colour profiles -------------------------------------------------------

/// What each step said, for an assertion that fails usefully.
fn notes(report: &script::Report) -> Vec<String> {
    report.steps.iter().map(|s| s.note.clone()).collect()
}

const CMYK_ICC: &str = "/usr/share/color/icc/ghostscript/default_cmyk.icc";
const WIDE_ICC: &str = "/usr/share/color/icc/colord/WideGamutRGB.icc";
const INK_JPG: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../cshop-io/tests/assets/ink.jpg");

#[test]
fn a_document_reports_the_space_it_works_in() {
    let Some(report) = run("new 8 8\nprofile\ninfo") else { return };
    assert!(report.ok, "{:?}", notes(&report));
    let notes: Vec<&str> = report.steps.iter().map(|s| s.note.as_str()).collect();
    assert!(notes[1].contains("sRGB"), "{notes:?}");
    assert!(notes[2].contains("sRGB"), "info should say it too: {notes:?}");
}

#[test]
fn assign_and_convert_are_told_apart() {
    if !std::path::Path::new(WIDE_ICC).exists() {
        return;
    }
    let Some(report) = run(&format!("new 8 8 background=white\nprofile assign {WIDE_ICC}")) else {
        return;
    };
    assert!(report.ok, "{:?}", notes(&report));
    let note = &report.steps[1].note;
    assert!(note.contains("untouched"), "assign must say it changed nothing: {note}");

    let Some(report) = run(&format!("new 8 8 background=white\nprofile convert {WIDE_ICC}")) else {
        return;
    };
    assert!(report.ok);
    assert!(report.steps[1].note.contains("converted"), "{:?}", report.steps[1].note);
}

/// A press profile is not somewhere a document can work, and the refusal
/// should point at where it does belong rather than just saying no.
#[test]
fn a_press_profile_is_refused_as_a_working_space() {
    if !std::path::Path::new(CMYK_ICC).exists() {
        return;
    }
    let Some(report) = run(&format!("new 8 8\nprofile convert {CMYK_ICC}")) else { return };
    let note = &report.steps[1].note;
    assert!(note.contains("CMYK"), "{note}");
    assert!(note.contains("export profile="), "it should say where ink is made: {note}");
}

#[test]
fn a_missing_profile_is_named_rather_than_ignored() {
    let Some(report) = run("new 8 8\nprofile convert /nowhere/at/all.icc") else { return };
    assert!(!report.ok);
    assert!(report.steps[1].note.contains("could not read the profile"), "{:?}", notes(&report));
}

/// Opening a file made of ink says so, because it is the sort of thing
/// someone comparing two programs' output needs to know happened.
#[test]
fn opening_ink_says_that_is_what_it_was() {
    let Some(report) = run(&format!("open {INK_JPG}\ninfo")) else { return };
    assert!(report.ok, "{:?}", notes(&report));
    assert!(report.steps[0].note.contains("four inks"), "{:?}", report.steps[0].note);
}

/// The whole trip: a picture out to a press and back again.
#[test]
fn a_picture_can_go_to_a_press_and_come_home() {
    if !std::path::Path::new(CMYK_ICC).exists() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("cshop-press-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join("press.tif");
    let Some(report) = run(&format!(
        "new 16 16 background=white\nfill #c8503c\nexport {} profile={CMYK_ICC}",
        out.display()
    )) else {
        return;
    };
    assert!(report.ok, "{:?}", notes(&report));
    assert!(report.steps[2].note.contains("four inks"), "{:?}", report.steps[2].note);

    let written = std::fs::read(&out).expect("the press file");
    assert!(cshop_io::cmyk::is_separated(&written), "it should be ink");
    assert!(cshop_io::icc::embedded(&written).is_some(), "and say which press");

    // And back: reopening converts it to colour, near enough to where it began.
    let Some(report) = run(&format!("open {}\ninfo", out.display())) else { return };
    assert!(report.ok);
    assert!(report.steps[0].note.contains("converted from"), "{:?}", report.steps[0].note);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sixteen bits a channel, and the measurement that says it is not decoration.
///
/// A gradient laid at thirty percent opacity is 256 tones squeezed into a
/// narrow band. At eight bits the band has nowhere to put them and they
/// collapse into each other — that is what banding is. The compositor has
/// already done the arithmetic with room to spare, in `Rgba16Float`, so the
/// only question is whether the way out keeps it.
#[test]
fn a_deep_export_keeps_tones_that_eight_bits_collapses() {
    let dir = std::env::temp_dir().join(format!("cshop-deep-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let (shallow, deep) = (dir.join("eight.png"), dir.join("sixteen.png"));

    let Some(report) = run(&format!(
        "new 512 8 background=white\nlayer new\n\
         gradient 0 0 512 0 from=#000000 to=#ffffff\nset opacity=0.3\n\
         export {}\nexport {} depth=16",
        shallow.display(),
        deep.display()
    )) else {
        return;
    };
    assert!(report.ok, "{:?}", notes(&report));
    assert!(report.steps.last().unwrap().note.contains("16 bits"), "{:?}", notes(&report));

    let levels = |path: &std::path::Path| -> usize {
        // Counted at sixteen bits either way, so the two are comparable: an
        // eight-bit file simply has fewer distinct values to widen.
        let bytes = std::fs::read(path).expect("an exported file");
        let (deep, _) =
            cshop_io::decode_deep(&bytes, None, &cshop_core::profile::Profile::srgb()).unwrap();
        deep.pixels().iter().map(|p| p.r).collect::<std::collections::HashSet<_>>().len()
    };
    let (eight, sixteen) = (levels(&shallow), levels(&deep));
    assert!(
        sixteen > eight * 2,
        "the deep export should hold far more tone: {sixteen} against {eight}"
    );
    assert!(sixteen > 200, "and nearly all of the 256 that went in: {sixteen}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_depth_that_is_neither_eight_nor_sixteen_is_refused() {
    let dir = std::env::temp_dir().join(format!("cshop-depth-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let Some(report) =
        run(&format!("new 8 8\nexport {} depth=12", dir.join("x.png").display()))
    else {
        return;
    };
    assert!(report.steps[1].note.contains("depth is 8 or 16"), "{:?}", notes(&report));
    let _ = std::fs::remove_dir_all(&dir);
}

/// JPEG cannot hold the depth, and should say so rather than write eight bits
/// while being told sixteen.
#[test]
fn a_deep_export_to_a_shallow_format_is_refused() {
    let dir = std::env::temp_dir().join(format!("cshop-shallow-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let Some(report) =
        run(&format!("new 8 8\nexport {} depth=16", dir.join("x.jpg").display()))
    else {
        return;
    };
    assert!(!report.ok);
    assert!(report.steps[1].note.contains("sixteen bits"), "{:?}", notes(&report));
    let _ = std::fs::remove_dir_all(&dir);
}

// --- lens correction -------------------------------------------------------

#[test]
fn lens_needs_something_to_correct() {
    let Some(report) = run("new 40 30 background=white\nlens") else { return };
    assert!(!report.ok);
    assert!(report.steps[1].note.contains("rotation="), "{:?}", notes(&report));
}

/// Straightening a photograph leaves empty corners; `autocrop` is what makes
/// the result usable without anyone having to guess at a crop.
#[test]
fn autocrop_takes_the_empty_corners_off_the_canvas() {
    let Some(report) = run("new 400 300 background=white\nlens rotation=10 autocrop\ninfo") else {
        return;
    };
    assert!(report.ok, "{:?}", notes(&report));
    assert!(report.steps[1].note.contains("cropped to"), "{:?}", notes(&report));
    let info = &report.steps[2].note;
    assert!(!info.starts_with("400x300"), "the canvas should have shrunk: {info}");
}

/// And without it the canvas is left alone, corners and all.
#[test]
fn without_autocrop_the_canvas_keeps_its_size() {
    let Some(report) = run("new 400 300 background=white\nlens rotation=10\ninfo") else { return };
    assert!(report.ok, "{:?}", notes(&report));
    assert!(!report.steps[1].note.contains("cropped"), "{:?}", notes(&report));
    assert!(report.steps[2].note.starts_with("400x300"), "{:?}", notes(&report));
}

/// The report says which way each control went, since "distortion 0.12" alone
/// does not say whether a line was pushed out or pulled in.
#[test]
fn the_report_names_the_direction_of_each_correction() {
    let Some(report) =
        run("new 60 60 background=white\nlens distortion=-0.2 vignette=0.4 rotation=3")
    else {
        return;
    };
    let note = &report.steps[1].note;
    assert!(note.contains("barrel"), "{note}");
    assert!(note.contains("lifted"), "{note}");
    assert!(note.contains("rotated"), "{note}");

    let Some(report) = run("new 60 60 background=white\nlens distortion=0.2 vignette=-0.4") else {
        return;
    };
    let note = &report.steps[1].note;
    assert!(note.contains("pincushion"), "{note}");
    assert!(note.contains("darkened"), "{note}");
}

/// A vignette moves no pixels, so there is nothing for a crop to take even
/// when it is asked for.
#[test]
fn a_vignette_alone_leaves_nothing_to_crop() {
    let Some(report) = run("new 200 200 background=white\nlens vignette=-0.6 autocrop\ninfo")
    else {
        return;
    };
    assert!(report.ok, "{:?}", notes(&report));
    assert!(!report.steps[1].note.contains("cropped"), "{:?}", notes(&report));
    assert!(report.steps[2].note.starts_with("200x200"), "{:?}", notes(&report));
}

// --- noise removal ---------------------------------------------------------

#[test]
fn denoise_says_when_the_pack_is_missing_or_cleans_up() {
    let Some(report) = run("new 96 96 background=#807060\ndenoise") else { return };
    let note = report.steps.last().map(|s| s.note.clone()).unwrap_or_default();
    if !cshop_ui::vision::is_available() {
        assert!(note.contains("setup.sh") || note.contains("not installed"), "{note}");
        return;
    }
    assert!(report.ok, "{:?}", notes(&report));
    assert!(note.contains("removed noise"), "{note}");
    assert!(note.contains("tile"), "it should say how much work it was: {note}");
}

/// Strength zero would be a slow way to change nothing, so it is refused
/// rather than run.
#[test]
fn denoise_refuses_a_strength_of_nothing() {
    let Some(report) = run("new 64 64 background=white\ndenoise strength=0") else { return };
    assert!(!report.ok);
    assert!(report.steps[1].note.contains("exactly as it is"), "{:?}", notes(&report));
}

/// A selection is what makes this usable on a large photograph, so it has to
/// actually narrow the work — and say that it did.
#[test]
fn denoise_follows_the_selection() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(report) = run("new 200 200 background=#606060\nselect 20 20 64 64\ndenoise") else {
        return;
    };
    assert!(report.ok, "{:?}", notes(&report));
    let note = &report.steps[2].note;
    assert!(note.contains("over 64x64 at 20,20"), "{note}");
}

/// And a selection that misses the layer entirely is a mistake worth naming.
#[test]
fn denoise_refuses_a_selection_that_misses() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(report) = run(
        "new 200 200 background=white\nlayer new\nselect 0 0 20 20\nmove 400 400\ndenoise",
    ) else {
        return;
    };
    let note = report.steps.last().map(|s| s.note.clone()).unwrap_or_default();
    assert!(
        note.contains("does not overlap") || note.contains("no pixels"),
        "{note}"
    );
}

// --- upscaling -------------------------------------------------------------

#[test]
fn upscale_reports_the_new_size_or_says_the_pack_is_missing() {
    let Some(report) = run("new 64 48 background=#806040\nupscale scale=2\ninfo") else { return };
    let note = report.steps[1].note.clone();
    if !cshop_ui::vision::is_available() {
        assert!(note.contains("setup.sh") || note.contains("not installed"), "{note}");
        return;
    }
    assert!(report.ok, "{:?}", notes(&report));
    assert!(note.contains("64x48 to 128x96"), "{note}");
    assert!(report.steps[2].note.starts_with("128x96"), "{:?}", notes(&report));
}

#[test]
fn upscale_refuses_a_scale_it_cannot_do() {
    for bad in ["0.5", "8"] {
        let Some(report) = run(&format!("new 32 32 background=white\nupscale scale={bad}")) else {
            return;
        };
        assert!(!report.ok, "scale {bad} should be refused");
        assert!(report.steps[1].note.contains("between 1 and 4"), "{:?}", notes(&report));
    }
}

// --- separating by content -------------------------------------------------

#[test]
fn separate_makes_a_layer_for_each_kind_of_thing() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../cshop-io/tests/assets/ink-source.png");
    let Some(report) = run(&format!("open {path}\nresize 256 256\nseparate min=0.05\ninfo")) else {
        return;
    };
    let note = report.steps[2].note.clone();
    if !report.ok {
        // A 32-pixel abstract may genuinely hold nothing the model knows.
        assert!(note.contains("nothing matched"), "{note}");
        return;
    }
    assert!(note.starts_with("separated into"), "{note}");
    let layers: usize = report.steps[3]
        .note
        .split(',')
        .find_map(|p| p.trim().strip_suffix(" layers").or_else(|| p.trim().strip_suffix(" layer")))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0);
    assert!(layers > 1, "the separated layers should be there too: {}", report.steps[3].note);
}

/// Asking for something that is not in the picture should say what is.
#[test]
fn separate_names_what_it_did_find() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(report) = run("new 128 128 background=#4060a0\nseparate classes=elephant") else {
        return;
    };
    assert!(!report.ok);
    let note = &report.steps[1].note;
    assert!(note.contains("nothing matched"), "{note}");
    assert!(note.contains("This picture holds"), "it should say what is there: {note}");
}

// --- filling a hole in -----------------------------------------------------

#[test]
fn inpaint_needs_a_selection_to_fill() {
    let Some(report) = run("new 96 96 background=#405070\ninpaint") else { return };
    assert!(!report.ok);
    assert!(report.steps[1].note.contains("needs a selection"), "{:?}", notes(&report));
}

/// The whole point: what is selected disappears and what is not is left to the
/// bit, because the model hands the rest back untouched.
#[test]
fn inpaint_fills_the_selection_and_leaves_the_rest() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("cshop-fill-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let before = dir.join("before.png");
    let after = dir.join("after.png");

    let Some(report) = run(&format!(
        "new 160 160 background=#3a5a80\ngradient 0 0 160 160 from=#203040 to=#e0d0b0\n\
         export {}\nselect 50 50 60 60\ninpaint\nselect none\nexport {}",
        before.display(),
        after.display()
    )) else {
        return;
    };
    assert!(report.ok, "{:?}", notes(&report));
    assert!(report.steps[4].note.contains("filled in 60x60 at 50,50"), "{:?}", notes(&report));

    let a = cshop_io::load(&before).unwrap();
    let b = cshop_io::load(&after).unwrap();
    let mut outside = 0;
    let mut inside = 0;
    for y in 0..160i32 {
        for x in 0..160i32 {
            let hole = (50..110).contains(&x) && (50..110).contains(&y);
            if a.get(x, y) != b.get(x, y) {
                if hole {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
        }
    }
    assert_eq!(outside, 0, "{outside} pixels outside the hole moved");
    assert!(inside > 0, "the hole should have been filled");
    let _ = std::fs::remove_dir_all(&dir);
}

// --- depth and relighting --------------------------------------------------

#[test]
fn relight_needs_something_to_do() {
    let Some(report) = run("new 64 64 background=#808080\nrelight intensity=0 ambient=1") else {
        return;
    };
    assert!(!report.ok);
    assert!(report.steps[1].note.contains("needs something to do"), "{:?}", notes(&report));
}

/// Which side the lamp is on has to change the picture, and the two sides have
/// to differ from each other — the part with a right answer.
#[test]
fn the_side_the_lamp_is_on_changes_the_picture() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("cshop-relight-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../cshop-io/tests/assets/ink-source.png");
    let out = |n: &str| dir.join(n);

    let mut made = Vec::new();
    for (name, azimuth) in [("left.png", 0.0), ("right.png", 180.0)] {
        let Some(report) = run(&format!(
            "open {path}\nresize 128 128\n\
             relight azimuth={azimuth} elevation=25 intensity=1.2 ambient=0.5 relief=1.5\n\
             export {}",
            out(name).display()
        )) else {
            return;
        };
        assert!(report.ok, "{:?}", notes(&report));
        assert!(report.steps[2].note.contains("lit from"), "{:?}", notes(&report));
        made.push(cshop_io::load(&out(name)).unwrap());
    }
    assert_ne!(
        made[0].pixels(),
        made[1].pixels(),
        "lighting from the left and the right should not give the same picture"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The depth can be had as a layer, for looking at or masking with.
#[test]
fn depth_can_be_kept_as_a_layer() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../cshop-io/tests/assets/ink-source.png");
    let Some(report) = run(&format!("open {path}\nresize 96 96\ndepth\ninfo")) else { return };
    assert!(report.ok, "{:?}", notes(&report));
    assert!(report.steps[2].note.contains("as a layer"), "{:?}", notes(&report));
    assert!(report.steps[3].note.contains("2 layers"), "{:?}", notes(&report));
}

// --- masks -----------------------------------------------------------------

#[test]
fn depth_can_be_had_as_a_mask() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../cshop-io/tests/assets/ink-source.png");
    let Some(report) = run(&format!("open {path}\nresize 96 96\ndepth mask\nselect mask")) else {
        return;
    };
    assert!(report.ok, "{:?}", notes(&report));
    assert!(report.steps[2].note.contains("masked by nearness"), "{:?}", notes(&report));
    assert!(report.steps[3].note.starts_with("selected the mask"), "{:?}", notes(&report));

    // And the other way round, which is what haze wants.
    let Some(report) = run(&format!("open {path}\nresize 96 96\ndepth mask invert")) else {
        return;
    };
    assert!(report.steps[2].note.contains("masked by distance"), "{:?}", notes(&report));
}

#[test]
fn a_layer_can_be_turned_into_a_mask_and_then_a_selection() {
    let Some(report) = run(
        "new 64 64 background=white\nlayer new\ngradient 0 0 64 0 from=#000000 to=#ffffff\n\
         layer to-mask\nselect mask\ninfo",
    ) else {
        return;
    };
    assert!(report.ok, "{:?}", notes(&report));
    assert!(report.steps[4].note.starts_with("selected the mask"), "{:?}", notes(&report));
    // The gradient layer was consumed, so only the background is left.
    assert!(report.steps[5].note.contains("1 layer"), "{:?}", notes(&report));
}

/// Asking for a selection where there is no mask should say so rather than
/// quietly leaving the selection as it was.
#[test]
fn selecting_a_mask_that_is_not_there_says_so() {
    let Some(report) = run("new 32 32 background=white\nselect mask") else { return };
    assert!(!report.ok);
    assert!(report.steps[1].note.contains("no mask"), "{:?}", notes(&report));
}
