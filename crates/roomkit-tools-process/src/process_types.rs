use std::num::{NonZeroU64, NonZeroUsize};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A command to start in the background.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessStartInput {
    /// Shell command text.
    pub command: String,
    /// Starting directory in the workspace.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Lifetime limit in ms, at most the host maximum. Default: five minutes.
    #[serde(default)]
    pub timeout_ms: Option<NonZeroU64>,
}

impl ProcessStartInput {
    /// Start in the configured root with the default lifetime.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            cwd: None,
            timeout_ms: None,
        }
    }
}

/// One process to inspect, or none to list them all.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessStatusInput {
    /// Omit to list every process, in creation order.
    #[serde(default)]
    pub process_id: Option<String>,
}

/// A page of a process's output.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessReadInput {
    /// Identifier from `process_start`.
    pub process_id: String,
    /// Stdout byte offset: the previous stdout `next_offset`.
    #[serde(default)]
    pub stdout_offset: u64,
    /// Stderr byte offset: the previous stderr `next_offset`.
    #[serde(default)]
    pub stderr_offset: u64,
    /// Bytes per stream, capped by the host.
    #[serde(default)]
    pub max_bytes: Option<NonZeroUsize>,
}

impl ProcessReadInput {
    /// Read both streams from their beginning using the host page size.
    pub fn new(process_id: impl Into<String>) -> Self {
        Self {
            process_id: process_id.into(),
            stdout_offset: 0,
            stderr_offset: 0,
            max_bytes: None,
        }
    }
}

/// Text for a process's stdin.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessWriteInput {
    /// Owned process identifier.
    pub process_id: String,
    /// Exact text; no newline is added, so end a line with `\n`. Omit to only close stdin.
    #[serde(default)]
    pub text: String,
    /// Close stdin (EOF) after the text; this may end the process. Only when input is done.
    #[serde(default)]
    pub close_stdin: bool,
}

impl ProcessWriteInput {
    /// Send nothing to `process_id` and keep stdin open, as Python's
    /// `write(process_id)`; set `text` or `close_stdin` by struct update.
    pub fn new(process_id: impl Into<String>) -> Self {
        Self {
            process_id: process_id.into(),
            text: String::new(),
            close_stdin: false,
        }
    }
}

/// A process to stop with SIGKILL.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessStopInput {
    /// Owned process identifier.
    pub process_id: String,
    /// Also release the finished record and its retained output.
    #[serde(default)]
    pub forget: bool,
}

impl ProcessStopInput {
    /// Stop `process_id` and keep its record, as Python's `stop(process_id)`.
    pub fn new(process_id: impl Into<String>) -> Self {
        Self {
            process_id: process_id.into(),
            forget: false,
        }
    }
}

/// Host-selected aggregate, per-process and per-call bounds.
#[derive(Clone, Copy, Debug)]
pub struct ProcessLimits {
    /// Maximum retained processes across all owners, including finished ones (32).
    pub max_processes: usize,
    /// Maximum retained processes for one owner (8).
    pub max_processes_per_owner: usize,
    /// Maximum command input bytes (16 KiB).
    pub max_command_bytes: usize,
    /// Retained most recent raw bytes per stream and process (1 MiB).
    pub max_output_bytes: usize,
    /// Maximum raw bytes per stream in a read response (16 KiB).
    pub max_read_bytes: usize,
    /// Maximum stdin bytes in one call (64 KiB).
    pub max_write_bytes: usize,
    /// Maximum wait for the stdin lock and pipe (1 second).
    pub write_timeout_ms: u64,
    /// Default process lifetime (5 minutes).
    pub default_timeout_ms: u64,
    /// Maximum requested process lifetime (1 hour, configurable up to 24 hours).
    pub max_timeout_ms: u64,
}

impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            max_processes: 32,
            max_processes_per_owner: 8,
            max_command_bytes: 16 * 1024,
            max_output_bytes: 1024 * 1024,
            max_read_bytes: 16 * 1024,
            max_write_bytes: 64 * 1024,
            write_timeout_ms: 1000,
            default_timeout_ms: 300_000,
            max_timeout_ms: 3_600_000,
        }
    }
}

