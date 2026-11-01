use std::num::{NonZeroU64, NonZeroUsize};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Start a command that survives subsequent tool calls, within a host-owned scope.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessStartInput {
    /// Shell command passed to `/bin/sh -c` (no login shell or PTY).
    pub command: String,
    /// Initial directory relative to the configured root.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Maximum lifetime, capped by the host. Default: five minutes.
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

/// Inspect one process or list all processes belonging to this owner.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessStatusInput {
    /// Omit to list the owner's running and retained completed processes,
    /// in creation order.
    #[serde(default)]
    pub process_id: Option<String>,
}

/// Read bounded output pages using independent byte cursors for each stream.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessReadInput {
    /// Identifier returned by `process_start` in this scope.
    pub process_id: String,
    /// Zero-based stdout byte cursor; use the previous stdout `next_offset`.
    #[serde(default)]
    pub stdout_offset: u64,
    /// Zero-based stderr byte cursor; use the previous stderr `next_offset`.
    #[serde(default)]
    pub stderr_offset: u64,
    /// Bytes per stream in this page, capped by the host.
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

/// Send UTF-8 bytes exactly as supplied; a newline is never implicit.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessWriteInput {
    /// Owned process identifier.
    pub process_id: String,
    /// Exact text to send. A requested newline must be encoded as JSON `\n`;
    /// the tool adds no newline. Omit text only when requesting EOF alone.
    #[serde(default)]
    pub text: String,
    /// Close stdin after sending all supplied bytes, delivering EOF. This may
    /// terminate the command. Keep false for sending text or reading output;
    /// set true only when closing input is intended. Reading never requires EOF.
    #[serde(default)]
    pub close_stdin: bool,
}

/// Stop a process group with SIGKILL, optionally releasing its retained state
/// and output. There is no graceful phase before the kill.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessStopInput {
    /// Owned process identifier.
    pub process_id: String,
    /// Remove the completed record and output after cleanup. Default: false.
    #[serde(default)]
    pub forget: bool,
}

/// Host-selected aggregate, per-process and per-call bounds.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
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
    /// Canonical initial working directory (not confinement).
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

/// Stop outcome; unfinished cleanup never forgets the record.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProcessStopResult {
    /// Latest status after at most two seconds of waiting for cleanup.
    pub status: ProcessStatus,
    /// Whether output and state were released; the identifier is then invalid.
    pub forgotten: bool,
}
