//! What a caller can ask the editor to do.
//!
//! Six tools, and the shape of them is deliberate. `run_script` is the whole
//! editor, because the script language already *is* the interface an agent
//! wants and wrapping each command as its own tool would only make a worse
//! copy of it. The other five exist because a caller arriving cold cannot use
//! `run_script` well: it needs to know what styles exist, what commands there
//! are, what files it may open, and — most of all — it needs to be able to
//! *look* at what it drew.
//!
//! That last one is the point of serving this at all. A tool result may carry
//! an image, so the loop the script harness was built for closes over the
//! network: describe, draw, look, correct.

use super::editor::{Editor, Outcome, Work};
use super::json::Json;
use crate::script::{style_dirs, Sandbox};

/// The largest picture we will send back, whatever was asked for.
///
/// Images travel as base64 inside a JSON string, which costs a third again on
/// top of the PNG. A caller wanting the full-size render should export it to
/// the workspace and fetch it as a file rather than through here.
const MAX_IMAGE_FIT: u32 = 2048;
const DEFAULT_IMAGE_FIT: u32 = 768;

pub struct Tool {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Json,
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "run_script",
        title: "Run a C-Shop script",
        description: "\
Run a C-Shop script and report what happened, optionally with a picture of the \
result. This is the editor: `new`, `open`, `place`, `text`, `shape`, `fill`, \
`gradient`, `select`, `filter`, `adjust`, `effect`, `style`, `layer`, `set`, \
`resize`, `export`. One command per line.

The document stays open between calls in the same session, so work can be \
built up over several calls and looked at in between. Paths are relative to \
the workspace — call `workspace` to see what is in it. Call `describe` for the \
command reference and `list_styles` for the style library.",
        schema: || {
            Json::object(vec![
                ("type", Json::from("object")),
                (
                    "properties",
                    Json::object(vec![
                        (
                            "script",
                            field("string", "The script to run, one command per line."),
                        ),
                        (
                            "session",
                            field(
                                "string",
                                "Which document to work on. Calls sharing a session share \
                                 the document; omit it to use the connection's own.",
                            ),
                        ),
                        (
                            "return_image",
                            field(
                                "boolean",
                                "Send a picture of the result back. Default true — seeing \
                                 the result is usually the point.",
                            ),
                        ),
                        (
                            "image_fit",
                            field(
                                "integer",
                                "Longest side of that picture, in pixels. Default 768, \
                                 capped at 2048; the full-size render is what `export` is for.",
                            ),
                        ),
                    ]),
                ),
                ("required", Json::Array(vec![Json::from("script")])),
            ])
        },
    },
    Tool {
        name: "render",
        title: "Look at the document",
        description: "\
Send back a picture of the session's document as it stands, without changing \
it. Use this to check work before continuing, or after a `run_script` call \
that asked for no image.",
        schema: || {
            Json::object(vec![
                ("type", Json::from("object")),
                (
                    "properties",
                    Json::object(vec![
                        ("session", field("string", "Which document to look at.")),
                        (
                            "fit",
                            field(
                                "integer",
                                "Longest side in pixels. Default 768, capped at 2048.",
                            ),
                        ),
                    ]),
                ),
            ])
        },
    },
    Tool {
        name: "list_styles",
        title: "List the style library",
        description: "\
The styles available to `style NAME`, with the parameters each takes and what \
it is for. A style is a named script fragment that scales itself to whatever \
size of image it is given.",
        schema: || {
            Json::object(vec![
                ("type", Json::from("object")),
                (
                    "properties",
                    Json::object(vec![(
                        "name",
                        field("string", "Show one style in full, including its body."),
                    )]),
                ),
            ])
        },
    },
    Tool {
        name: "describe",
        title: "The command reference",
        description: "\
What the script language can do. Ask for a topic: `commands`, `filters`, \
`adjustments`, `effects`, `blends`, `fonts`, or `syntax`. With no topic it \
gives the summary.",
        schema: || {
            Json::object(vec![
                ("type", Json::from("object")),
                (
                    "properties",
                    Json::object(vec![(
                        "topic",
                        field(
                            "string",
                            "commands | filters | adjustments | effects | blends | fonts | syntax",
                        ),
                    )]),
                ),
            ])
        },
    },
    Tool {
        name: "workspace",
        title: "List the workspace",
        description: "\
The files a script may open and where its output goes. Every path in a script \
is relative to this directory and cannot leave it.",
        schema: || {
            Json::object(vec![
                ("type", Json::from("object")),
                (
                    "properties",
                    Json::object(vec![(
                        "path",
                        field("string", "A subdirectory to list. Defaults to the root."),
                    )]),
                ),
            ])
        },
    },
    Tool {
        name: "reset",
        title: "Close the document",
        description: "Throw away a session's document and start again with nothing open.",
        schema: || {
            Json::object(vec![
                ("type", Json::from("object")),
                (
                    "properties",
                    Json::object(vec![("session", field("string", "Which session to clear."))]),
                ),
            ])
        },
    },
];

