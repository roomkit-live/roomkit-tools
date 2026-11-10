mod definitions;
mod registry;
mod stdin;

use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use roomkit_tools_core::{Output, ToolError};
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

use crate::output::nullable;
use registry::Registry;
pub(crate) use registry::{Entry, Record, initial_status};

use crate::{
    CommandEnv, ProcessEnd, ProcessLimits, ProcessReadInput, ProcessReadResult, ProcessStartInput,
    ProcessState, ProcessStatus, ProcessStatusInput, ProcessStatusResult, ProcessStopInput,
    ProcessStopResult,
};

#[derive(Debug)]
struct Inner {
    root: PathBuf,
    workspace: roomkit_tools_core::WorkspaceRoot,
    limits: ProcessLimits,
    env: CommandEnv,
    registry: Mutex<Registry>,
    closed: CancellationToken,
    /// Cancelled by [`ProcessSession::detach`], before `closed`.
    detached: CancellationToken,
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
    /// Trusted owner key, not a model argument or OS PID; `None` for the
    /// standalone owner, apart from every named one.
    pub owner: Option<String>,
    /// Exact command supplied when starting the process.
    pub command: String,
    /// Elapsed runtime, frozen once cleanup finishes.
    pub elapsed: Duration,
    /// Authoritative current lifecycle and capture metadata.
    pub status: ProcessStatus,
}

/// Host-owned process collection with explicit owner boundaries, independent of RoomKit.
///
/// Every job belongs to one owner, a trusted key the host passes on each
/// operation, never from model arguments; `None` is the one standalone owner,
/// apart from every named owner, `"default"` included.
/// Clones share ownership; dropping the last clone requests all process cleanup.
/// Keep the Tokio runtime alive and prefer awaiting [`Self::close`]. This is not
/// a sandbox: commands run with host permissions. They receive only the
/// environment selected by [`Self::with_env`], by default the host variables in
/// [`crate::DEFAULT_INHERITED_ENV`]. Each command leads a new Unix session
/// without a controlling terminal.
#[derive(Clone, Debug)]
pub struct ProcessSession {
    inner: Arc<Inner>,
}

