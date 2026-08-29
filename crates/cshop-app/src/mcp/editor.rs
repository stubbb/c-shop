//! The one thread that owns the editor.
//!
//! Connections arrive on many threads; there is one GPU and one document per
//! session, so the editing itself is funnelled onto a single thread and the
//! HTTP side talks to it over a channel. That is not a compromise made for
//! borrow-checking — serialising the work is what we want. Two requests
//! composing into the same document at once would race whatever they disagreed
//! about, and the failure would look like a rendering bug rather than what it
//! was.
//!
//! A session is one document being worked on over several calls. Holding the
//! runner between calls is most of the value of serving at all: the GPU
//! context costs far more than a typical script, and an agent that opens a
//! photograph, looks at it, and then adjusts it needs the photograph to still
//! be open on the second call.

use std::collections::HashMap;
use std::sync::mpsc::{sync_channel, Receiver, Sender, SyncSender};
use std::time::{Duration, Instant};

use crate::script::{Report, Runner, Sandbox};
use cshop_gpu::context::GpuContext;

/// How long a session may sit untouched before it is dropped.
///
/// Each one holds a document and its GPU textures, so an abandoned session is
/// a leak. Callers that mean to come back can simply act again within the
/// window, and one that does not is not owed its memory forever.
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_SESSIONS: usize = 32;

pub enum Work {
    /// Run a script in this session, keeping whatever it leaves behind.
    Script { source: String },
    /// The current composite, as a PNG, scaled so its longest side is `fit`.
    Render { fit: Option<u32> },
    /// Throw the session's document away.
    Reset,
    /// What sessions exist, and what each is holding.
    Sessions,
}

pub enum Outcome {
    Ran { report: Report, image: Option<Vec<u8>>, size: Option<(u32, u32)> },
    Image { png: Vec<u8>, size: (u32, u32) },
    Done(String),
    Sessions(Vec<SessionInfo>),
    Failed(String),
}

pub struct SessionInfo {
    pub id: String,
    pub document: Option<(String, u32, u32)>,
    pub layers: usize,
    pub idle_seconds: u64,
}

pub struct Job {
    pub session: String,
    pub work: Work,
    /// Whether a script should also hand back a picture of what it did.
    pub want_image: Option<u32>,
    pub reply: SyncSender<Outcome>,
}

/// A handle to the editor thread.
#[derive(Clone)]
pub struct Editor {
    jobs: Sender<Job>,
}

impl Editor {
    /// Start the thread. Fails only if there is no GPU to render on.
    pub fn start(workspace: Sandbox) -> Result<Editor, String> {
        // Built here rather than on the thread so that a machine with no GPU
        // is reported at startup, where someone is watching, instead of on
        // the first request.
        let gpu = GpuContext::headless().map_err(|e| format!("no GPU: {e}"))?;
        let (jobs, inbox) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("cshop-editor".into())
            .spawn(move || run_editor(gpu, workspace, inbox))
            .map_err(|e| format!("could not start the editor thread: {e}"))?;
        Ok(Editor { jobs })
    }

    /// Hand work to the editor and wait for it.
    ///
    /// A closed channel means the editor thread died, which is not something
    /// this process recovers from — but the connection that noticed is still
    /// owed an answer rather than a hang.
    pub fn submit(&self, session: &str, work: Work, want_image: Option<u32>) -> Outcome {
        let (reply, answer) = sync_channel(1);
        let job = Job { session: session.to_string(), work, want_image, reply };
        if self.jobs.send(job).is_err() {
            return Outcome::Failed("the editor thread is gone".into());
        }
        answer.recv().unwrap_or_else(|_| Outcome::Failed("the editor dropped the job".into()))
    }
}

struct Session {
    runner: Runner,
    touched: Instant,
}

