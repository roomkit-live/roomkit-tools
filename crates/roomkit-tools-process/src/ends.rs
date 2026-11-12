//! The ends of background jobs, told to the host that subscribed to them.

use std::pin::Pin;
use std::sync::{Mutex, PoisonError};
use std::task::{Context, Poll};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::ProcessEnd;

/// The tool whose job ended.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobTool {
    /// A process started with `process_start`.
    Process,
    /// The background jobs a `shell` command left running when it returned.
    Shell,
}

/// A background job that ended: a process, or every job a shell command left
/// running. For the host, never a model result: `owner` is the host's key.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct JobEnd {
    /// The trusted owner the job was started under; `None` for the
    /// standalone owner.
    pub owner: Option<String>,
    /// The tool that started the job.
    pub tool: JobTool,
    /// The process's identifier; `None` for a shell command's jobs, which
    /// have none.
    pub process_id: Option<String>,
    /// The command as it was given.
    pub command: String,
    /// Initial working directory, relative to the workspace (`.` for its root).
    pub cwd: String,
    /// How it ended. A shell command's jobs end `exited`: they ended by
    /// themselves, the host never stops them while it follows them.
    pub end: ProcessEnd,
    /// Normal exit code; always `None` for a shell command's jobs, which are
    /// not the host's children.
    pub exit_code: Option<i32>,
    /// Terminating Unix signal; always `None` for a shell command's jobs.
    pub signal: Option<i32>,
    /// Milliseconds from the command's start to the job's end.
    pub elapsed_ms: u64,
}

/// The ends of a session's jobs, from the moment [`ends`](crate::ProcessSession::ends)
/// was called: a [`Stream`](futures_core::Stream), Python's `async for end in
/// session.ends()`, where each end is a dict. Each subscriber has its own
/// queue: no end is dropped while it is held. The stream ends once the
/// session closed and every end before was read.
#[derive(Debug)]
pub struct JobEnds {
    receiver: mpsc::UnboundedReceiver<JobEnd>,
}

impl futures_core::Stream for JobEnds {
    type Item = JobEnd;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<JobEnd>> {
        self.receiver.poll_recv(context)
    }
}

/// The subscribers of one session's ends.
#[derive(Debug, Default)]
pub(crate) struct Ends {
    subscribers: Mutex<Option<Vec<mpsc::UnboundedSender<JobEnd>>>>,
}

impl Ends {
    pub(crate) fn new() -> Self {
        Self {
            subscribers: Mutex::new(Some(Vec::new())),
        }
    }

    /// A new subscriber; one subscribing after [`Self::close`] reads nothing.
    pub(crate) fn subscribe(&self) -> JobEnds {
        let (sender, receiver) = mpsc::unbounded_channel();
        if let Some(subscribers) = self.lock().as_mut() {
            subscribers.push(sender);
        }
        JobEnds { receiver }
    }

    /// Tell every subscriber still listening.
    pub(crate) fn send(&self, end: &JobEnd) {
        if let Some(subscribers) = self.lock().as_mut() {
            subscribers.retain(|subscriber| subscriber.send(end.clone()).is_ok());
        }
    }

    /// End every subscription once its queued ends are read.
    pub(crate) fn close(&self) {
        self.lock().take();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Vec<mpsc::UnboundedSender<JobEnd>>>> {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Milliseconds in `elapsed`, saturated.
pub(crate) fn millis(elapsed: std::time::Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}
