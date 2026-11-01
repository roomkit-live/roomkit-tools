//! Refuse native work in a child forked after the bridge started.
//!
//! `fork()` copies the bridge's state but not the Tokio runtime's threads: a
//! call in such a child would never complete, and its exit hook would wait for
//! calls that only the parent runs. A child forked before the first native
//! call builds its own runtime on first use, like any other process.
//!
//! The bridge records the process that started it and compares that record
//! with the current process. Nothing runs inside `fork()`: an at-fork hook
//! would enter PyO3 in every child, and PyO3 takes a lock there that a parent
//! worker may hold at the moment of the fork, so the child would wait forever.

use std::sync::atomic::{AtomicU32, Ordering};

use pyo3::prelude::*;

use super::ForkedProcessError;

/// The process that started the runtime's threads; zero before the first call.
static STARTED_BY: AtomicU32 = AtomicU32::new(0);

/// Record that this process may now own runtime threads.
pub(super) fn mark_started() {
    if STARTED_BY.load(Ordering::Relaxed) == 0 {
        STARTED_BY.store(std::process::id(), Ordering::Relaxed);
    }
}

/// Whether this process was forked after its parent started native work.
pub(super) fn is_forked() -> bool {
    let started_by = STARTED_BY.load(Ordering::Relaxed);
    started_by != 0 && started_by != std::process::id()
}

/// Fail at once, without touching state that a parent thread may have held.
pub(super) fn ensure_unforked() -> PyResult<()> {
    if is_forked() {
        return Err(ForkedProcessError::new_err(
            "native tools cannot run in a process forked after its parent's first native \
             call; make native calls only after forking, or use the spawn or forkserver \
             start method",
        ));
    }
    Ok(())
}
