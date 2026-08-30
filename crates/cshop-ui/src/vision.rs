//! Talking to the vision pack.
//!
//! The models run in a Python sidecar rather than in this process. That is not
//! squeamishness about the language: a neural network runtime is tens of
//! megabytes of platform-specific binary that changes every few months, and
//! the editor is one static binary with almost no dependencies that builds
//! offline. Bolting the first onto the second would cost the second its whole
//! character, and for a feature most sessions never touch.
//!
//! So the boundary is a process and a line of JSON. Not installing the pack
//! costs nothing and breaks nothing; `detect` and `segment` say what is
//! missing and how to get it, and everything else carries on as before.

use std::path::{Path, PathBuf};
use std::process::Command;

use cshop_core::json::{self, Json};

/// Where the pack installs itself, matching `vision/setup.sh`.
fn home() -> PathBuf {
    if let Some(set) = std::env::var_os("CSHOP_VISION_HOME") {
        return PathBuf::from(set);
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    cache.join("cshop").join("vision")
}

/// The interpreter the pack installed.
fn python() -> Option<PathBuf> {
    let p = home().join("venv/bin/python");
    p.is_file().then_some(p)
}

/// The sidecar script, wherever this copy of the editor keeps it.
///
/// Searched rather than compiled in, because the binary is often run out of a
/// build tree during development and out of a prefix when installed.
fn script() -> Option<PathBuf> {
    let mut roots = vec![PathBuf::from("vision")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.join("vision"));
            for up in 1..=3 {
                if let Some(root) = dir.ancestors().nth(up) {
                    roots.push(root.join("vision"));
                }
            }
        }
    }
    roots.push(home().join("vision"));
    roots.into_iter().map(|r| r.join("cshop-vision.py")).find(|p| p.is_file())
}

/// A directory of this call's own, for the image and the mask.
///
/// Unique per call rather than per process: two segmentations running at once
/// — two tests, or a window and a script — would otherwise write `source.png`
/// over each other and read back whichever landed last.
pub fn scratch() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("cshop-vision-{}-{n}", std::process::id()))
}

/// What to tell someone who has not installed it.
pub const NOT_INSTALLED: &str = "the vision pack is not installed — run vision/setup.sh";

pub fn is_available() -> bool {
    python().is_some() && script().is_some()
}

/// Run a subcommand and read its answer.
///
/// Every failure — a missing pack, a crash, unreadable output — comes back as
/// a sentence rather than as an exit status, because the caller is usually a
/// script that will print it and stop.
pub fn run(args: &[&str]) -> Result<Json, String> {
    let (Some(python), Some(script)) = (python(), script()) else {
        return Err(NOT_INSTALLED.to_string());
    };
    let out = Command::new(&python)
        .arg(&script)
        .args(args)
        .output()
        .map_err(|e| format!("could not run the vision pack: {e}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed = json::parse(stdout.trim()).map_err(|_| {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().join(" / ");
        if tail.is_empty() {
            format!("the vision pack said nothing (exit {:?})", out.status.code())
        } else {
            format!("the vision pack failed: {tail}")
        }
    })?;

    if parsed.get("ok").and_then(Json::as_bool) == Some(false) {
        let why = parsed.str_field("error").unwrap_or("it did not say why");
        let hint = parsed.str_field("hint").map(|h| format!(" ({h})")).unwrap_or_default();
        return Err(format!("{why}{hint}"));
    }
    Ok(parsed)
}

/// One thing the detector found.
#[derive(Debug, Clone)]
pub struct Found {
    pub class: String,
    pub score: f32,
    /// x0, y0, x1, y1 in image pixels.
    pub box_: [f32; 4],
}

impl Found {
    pub fn width(&self) -> f32 {
        self.box_[2] - self.box_[0]
    }

    pub fn height(&self) -> f32 {
        self.box_[3] - self.box_[1]
    }
}

pub fn detect(image: &Path, conf: f32, classes: &str) -> Result<Vec<Found>, String> {
    let conf = conf.to_string();
    let mut args = vec!["detect", "--image", image_arg(image)?, "--conf", &conf];
    if !classes.is_empty() {
        args.push("--classes");
        args.push(classes);
    }
    let answer = run(&args)?;
    let objects = answer.get("objects").and_then(Json::as_array).unwrap_or(&[]);
    Ok(objects
        .iter()
        .filter_map(|o| {
            let b = o.get("box")?.as_array()?;
            if b.len() != 4 {
                return None;
            }
            let v = |i: usize| b[i].as_f64().unwrap_or(0.0) as f32;
            Some(Found {
                class: o.str_field("class")?.to_string(),
                score: o.get("score")?.as_f64()? as f32,
                box_: [v(0), v(1), v(2), v(3)],
            })
        })
        .collect())
}

/// What a prompt for the segmenter looks like.
pub enum Prompt {
    /// Everything the detector calls this, best match first.
    Class(String),
    Box([f32; 4]),
    /// Points to include, and points to leave out.
    Points(Vec<(f32, f32)>, Vec<(f32, f32)>),
}

pub struct Segmented {
    pub mask: PathBuf,
    pub confidence: f32,
    /// Share of the image the mask covers, `0..=1`.
    pub coverage: f32,
    /// What the detector found, when the prompt was a class.
    pub detected: Option<Found>,
}

pub fn segment(image: &Path, prompt: &Prompt, out: &Path, conf: f32) -> Result<Segmented, String> {
    let image = image_arg(image)?;
    let out_s = out.to_str().ok_or("that mask path is not text")?;
    let conf = conf.to_string();
    let mut owned: Vec<String> = Vec::new();
    let mut args: Vec<&str> = vec!["segment", "--image", image, "--out", out_s, "--conf", &conf];

    match prompt {
        Prompt::Class(name) => {
            args.push("--class");
            args.push(name);
        }
        Prompt::Box(b) => {
            owned.push(format!("{},{},{},{}", b[0], b[1], b[2], b[3]));
        }
        Prompt::Points(yes, no) => {
            for (x, y) in yes {
                owned.push(format!("{x},{y}"));
            }
            for (x, y) in no {
                owned.push(format!("{x},{y}"));
            }
        }
    }
    // Built after the strings exist, so the borrows outlive the call.
    match prompt {
        Prompt::Box(_) => {
            args.push("--box");
            args.push(&owned[0]);
        }
        Prompt::Points(yes, _) => {
            for (i, s) in owned.iter().enumerate() {
                args.push(if i < yes.len() { "--point" } else { "--not-point" });
                args.push(s);
            }
        }
        Prompt::Class(_) => {}
    }

    let answer = run(&args)?;
    let detected = answer.get("detected").filter(|d| !matches!(d, Json::Null)).and_then(|d| {
        let b = d.get("box")?.as_array()?;
        let v = |i: usize| b.get(i).and_then(Json::as_f64).unwrap_or(0.0) as f32;
        Some(Found {
            class: d.str_field("class")?.to_string(),
            score: d.get("score")?.as_f64()? as f32,
            box_: [v(0), v(1), v(2), v(3)],
        })
    });
    Ok(Segmented {
        mask: PathBuf::from(answer.str_field("mask").unwrap_or(out_s)),
        confidence: answer.get("confidence").and_then(Json::as_f64).unwrap_or(0.0) as f32,
        coverage: answer.get("coverage").and_then(Json::as_f64).unwrap_or(0.0) as f32,
        detected,
    })
}

fn image_arg(path: &Path) -> Result<&str, String> {
    path.to_str().ok_or_else(|| "that image path is not text".to_string())
}