fn field(kind: &str, description: &str) -> Json {
    Json::object(vec![
        ("type", Json::from(kind)),
        ("description", Json::from(description)),
    ])
}

/// The `tools/list` payload.
pub fn list() -> Json {
    Json::object(vec![(
        "tools",
        Json::Array(
            TOOLS
                .iter()
                .map(|t| {
                    Json::object(vec![
                        ("name", Json::from(t.name)),
                        ("title", Json::from(t.title)),
                        ("description", Json::from(t.description)),
                        ("inputSchema", (t.schema)()),
                    ])
                })
                .collect(),
        ),
    )])
}

/// What a tool gives back: some text, and sometimes a picture.
pub struct ToolResult {
    pub text: String,
    pub image_png: Option<Vec<u8>>,
    pub is_error: bool,
}

impl ToolResult {
    fn text(body: impl Into<String>) -> ToolResult {
        ToolResult { text: body.into(), image_png: None, is_error: false }
    }

    fn error(body: impl Into<String>) -> ToolResult {
        ToolResult { text: body.into(), image_png: None, is_error: true }
    }

    /// The MCP content blocks for this result.
    pub fn to_content(&self) -> Json {
        let mut blocks = vec![Json::object(vec![
            ("type", Json::from("text")),
            ("text", Json::from(self.text.clone())),
        ])];
        if let Some(png) = &self.image_png {
            blocks.push(Json::object(vec![
                ("type", Json::from("image")),
                ("data", Json::from(super::base64::encode(png))),
                ("mimeType", Json::from("image/png")),
            ]));
        }
        Json::object(vec![
            ("content", Json::Array(blocks)),
            ("isError", Json::from(self.is_error)),
        ])
    }
}

pub fn call(
    editor: &Editor,
    workspace: &Sandbox,
    name: &str,
    arguments: &Json,
    default_session: &str,
) -> ToolResult {
    let session = arguments.str_field("session").unwrap_or(default_session).to_string();
    match name {
        "run_script" => run_script(editor, arguments, &session),
        "render" => render(editor, arguments, &session),
        "list_styles" => list_styles(workspace, arguments),
        "describe" => ToolResult::text(super::reference::describe(
            arguments.str_field("topic").unwrap_or(""),
        )),
        "workspace" => list_workspace(workspace, arguments),
        "reset" => match editor.submit(&session, Work::Reset, None) {
            Outcome::Done(note) => ToolResult::text(note),
            other => ToolResult::error(unexpected(other)),
        },
        other => ToolResult::error(format!(
            "no tool called {other:?}. There are: {}",
            TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
        )),
    }
}

fn wanted_fit(arguments: &Json, key: &str) -> u32 {
    arguments
        .get(key)
        .and_then(Json::as_f64)
        .map(|v| (v.max(1.0) as u32).min(MAX_IMAGE_FIT))
        .unwrap_or(DEFAULT_IMAGE_FIT)
}

fn run_script(editor: &Editor, arguments: &Json, session: &str) -> ToolResult {
    let Some(source) = arguments.str_field("script") else {
        return ToolResult::error("run_script needs a `script`");
    };
    let want_image = arguments
        .get("return_image")
        .and_then(Json::as_bool)
        .unwrap_or(true)
        .then(|| wanted_fit(arguments, "image_fit"));

    match editor.submit(session, Work::Script { source: source.to_string() }, want_image) {
        Outcome::Ran { report, image, size } => {
            let mut text = report.summary();
            if let Some((w, h)) = size {
                text.push_str(&format!("\ndocument is {w}x{h}"));
            }
            // The report already says which steps failed and why; the flag
            // just tells the caller whether to read it as a correction.
            ToolResult { text, image_png: image, is_error: !report.ok }
        }
        other => ToolResult::error(unexpected(other)),
    }
}

