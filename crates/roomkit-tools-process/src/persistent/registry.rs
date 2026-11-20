//! Retained job records and the registry that owns them.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use roomkit_tools_core::ToolError;
use tokio::process::ChildStdin;
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

use crate::journal::Journal;
use crate::{ProcessLimits, ProcessState, ProcessStatus};

#[derive(Debug)]
pub(crate) struct Record {
    pub(crate) status: ProcessStatus,
    pub(crate) stdout: Journal,
    pub(crate) stderr: Journal,
    pub(crate) stdout_archive: Option<crate::output::View>,
    pub(crate) stderr_archive: Option<crate::output::View>,
    /// Signalled after each captured write and at each stream's end.
    pub(crate) output: watch::Sender<()>,
}

impl Record {
    /// Whether a read at `input`'s offsets has something to show at once:
    /// bytes past an offset, or an offset past its stream's end, which the
    /// read refuses rather than waits on.
    pub(crate) fn ready_for(&self, input: &crate::ProcessReadInput) -> bool {
        self.stdout.total != input.stdout_offset || self.stderr.total != input.stderr_offset
    }

    pub(crate) fn status(&self) -> ProcessStatus {
        let mut status = self.status.clone();
        status.stdout_bytes = self.stdout.total;
        status.stderr_bytes = self.stderr.total;
        status.stdout_retained_from = self.stdout.first();
        status.stderr_retained_from = self.stderr.first();
        status.stdout_archive = self
            .stdout_archive
            .as_ref()
            .map(crate::output::View::snapshot);
        status.stderr_archive = self
            .stderr_archive
            .as_ref()
            .map(crate::output::View::snapshot);
        status
    }
}

#[derive(Debug)]
pub(crate) struct Entry {
    /// The trusted owner, `None` for the standalone one.
    pub(crate) owner: Option<String>,
    pub(crate) command: String,
    pub(crate) started: Instant,
    pub(crate) finished: OnceLock<Duration>,
    pub(crate) record: Arc<Mutex<Record>>,
    pub(crate) stdin: Mutex<Option<ChildStdin>>,
    pub(crate) stop: CancellationToken,
    /// Cancelled when the session is detached: the job is let go, still running.
    pub(crate) detach: CancellationToken,
    pub(crate) done: watch::Receiver<bool>,
}

impl Entry {
    /// When the job finished, as its status reads: what `forget` checks.
    fn finished_at(&self) -> Option<Instant> {
        self.started.checked_add(*self.finished.get()?)
    }
}

#[derive(Debug, Default)]
pub(super) struct Registry {
    pub(super) next_id: u64,
    pub(super) entries: BTreeMap<String, Arc<Entry>>,
}

impl Registry {
    fn held(&self, owner: Option<&str>) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.owner.as_deref() == owner)
            .count()
    }

    /// The records of `owner` to release for one more job to fit the quotas,
    /// as `forget` would, their archives kept: those that finished first.
    /// The latest end is the one a model goes back to, told of it. A job
    /// still running is never released, nor another owner's.
    fn room_for(&self, owner: Option<&str>, limits: ProcessLimits) -> Vec<String> {
        let mut finished: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.owner.as_deref() == owner)
            .filter_map(|(id, entry)| Some((entry.finished_at()?, id)))
            .collect();
        finished.sort_unstable();
        let (mut held, mut total) = (self.held(owner), self.entries.len());
        let mut releasing = Vec::new();
        for (_, id) in finished {
            if held < limits.max_processes_per_owner && total < limits.max_processes {
                break;
            }
            releasing.push(id.clone());
            held -= 1;
            total -= 1;
        }
        releasing
    }

    /// The next identifier number for `owner`, within the host quotas, and
    /// the records to release for it once it is committed: a start that
    /// fails before then releases nothing.
    pub(super) fn admit(
        &self,
        owner: Option<&str>,
        limits: ProcessLimits,
    ) -> Result<(u64, Vec<String>), ToolError> {
        let releasing = self.room_for(owner, limits);
        if self.held(owner) - releasing.len() >= limits.max_processes_per_owner {
            return Err(ToolError::LimitExceeded(format!(
                "process quota of {} reached: every process of this conversation still runs; \
                 stop one with process_stop",
                limits.max_processes_per_owner
            )));
        }
        if self.entries.len() - releasing.len() >= limits.max_processes {
            return Err(ToolError::LimitExceeded(format!(
                "session process quota of {} reached: every process still runs or is another \
                 conversation's",
                limits.max_processes
            )));
        }
        let next = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| ToolError::LimitExceeded("process identifiers exhausted".into()))?;
        Ok((next, releasing))
    }

    /// Retained entries that `keep` selects, in creation order.
    pub(super) fn ordered(&self, keep: impl Fn(&Entry) -> bool) -> Vec<Arc<Entry>> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| keep(entry))
            .collect();
        entries.sort_by_key(|&(id, _)| roomkit_tools_core::identifiers::creation_order(id));
        entries
            .into_iter()
            .map(|(_, entry)| Arc::clone(entry))
            .collect()
    }
}

/// The status of a process that has just started.
pub(crate) fn initial_status(process_id: String, cwd: String, timeout_ms: u64) -> ProcessStatus {
    ProcessStatus {
        process_id,
        cwd,
        state: ProcessState::Running,
        end: None,
        exit_code: None,
        signal: None,
        timeout_ms,
        output_complete: false,
        stdin_closed: false,
        stdout_bytes: 0,
        stderr_bytes: 0,
        stdout_retained_from: 0,
        stderr_retained_from: 0,
        error: None,
        stdout_archive: None,
        stderr_archive: None,
    }
}
