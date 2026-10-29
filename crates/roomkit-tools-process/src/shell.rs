use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use roomkit_tools_core::{Definition, Error, Output};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::CapturedOutput;

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
    /// Requested process runtime in milliseconds, capped by the host maximum.
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
#[non_exhaustive]
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
    /// Canonical initial working directory.
    pub cwd: String,
    /// Exit code when the shell exited normally; nonzero is a command result.
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
/// Executes `/bin/sh -c` without a login shell; stdin is closed. Commands inherit
/// the host environment and permissions and can access paths outside the root.
/// Unix process-group cleanup covers children that remain in the group, including
/// background jobs after a normal shell exit. It is not an OS sandbox.
#[derive(Clone, Debug)]
pub struct ShellTool {
    root: PathBuf,
    limits: ShellLimits,
    outputs: Option<crate::OutputStore>,
}

impl ShellTool {
    /// Resolve an existing working-directory root.
    ///
    /// # Errors
    /// Fails for an inaccessible root or a non-directory.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, Error> {
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|source| Error::Io {
                operation: "resolve shell root",
                source,
            })?;
        if !tokio::fs::metadata(&root)
            .await
            .map_err(|source| Error::Io {
                operation: "inspect shell root",
                source,
            })?
            .is_dir()
        {
            return Err(Error::InvalidInput("shell root must be a directory".into()));
        }
        Ok(Self {
            root,
            limits: ShellLimits::default(),
            outputs: None,
        })
    }

    /// Attach optional output storage. Archive reads use the same trusted owner.
    /// Standalone `shell` uses owner `default`; multi-owner hosts call `shell_owned`.
    #[must_use]
    pub fn with_output_store(mut self, outputs: crate::OutputStore) -> Self {
        self.outputs = Some(outputs);
        self
    }

    /// Configure positive host limits; the default timeout cannot exceed its cap.
    ///
    /// # Errors
    /// Rejects zero bounds and timeouts greater than 24 hours (timer overflow guard).
    pub fn with_limits(mut self, limits: ShellLimits) -> Result<Self, Error> {
        if limits.max_command_bytes == 0
            || limits.max_output_bytes == 0
            || limits.default_timeout_ms == 0
            || limits.max_timeout_ms > 86_400_000
            || limits.default_timeout_ms > limits.max_timeout_ms
        {
            return Err(Error::InvalidInput("invalid shell limits".into()));
        }
        self.limits = limits;
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
    ) -> Result<Output<ShellResult>, Error> {
        self.shell_owned("default", input, cancel).await
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
    ) -> Result<Output<ShellResult>, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if input.command.trim().is_empty() || input.command.contains('\0') {
            return Err(Error::InvalidInput(
                "command must be nonempty and contain no NUL".into(),
            ));
        }
        if input.command.len() > self.limits.max_command_bytes {
            return Err(Error::LimitExceeded(
                "shell command exceeds the host limit".into(),
            ));
        }
        let path = self.root.join(input.cwd.as_deref().unwrap_or("."));
        let cwd = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Error::Cancelled),
            path = tokio::fs::canonicalize(path) => path.map_err(|source| Error::Io { operation: "resolve shell working directory", source })?,
        };
        if !cwd.starts_with(&self.root) {
            return Err(Error::Refused(
                "shell working directory is outside the root".into(),
            ));
        }
        let timeout_ms = input
            .timeout_ms
            .map_or(self.limits.default_timeout_ms, NonZeroU64::get)
            .min(self.limits.max_timeout_ms);
        let child_cancel = cancel.child_token();
        let _cancel_on_drop = child_cancel.clone().drop_guard();
        #[cfg(unix)]
        {
            let limits = self.limits;
            let outputs = self
                .outputs
                .as_ref()
                .map(|store| (store.clone(), owner.to_owned()));
            tokio::spawn(crate::unix::run(
                input.command,
                cwd,
                limits,
                timeout_ms,
                child_cancel,
                outputs,
            ))
            .await
            .map_err(|error| Error::Io {
                operation: "join shell worker",
                source: std::io::Error::other(error),
            })?
        }
        #[cfg(not(unix))]
        {
            let _ = (cwd, timeout_ms);
            Err(Error::Unsupported(
                "shell process-group cleanup currently requires Unix".into(),
            ))
        }
    }
}
