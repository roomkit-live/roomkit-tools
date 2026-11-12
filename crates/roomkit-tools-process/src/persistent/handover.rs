//! Jobs a session adopts once spawned: its own, and the shell tool's
//! background commands, started there or moved there at their timeout.

use std::num::NonZeroU64;
use std::path::Path;
use std::time::Duration;

use roomkit_tools_core::ToolError;
use tokio::sync::MutexGuard;
use tokio_util::sync::CancellationToken;

use super::{ProcessSession, Registry, initial_status, notify_finished};
use crate::persistent_unix::{Host, Spawned};
use crate::{CommandEnv, ProcessStatus};

/// An identifier the quota admits, held with the registry until committed.
pub(super) struct Reserved<'a> {
    registry: MutexGuard<'a, Registry>,
    next: u64,
}

impl Reserved<'_> {
    pub(super) fn process_id(&self) -> String {
        format!("p{}", self.next)
    }
}

/// A shell command the shell tool hands over, as it resolved it.
pub(crate) struct ShellJob<'a> {
    /// The caller's trusted owner, `None` for the standalone one.
    pub(crate) owner: Option<&'a str>,
    pub(crate) command: &'a str,
    pub(crate) cwd: &'a Path,
    /// The initial directory relative to the workspace, as results name it.
    pub(crate) shown_cwd: &'a str,
    /// The shell tool's environment, which its commands keep.
    pub(crate) env: &'a CommandEnv,
}

impl ProcessSession {
    /// Reserve `owner`'s next identifier, once the session is open and its
    /// quota admits one more job.
    pub(super) async fn reserve(
        &self,
        owner: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Reserved<'_>, ToolError> {
        let registry = self.inner.registry.lock().await;
        self.check(owner, cancel)?;
        let next = registry.next_for(owner, self.inner.limits)?;
        Ok(Reserved { registry, next })
    }

    /// Adopt `spawned` under the reserved identifier, which `status` carries.
    pub(super) fn commit(
        &self,
        mut reserved: Reserved<'_>,
        (owner, command): (Option<&str>, &str),
        status: ProcessStatus,
        spawned: Spawned,
    ) -> ProcessStatus {
        let host = Host {
            limits: self.inner.limits,
            outputs: self.inner.outputs.as_ref(),
            ends: &self.inner.ends,
            stop: self.inner.closed.child_token(),
            detach: self.inner.detached.clone(),
        };
        let (entry, status) =
            crate::persistent_unix::adopt(spawned, (owner, command), status, host);
        reserved.registry.next_id = reserved.next;
        let done = entry.done.clone();
        reserved
            .registry
            .entries
            .insert(status.process_id.clone(), entry);
        self.inner.changes.send_replace(());
        notify_finished(done, self.inner.changes.clone());
        status
    }

    /// Start `job` as its owner's next process, stdin closed, for the
    /// lifetime `timeout_ms` within this session's limits.
    pub(crate) async fn start_shell(
        &self,
        job: &ShellJob<'_>,
        timeout_ms: Option<NonZeroU64>,
        cancel: &CancellationToken,
    ) -> Result<ProcessStatus, ToolError> {
        self.check(job.owner, cancel)?;
        let limits = self.inner.limits;
        let timeout_ms =
            crate::requested_timeout(timeout_ms, limits.default_timeout_ms, limits.max_timeout_ms)?;
        let reserved = self.reserve(job.owner, cancel).await?;
        let status = initial_status(reserved.process_id(), job.shown_cwd.into(), timeout_ms);
        let spawned = crate::persistent_unix::launch(
            job.command,
            job.cwd,
            job.env,
            std::process::Stdio::null(),
        )?;
        Ok(self.commit(reserved, (job.owner, job.command), status, spawned))
    }

    /// Take over `spawned`, `job`'s command still running at its foreground
    /// timeout, as its owner's next process: it runs on until this session's
    /// maximum lifetime, counted from its start. A session that refuses it,
    /// closed or its quota reached, gives it back with the reason.
    ///
    /// # Errors
    /// The refusal and the command, still running.
    pub(crate) async fn take_over(
        &self,
        job: &ShellJob<'_>,
        spawned: Spawned,
        cancel: &CancellationToken,
    ) -> Result<ProcessStatus, (ToolError, Box<Spawned>)> {
        let lifetime = self.inner.limits.max_timeout_ms;
        if spawned.started.elapsed() >= Duration::from_millis(lifetime) {
            let error = ToolError::LimitExceeded(
                "the command already ran for the process maximum lifetime".into(),
            );
            return Err((error, Box::new(spawned)));
        }
        let reserved = match self.reserve(job.owner, cancel).await {
            Ok(reserved) => reserved,
            Err(error) => return Err((error, Box::new(spawned))),
        };
        let status = initial_status(reserved.process_id(), job.shown_cwd.into(), lifetime);
        Ok(self.commit(reserved, (job.owner, job.command), status, spawned))
    }
}
