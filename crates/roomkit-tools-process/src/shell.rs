use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{CapturedOutput, CommandEnv};

/// A command to run in the foreground, or in the background as a process.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShellInput {
    /// Shell command text.
    pub command: String,
    /// Starting directory in the workspace; defaults to its root.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Runtime limit in ms, or a background command's lifetime; one above the
    /// host maximum is refused.
    #[serde(default)]
    pub timeout_ms: Option<NonZeroU64>,
    /// Start it as a background process and return its `process_id` at once,
    /// for a server or a long job; read its output with `process_read`.
    #[serde(default)]
    pub run_in_background: bool,
}

impl ShellInput {
    /// Run a command from the tool root using the host's default timeout.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            cwd: None,
            timeout_ms: None,
            run_in_background: false,
        }
    }
}

/// Host-owned limits; models cannot increase these bounds.
#[derive(Clone, Copy, Debug)]
pub struct ShellLimits {
    /// Maximum command input size. Default: 16 KiB.
    pub max_command_bytes: usize,
    /// Retained raw bytes per stream. Default: 32 KiB each.
    /// UTF-8 replacement, omission markers and result metadata are additional.
    pub max_output_bytes: usize,
    /// Default process runtime before termination. Default: 10 seconds.
    pub default_timeout_ms: u64,
    /// Maximum requested process runtime. Default: 120 seconds.
    pub max_timeout_ms: u64,
    /// Leave a command's background jobs running once its shell exits on its
    /// own, instead of stopping its whole process group. A job still holding
    /// the command's pipes then writes to a drainer that discards its output
    /// and outlives the host. A timeout or a cancellation still stops every
    /// member. The tool follows the jobs it keeps: [`ShellTool::ends`] tells
    /// when they ended, [`ShellTool::close`] stops them and
    /// [`ShellTool::detach`] lets them go. Default: false.
    pub keep_background_jobs: bool,
    /// Move a command still running at its timeout to the background instead
    /// of stopping it: it runs on as a process of the session attached with
    /// [`ShellTool::with_processes`], until that session's maximum lifetime
    /// counted from its start, and the result names it in `process_id`. A
    /// move the session refuses, its quota reached, stops the command as
    /// without this. Needs that session. Default: false.
    pub background_on_timeout: bool,
}

impl Default for ShellLimits {
    fn default() -> Self {
        Self {
            max_command_bytes: 16 * 1024,
            max_output_bytes: 32 * 1024,
            default_timeout_ms: 10_000,
            max_timeout_ms: 120_000,
            keep_background_jobs: false,
            background_on_timeout: false,
        }
    }
}

/// A completed command, including nonzero exits and timeouts, or one that
/// runs on in the background.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ShellResult {
    /// Initial working directory, relative to the workspace (`.` for its
    /// root); the host's path is never shown.
    pub cwd: String,
    /// Exit code when the shell exited normally; nonzero is a command result.
    /// Both this and `signal` are null only after a timeout when the shell
    /// itself runs under other credentials and refused SIGKILL: it was left
    /// running, outside cleanup.
    pub exit_code: Option<i32>,
    /// Unix signal number when terminated by a signal.
    pub signal: Option<i32>,
    /// Whether the tool's runtime deadline initiated termination.
    pub timed_out: bool,
    /// Effective runtime budget, after host capping.
    pub timeout_ms: u64,
    /// Whether both output pipes reached EOF. False means capture was cut short.
    pub output_complete: bool,
    /// Captured standard output.
    pub stdout: CapturedOutput,
    /// Captured standard error, kept separate from stdout.
    pub stderr: CapturedOutput,
    /// Set when the command runs on in the background as this process:
    /// started with `run_in_background`, or still running at its timeout and
    /// moved there. `exit_code` and `signal` are then null; read the rest of
    /// its output with `process_read`.
    #[serde(default)]
    pub process_id: Option<String>,
}

/// A shell with a host-selected root and limits, independent of any agent runtime.
///
/// Executes `/bin/sh -c` without a login shell; stdin is closed. Commands run
/// with the host's permissions and can access paths outside the root. They
/// receive only the environment selected by [`Self::with_env`], by default the
/// host variables in [`crate::DEFAULT_INHERITED_ENV`].
/// Each command leads a new Unix session without a controlling terminal, so
/// opening `/dev/tty` fails. Process-group cleanup covers children that remain
/// in the group, including background jobs after a normal shell exit unless
/// [`ShellLimits::keep_background_jobs`] keeps them. Members
/// the host cannot signal, such as ones running under other credentials, stay
/// outside cleanup and the result is still returned. It is not an OS sandbox.
#[derive(Clone, Debug)]
pub struct ShellTool {
    root: PathBuf,
    workspace: roomkit_tools_core::WorkspaceRoot,
    limits: ShellLimits,
    env: Arc<CommandEnv>,
    outputs: Option<crate::OutputStore>,
    jobs: Arc<crate::shell_jobs::ShellJobs>,
    processes: Option<crate::ProcessSession>,
}