impl ProcessSession {
    /// Resolve an existing root for initial working directories.
    ///
    /// # Errors
    /// Inaccessible or non-directory roots are rejected.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        let given = root.as_ref().to_path_buf();
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|source| ToolError::Io {
                operation: "resolve process root",
                source,
            })?;
        if !tokio::fs::metadata(&root)
            .await
            .map_err(|source| ToolError::Io {
                operation: "inspect process root",
                source,
            })?
            .is_dir()
        {
            return Err(ToolError::InvalidInput(
                "process root must be a directory".into(),
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                workspace: crate::workspace(&root, &given),
                root,
                limits: ProcessLimits::default(),
                env: CommandEnv::default(),
                registry: Mutex::new(Registry::default()),
                closed: CancellationToken::new(),
                detached: CancellationToken::new(),
                outputs: None,
                changes: watch::channel(()).0,
            }),
        })
    }

    /// Whether shutdown was requested, not whether all cleanup has finished.
    /// A closed session cannot reopen; every clone shares this state.
    pub fn closed(&self) -> bool {
        self.inner.closed.is_cancelled()
    }

    /// Subscribe to coalesced lifecycle changes (start, finish, forget, close).
    /// Subscribe before taking an observation to avoid a missed wakeup. Output
    /// bytes do not trigger notifications; page output only when needed.
    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.inner.changes.subscribe()
    }

    /// Inspect all retained jobs for the owning host, including completed jobs,
    /// in creation order. This privileged API is deliberately separate from
    /// owner-scoped model tools. The caller must filter results before
    /// presenting them to another principal.
    pub async fn observe(&self) -> Vec<ObservedProcess> {
        let entries = self.inner.registry.lock().await.ordered(|_| true);
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

    /// Attach optional output storage before cloning/starting. Closing this session
    /// stops capture but keeps disk copies until the store is explicitly closed.
    ///
    /// # Errors
    /// Refuses shared or previously used sessions.
    pub fn with_output_store(mut self, outputs: crate::OutputStore) -> Result<Self, ToolError> {
        let inner = Arc::get_mut(&mut self.inner).ok_or_else(|| {
            ToolError::AccessRefused("configure output storage before cloning".into())
        })?;
        if inner.registry.get_mut().next_id != 0 {
            return Err(ToolError::AccessRefused(
                "configure output storage before starting".into(),
            ));
        }
        inner.outputs = Some(outputs);
        Ok(self)
    }

    /// Set positive host bounds before cloning or starting processes.
    ///
    /// # Errors
    /// Rejects shared/used sessions, inconsistent bounds and timers above 24 hours.
    pub fn with_limits(mut self, limits: ProcessLimits) -> Result<Self, ToolError> {
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
            || limits.max_wait_ms == 0
            || limits.max_wait_ms > 86_400_000
        {
            return Err(ToolError::InvalidInput("invalid process limits".into()));
        }
        let inner = Arc::get_mut(&mut self.inner).ok_or_else(|| {
            ToolError::AccessRefused("configure process limits before cloning".into())
        })?;
        if inner.registry.get_mut().next_id != 0 {
            return Err(ToolError::AccessRefused(
                "configure process limits before starting".into(),
            ));
        }
        inner.limits = limits;
        Ok(self)
    }

    /// Select the environment commands receive, before cloning or starting
    /// processes; see [`CommandEnv`].
    ///
    /// # Errors
    /// Rejects an empty variable name, a name containing `=` or NUL, a value
    /// containing NUL, and shared or previously used sessions.
    pub fn with_env(mut self, env: CommandEnv) -> Result<Self, ToolError> {
        env.validate()?;
        let inner = Arc::get_mut(&mut self.inner).ok_or_else(|| {
            ToolError::AccessRefused("configure the process environment before cloning".into())
        })?;
        if inner.registry.get_mut().next_id != 0 {
            return Err(ToolError::AccessRefused(
                "configure the process environment before starting".into(),
            ));
        }
        inner.env = env;
        Ok(self)
    }

    fn check(&self, owner: Option<&str>, cancel: &CancellationToken) -> Result<(), ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        if self.inner.closed.is_cancelled() {
            return Err(ToolError::AccessRefused("process session is closed".into()));
        }
        if owner.is_some_and(|owner| owner.is_empty() || owner.len() > 1024) {
            return Err(ToolError::InvalidInput(
                "owner key must contain 1 to 1024 bytes".into(),
            ));
        }
        Ok(())
    }

    async fn entry(
        &self,
        owner: Option<&str>,
        id: &str,
        cancel: &CancellationToken,
    ) -> Result<Arc<Entry>, ToolError> {
        self.check(owner, cancel)?;
        let registry = self.inner.registry.lock().await;
        registry
            .entries
            .get(id)
            .filter(|entry| entry.owner.as_deref() == owner)
            .map(Arc::clone)
            .ok_or_else(|| ToolError::AccessRefused("unknown process in this owner scope".into()))
    }

    /// Check a start request and resolve its initial directory inside the root.
    async fn start_directory(
        &self,
        input: &ProcessStartInput,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, ToolError> {
        if input.command.trim().is_empty() || input.command.contains('\0') {
            return Err(ToolError::InvalidInput(
                "command must be nonempty without NUL".into(),
            ));
        }
        if input.command.len() > self.inner.limits.max_command_bytes {
            return Err(ToolError::LimitExceeded(
                "process command exceeds host limit".into(),
            ));
        }
        crate::working_directory(
            &self.inner.workspace,
            input.cwd.as_deref(),
            "resolve process working directory",
            cancel,
        )
        .await
    }

    /// Spawn and transfer ownership to the session before returning its identifier.
    ///
    /// # Errors
    /// Rejects invalid commands, escaping initial paths, quotas and cancellation
    /// before spawn. Once spawned, success wins cancellation; use status to recover
    /// a lost response. Non-Unix platforms are unsupported.
    pub async fn start(
        &self,
        owner: Option<&str>,
        input: ProcessStartInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ProcessStatus>, ToolError> {
        self.check(owner, cancel)?;
        let limits = self.inner.limits;
        let timeout_ms = crate::requested_timeout(
            input.timeout_ms,
            limits.default_timeout_ms,
            limits.max_timeout_ms,
        )?;
        let cwd = self.start_directory(&input, cancel).await?;
        let mut registry = self.inner.registry.lock().await;
        self.check(owner, cancel)?;
        let next = registry.next_for(owner, limits)?;
        let shown = crate::relative_cwd(&self.inner.root, &cwd)?;
        let status = initial_status(format!("p{next}"), shown, timeout_ms);
        #[cfg(unix)]
        {
            let request = crate::persistent_unix::Launch {
                owner,
                command: &input.command,
                cwd: &cwd,
                env: &self.inner.env,
            };
            let (entry, status) = crate::persistent_unix::launch(
                &request,
                status.clone(),
                limits,
                self.inner.closed.child_token(),
                self.inner.detached.clone(),
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
            Err(ToolError::UnsupportedResource(
                "persistent process groups require Unix".into(),
            ))
        }
    }

    /// Inspect or list this owner's records, in creation order, without consuming output.
    ///
    /// # Errors
    /// Rejects unknown/foreign identifiers, closed sessions and cancellation.
    pub async fn status(
        &self,
        owner: Option<&str>,
        input: ProcessStatusInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ProcessStatusResult>, ToolError> {
        self.check(owner, cancel)?;
        let entries = if let Some(id) = input.process_id {
            vec![self.entry(owner, &id, cancel).await?]
        } else {
            self.inner
                .registry
                .lock()
                .await
                .ordered(|entry| entry.owner.as_deref() == owner)
        };
        let mut states = Vec::with_capacity(entries.len());
        for entry in entries {
            states.push(entry.record.lock().await.status());
        }
        let mut text = String::new();
        if states.is_empty() {
            text.push_str("no processes\n");
        }
        for s in &states {
            let _written = writeln!(text, "{}", status_line(s));
            crate::output::append_metadata(
                &mut text,
                s.stdout_archive.as_ref(),
                s.stderr_archive.as_ref(),
            );
        }
        Ok(Output::new(ProcessStatusResult { processes: states }, text))
    }

    /// Read non-destructive pages. Stale cursors expose retention gaps explicitly.
    ///
    /// # Errors
    /// Rejects cursors past observed EOF, unknown owners/IDs and cancellation.
    pub async fn read(
        &self,
        owner: Option<&str>,
        input: ProcessReadInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ProcessReadResult>, ToolError> {
        let entry = self.entry(owner, &input.process_id, cancel).await?;
        if let Some(wait) = input.wait_ms {
            let wait = wait.get().min(self.inner.limits.max_wait_ms);
            wait_for_output(&entry, &input, Duration::from_millis(wait), cancel).await?;
        }
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
        let text = read_text(&data);
        Ok(Output::new(data, text))
    }

    /// Send SIGKILL to the process group, wait up to two seconds and optionally
    /// forget. There is no graceful phase: the command gets no chance to clean up.
    ///
    /// # Errors
    /// Rejects unknown/foreign identifiers and pre-call cancellation. Once stop is
    /// requested it continues even if this call is cancelled or dropped.
    pub async fn stop(
        &self,
        owner: Option<&str>,
        input: ProcessStopInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ProcessStopResult>, ToolError> {
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
        let mut text = format!("{}, forgotten={forgotten}\n", status_line(&status));
        crate::output::append_metadata(
            &mut text,
            status.stdout_archive.as_ref(),
            status.stderr_archive.as_ref(),
        );
        Ok(Output::new(ProcessStopResult { status, forgotten }, text))
    }

    /// Stop and release one owner's records; future calls may establish a new scope.
    /// Returns whether cleanup finished within the collective two-second wait.
    pub async fn close_owner(&self, owner: Option<&str>) -> bool {
        let registry = self.inner.registry.lock().await;
        let entries = registry
            .entries
            .iter()
            .filter(|(_, e)| e.owner.as_deref() == owner)
            .map(|(id, entry)| (id.clone(), Arc::clone(entry)))
            .collect();
        drop(registry);
        self.finish_close(entries).await
    }

    /// Permanently close the session, stop every group and release retained output.
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

    /// Permanently close the session but leave its running processes
    /// running, as when the host exits and they must outlive it: a server
    /// started for a later check, for one. Their records and retained output
    /// are released, and their output pipes go to drainers that discard what
    /// they write and outlive the host, so their writes never fail. Nothing
    /// reaps them but the runtime, whenever they exit.
    /// Finished jobs are released as by [`Self::close`]. Returns whether every
    /// job was let go within the collective two-second wait.
    pub async fn detach(&self) -> bool {
        self.inner.detached.cancel();
        self.close().await
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

/// What a model reads of a read: the status line, the archives, then each
/// stream's page.
fn read_text(data: &ProcessReadResult) -> String {
    let mut text = status_line(&data.status);
    text.push('\n');
    // Before what the command wrote, which could imitate these lines.
    crate::output::append_metadata(
        &mut text,
        data.status.stdout_archive.as_ref(),
        data.status.stderr_archive.as_ref(),
    );
    for (name, page) in [("stdout", &data.stdout), ("stderr", &data.stderr)] {
        let _written = write!(text, "{name} (next_offset={}", page.next_offset);
        if page.omitted_bytes > 0 {
            let _written = write!(text, ", omitted_bytes={}", page.omitted_bytes);
        }
        if page.has_more {
            text.push_str(", has_more=true");
        }
        text.push_str("):\n");
        if !page.text.is_empty() {
            text.push_str(&page.text);
            if !page.text.ends_with('\n') {
                text.push('\n');
            }
        }
    }
    text
}

/// Wait until `entry` finishes or writes past `input`'s offsets, or `wait`
/// elapses; a cancelled call stops waiting as cancelled.
async fn wait_for_output(
    entry: &Entry,
    input: &ProcessReadInput,
    wait: Duration,
    cancel: &CancellationToken,
) -> Result<(), ToolError> {
    let deadline = tokio::time::Instant::now() + wait;
    let mut done = entry.done.clone();
    // Subscribed before the first look, so no write between them is missed.
    let mut output = entry.record.lock().await.output.subscribe();
    loop {
        if *done.borrow_and_update() || entry.record.lock().await.ready_for(input) {
            return Ok(());
        }
        tokio::select! {
            () = cancel.cancelled() => return Err(ToolError::Cancelled),
            () = tokio::time::sleep_until(deadline) => return Ok(()),
            changed = done.changed() => if changed.is_err() { return Ok(()) },
            changed = output.changed() => if changed.is_err() { return Ok(()) },
        }
    }
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
        let manager = ProcessSession::new(root.path())
            .await
            .unwrap()
            .with_limits(limits)
            .unwrap();
        let token = CancellationToken::new();
        let id = manager
            .start(Some("a"), ProcessStartInput::new("cat"), &token)
            .await
            .unwrap()
            .data
            .process_id;
        let entry = manager.entry(Some("a"), &id, &token).await.unwrap();
        // Hold final status publication to make the pending-cleanup boundary
        // deterministic, without relying on OS scheduling or sleeps.
        let publishing = entry.record.lock().await;
        let closer = manager.clone();
        let task = tokio::spawn(async move { closer.close_owner(Some("a")).await });
        entry.stop.cancelled().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(matches!(
            manager
                .start(Some("a"), ProcessStartInput::new("true"), &token)
                .await,
            Err(ToolError::LimitExceeded(_))
        ));
        drop(publishing);
        assert!(manager.close_owner(Some("a")).await);
        manager
            .start(Some("a"), ProcessStartInput::new("true"), &token)
            .await
            .unwrap();
        assert!(manager.close().await);
    }
}

/// One process as a model reads it, under the names its tools describe.
fn status_line(status: &ProcessStatus) -> String {
    let state = match status.state {
        ProcessState::Running => "running",
        ProcessState::Finished => "finished",
    };
    let mut line = format!("{}: state={state}", status.process_id);
    if status.state == ProcessState::Finished {
        let _written = write!(
            line,
            ", end={}, exit_code={}, signal={}",
            nullable(status.end.map(end_name)),
            nullable(status.exit_code),
            nullable(status.signal)
        );
        if !status.output_complete {
            line.push_str(", output_complete=false");
        }
    }
    if let Some(error) = &status.error {
        let _written = write!(line, ", error={error:?}");
    }
    line
}

/// How a process ended, under its name in the structured result.
fn end_name(end: ProcessEnd) -> &'static str {
    match end {
        ProcessEnd::Exited => "exited",
        ProcessEnd::Stopped => "stopped",
        ProcessEnd::TimedOut => "timed_out",
        ProcessEnd::Failed => "failed",
    }
}

// Wait on the existing worker signal, without polling output or retaining the
// session itself past its owning host.
#[cfg(unix)]
fn notify_finished(mut done: watch::Receiver<bool>, changes: watch::Sender<()>) {
    tokio::spawn(async move {
        if !*done.borrow_and_update() {
            let _ = done.changed().await;
        }
        changes.send_replace(());
    });
}
