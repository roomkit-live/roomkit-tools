//! The ends of background jobs, read by Python's `async for`; the
//! subscription itself remains Rust's `JobEnds`.

use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};

use futures_core::Stream;

use pyo3::prelude::*;
use roomkit_tools_process::{JobEnds, ProcessSession};
use tokio::sync::{Mutex as TaskMutex, oneshot};

use super::ToolError;

/// Where a subscription reads from: Rust's ends, or a session's that does
/// not exist yet and will hand them over once built.
#[derive(Debug)]
enum Source {
    Subscribed(JobEnds),
    Waiting(oneshot::Receiver<JobEnds>),
    Ended,
}

#[pyclass(name = "_JobEnds", frozen, module = "roomkit_tools._native")]
#[derive(Debug)]
pub(super) struct PythonJobEnds {
    source: Arc<TaskMutex<Source>>,
}

impl PythonJobEnds {
    pub(super) fn subscribed(ends: JobEnds) -> Self {
        Self::of(Source::Subscribed(ends))
    }

    pub(super) fn ended() -> Self {
        Self::of(Source::Ended)
    }

    fn of(source: Source) -> Self {
        Self {
            source: Arc::new(TaskMutex::new(source)),
        }
    }
}

#[pymethods]
impl PythonJobEnds {
    /// The next end as JSON, or None once the ends stopped.
    fn next<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let source = Arc::clone(&self.source);
        crate::runtime::future_into_py(py, async move {
            let mut source = source.lock().await;
            if let Source::Waiting(waiting) = &mut *source {
                *source = waiting.await.map_or(Source::Ended, Source::Subscribed);
            }
            let Source::Subscribed(ends) = &mut *source else {
                return Ok(None);
            };
            let next = std::future::poll_fn(|context| Pin::new(&mut *ends).poll_next(context));
            let Some(end) = next.await else {
                *source = Source::Ended;
                return Ok(None);
            };
            serde_json::to_string(&end)
                .map(Some)
                .map_err(|_| ToolError::new_err("Unable to serialize the job end"))
        })
    }
}

/// The JSON schemas of the contract types no tool contract holds, by name,
/// for the API snapshot: a job end, the dict a session's ends give.
#[pyfunction]
pub(super) fn contract_types_json() -> PyResult<String> {
    let types =
        serde_json::json!({ "JobEnd": schemars::schema_for!(roomkit_tools_process::JobEnd) });
    serde_json::to_string(&types)
        .map_err(|_| ToolError::new_err("Unable to serialize the contract types"))
}

/// The subscribers of a process session, which is built lazily, at its
/// first use: each is handed the session's ends once it exists. No job runs
/// before.
#[derive(Debug, Default)]
pub(super) struct Pending {
    state: Mutex<(Option<ProcessSession>, Vec<oneshot::Sender<JobEnds>>)>,
}

impl Pending {
    /// Subscribe now if the session exists, else once it is built.
    pub(super) fn subscribe(&self) -> PythonJobEnds {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(session) = &state.0 {
            return PythonJobEnds::subscribed(session.ends());
        }
        let (sender, receiver) = oneshot::channel();
        state.1.push(sender);
        PythonJobEnds::of(Source::Waiting(receiver))
    }

    /// The session is built: hand its ends to every waiting subscriber,
    /// before it runs any job.
    pub(super) fn built(&self, session: &ProcessSession) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.0 = Some(session.clone());
        for waiting in state.1.drain(..) {
            let _ = waiting.send(session.ends());
        }
    }

    /// The session closed: every subscriber still waiting ends.
    pub(super) fn close(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .1
            .clear();
    }
}
