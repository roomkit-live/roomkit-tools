//! Individually selectable tools for RoomKit Rust.
//!
//! Each adapter is a RoomKit [`Tool`], the Rust form of
//! Python's Tool protocol. Declare each of [`Read`], [`List`],
//! [`Glob`] and [`Grep`], [`Write`], [`Edit`] and
//! [`Shell`] independently in an AI channel's
//! `AiChannelOptions::tools`, as `ToolEntry::Tool(Arc::new(..))`. RoomKit
//! retains argument validation, policies, hooks, timeouts and turn
//! cancellation; dropping a call cancels it. The filesystem implementation
//! remains usable without RoomKit. [`roomkit_owner`] names the owner under
//! which the shell, process and output adapters keep a room and agent's jobs
//! and archives.
//!
//! Each adapter has the name of its Python RoomKit tool (`Read`, `Write`,
//! `Shell`, `ProcessStart`, ...). Where `std::io::Read` or `std::io::Write`
//! is in scope, name the adapter by path, `roomkit_tools_roomkit::Read`.

mod bridge;
#[cfg(feature = "plans")]
mod plans;
#[cfg(feature = "plans")]
pub use plans::{PlanCreate, PlanDelete, PlanList, PlanRead, PlanUpdate};
#[cfg(feature = "docs")]
mod docs;
#[cfg(feature = "docs")]
pub use docs::{DocsList, DocsRead, DocsSearch};
mod output;
mod process;
pub use output::{OutputForget, OutputRead, Shell};

pub use process::{ProcessRead, ProcessStart, ProcessStatus, ProcessStop, ProcessWrite};

use std::sync::Arc;

use roomkit::model::Metadata;
use roomkit_ai::{AiTool, Tool, ToolFuture};
use roomkit_tools_code::CodeOutlineTool;
use roomkit_tools_fs::{
    DiffTool, EditTool, GlobTool, GrepTool, ListTool, PatchTool, ReadTool, WriteTool,
};
#[cfg(feature = "ocr")]
use roomkit_tools_ocr::OcrTool;
use roomkit_tools_search::WebSearchTool;
use roomkit_tools_web::WebFetchTool;

use bridge::{ai_tool, anyone, serve, tool_error};

/// The owner key of the shell, process and output adapters for the calls of
/// the agent `channel_id` in the room `room_id`.
///
/// Those adapters file the jobs and archives of each call under this key:
/// compact JSON of `[room_id, channel_id]`, the key Python's `roomkit_owner`
/// also returns, so neither part can be mistaken for the other. Pass it to
/// [`ProcessSession`](roomkit_tools_process::ProcessSession) and
/// [`OutputStore`](roomkit_tools_process::OutputStore) methods to list, read,
/// stop or release them from the host.
pub fn roomkit_owner(room_id: &str, channel_id: &str) -> String {
    serde_json::Value::from(vec![room_id, channel_id]).to_string()
}

// Each adapter exposes one capability. Composing tools remains the host's
// choice; this macro only shares the RoomKit argument/result/lifecycle bridge.
macro_rules! adapter {
    ($(#[$doc:meta])* $name:ident, $tool:ident, $method:ident, $wire:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug)]
        pub struct $name {
            tool: Arc<$tool>,
        }

        impl $name {
            /// Adapt a configured tool without expanding its root or limits.
            pub fn new(tool: $tool) -> Self {
                Self { tool: Arc::new(tool) }
            }
        }

        impl Tool for $name {
            fn definition(&self) -> AiTool {
                ai_tool($tool::definition())
            }

            fn handler(&self, name: String, arguments: Metadata) -> ToolFuture {
                let tool = Arc::clone(&self.tool);
                serve($wire, name, arguments, anyone, move |(), input, cancel| async move {
                    tool.$method(input, &cancel).await.map_err(|error| tool_error(&error))
                })
            }
        }
    };
}

adapter!(
    /// Analyze Rust or Python syntax without executing code or following imports.
    CodeOutline, CodeOutlineTool, outline, "code_outline"
);

adapter!(
    /// The `read` tool with its host-selected root and limits.
    ///
    /// Returns both structured `data` and numbered `text`. Dropping a call
    /// cancels its token; already-issued Tokio file operations may finish in
    /// the blocking pool.
    Read, ReadTool, read, "read"
);
adapter!(
    /// The non-recursive `list` tool, independently selectable from `read`.
    ///
    /// Returns entry types, workspace-relative paths and continuation offsets.
    /// Dropping a call uses the same lifecycle as [`Read`].
    List, ListTool, list, "list"
);
adapter!(
    /// The recursive `glob` path search, independently selectable.
    ///
    /// Keeps the tool's scan and output bounds and structured pagination.
    /// Dropping a call uses the same lifecycle as [`Read`].
    Glob, GlobTool, glob, "glob"
);
adapter!(
    /// The `grep` text search, independently selectable.
    ///
    /// Keeps matching lines, file paths, skipped-file counts and pagination.
    /// Dropping a call uses the same lifecycle as [`Read`].
    Grep, GrepTool, grep, "grep"
);

adapter!(
    /// Create or explicitly replace text files using a host-configured `write` tool.
    ///
    /// Share a `Workspace` with `edit` to serialize mutations. Dropping
    /// a call cancels it before commit; an already-started commit may finish.
    Write, WriteTool, write, "write"
);
adapter!(
    /// Replace exact text using an independently selected `edit` tool.
    ///
    /// Preserves the tool's ambiguity checks, limits and mutation workspace.
    /// Dropping a call cancels it before commit; an already-started commit may finish.
    Edit, EditTool, edit, "edit"
);

adapter!(
    /// Compare two files without writing, with complete bounded hunks.
    Diff, DiffTool, diff, "diff"
);
adapter!(
    /// Prepare or apply exact structured changes under a shared mutation scope.
    ///
    /// A partial application returns structured completed/applied/failure fields.
    /// The host and model must inspect these; a served call is not proof that
    /// every requested change was applied. Dropping a call cancels between effects.
    Patch, PatchTool, patch, "patch"
);

adapter!(
    /// Fetch textual HTTP content under host origin rules and resource bounds.
    /// Dropping a call cancels it; issued blocking conversion may finish later.
    WebFetch, WebFetchTool, fetch, "web_fetch"
);

adapter!(
    /// Search using the host-selected backend, preserving typed results and citations.
    WebSearch, WebSearchTool, search, "web_search"
);

#[cfg(feature = "ocr")]
adapter!(
    /// Transcribe local images using a separately configured recognition backend.
    Ocr, OcrTool, ocr, "ocr"
);