/// Lifecycle state; `finished` means cleanup and bounded pipe draining are done.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    /// Still executing or cleaning up.
    Running,
    /// Capture has stopped and the direct child has been reaped, or could not
    /// be stopped and was left running: `end` is then `failed`.
    Finished,
}

/// What initiated termination; command exit codes are separate from tool failures.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessEnd {
    /// The shell exited by itself, possibly with a nonzero code or signal.
    Exited,
    /// An explicit stop or owner shutdown requested termination.
    Stopped,
    /// The configured lifetime expired.
    TimedOut,
    /// Capture or cleanup failed; inspect `error`.
    Failed,
}

/// An owned process snapshot, without output contents.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProcessStatus {
    /// Scope-local identifier, never an OS PID or an authorization token.
    pub process_id: String,
    /// Initial working directory, relative to the workspace (`.` for its
    /// root); the host's path is never shown. Not confinement.
    pub cwd: String,
    /// Running includes cleanup; finished means final output is available.
    pub state: ProcessState,
    /// Present once cleanup finishes.
    pub end: Option<ProcessEnd>,
    /// Normal exit code, if available.
    pub exit_code: Option<i32>,
    /// Terminating Unix signal, if available.
    pub signal: Option<i32>,
    /// Effective process lifetime limit.
    pub timeout_ms: u64,
    /// Both output streams reached EOF; false while running or on incomplete capture.
    pub output_complete: bool,
    /// Whether stdin has been closed by the host or cleanup.
    pub stdin_closed: bool,
    /// Observed stdout bytes, including evicted bytes.
    pub stdout_bytes: u64,
    /// Observed stderr bytes, including evicted bytes.
    pub stderr_bytes: u64,
    /// Earliest retained stdout byte offset.
    pub stdout_retained_from: u64,
    /// Earliest retained stderr byte offset.
    pub stderr_retained_from: u64,
    /// Safe infrastructure failure, if any. No OS diagnostic details are exposed.
    pub error: Option<String>,
    /// Optional live stdout archive metadata; absent when storage is disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_archive: Option<crate::OutputArchive>,
    /// Optional live stderr archive metadata; independent of journal retention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_archive: Option<crate::OutputArchive>,
}

/// One raw-byte page and its lossy UTF-8 display, with explicit retention gaps.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProcessChunk {
    /// Actual first returned byte offset, after any retention gap.
    pub offset: u64,
    /// Cursor to use for the next call. Reading does not consume shared output.
    pub next_offset: u64,
    /// Observed stream length at the snapshot.
    pub total_bytes: u64,
    /// Bytes between the requested cursor and `offset` that were evicted.
    pub omitted_bytes: u64,
    /// Exact retained bytes; use these for binary data or split UTF-8 sequences.
    pub bytes: Vec<u8>,
    /// UTF-8 display with replacement for invalid or split byte sequences.
    pub text: String,
    /// More observed bytes remain beyond this page.
    pub has_more: bool,
    /// The pipe reached EOF, independent of whether all retained bytes were read.
    pub eof: bool,
}

/// A consistent snapshot of status and both independently paged streams.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProcessReadResult {
    /// Process state at the time of reading.
    pub status: ProcessStatus,
    /// Standard output page.
    pub stdout: ProcessChunk,
    /// Standard error page.
    pub stderr: ProcessChunk,
}

/// Stdin effects, including partial writes; never blindly retry incomplete input.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProcessWriteResult {
    /// Owned process identifier.
    pub process_id: String,
    /// Bytes accepted by the OS pipe, not proof the application consumed them.
    pub bytes_written: usize,
    /// All input and any requested EOF were delivered to the pipe.
    pub complete: bool,
    /// Whether the pipe was closed by this call or earlier cleanup.
    pub stdin_closed: bool,
    /// Safe reason for incomplete delivery (timeout, cancellation, exit or I/O).
    pub error: Option<String>,
}

/// The processes `process_status` reports: the one named, or every one this
/// owner retains, in creation order.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProcessStatusResult {
    /// Each process's latest status.
    pub processes: Vec<ProcessStatus>,
}

/// Stop outcome; unfinished cleanup never forgets the record.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProcessStopResult {
    /// Latest status after at most two seconds of waiting for cleanup.
    pub status: ProcessStatus,
    /// Whether output and state were released; the identifier is then invalid.
    pub forgotten: bool,
}
