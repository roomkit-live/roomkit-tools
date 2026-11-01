//! Public limits, inputs and results of the output archive tools.

use std::num::NonZeroUsize;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Host-owned disk and paging limits. Reserved queued bytes also count as usage.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct OutputLimits {
    /// Maximum retained streams, including empty and failed copies. Default: 64.
    pub max_archives: usize,
    /// Maximum retained streams for one owner. Default: 16.
    pub max_archives_per_owner: usize,
    /// Maximum prefix retained for one stream. Default: 8 MiB.
    pub max_stream_bytes: u64,
    /// Aggregate stored/reserved bytes. Default: 64 MiB.
    pub max_total_bytes: u64,
    /// Maximum raw bytes per read. Default: 16 KiB.
    pub max_read_bytes: usize,
    /// Maximum wait for writer capacity or final flush, 1–60,000 ms. Default: 1 s.
    pub write_timeout_ms: u64,
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            max_archives: 64,
            max_archives_per_owner: 16,
            max_stream_bytes: 8 * 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
            max_read_bytes: 16 * 1024,
            write_timeout_ms: 1000,
        }
    }
}

/// A snapshot of one disk copy. An incomplete copy always describes a prefix.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct OutputArchive {
    /// Scope-local identifier; absent if no record could be allocated.
    pub archive_id: Option<String>,
    /// Bytes observed from the pipe, including bytes not retained on disk.
    pub observed_bytes: u64,
    /// Confirmed, readable prefix length; excludes queued or in-flight writes.
    pub stored_bytes: u64,
    /// The source pipe reached EOF.
    pub eof: bool,
    /// The disk writer has stopped; independent of source/process completion.
    pub finished: bool,
    /// EOF, writer finished, no error, and all observed bytes were stored.
    pub complete: bool,
    /// Safe reason for a partial copy, such as quota, I/O, closure or timeout.
    pub error: Option<String>,
}

/// Read a retained output prefix without consuming or modifying it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputReadInput {
    /// Identifier returned by shell/process archive metadata, not a file path.
    pub archive_id: String,
    /// Zero-based raw byte offset, default 0.
    #[serde(default)]
    pub offset: u64,
    /// Requested raw page size, capped by the host.
    #[serde(default)]
    pub max_bytes: Option<NonZeroUsize>,
}

/// A raw archive page and its lossy UTF-8 display.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OutputPage {
    /// Archive state observed before the page read.
    pub archive: OutputArchive,
    /// First returned byte offset.
    pub offset: u64,
    /// Offset for the next read.
    pub next_offset: u64,
    /// More confirmed bytes were available at this snapshot.
    pub has_more: bool,
    /// Exact raw bytes; join these before decoding split Unicode.
    pub bytes: Vec<u8>,
    /// UTF-8 display with replacement for invalid or split sequences.
    pub text: String,
}

/// Release a finished archive. Active capture is never implicitly stopped.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputForgetInput {
    /// Owned archive identifier.
    pub archive_id: String,
}

/// Confirmation that a retained identifier was released.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OutputForgetResult {
    /// Released identifier; never reused by this store.
    pub archive_id: String,
    /// True after successful release.
    pub forgotten: bool,
}
