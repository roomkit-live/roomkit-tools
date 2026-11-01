use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use roomkit_tools_core::{Definition, Error, Output};
use tokio::io::AsyncWriteExt;
use tokio::process::ChildStdin;
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

use crate::journal::Journal;
use crate::{
    ProcessLimits, ProcessReadInput, ProcessReadResult, ProcessStartInput, ProcessState,
    ProcessStatus, ProcessStatusInput, ProcessStopInput, ProcessStopResult, ProcessWriteInput,
    ProcessWriteResult,
};

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
    pub(crate) owner: String,
    pub(crate) command: String,
    pub(crate) started: Instant,
    pub(crate) finished: OnceLock<Duration>,
    pub(crate) record: Arc<Mutex<Record>>,
    pub(crate) stdin: Mutex<Option<ChildStdin>>,
    pub(crate) stop: CancellationToken,
    pub(crate) done: watch::Receiver<bool>,
}

#[derive(Debug, Default)]
struct Registry {
    next_id: u64,
    entries: BTreeMap<String, Arc<Entry>>,
}

#[derive(Debug)]
struct Inner {
    root: PathBuf,
    limits: ProcessLimits,
    registry: Mutex<Registry>,
    closed: CancellationToken,
    outputs: Option<crate::OutputStore>,
    changes: watch::Sender<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.closed.cancel();
    }
}

/// Host-only observation of a retained process. Never expose this unfiltered
/// across authorization boundaries; owner keys are supplied by the host.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ObservedProcess {
    /// Trusted owner key, not a model argument or OS PID.
    pub owner: String,
    /// Exact command supplied when starting the process.
    pub command: String,
    /// Elapsed runtime, frozen once cleanup finishes.
    pub elapsed: Duration,
    /// Authoritative current lifecycle and capture metadata.
    pub status: ProcessStatus,
}

/// Host-owned process collection with explicit owner boundaries, independent of RoomKit.
///
/// Hosts supply a trusted owner key on every operation, never from model arguments.
/// Clones share ownership; dropping the last clone requests all process cleanup.
/// Keep the Tokio runtime alive and prefer awaiting [`Self::close`]. This is not
/// a sandbox: commands inherit host permissions and environment. Each command
/// leads a new Unix session without a controlling terminal.
#[derive(Clone, Debug)]
pub struct ProcessManager {
    inner: Arc<Inner>,
}

