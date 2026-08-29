//! The scripted pathway: intake, draw, analyse, return.
//!
//! The parsing tests run anywhere; the ones that draw need a GPU and skip
//! themselves without one.

use std::path::Path;

/// The binary's own module, reached the way an integration test can.
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

/// The two styles that ship with the editor have to keep working, since the
/// documentation walks through them.
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
    let script = format!("open {}\nstyle coloured-pencil blur=3", src.display());
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
