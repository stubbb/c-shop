//! What a long operation says while it runs, and how it is told to stop.
//!
//! Everything here is a shared counter behind an `Arc`, because the two ends
//! are on different threads: the operation writes, the window reads once a
//! frame. Nothing blocks and nothing waits — a reader that catches a torn
//! moment sees a percentage that is out of date by microseconds, which is not
//! a kind of wrongness a progress bar can express anyway.
//!
//! Cancelling does not unwind. The operation checks [`Progress::cancelled`]
//! where it is cheap to do so, stops filling in the rest, and returns whatever
//! it has; the caller sees the flag and throws the result away. That keeps
//! every signature the shape it already was — no `Result` threaded through
//! twenty filters to describe a condition only the caller acts on.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A handle on one running operation. Cloning shares the same counter.
#[derive(Clone)]
pub struct Progress {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    done: AtomicU64,
    total: AtomicU64,
    /// What is happening, for the label beside the bar.
    what: Mutex<String>,
    cancelled: AtomicBool,
}

impl Default for Progress {
    fn default() -> Self {
        Progress::new()
    }
}

impl Progress {
    pub fn new() -> Progress {
        Progress { inner: Arc::new(Inner::default()) }
    }

    /// A handle for a caller that is not watching.
    ///
    /// Named rather than `new` at the call sites that use it, so that reading
    /// `filter.apply(px, ctx)` makes it obvious the progress goes nowhere.
    pub fn ignored() -> Progress {
        Progress::new()
    }

    /// Declare a phase: what it is and how many units it will take.
    ///
    /// Resets the count, so an operation made of several phases reports each
    /// in turn rather than one bar that stalls and then leaps.
    pub fn begin(&self, what: impl Into<String>, total: u64) {
        *self.inner.what.lock().unwrap_or_else(|e| e.into_inner()) = what.into();
        self.inner.total.store(total, Ordering::Relaxed);
        self.inner.done.store(0, Ordering::Relaxed);
    }

    /// Change the description without disturbing the count.
    pub fn say(&self, what: impl Into<String>) {
        *self.inner.what.lock().unwrap_or_else(|e| e.into_inner()) = what.into();
    }

    /// Another `n` units are done.
    pub fn advance(&self, n: u64) {
        self.inner.done.fetch_add(n, Ordering::Relaxed);
    }

    /// `n` units are done in total.
    ///
    /// For work that reports where it has got to rather than what it has just
    /// finished — a subprocess saying which tile it is on, say.
    pub fn set(&self, n: u64) {
        self.inner.done.store(n, Ordering::Relaxed);
    }

    /// How far along, or `None` when nobody said how far there is to go.
    ///
    /// Clamped: an estimate that undercounts would otherwise report more than
    /// a whole, and a bar past its own end is worse than one that sits full
    /// for a moment.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.inner.total.load(Ordering::Relaxed);
        if total == 0 {
            return None;
        }
        let done = self.inner.done.load(Ordering::Relaxed);
        Some((done as f32 / total as f32).clamp(0.0, 1.0))
    }

    pub fn what(&self) -> String {
        self.inner.what.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Units counted so far, which is how a test checks an estimate.
    pub fn done(&self) -> u64 {
        self.inner.done.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> u64 {
        self.inner.total.load(Ordering::Relaxed)
    }

    /// Ask the operation to stop. It will finish the unit it is on.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Relaxed);
    }

    /// Checked inside the loops. Relaxed, because a row's delay in noticing
    /// costs a row.
    pub fn cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Relaxed)
    }
}

impl std::fmt::Debug for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Progress")
            .field("what", &self.what())
            .field("done", &self.done())
            .field("total", &self.total())
            .field("cancelled", &self.cancelled())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_known_until_a_phase_says_so() {
        let p = Progress::new();
        assert_eq!(p.fraction(), None, "a bar with no total has nothing to draw");
        p.begin("Blurring", 10);
        assert_eq!(p.fraction(), Some(0.0));
        p.advance(5);
        assert_eq!(p.fraction(), Some(0.5));
    }

    /// An estimate that is short must not push the bar past its own end.
    #[test]
    fn overrunning_an_estimate_stops_at_full() {
        let p = Progress::new();
        p.begin("Blurring", 10);
        p.advance(25);
        assert_eq!(p.fraction(), Some(1.0));
    }

    #[test]
    fn a_second_phase_starts_again_from_nothing() {
        let p = Progress::new();
        p.begin("Reading", 4);
        p.advance(4);
        p.begin("Writing", 4);
        assert_eq!(p.fraction(), Some(0.0));
        assert_eq!(p.what(), "Writing");
    }

    #[test]
    fn cancelling_is_seen_by_every_holder() {
        let p = Progress::new();
        let copy = p.clone();
        assert!(!copy.cancelled());
        p.cancel();
        assert!(copy.cancelled(), "the flag is shared, not copied");
    }
}
