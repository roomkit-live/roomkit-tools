//! Retained job records and the registry that owns them.

use std::collections::BTreeMap;
use std::path::Path;
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
}

impl Record {
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
    pub(crate) done: watch::Receiver<bool>,
}

#[derive(Debug, Default)]
pub(super) struct Registry {
    pub(super) next_id: u64,
    pub(super) entries: BTreeMap<String, Arc<Entry>>,
}

impl Registry {
    /// The next identifier number for `owner`, within the host quotas.
    pub(super) fn next_for(
        &self,
        owner: Option<&str>,
        limits: ProcessLimits,
    ) -> Result<u64, ToolError> {
        let retained = self
            .entries
            .values()
            .filter(|e| e.owner.as_deref() == owner)
            .count();
        if self.entries.len() >= limits.max_processes || retained >= limits.max_processes_per_owner
        {
            return Err(ToolError::LimitExceeded(
                "process quota reached; stop with forget=true to release completed records".into(),
            ));
        }
        self.next_id
            .checked_add(1)
            .ok_or_else(|| ToolError::LimitExceeded("process identifiers exhausted".into()))
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
pub(crate) fn initial_status(process_id: String, cwd: &Path, timeout_ms: u64) -> ProcessStatus {
    ProcessStatus {
        process_id,
        cwd: cwd.to_string_lossy().into_owned(),
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
