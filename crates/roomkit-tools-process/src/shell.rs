use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{CapturedOutput, CommandEnv};

/// A foreground command; shell syntax is interpreted by `/bin/sh`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShellInput {
    /// Shell program text, passed as one argument to `sh -c`.
    pub command: String,
    /// Initial directory relative to the tool root, defaulting to that root.
    /// Absolute paths within the root are also accepted. This is not confinement.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Requested process runtime in milliseconds. A value above the host
    /// maximum is refused.
    #[serde(default)]
    pub timeout_ms: Option<NonZeroU64>,
}

impl ShellInput {
    /// Run a command from the tool root using the host's default timeout.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            cwd: None,
            timeout_ms: None,
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
}

impl Default for ShellLimits {
    fn default() -> Self {
        Self {
            max_command_bytes: 16 * 1024,
            max_output_bytes: 32 * 1024,
            default_timeout_ms: 10_000,
            max_timeout_ms: 120_000,
        }
    }
}

/// A completed command, including nonzero exits and timeouts.
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
}

/// A shell with a host-selected root and limits, independent of any agent runtime.
///
/// Executes `/bin/sh -c` without a login shell; stdin is closed. Commands run
/// with the host's permissions and can access paths outside the root. They
/// receive only the environment selected by [`Self::with_env`], by default the
/// host variables in [`crate::DEFAULT_INHERITED_ENV`].
/// Each command leads a new Unix session without a controlling terminal, so
/// opening `/dev/tty` fails. Process-group cleanup covers children that remain
/// in the group, including background jobs after a normal shell exit. Members
/// the host cannot signal, such as ones running under other credentials, stay
/// outside cleanup and the result is still returned. It is not an OS sandbox.
#[derive(Clone, Debug)]
pub struct ShellTool {
    root: PathBuf,
    limits: ShellLimits,
    env: Arc<CommandEnv>,
    outputs: Option<crate::OutputStore>,
}

impl ShellTool {
    /// Resolve an existing working-directory root.
    ///
    /// # Errors
    /// Fails for an inaccessible root or a non-directory.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
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
            root,
            limits: ShellLimits::default(),
            env: Arc::default(),
            outputs: None,
        })
    }

    /// Attach optional output storage. Archive reads use the same trusted owner.
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

    /// Configure positive host limits; the default timeout cannot exceed its cap.
    ///
    /// # Errors
    /// Rejects zero bounds and timeouts greater than 24 hours (timer overflow guard).
    pub fn with_limits(mut self, limits: ShellLimits) -> Result<Self, ToolError> {
        if limits.max_command_bytes == 0
            || limits.max_output_bytes == 0
            || limits.default_timeout_ms == 0
            || limits.max_timeout_ms > 86_400_000
            || limits.default_timeout_ms > limits.max_timeout_ms
        {
            return Err(ToolError::InvalidInput("invalid shell limits".into()));
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

    /// Schemas derived from the actual input and result types.
    pub fn definition() -> Definition {
        Definition::new::<ShellInput, ShellResult>(
            "shell",
            "Run a foreground command with /bin/sh -c. Returns separate stdout/stderr, exit status and timeout metadata. cwd starts within the configured workspace, but commands run with host permissions: this tool is not a sandbox. Output is bounded; no interactive stdin or persistent sessions.",
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
        let timeout_ms = crate::requested_timeout(
            input.timeout_ms,
            self.limits.default_timeout_ms,
            self.limits.max_timeout_ms,
        )?;
        let path = self.root.join(input.cwd.as_deref().unwrap_or("."));
        let cwd = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(ToolError::Cancelled),
            path = tokio::fs::canonicalize(path) => path.map_err(|source| ToolError::Io { operation: "resolve shell working directory", source })?,
        };
        if !cwd.starts_with(&self.root) {
            return Err(ToolError::AccessRefused(
                "shell working directory is outside the root".into(),
            ));
        }
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
}
