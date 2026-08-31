//! Work that would otherwise freeze the window.
//!
//! On a twelve-megapixel picture almost everything here is slow: a median is
//! four seconds, a surface blur two and a half, an alignment one and a half a
//! pair, a Lanczos enlargement two. Held on the drawing thread each of those
//! is a window that stops repainting, stops answering the mouse, and — on
//! every desktop that watches for it — gets offered to the user for killing.
//!
//! So the work goes to a thread and the window keeps drawing. What it draws
//! comes from [`cshop_core::progress::Progress`]: a shared counter the worker
//! writes and the status bar reads once a frame.
//!
//! # Why there is a way to turn it off
//!
//! A worker is for keeping a window responsive. Nothing without a window
//! needs one, and everything without a window is worse off for having one:
//! a script wants the next line to see the finished picture, and a test wants
//! to dispatch an action and then look at the result. Both set
//! [`Jobs::run_here`], and then a job runs where it was started and is
//! finished before `start` returns — the same code, the same order, no pump
//! and no sleeping.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use cshop_core::progress::Progress;

/// One running piece of work, as the window sees it.
struct Entry {
    name: String,
    progress: Progress,
    started: Instant,
}

/// A running job, copied out for drawing.
pub struct Running {
    pub name: String,
    pub progress: Progress,
    pub elapsed: Duration,
}

/// The list of what is running, and the choice of where to run it.
#[derive(Clone)]
pub struct Jobs {
    /// Held weakly: an entry lives exactly as long as the [`Job`] that owns
    /// it, so a job whose handle is dropped leaves the list by itself. There
    /// is no deregistering to forget.
    running: Arc<Mutex<Vec<Weak<Entry>>>>,
    here: bool,
}

impl Default for Jobs {
    /// Work runs where it is started. The window turns that off; see the
    /// module note for why that is the way round it is.
    fn default() -> Self {
        Jobs { running: Arc::new(Mutex::new(Vec::new())), here: true }
    }
}

impl Jobs {
    /// Run work on this thread instead of a worker's. See the module note.
    pub fn run_here(&mut self, yes: bool) {
        self.here = yes;
    }

    pub fn runs_here(&self) -> bool {
        self.here
    }

    /// Start a piece of work, and hand back the handle that owns it.
    ///
    /// Dropping the handle does not stop the work — a thread cannot be
    /// stopped from outside — but it does take the job off the list and
    /// discard the answer. To stop it, cancel it.
    pub fn start<T: Send + 'static>(
        &self,
        name: impl Into<String>,
        work: impl FnOnce(&Progress) -> T + Send + 'static,
    ) -> Job<T> {
        let name = name.into();
        let progress = Progress::new();
        progress.say(name.clone());
        let entry = Arc::new(Entry {
            name: name.clone(),
            progress: progress.clone(),
            started: Instant::now(),
        });

        let (tx, rx) = std::sync::mpsc::channel();
        if self.here {
            let _ = tx.send(work(&progress));
        } else {
            // A machine that cannot spare a thread drops the closure, and
            // with it the sending end — so the job reports itself lost at the
            // next poll and the caller says so, which is the same path a
            // panicking worker takes. Nothing extra to write.
            let _ = std::thread::Builder::new().name(name).spawn(move || {
                let _ = tx.send(work(&progress));
            });
        }

        self.running.lock().unwrap_or_else(|e| e.into_inner()).push(Arc::downgrade(&entry));
        Job { entry, result: rx }
    }

    /// Everything still running, and everything still holding a handle.
    ///
    /// Prunes as it goes, so the list does not grow across a session.
    pub fn running(&self) -> Vec<Running> {
        let mut list = self.running.lock().unwrap_or_else(|e| e.into_inner());
        list.retain(|weak| weak.strong_count() > 0);
        list.iter()
            .filter_map(|weak| weak.upgrade())
            .map(|entry| Running {
                name: entry.name.clone(),
                progress: entry.progress.clone(),
                elapsed: entry.started.elapsed(),
            })
            .collect()
    }

    /// Whether anything is outstanding, which is what asks for a repaint.
    pub fn any(&self) -> bool {
        let mut list = self.running.lock().unwrap_or_else(|e| e.into_inner());
        list.retain(|weak| weak.strong_count() > 0);
        !list.is_empty()
    }
}

