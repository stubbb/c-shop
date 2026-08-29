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
