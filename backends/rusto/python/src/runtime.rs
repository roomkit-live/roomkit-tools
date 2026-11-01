//! Drain Python-facing async work before interpreter finalization.
//!
//! Kept in sync with bindings/python/src/runtime.rs. Each extension requires
//! its own shutdown boundary; this cannot share Rust statics across dylibs.
//!
//! The upstream Tokio runtime is process-wide and intentionally stays alive.
//! Its Python completion callbacks must not outlive the interpreter. Tracking
//! the generic bridge's nested tasks (including blocking callbacks), rather
//! than only tool futures, gives a shutdown boundary for those callbacks.
//! Each call's tasks also carry its event loop, so a completion that arrives
//! after the host closed that loop is dropped instead of reported as an error.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use pyo3::intern;
use pyo3::prelude::*;
use pyo3_async_runtimes::TaskLocals;
use pyo3_async_runtimes::generic::{self, ContextExt, Runtime};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{ToolError, fork};

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
        fork::ensure_unforked()?;
        let mut state = activity();
        if state.closing {
            return Err(ToolError::new_err("Python tool bridge is shutting down"));
        }
        // Before the count or the runtime changes: a child forked after
        // either one copies this mark and refuses native work.
        fork::mark_started();
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

/// The asyncio loop that receives one bridge call's completion.
type CompletionLoop = Option<Arc<Py<PyAny>>>;

tokio::task_local! {
    static LOCALS: TaskLocals;
    static COMPLETION_LOOP: CompletionLoop;
}

thread_local! {
    // Set on the Python thread while the bridge spawns a call's first task.
    static SPAWNING_LOOP: RefCell<CompletionLoop> = const { RefCell::new(None) };
}

/// The loop of the call being spawned: inherited by its nested tasks.
fn completion_loop() -> CompletionLoop {
    COMPLETION_LOOP
        .try_with(Clone::clone)
        .unwrap_or_else(|_| SPAWNING_LOOP.with(|spawning| spawning.borrow().clone()))
}

/// Publish a call's loop to the tasks spawned while this guard lives.
struct Spawning {
    previous: CompletionLoop,
}

impl Spawning {
    fn enter(event_loop: Arc<Py<PyAny>>) -> Self {
        let previous = SPAWNING_LOOP.with(|spawning| spawning.replace(Some(event_loop)));
        Self { previous }
    }
}

impl Drop for Spawning {
    fn drop(&mut self) {
        let previous = self.previous.take();
        SPAWNING_LOOP.with(|spawning| spawning.replace(previous));
    }
}

/// Run an upstream completion callback unless its loop is already closed.
///
/// A host can abandon a call, close its loop and keep running; scheduling the
/// result there would raise and print "Event loop is closed". The check and
/// the callback share one GIL hold. A loop closed by another thread while the
/// callback runs Python code can still make upstream report that error.
fn complete(event_loop: CompletionLoop, callback: impl FnOnce()) {
    let Some(event_loop) = event_loop else {
        callback();
        return;
    };
    Python::attach(move |py| {
        let closed = event_loop
            .bind(py)
            .call_method0(intern!(py, "is_closed"))
            .and_then(|closed| closed.is_truthy())
            .unwrap_or(false);
        if closed {
            drop(callback);
        } else {
            callback();
        }
    });
}

impl Runtime for BridgeRuntime {
    type JoinError = tokio::task::JoinError;
    type JoinHandle = JoinHandle<()>;

    fn spawn<F>(future: F) -> Self::JoinHandle
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let future = COMPLETION_LOOP.scope(completion_loop(), future);
        pyo3_async_runtimes::tokio::get_runtime().spawn(Tracked {
            future: Box::pin(future),
            _ticket: Ticket::nested(),
        })
    }

    fn spawn_blocking<F>(callback: F) -> Self::JoinHandle
    where
        F: FnOnce() + Send + 'static,
    {
        let event_loop = completion_loop();
        let ticket = Ticket::nested();
        pyo3_async_runtimes::tokio::get_runtime().spawn_blocking(move || {
            // Once exit begins, there is no awaiting Python loop to notify.
            // Already-running callbacks may still finish: shutdown releases
            // the GIL and waits for their tickets before returning to Python.
            if activity().closing {
                drop(callback);
            } else {
                complete(event_loop, callback);
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
    let locals = generic::get_current_locals::<BridgeRuntime>(py)?;
    let _spawning = Spawning::enter(Arc::new(locals.event_loop(py).unbind()));
    generic::future_into_py_with_locals::<BridgeRuntime, _, _>(py, locals, future)
}

/// Private diagnostic: bridge tasks and completion callbacks still running.
///
/// A forked child runs none: the count it copied belongs to its parent.
#[pyfunction]
fn _activity() -> usize {
    if fork::is_forked() {
        return 0;
    }
    activity().count
}

/// Private atexit hook. Returns the initial activity count for diagnostics.
///
/// A forked child returns at once: no thread there would ever finish the
/// parent's calls, and its copied locks may be held by threads it lacks.
#[pyfunction]
fn _shutdown(py: Python<'_>) -> usize {
    if fork::is_forked() {
        return 0;
    }
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
    module.add_function(wrap_pyfunction!(_activity, module)?)?;
    module.add_function(wrap_pyfunction!(_shutdown, module)?)?;
    module
        .py()
        .import("atexit")?
        .call_method1("register", (module.getattr("_shutdown")?,))?;
    Ok(())
}