fn render(editor: &Editor, arguments: &Json, session: &str) -> ToolResult {
    let fit = wanted_fit(arguments, "fit");
    match editor.submit(session, Work::Render { fit: Some(fit) }, None) {
        Outcome::Image { png, size } => ToolResult {
            text: format!("{}x{}, shown at no more than {fit} across", size.0, size.1),
            image_png: Some(png),
            is_error: false,
        },
        other => ToolResult::error(unexpected(other)),
    }
}

fn unexpected(outcome: Outcome) -> String {
    match outcome {
        Outcome::Failed(why) => why,
        Outcome::Done(note) => note,
        _ => "the editor answered with something else entirely".to_string(),
    }
}

fn list_styles(workspace: &Sandbox, arguments: &Json) -> ToolResult {
    let mut found: Vec<(String, std::path::PathBuf)> = Vec::new();
    for dir in style_dirs(workspace.root()) {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "style") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    if !found.iter().any(|(seen, _)| seen == name) {
                        found.push((name.to_string(), path));
                    }
                }
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));

    if found.is_empty() {
        return ToolResult::text("no styles are installed");
    }

    // One style in full, when asked for by name.
    if let Some(want) = arguments.str_field("name") {
        let Some((name, path)) = found.iter().find(|(n, _)| n == want) else {
            return ToolResult::error(format!(
                "no style called {want:?}. There are: {}",
                found.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
            ));
        };
        return match std::fs::read_to_string(path) {
            Ok(body) => ToolResult::text(format!("# {name}\n\n{body}")),
            Err(e) => ToolResult::error(format!("could not read {name}: {e}")),
        };
    }

    let mut out = String::from("Styles, applied with `style NAME [param=value ...]`:\n\n");
    for (name, path) in &found {
        let Ok(body) = std::fs::read_to_string(path) else { continue };
        out.push_str(&format!("{name}\n"));
        if let Some(purpose) = first_sentence(&body) {
            out.push_str(&format!("  {purpose}\n"));
        }
        let params: Vec<String> = body
            .lines()
            .filter_map(|l| l.trim().strip_prefix("param "))
            .filter_map(|rest| rest.split_once('=').map(|(k, v)| format!("{}={}", k.trim(), v.trim())))
            .collect();
        if !params.is_empty() {
            out.push_str(&format!("  takes: {}\n", params.join("  ")));
        }
        out.push('\n');
    }
    out.push_str("Ask for one by name to see how it is built and why.");
    ToolResult::text(out)
}

/// A style file's opening line of comment, which is where each says what it is.
fn first_sentence(body: &str) -> Option<String> {
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with('#') && l.len() > 2)?
        .trim_start_matches('#')
        .trim();
    Some(line.to_string())
}

fn list_workspace(workspace: &Sandbox, arguments: &Json) -> ToolResult {
    let given = arguments.str_field("path").unwrap_or("");
    let dir = if given.is_empty() {
        workspace.root().to_path_buf()
    } else {
        match workspace.resolve(given) {
            Ok(path) => path,
            Err(why) => return ToolResult::error(why),
        }
    };

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return ToolResult::error(format!("{:?} is not a directory here", workspace.relative(&dir)));
    };
    let mut rows: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = workspace.relative(&path);
        // A listing that offers a path the sandbox will then refuse is worse
        // than one that leaves it out — a symlink out of the workspace is the
        // case that matters, and it looks like an ordinary file from here.
        if workspace.resolve(&name).is_err() {
            rows.push(format!("{name}  (leaves the workspace; cannot be opened)"));
            continue;
        }
        // Note that this metadata does not follow links, which is what we
        // want: the question is what the entry is, not what it points at.
        let row = match entry.metadata() {
            Ok(meta) if meta.is_dir() => format!("{name}/"),
            Ok(meta) => format!("{name}  ({})", human_bytes(meta.len())),
            Err(_) => name,
        };
        rows.push(row);
    }
    rows.sort();

    if rows.is_empty() {
        return ToolResult::text(
            "the workspace is empty; `new` a document, or put a picture in it to open",
        );
    }
    ToolResult::text(format!(
        "Paths are relative to the workspace and cannot leave it.\n\n{}",
        rows.join("\n")
    ))
}

fn human_bytes(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.0} kB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}
