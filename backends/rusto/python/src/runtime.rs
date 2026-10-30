//! Drain Python-facing async work before interpreter finalization.
//!
//! Kept in sync with bindings/python/src/runtime.rs. Each extension requires
//! its own shutdown boundary; this cannot share Rust statics across dylibs.
//!
//! The upstream Tokio runtime is process-wide and intentionally stays alive.
//! Its Python completion callbacks must not outlive the interpreter. Tracking
//! the generic bridge's nested tasks (including blocking callbacks), rather
//! than only tool futures, gives a shutdown boundary for those callbacks.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use pyo3::prelude::*;
use pyo3_async_runtimes::TaskLocals;
use pyo3_async_runtimes::generic::{self, ContextExt, Runtime};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::ToolError;

#[derive(Debug, Default)]
struct Activity {
    closing: bool,
    count: usize,
}

#[derive(Debug, Default)]
struct Bridge {
    activity: Mutex<Activity>,
    idle: Condvar,
    cancellation: CancellationToken,
}

static BRIDGE: LazyLock<Bridge> = LazyLock::new(Bridge::default);

fn activity() -> MutexGuard<'static, Activity> {
    BRIDGE
        .activity
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug)]
struct Ticket;

impl Ticket {
    fn nested() -> Self {
        activity().count += 1;
        Self
    }

    fn enter() -> PyResult<Self> {
        let mut state = activity();
        if state.closing {
            return Err(ToolError::new_err("Python tool bridge is shutting down"));
        }
        state.count += 1;
        Ok(Self)
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut state = activity();
        state.count -= 1;
        if state.count == 0 {
            BRIDGE.idle.notify_all();
        }
    }
}

// Fields drop in declaration order: release the entire future (including its
// Python references) before reporting that its activity has finished.
#[derive(Debug)]
struct Tracked<F> {
    future: Pin<Box<F>>,
    _ticket: Ticket,
}

impl<F: Future> Future for Tracked<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().future.as_mut().poll(cx)
    }
}

struct BridgeRuntime;

tokio::task_local! {
    static LOCALS: TaskLocals;
}

impl Runtime for BridgeRuntime {
    type JoinError = tokio::task::JoinError;
    type JoinHandle = JoinHandle<()>;

    fn spawn<F>(future: F) -> Self::JoinHandle
    where
        F: Future<Output = ()> + Send + 'static,
    {
        pyo3_async_runtimes::tokio::get_runtime().spawn(Tracked {
            future: Box::pin(future),
            _ticket: Ticket::nested(),
        })
    }

    fn spawn_blocking<F>(callback: F) -> Self::JoinHandle
    where
        F: FnOnce() + Send + 'static,
    {
        let ticket = Ticket::nested();
        pyo3_async_runtimes::tokio::get_runtime().spawn_blocking(move || {
            // Once exit begins, there is no awaiting Python loop to notify.
            // Already-running callbacks may still finish: shutdown releases
            // the GIL and waits for their tickets before returning to Python.
            if activity().closing {
                drop(callback);
            } else {
                callback();
            }
            drop(ticket);
        })
    }
}

impl ContextExt for BridgeRuntime {
    fn scope<F, R>(locals: TaskLocals, future: F) -> Pin<Box<dyn Future<Output = R> + Send>>
    where
        F: Future<Output = R> + Send + 'static,
    {
        Box::pin(LOCALS.scope(locals, future))
    }

    fn get_task_locals() -> Option<TaskLocals> {
        LOCALS.try_with(Clone::clone).ok()
    }
}

/// Cancellation remains per call; interpreter shutdown also cancels its parent.
pub(super) fn cancellation_token() -> CancellationToken {
    BRIDGE.cancellation.child_token()
}

/// Keep all conversions on the tracked bridge, preserving upstream semantics.
pub(super) fn future_into_py<'py, F, T>(py: Python<'py>, future: F) -> PyResult<Bound<'py, PyAny>>
where
    F: Future<Output = PyResult<T>> + Send + 'static,
    T: for<'a> IntoPyObject<'a> + Send + 'static,
{
    let _entry = Ticket::enter()?;
    generic::future_into_py::<BridgeRuntime, _, _>(py, future)
}

/// Private atexit hook. Returns the initial activity count for diagnostics.
#[pyfunction]
fn _shutdown(py: Python<'_>) -> usize {
    let count = {
        let mut state = activity();
        state.closing = true;
        state.count
    };
    BRIDGE.cancellation.cancel();
    py.detach(|| {
        let mut state = activity();
        while state.count != 0 {
            state = BRIDGE
                .idle
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    });
    count
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(_shutdown, module)?)?;
    module
        .py()
        .import("atexit")?
        .call_method1("register", (module.getattr("_shutdown")?,))?;
    Ok(())
}