fn run_editor(gpu: GpuContext, workspace: Sandbox, inbox: Receiver<Job>) {
    let mut sessions: HashMap<String, Session> = HashMap::new();

    for job in inbox {
        expire(&mut sessions);

        let outcome = match job.work {
            Work::Sessions => Outcome::Sessions(describe(&sessions)),
            Work::Reset => match sessions.remove(&job.session) {
                Some(_) => Outcome::Done(format!("session {:?} reset", job.session)),
                None => Outcome::Done(format!("session {:?} was already empty", job.session)),
            },
            Work::Script { source } => {
                match session_for(&mut sessions, &job.session, &gpu, &workspace) {
                    Err(why) => Outcome::Failed(why),
                    Ok(session) => {
                        let report = session.runner.run(&source);
                        let size = session.runner.size();
                        // A picture of the result, when one was asked for and
                        // there is a document to take one of.
                        let image = job
                            .want_image
                            .filter(|_| session.runner.has_document())
                            .and_then(|fit| render(&mut session.runner, Some(fit)).ok())
                            .map(|(png, _)| png);
                        Outcome::Ran { report, image, size }
                    }
                }
            }
            Work::Render { fit } => match sessions.get_mut(&job.session) {
                None => Outcome::Failed(format!(
                    "session {:?} has nothing open; run a script with `new` or `open` first",
                    job.session
                )),
                Some(session) => {
                    session.touched = Instant::now();
                    if !session.runner.has_document() {
                        Outcome::Failed("there is no document to render".into())
                    } else {
                        match render(&mut session.runner, fit) {
                            Ok((png, size)) => Outcome::Image { png, size },
                            Err(why) => Outcome::Failed(why),
                        }
                    }
                }
            },
        };

        // A caller that has hung up is not an error; it just means nobody is
        // waiting for this any more.
        let _ = job.reply.send(outcome);
    }
}

fn session_for<'a>(
    sessions: &'a mut HashMap<String, Session>,
    id: &str,
    gpu: &GpuContext,
    workspace: &Sandbox,
) -> Result<&'a mut Session, String> {
    if !sessions.contains_key(id) {
        if sessions.len() >= MAX_SESSIONS {
            // Drop the least recently used rather than refuse: a client that
            // has lost track of its session ids should not be able to wedge
            // the server by asking for one more.
            if let Some(stale) =
                sessions.iter().min_by_key(|(_, s)| s.touched).map(|(k, _)| k.clone())
            {
                log::info!("evicting session {stale:?} to make room for {id:?}");
                sessions.remove(&stale);
            }
        }
        sessions.insert(
            id.to_string(),
            Session {
                runner: Runner::new(
                    gpu.clone(),
                    workspace.root().to_path_buf(),
                    Some(workspace.clone()),
                ),
                touched: Instant::now(),
            },
        );
    }
    let session = sessions.get_mut(id).expect("just inserted");
    session.touched = Instant::now();
    Ok(session)
}

/// Render, scaled so the longest side is `fit`.
///
/// Scaling here rather than at the caller keeps the response small: a full
/// print-size composite is megabytes, and it has to travel as base64 inside a
/// JSON string, which costs a third again on top.
fn render(runner: &mut Runner, fit: Option<u32>) -> Result<(Vec<u8>, (u32, u32)), String> {
    let png = match fit {
        None => runner.composite_png()?,
        Some(fit) => runner.composite_png_fit(fit)?,
    };
    let size = runner.size().unwrap_or((0, 0));
    Ok((png, size))
}

fn expire(sessions: &mut HashMap<String, Session>) {
    let now = Instant::now();
    sessions.retain(|id, session| {
        let keep = now.duration_since(session.touched) < SESSION_IDLE_TIMEOUT;
        if !keep {
            log::info!("session {id:?} expired");
        }
        keep
    });
}

fn describe(sessions: &HashMap<String, Session>) -> Vec<SessionInfo> {
    let now = Instant::now();
    let mut out: Vec<SessionInfo> = sessions
        .iter()
        .map(|(id, session)| SessionInfo {
            id: id.clone(),
            document: session.runner.document_summary(),
            layers: session.runner.layer_count(),
            idle_seconds: now.duration_since(session.touched).as_secs(),
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}