impl ProcessManager {
    /// Resolve an existing root for initial working directories.
    ///
    /// # Errors
    /// Inaccessible or non-directory roots are rejected.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, Error> {
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|source| Error::Io {
                operation: "resolve process root",
                source,
            })?;
        if !tokio::fs::metadata(&root)
            .await
            .map_err(|source| Error::Io {
                operation: "inspect process root",
                source,
            })?
            .is_dir()
        {
            return Err(Error::InvalidInput(
                "process root must be a directory".into(),
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                limits: ProcessLimits::default(),
                registry: Mutex::new(Registry::default()),
                closed: CancellationToken::new(),
                outputs: None,
                changes: watch::channel(()).0,
            }),
        })
    }

    /// Subscribe to coalesced lifecycle changes (start, finish, forget, close).
    /// Subscribe before taking an observation to avoid a missed wakeup. Output
    /// bytes do not trigger notifications; page output only when needed.
    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.inner.changes.subscribe()
    }

    /// Inspect all retained jobs for the owning host, including completed jobs.
    /// This privileged API is deliberately separate from owner-scoped model tools.
    /// The caller must filter results before presenting them to another principal.
    pub async fn observe(&self) -> Vec<ObservedProcess> {
        let entries: Vec<_> = self
            .inner
            .registry
            .lock()
            .await
            .entries
            .values()
            .cloned()
            .collect();
        let mut processes = Vec::with_capacity(entries.len());
        for entry in entries {
            let record = entry.record.lock().await;
            processes.push(ObservedProcess {
                owner: entry.owner.clone(),
                command: entry.command.clone(),
                elapsed: entry
                    .finished
                    .get()
                    .copied()
                    .unwrap_or_else(|| entry.started.elapsed()),
                status: record.status(),
            });
        }
        processes
    }

    /// Attach optional output storage before cloning/starting. Closing this manager
    /// stops capture but keeps disk copies until the store is explicitly closed.
    ///
    /// # Errors
    /// Refuses shared or previously used managers.
    pub fn with_output_store(mut self, outputs: crate::OutputStore) -> Result<Self, Error> {
        let inner = Arc::get_mut(&mut self.inner)
            .ok_or_else(|| Error::Refused("configure output storage before cloning".into()))?;
        if inner.registry.get_mut().next_id != 0 {
            return Err(Error::Refused(
                "configure output storage before starting".into(),
            ));
        }
        inner.outputs = Some(outputs);
        Ok(self)
    }

    /// Set positive host bounds before cloning or starting processes.
    ///
    /// # Errors
    /// Rejects shared/used managers, inconsistent bounds and timers above 24 hours.
    pub fn with_limits(mut self, limits: ProcessLimits) -> Result<Self, Error> {
        if limits.max_processes == 0
            || limits.max_processes_per_owner == 0
            || limits.max_processes_per_owner > limits.max_processes
            || limits.max_command_bytes == 0
            || limits.max_output_bytes == 0
            || limits.max_read_bytes == 0
            || limits.max_write_bytes == 0
            || limits.write_timeout_ms == 0
            || limits.write_timeout_ms > 60_000
            || limits.default_timeout_ms == 0
            || limits.default_timeout_ms > limits.max_timeout_ms
            || limits.max_timeout_ms > 86_400_000
        {
            return Err(Error::InvalidInput("invalid process limits".into()));
        }
        let inner = Arc::get_mut(&mut self.inner)
            .ok_or_else(|| Error::Refused("configure process limits before cloning".into()))?;
        if inner.registry.get_mut().next_id != 0 {
            return Err(Error::Refused(
                "configure process limits before starting".into(),
            ));
        }
        inner.limits = limits;
        Ok(self)
    }

    /// Derived schema for starting a process.
    pub fn start_definition() -> Definition {
        Definition::new::<ProcessStartInput, ProcessStatus>(
            "process_start",
            "Start /bin/sh -c with open stdin and a bounded lifetime. Returns immediately with process_id. Use process_read for output, process_write for input, process_stop for cleanup. Processes belong to this conversation; host permissions apply, no sandbox or PTY.",
        )
    }
    /// Derived schema for inspecting or listing owned processes.
    pub fn status_definition() -> Definition {
        Definition::new::<ProcessStatusInput, Vec<ProcessStatus>>(
            "process_status",
            "Inspect process_id or omit it to list this conversation's processes, including retained completed jobs. Read-only: does not write, close stdin or stop jobs. State finished means cleanup and capture ended. Report the actual end, exit_code, signal and error: exit_code 0 with signal null means normal successful exit, not signal termination.",
        )
    }
    /// Derived schema for cursor-based output reading.
    pub fn read_definition() -> Definition {
        Definition::new::<ProcessReadInput, ProcessReadResult>(
            "process_read",
            "Read available stdout and stderr without writing input, closing stdin or stopping the process. Call this tool directly to read a running job; no preparatory process_write or EOF is needed. Use independent zero-based byte offsets, initially 0, then each next_offset. Reading does not consume output. Empty output means no new bytes yet, not completion. omitted_bytes reports eviction. Each stream's bytes and total_bytes are authoritative; the outer rendered text contains formatting not produced by the process.",
        )
    }
    /// Derived schema for writing or closing stdin.
    pub fn write_definition() -> Definition {
        Definition::new::<ProcessWriteInput, ProcessWriteResult>(
            "process_write",
            "Send requested text to process stdin, or close stdin when EOF was requested. Encode an actual newline as the JSON escape \\n when requested; no newline is added automatically. Keep close_stdin false unless closing input is intended: EOF may terminate the process. Do not call this tool to read or poll output; use process_read directly. Inspect complete and bytes_written, since input may be partial on timeout/cancellation; never blindly replay it. If stdin_closed is already true, no further close is needed.",
        )
    }
    /// Derived schema for terminating and optionally forgetting a process.
    pub fn stop_definition() -> Definition {
        Definition::new::<ProcessStopInput, ProcessStopResult>(
            "process_stop",
            "Stop an owned process group and await bounded cleanup. Set forget=true to release the completed process record and all retained output. Check status.state and forgotten. Repeated stop of a retained completed job is safe.",
        )
    }

    fn check(&self, owner: &str, cancel: &CancellationToken) -> Result<(), Error> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if self.inner.closed.is_cancelled() {
            return Err(Error::Refused("process manager is closed".into()));
        }
        if owner.is_empty() || owner.len() > 1024 {
            return Err(Error::InvalidInput(
                "owner key must contain 1 to 1024 bytes".into(),
            ));
        }
        Ok(())
    }

    async fn entry(
        &self,
        owner: &str,
        id: &str,
        cancel: &CancellationToken,
    ) -> Result<Arc<Entry>, Error> {
        self.check(owner, cancel)?;
        let registry = self.inner.registry.lock().await;
        registry
            .entries
            .get(id)
            .filter(|entry| entry.owner == owner)
            .map(Arc::clone)
            .ok_or_else(|| Error::Refused("unknown process in this owner scope".into()))
    }

    /// Spawn and transfer ownership to the manager before returning its identifier.
    ///
    /// # Errors
    /// Rejects invalid commands, escaping initial paths, quotas and cancellation
    /// before spawn. Once spawned, success wins cancellation; use status to recover
    /// a lost response. Non-Unix platforms are unsupported.
    pub async fn start(
        &self,
        owner: &str,
        input: ProcessStartInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ProcessStatus>, Error> {
        self.check(owner, cancel)?;
        if input.command.trim().is_empty() || input.command.contains('\0') {
            return Err(Error::InvalidInput(
                "command must be nonempty without NUL".into(),
            ));
        }
        if input.command.len() > self.inner.limits.max_command_bytes {
            return Err(Error::LimitExceeded(
                "process command exceeds host limit".into(),
            ));
        }
        let cwd = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Error::Cancelled),
            result = tokio::fs::canonicalize(self.inner.root.join(input.cwd.as_deref().unwrap_or("."))) => result.map_err(|source| Error::Io { operation: "resolve process working directory", source })?,
        };
        if !cwd.starts_with(&self.inner.root) {
            return Err(Error::Refused(
                "process working directory is outside root".into(),
            ));
        }
        let mut registry = self.inner.registry.lock().await;
        self.check(owner, cancel)?;
        let limits = self.inner.limits;
        if registry.entries.len() >= limits.max_processes
            || registry
                .entries
                .values()
                .filter(|e| e.owner == owner)
                .count()
                >= limits.max_processes_per_owner
        {
            return Err(Error::LimitExceeded(
                "process quota reached; stop with forget=true to release completed records".into(),
            ));
        }
        let next = registry
            .next_id
            .checked_add(1)
            .ok_or_else(|| Error::LimitExceeded("process identifiers exhausted".into()))?;
        let status = ProcessStatus {
            process_id: format!("p{next}"),
            cwd: cwd.to_string_lossy().into_owned(),
            state: ProcessState::Running,
            end: None,
            exit_code: None,
            signal: None,
            timeout_ms: input
                .timeout_ms
                .map_or(limits.default_timeout_ms, std::num::NonZeroU64::get)
                .min(limits.max_timeout_ms),
            output_complete: false,
            stdin_closed: false,
            stdout_bytes: 0,
            stderr_bytes: 0,
            stdout_retained_from: 0,
            stderr_retained_from: 0,
            error: None,
            stdout_archive: None,
            stderr_archive: None,
        };
        #[cfg(unix)]
        {
            let (entry, status) = crate::persistent_unix::launch(
                owner,
                &input.command,
                &cwd,
                status.clone(),
                limits,
                self.inner.closed.child_token(),
                self.inner.outputs.as_ref(),
            )?;
            registry.next_id = next;
            let done = entry.done.clone();
            registry.entries.insert(status.process_id.clone(), entry);
            self.inner.changes.send_replace(());
            notify_finished(done, self.inner.changes.clone());
            // No await after spawn: ownership and the recoverable ID are committed together.
            let mut text = format!(
                "{} started; use process_read for output\n",
                status.process_id
            );
            crate::output::append_metadata(
                &mut text,
                status.stdout_archive.as_ref(),
                status.stderr_archive.as_ref(),
            );
            Ok(Output::new(status, text))
        }
        #[cfg(not(unix))]
        {
            let _ = (&mut registry, status);
            Err(Error::Unsupported(
                "persistent process groups require Unix".into(),
            ))
        }
    }

    /// Inspect or list this owner's records without consuming output.
    ///
    /// # Errors
    /// Rejects unknown/foreign identifiers, closed managers and cancellation.
    pub async fn status(
        &self,
        owner: &str,
        input: ProcessStatusInput,
        cancel: &CancellationToken,
    ) -> Result<Output<Vec<ProcessStatus>>, Error> {
        self.check(owner, cancel)?;
        let entries = if let Some(id) = input.process_id {
            vec![self.entry(owner, &id, cancel).await?]
        } else {
            self.inner
                .registry
                .lock()
                .await
                .entries
                .values()
                .filter(|e| e.owner == owner)
                .map(Arc::clone)
                .collect()
        };
        let mut states = Vec::with_capacity(entries.len());
        for entry in entries {
            states.push(entry.record.lock().await.status());
        }
        let mut text = String::new();
        for s in &states {
            let _written = writeln!(
                text,
                "{}: {:?}, end={:?}, exit_code={:?}, signal={:?}",
                s.process_id, s.state, s.end, s.exit_code, s.signal
            );
            crate::output::append_metadata(
                &mut text,
                s.stdout_archive.as_ref(),
                s.stderr_archive.as_ref(),
            );
        }
        Ok(Output::new(states, text))
    }

    /// Read non-destructive pages. Stale cursors expose retention gaps explicitly.
    ///
    /// # Errors
    /// Rejects cursors past observed EOF, unknown owners/IDs and cancellation.
    pub async fn read(
        &self,
        owner: &str,
        input: ProcessReadInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ProcessReadResult>, Error> {
        let entry = self.entry(owner, &input.process_id, cancel).await?;
        let record = entry.record.lock().await;
        let limit = input
            .max_bytes
            .map_or(
                self.inner.limits.max_read_bytes,
                std::num::NonZeroUsize::get,
            )
            .min(self.inner.limits.max_read_bytes);
        let data = ProcessReadResult {
            status: record.status(),
            stdout: record.stdout.page(input.stdout_offset, limit)?,
            stderr: record.stderr.page(input.stderr_offset, limit)?,
        };
        let mut text = format!(
            "{} {:?}\nstdout (next={}, omitted={}):\n{}\nstderr (next={}, omitted={}):\n{}",
            data.status.process_id,
            data.status.state,
            data.stdout.next_offset,
            data.stdout.omitted_bytes,
            data.stdout.text,
            data.stderr.next_offset,
            data.stderr.omitted_bytes,
            data.stderr.text
        );
        crate::output::append_metadata(
            &mut text,
            data.status.stdout_archive.as_ref(),
            data.status.stderr_archive.as_ref(),
        );
        Ok(Output::new(data, text))
    }

    /// Write exact stdin bytes, with bounded waiting and explicit partial effects.
    ///
    /// # Errors
    /// Rejects unknown IDs, oversized input and pre-call cancellation. Once a write
    /// starts, failure is a structured result; dropped futures may lose that result.
    pub async fn write(
        &self,
        owner: &str,
        input: ProcessWriteInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ProcessWriteResult>, Error> {
        if input.text.len() > self.inner.limits.max_write_bytes {
            return Err(Error::LimitExceeded(
                "process stdin exceeds host limit".into(),
            ));
        }
        let entry = self.entry(owner, &input.process_id, cancel).await?;
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.inner.limits.write_timeout_ms);
        let mut data = ProcessWriteResult {
            process_id: input.process_id,
            bytes_written: 0,
            complete: false,
            stdin_closed: false,
            error: None,
        };
        let outcome = tokio::select! {
            biased;
            () = cancel.cancelled() => Err("stdin call cancelled"),
            () = entry.stop.cancelled() => Err("process stopped or finished"),
            () = tokio::time::sleep_until(deadline) => Err("stdin write timed out"),
            result = send_input(&entry, &input.text, input.close_stdin, &mut data) => result,
        };
        match outcome {
            Ok(()) => data.complete = true,
            Err(message) => data.error = Some(message.into()),
        }
        data.stdin_closed = entry.record.lock().await.status.stdin_closed;
        let text = format!(
            "{}: bytes_written={}, complete={}, stdin_closed={}, error={:?}\n",
            data.process_id, data.bytes_written, data.complete, data.stdin_closed, data.error
        );
        Ok(Output::new(data, text))
    }

    /// Request group termination, wait up to two seconds and optionally forget.
    ///
    /// # Errors
    /// Rejects unknown/foreign identifiers and pre-call cancellation. Once stop is
    /// requested it continues even if this call is cancelled or dropped.
    pub async fn stop(
        &self,
        owner: &str,
        input: ProcessStopInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ProcessStopResult>, Error> {
        let entry = self.entry(owner, &input.process_id, cancel).await?;
        entry.stop.cancel();
        wait_done(&entry, tokio::time::Instant::now() + Duration::from_secs(2)).await;
        let status = entry.record.lock().await.status();
        let forgotten = if input.forget && status.state == ProcessState::Finished {
            self.inner
                .registry
                .lock()
                .await
                .entries
                .remove(&input.process_id)
                .is_some()
        } else {
            false
        };
        if forgotten {
            self.inner.changes.send_replace(());
        }
        let mut text = format!(
            "{}: {:?}, forgotten={}\n",
            status.process_id, status.state, forgotten
        );
        crate::output::append_metadata(
            &mut text,
            status.stdout_archive.as_ref(),
            status.stderr_archive.as_ref(),
        );
        Ok(Output::new(ProcessStopResult { status, forgotten }, text))
    }

    /// Stop and release one owner's records; future calls may establish a new scope.
    /// Returns whether cleanup finished within the collective two-second wait.
    pub async fn close_owner(&self, owner: &str) -> bool {
        let registry = self.inner.registry.lock().await;
        let entries = registry
            .entries
            .iter()
            .filter(|(_, e)| e.owner == owner)
            .map(|(id, entry)| (id.clone(), Arc::clone(entry)))
            .collect();
        drop(registry);
        self.finish_close(entries).await
    }

    /// Permanently close the manager, stop every group and release retained output.
    /// Returns whether cleanup finished within the collective two-second wait.
    pub async fn close(&self) -> bool {
        self.inner.closed.cancel();
        let entries = self
            .inner
            .registry
            .lock()
            .await
            .entries
            .iter()
            .map(|(id, entry)| (id.clone(), Arc::clone(entry)))
            .collect();
        self.finish_close(entries).await
    }

    async fn finish_close(&self, entries: Vec<(String, Arc<Entry>)>) -> bool {
        for (_, entry) in &entries {
            entry.stop.cancel();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let mut finished = true;
        for (id, entry) in entries {
            wait_done(&entry, deadline).await;
            if *entry.done.borrow() {
                self.inner.registry.lock().await.entries.remove(&id);
                self.inner.changes.send_replace(());
            } else {
                // Keep pending cleanup observable to another close call, even
                // if this waiter times out or is dropped before it completes.
                finished = false;
            }
        }
        finished
    }
}

async fn send_input(
    entry: &Entry,
    text: &str,
    close: bool,
    data: &mut ProcessWriteResult,
) -> Result<(), &'static str> {
    let mut guard = entry.stdin.lock().await;
    let Some(stdin) = guard.as_mut() else {
        return Err("stdin is closed");
    };
    while data.bytes_written < text.len() {
        let bytes = text
            .as_bytes()
            .get(data.bytes_written..)
            .ok_or("invalid stdin offset")?;
        match stdin.write(bytes).await {
            Ok(0) | Err(_) => {
                let mut record = entry.record.lock().await;
                guard.take();
                record.status.stdin_closed = true;
                return Err("stdin pipe write failed");
            }
            Ok(count) => data.bytes_written += count,
        }
    }
    if close {
        let mut record = entry.record.lock().await;
        guard.take();
        record.status.stdin_closed = true;
    }
    Ok(())
}

async fn wait_done(entry: &Entry, deadline: tokio::time::Instant) {
    let mut done = entry.done.clone();
    let _result = tokio::time::timeout_at(deadline, done.wait_for(|done| *done)).await;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dropped_owner_close_keeps_pending_jobs_accounted_until_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let limits = ProcessLimits {
            max_processes: 1,
            max_processes_per_owner: 1,
            ..ProcessLimits::default()
        };
        let manager = ProcessManager::new(root.path())
            .await
            .unwrap()
            .with_limits(limits)
            .unwrap();
        let token = CancellationToken::new();
        let id = manager
            .start("a", ProcessStartInput::new("cat"), &token)
            .await
            .unwrap()
            .data
            .process_id;
        let entry = manager.entry("a", &id, &token).await.unwrap();
        // Hold final status publication to make the pending-cleanup boundary
        // deterministic, without relying on OS scheduling or sleeps.
        let publishing = entry.record.lock().await;
        let closer = manager.clone();
        let task = tokio::spawn(async move { closer.close_owner("a").await });
        entry.stop.cancelled().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(matches!(
            manager
                .start("a", ProcessStartInput::new("true"), &token)
                .await,
            Err(Error::LimitExceeded(_))
        ));
        drop(publishing);
        assert!(manager.close_owner("a").await);
        manager
            .start("a", ProcessStartInput::new("true"), &token)
            .await
            .unwrap();
        assert!(manager.close().await);
    }
}

// Wait on the existing worker signal, without polling output or retaining the
// manager itself past its owning host.
#[cfg(unix)]
fn notify_finished(mut done: watch::Receiver<bool>, changes: watch::Sender<()>) {
    tokio::spawn(async move {
        if !*done.borrow_and_update() {
            let _ = done.changed().await;
        }
        changes.send_replace(());
    });
}
