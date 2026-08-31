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

/// Make a scratch directory that only its owner can look in.
///
/// The picture being worked on is written here on its way to the models, and
/// the temporary directory is shared with every other account on the machine.
/// Left at the default the directory is world-readable and its name is a
/// process id, so anyone with a login could read what someone else was
/// editing — and, by making the directory first, choose where it went.
///
/// On anything without Unix permissions this is an ordinary `create_dir_all`,
/// which is the best that can be said for it.
#[cfg(unix)]
pub fn make_scratch(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

#[cfg(not(unix))]
pub fn make_scratch(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
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

// --- denoising -------------------------------------------------------------

/// What a denoising run came back with.
#[derive(Debug, Clone)]
pub struct Denoised {
    pub path: PathBuf,
    /// How many tiles the picture was taken in.
    pub tiles: u32,
    /// Mean absolute change per channel, in 8-bit levels. Near zero means the
    /// picture had nothing the model recognised as noise — which is a
    /// different answer from "it failed", and worth being able to say.
    pub moved: f32,
}

/// How far a denoising run has got, shared with whoever is drawing the bar.
#[derive(Debug, Default)]
pub struct DenoiseProgress {
    pub done: std::sync::atomic::AtomicU32,
    pub total: std::sync::atomic::AtomicU32,
}

impl DenoiseProgress {
    /// Zero until the sidecar has said how many tiles there are, and then the
    /// fraction of them finished.
    pub fn fraction(&self) -> f32 {
        use std::sync::atomic::Ordering::Relaxed;
        let total = self.total.load(Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.done.load(Relaxed) as f32 / total as f32).min(1.0)
    }
}

/// Run a subcommand that takes a while, reading its progress as it arrives.
///
/// Unlike [`run`], this reads the child's stderr line by line rather than
/// waiting for it to finish. The slow models — denoising, enlarging — are slow
/// enough that a caller with no idea how far they have got cannot tell them
/// apart from ones that have hung.
fn run_streaming(args: &[&str], progress: &DenoiseProgress) -> Result<Json, String> {
    use std::io::BufRead;
    use std::sync::atomic::Ordering::Relaxed;

    let (Some(python), Some(script)) = (python(), script()) else {
        return Err(NOT_INSTALLED.to_string());
    };
    let mut child = Command::new(&python)
        .arg(&script)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run the vision pack: {e}"))?;

    // Anything that is not a progress line is kept, in case the run fails and
    // those lines turn out to be the explanation.
    let mut tail: Vec<String> = Vec::new();
    if let Some(err) = child.stderr.take() {
        for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
            if let Ok(note) = json::parse(line.trim()) {
                if let Some(total) = note.get("tiles").and_then(Json::as_f64) {
                    progress.total.store(total as u32, Relaxed);
                }
                if let Some(done) = note.get("tile").and_then(Json::as_f64) {
                    progress.done.store(done as u32, Relaxed);
                }
                continue;
            }
            tail.push(line);
            if tail.len() > 4 {
                tail.remove(0);
            }
        }
    }

    let finished = child
        .wait_with_output()
        .map_err(|e| format!("the vision pack could not be waited for: {e}"))?;
    let stdout = String::from_utf8_lossy(&finished.stdout);
    let parsed = json::parse(stdout.trim()).map_err(|_| {
        if tail.is_empty() {
            format!("the vision pack said nothing (exit {:?})", finished.status.code())
        } else {
            format!("the vision pack failed: {}", tail.join(" / "))
        }
    })?;
    if parsed.get("ok").and_then(Json::as_bool) == Some(false) {
        let why = parsed.str_field("error").unwrap_or("it did not say why");
        let hint = parsed.str_field("hint").map(|h| format!(" ({h})")).unwrap_or_default();
        return Err(format!("{why}{hint}"));
    }
    Ok(parsed)
}

/// Remove noise, reporting progress as the sidecar works through the tiles.
pub fn denoise(
    image: &Path,
    out: &Path,
    strength: f32,
    progress: &DenoiseProgress,
) -> Result<Denoised, String> {
    let image = image_arg(image)?;
    let out_s = out.to_str().ok_or("that output path is not text")?;
    let strength = strength.clamp(0.0, 1.0).to_string();
    let parsed = run_streaming(
        &["denoise", image, "--out", out_s, "--strength", &strength],
        progress,
    )?;
    Ok(Denoised {
        path: PathBuf::from(parsed.str_field("path").unwrap_or(out_s)),
        tiles: parsed.get("tiles").and_then(Json::as_f64).unwrap_or(0.0) as u32,
        moved: parsed.get("moved").and_then(Json::as_f64).unwrap_or(0.0) as f32,
    })
}

/// What an enlargement came back with.
#[derive(Debug, Clone)]
pub struct Upscaled {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub tiles: u32,
}

/// Enlarge, reporting progress as it goes.
///
/// The model only knows four times; anything else is reached by reducing its
/// answer afterwards, which the sidecar does. See its `upscale` for why that
/// is better than it sounds.
pub fn upscale(
    image: &Path,
    out: &Path,
    scale: f32,
    progress: &DenoiseProgress,
) -> Result<Upscaled, String> {
    let image = image_arg(image)?;
    let out_s = out.to_str().ok_or("that output path is not text")?;
    let scale = scale.clamp(0.1, 8.0).to_string();
    let parsed =
        run_streaming(&["upscale", image, "--out", out_s, "--scale", &scale], progress)?;
    Ok(Upscaled {
        path: PathBuf::from(parsed.str_field("path").unwrap_or(out_s)),
        width: parsed.get("width").and_then(Json::as_f64).unwrap_or(0.0) as u32,
        height: parsed.get("height").and_then(Json::as_f64).unwrap_or(0.0) as u32,
        tiles: parsed.get("tiles").and_then(Json::as_f64).unwrap_or(0.0) as u32,
    })
}

/// One kind of thing the labeller found, and how much of the picture it is.
#[derive(Debug, Clone)]
pub struct Region {
    pub class: String,
    /// The class number, which is also the value it has in the map.
    pub id: u8,
    pub coverage: f32,
}

/// A map of what every pixel is, and a list of what is in it.
#[derive(Debug, Clone)]
pub struct Classified {
    /// A greyscale image whose pixel values are class numbers.
    pub map: PathBuf,
    pub regions: Vec<Region>,
}

/// Label every pixel with what it is.
///
/// One pass at a fixed size, so it costs the same half-second whatever the
/// picture is — and its boundaries are approximate for the same reason. It
/// answers "what is here and roughly where", which is a different question
/// from the one [`segment`] answers.
pub fn classify(image: &Path, out: &Path) -> Result<Classified, String> {
    let image = image_arg(image)?;
    let out_s = out.to_str().ok_or("that output path is not text")?;
    let answer = run(&["classify", image, "--out", out_s])?;
    let regions = answer
        .get("regions")
        .and_then(Json::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|r| {
            Some(Region {
                class: r.str_field("class")?.to_string(),
                id: r.get("id")?.as_f64()? as u8,
                coverage: r.get("coverage")?.as_f64()? as f32,
            })
        })
        .collect();
    Ok(Classified { map: PathBuf::from(answer.str_field("map").unwrap_or(out_s)), regions })
}

/// Fill a hole in with what was probably behind it.
///
/// `mask` is white where the picture should be invented and black where it
/// should be left. Only the masked pixels come back changed — the model
/// returns the rest bit for bit — so there is no seam to hide.
pub fn inpaint(image: &Path, mask: &Path, out: &Path) -> Result<PathBuf, String> {
    let image = image_arg(image)?;
    let mask_s = mask.to_str().ok_or("that mask path is not text")?;
    let out_s = out.to_str().ok_or("that output path is not text")?;
    let answer = run(&["inpaint", image, "--mask", mask_s, "--out", out_s])?;
    Ok(PathBuf::from(answer.str_field("path").unwrap_or(out_s)))
}

/// Guess how far away everything in a picture is.
///
/// The answer is written as a sixteen-bit greyscale image, near-white, and
/// read back at that depth: lighting reads the *gradient* of it, and at eight
/// bits a gentle slope arrives as a staircase and lights like one.
pub fn depth(image: &Path, out: &Path) -> Result<PathBuf, String> {
    let image = image_arg(image)?;
    let out_s = out.to_str().ok_or("that output path is not text")?;
    let answer = run(&["depth", image, "--out", out_s])?;
    Ok(PathBuf::from(answer.str_field("map").unwrap_or(out_s)))
}

/// Read a depth image back into the form the lighting wants.
pub fn depth_map(path: &Path) -> Result<cshop_core::relight::DepthMap, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("could not read the depth map: {e}"))?;
    let (deep, _) = cshop_io::decode_deep(&bytes, Some(path), &cshop_core::profile::Profile::srgb())
        .map_err(|e| format!("could not read the depth map: {e}"))?;
    let data = deep.pixels().iter().map(|p| p.r as f32 / 65535.0).collect();
    cshop_core::relight::DepthMap::from_values(deep.width(), deep.height(), data)
        .ok_or_else(|| "the depth map came back the wrong size".to_string())
}