impl ShellTool {
    /// Resolve an existing working-directory root.
    ///
    /// # Errors
    /// Fails for an inaccessible root or a non-directory.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        let given = root.as_ref().to_path_buf();
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|source| ToolError::Io {
                operation: "resolve shell root",
                source,
            })?;
        if !tokio::fs::metadata(&root)
            .await
            .map_err(|source| ToolError::Io {
                operation: "inspect shell root",
                source,
            })?
            .is_dir()
        {
            return Err(ToolError::InvalidInput(
                "shell root must be a directory".into(),
            ));
        }
        Ok(Self {
            workspace: crate::workspace(&root, &given),
            root,
            limits: ShellLimits::default(),
            env: Arc::default(),
            outputs: None,
            jobs: Arc::default(),
            processes: None,
        })
    }

    /// Attach optional output storage. Archive reads use the same trusted owner.
    /// Only a stream cut short keeps its archive, which holds the full output;
    /// one kept whole is released as the call ends, unless it was cancelled.
    /// Standalone `shell` files them under the standalone owner, the one
    /// [`OutputStore`](crate::OutputStore) methods reach with `None`, apart
    /// from every named owner; multi-owner hosts call `shell_owned`.
    #[must_use]
    pub fn with_output_store(mut self, outputs: crate::OutputStore) -> Self {
        self.outputs = Some(outputs);
        self
    }

    /// Whether output storage is attached: only then does a call keep
    /// archives, under the trusted owner a multi-owner host passes. The
    /// RoomKit adapter reads it; it is no part of the public API.
    #[doc(hidden)]
    #[must_use]
    pub fn has_output_store(&self) -> bool {
        self.outputs.is_some()
    }

    /// Run background commands as processes of `processes`: those started
    /// with `run_in_background`, and those [`ShellLimits::background_on_timeout`]
    /// moves there. They take its identifiers, quotas, lifetimes, output
    /// retention and ends, under the caller's owner, so its process tools read
    /// and stop them; they keep this tool's environment, and stdin stays
    /// closed. Closing this tool leaves them to the session.
    #[must_use]
    pub fn with_processes(mut self, processes: crate::ProcessSession) -> Self {
        self.processes = Some(processes);
        self
    }

    /// Configure positive host limits; the default timeout cannot exceed its cap.
    ///
    /// # Errors
    /// Rejects zero bounds, timeouts greater than 24 hours (timer overflow
    /// guard) and `background_on_timeout` before [`Self::with_processes`].
    pub fn with_limits(mut self, limits: ShellLimits) -> Result<Self, ToolError> {
        if limits.max_command_bytes == 0
            || limits.max_output_bytes == 0
            || limits.default_timeout_ms == 0
            || limits.max_timeout_ms > 86_400_000
            || limits.default_timeout_ms > limits.max_timeout_ms
        {
            return Err(ToolError::InvalidInput("invalid shell limits".into()));
        }
        if limits.background_on_timeout && self.processes.is_none() {
            return Err(ToolError::InvalidInput(
                "background_on_timeout needs a process session".into(),
            ));
        }
        self.limits = limits;
        Ok(self)
    }

    /// Select the environment commands receive; see [`CommandEnv`].
    ///
    /// # Errors
    /// Rejects an empty variable name, a name containing `=` or NUL, and a
    /// value containing NUL.
    pub fn with_env(mut self, env: CommandEnv) -> Result<Self, ToolError> {
        env.validate()?;
        self.env = Arc::new(env);
        Ok(self)
    }

    /// The ends of the background jobs that commands leave running from now
    /// on, with [`ShellLimits::keep_background_jobs`]: one end once every job
    /// a command left has ended, whatever its owner. A job that leaves the
    /// command's process group, such as a daemon, is not followed. Call it
    /// before running the commands to follow; the ends stop once the tool
    /// closes. Filter by `owner` before telling another principal.
    pub fn ends(&self) -> crate::JobEnds {
        self.jobs.ends.subscribe()
    }

    /// Permanently close the tool: stop the background jobs it follows, tell
    /// their ends, then end [`Self::ends`]. Every clone shares this state;
    /// later commands are refused. Returns whether every job stopped within
    /// two seconds. Dropping the tool instead leaves its jobs running, as
    /// before the tool followed them.
    pub async fn close(&self) -> bool {
        self.jobs.close(false).await
    }

    /// Permanently close the tool but leave the background jobs it follows
    /// running, untold, as when the host exits and they must outlive it.
    /// Returns whether every job was let go within two seconds.
    pub async fn detach(&self) -> bool {
        self.jobs.close(true).await
    }

    /// Schemas derived from the actual input and result types.
    pub fn definition() -> Definition {
        Definition::new::<ShellInput, ShellResult>(
            "shell",
            "Run a command with /bin/sh -c and wait for it. Check exit_code and timed_out; output is bounded. No stdin. Each call starts a new shell: cd and exported variables do not carry over. For a server or a long job, set run_in_background rather than ending the command with &. A result with a process_id runs on in the background: read it with process_read. Not a sandbox: host permissions apply.",
        )
    }

    /// Run, capture and reap a foreground command.
    ///
    /// Cancelling returns an error after cleanup; dropping this future requests
    /// the same cleanup in an owned worker. Keep the Tokio runtime alive for that
    /// cleanup. A timeout returns partial output with `timed_out: true`.
    ///
    /// # Errors
    /// Rejects invalid arguments and starting directories outside the root.
    /// Returns cancellation, unsupported-platform or safe I/O failures.
    pub async fn shell(
        &self,
        input: ShellInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ShellResult>, ToolError> {
        self.run(None, input, cancel).await
    }

    /// Execute with a trusted archive owner supplied by the host, never the model.
    ///
    /// # Errors
    /// Same failures and cleanup semantics as [`Self::shell`]. Storage failures
    /// appear in archive metadata and do not change command exit status.
    pub async fn shell_owned(
        &self,
        owner: &str,
        input: ShellInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ShellResult>, ToolError> {
        self.run(Some(owner), input, cancel).await
    }

    /// A call under its archives' owner, `None` for the standalone one.
    async fn run(
        &self,
        owner: Option<&str>,
        input: ShellInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ShellResult>, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        if self.jobs.is_closed() {
            return Err(ToolError::AccessRefused("shell tool is closed".into()));
        }
        if input.command.trim().is_empty() || input.command.contains('\0') {
            return Err(ToolError::InvalidInput(
                "command must be nonempty and contain no NUL".into(),
            ));
        }
        if input.command.len() > self.limits.max_command_bytes {
            return Err(ToolError::LimitExceeded(
                "shell command exceeds the host limit".into(),
            ));
        }
        if input.run_in_background {
            return self.background(owner, input, cancel).await;
        }
        let timeout_ms = crate::requested_timeout(
            input.timeout_ms,
            self.limits.default_timeout_ms,
            self.limits.max_timeout_ms,
        )?;
        let cwd = crate::working_directory(
            &self.workspace,
            input.cwd.as_deref(),
            "resolve shell working directory",
            cancel,
        )
        .await?;
        let child_cancel = cancel.child_token();
        let _cancel_on_drop = child_cancel.clone().drop_guard();
        #[cfg(unix)]
        {
            let foreground = crate::unix::Foreground {
                command: input.command,
                shown_cwd: crate::relative_cwd(&self.root, &cwd)?,
                cwd,
                env: Arc::clone(&self.env),
                limits: self.limits,
                timeout_ms,
                outputs: self
                    .outputs
                    .as_ref()
                    .map(|store| (store.clone(), owner.map(str::to_owned))),
                owner: owner.map(str::to_owned),
                jobs: Arc::clone(&self.jobs),
                started: std::time::Instant::now(),
                processes: self
                    .processes
                    .clone()
                    .filter(|_| self.limits.background_on_timeout),
            };
            tokio::spawn(crate::unix::run(foreground, child_cancel))
                .await
                .map_err(|error| ToolError::Io {
                    operation: "join shell worker",
                    source: std::io::Error::other(error),
                })?
        }
        #[cfg(not(unix))]
        {
            let _ = (cwd, timeout_ms);
            Err(ToolError::UnsupportedResource(
                "shell process-group cleanup currently requires Unix".into(),
            ))
        }
    }

    /// Start `input` as a process of the attached session and return at once.
    async fn background(
        &self,
        owner: Option<&str>,
        input: ShellInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ShellResult>, ToolError> {
        let Some(processes) = &self.processes else {
            return Err(ToolError::InvalidInput(
                "run_in_background is not available: this host gives the shell no process session"
                    .into(),
            ));
        };
        let cwd = crate::working_directory(
            &self.workspace,
            input.cwd.as_deref(),
            "resolve shell working directory",
            cancel,
        )
        .await?;
        let shown_cwd = crate::relative_cwd(&self.root, &cwd)?;
        #[cfg(unix)]
        {
            let job = crate::persistent::ShellJob {
                owner,
                command: &input.command,
                cwd: &cwd,
                shown_cwd: &shown_cwd,
                env: &self.env,
            };
            let status = processes
                .start_shell(&job, input.timeout_ms, cancel)
                .await?;
            Ok(crate::shell_handover::started(status))
        }
        #[cfg(not(unix))]
        {
            let _ = (processes, owner, shown_cwd);
            Err(ToolError::UnsupportedResource(
                "background shell commands require Unix".into(),
            ))
        }
    }
}