/// What a job has to say when asked.
pub enum Poll<T> {
    /// Still going.
    Waiting,
    Done(T),
    /// Stopped on request. Whatever it produced is not worth having.
    Cancelled,
    /// The worker went away without answering — a panic, almost always.
    Lost,
}

/// A handle on one running job. Dropping it forgets the job.
pub struct Job<T> {
    entry: Arc<Entry>,
    result: Receiver<T>,
}

impl<T> Job<T> {
    pub fn name(&self) -> &str {
        &self.entry.name
    }

    pub fn progress(&self) -> &Progress {
        &self.entry.progress
    }

    /// Ask the work to stop. It finishes the row it is on and returns.
    pub fn cancel(&self) {
        self.entry.progress.cancel();
    }

    pub fn cancelled(&self) -> bool {
        self.entry.progress.cancelled()
    }

    pub fn elapsed(&self) -> Duration {
        self.entry.started.elapsed()
    }

    /// Collect the answer, if there is one yet.
    ///
    /// A cancelled job reports [`Poll::Cancelled`] as soon as it is asked,
    /// without waiting for the worker to notice: the answer is being thrown
    /// away either way, and the caller should not be made to hold a dialog
    /// open until a four-second filter reaches its next row.
    pub fn poll(&self) -> Poll<T> {
        if self.entry.progress.cancelled() {
            return Poll::Cancelled;
        }
        match self.result.try_recv() {
            Ok(value) => Poll::Done(value),
            Err(TryRecvError::Empty) => Poll::Waiting,
            Err(TryRecvError::Disconnected) => Poll::Lost,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_run_here_is_finished_before_start_returns() {
        let jobs = Jobs::default();
        let job = jobs.start("Counting", |p| {
            p.begin("Counting", 3);
            p.advance(3);
            41 + 1
        });
        match job.poll() {
            Poll::Done(v) => assert_eq!(v, 42),
            _ => panic!("running here should have finished it"),
        }
    }

    #[test]
    fn a_worker_is_seen_as_running_until_it_answers() {
        let mut jobs = Jobs::default();
        jobs.run_here(false);
        let (gate, wait) = std::sync::mpsc::channel::<()>();
        let job = jobs.start("Waiting", move |_| {
            let _ = wait.recv();
            7
        });
        assert_eq!(jobs.running().len(), 1, "a started job should be on the list");
        assert!(matches!(job.poll(), Poll::Waiting));
        let _ = gate.send(());
        let mut answer = None;
        for _ in 0..1000 {
            if let Poll::Done(v) = job.poll() {
                answer = Some(v);
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(answer, Some(7));
    }

    /// The list is held weakly, so nothing has to remember to clear it.
    #[test]
    fn dropping_a_handle_takes_the_job_off_the_list() {
        let jobs = Jobs::default();
        let job = jobs.start("Counting", |_| 1);
        assert_eq!(jobs.running().len(), 1);
        drop(job);
        assert!(jobs.running().is_empty(), "a forgotten job should not still be listed");
    }

    #[test]
    fn a_cancelled_job_says_so_without_waiting_for_the_worker() {
        let mut jobs = Jobs::default();
        jobs.run_here(false);
        let (gate, wait) = std::sync::mpsc::channel::<()>();
        let job = jobs.start("Waiting", move |_| {
            let _ = wait.recv();
            7
        });
        job.cancel();
        assert!(matches!(job.poll(), Poll::Cancelled));
        let _ = gate.send(());
    }
}
