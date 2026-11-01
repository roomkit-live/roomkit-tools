//! Refuse native work in a child forked after the bridge started.
//!
//! `fork()` copies the bridge's state but not the Tokio runtime's threads: a
//! call in such a child would never complete, and its exit hook would wait for
//! calls that only the parent runs. A child forked before the first native
//! call builds its own runtime on first use, like any other process.

use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::prelude::*;
use pyo3::types::PyDict;

use super::ForkedProcessError;

/// Set before the first native call can start the runtime's threads.
static STARTED: AtomicBool = AtomicBool::new(false);
/// Set in a child forked after `STARTED`; that child runs no native work.
static FORKED: AtomicBool = AtomicBool::new(false);

/// Record that this process may now own runtime threads.
pub(super) fn mark_started() {
    STARTED.store(true, Ordering::Relaxed);
}

/// Whether this process was forked after its parent started native work.
pub(super) fn is_forked() -> bool {
    FORKED.load(Ordering::Relaxed)
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

/// Private `os.register_at_fork` hook; Python runs it in the child only.
#[pyfunction]
fn _after_fork_in_child() {
    if STARTED.load(Ordering::Relaxed) {
        FORKED.store(true, Ordering::Relaxed);
    }
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(_after_fork_in_child, module)?)?;
    let py = module.py();
    let os = py.import("os")?;
    // Platforms without fork() have no hook to register.
    if !os.hasattr("register_at_fork")? {
        return Ok(());
    }
    let hooks = PyDict::new(py);
    hooks.set_item("after_in_child", module.getattr("_after_fork_in_child")?)?;
    os.call_method("register_at_fork", (), Some(&hooks))?;
    Ok(())
}
