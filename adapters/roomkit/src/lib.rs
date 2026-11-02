//! Individually selectable tools for RoomKit Rust.
//!
//! Each adapter is a RoomKit [`Tool`], the Rust form of
//! Python's Tool protocol. Declare each of [`ReadToolset`], [`ListToolset`],
//! [`GlobToolset`] and [`GrepToolset`], [`WriteToolset`], [`EditToolset`] and
//! [`ShellToolset`] independently in an AI channel's
//! `AiChannelOptions::tools`, as `ToolEntry::Tool(Arc::new(..))`. RoomKit
//! retains argument validation, policies, hooks, timeouts and turn
//! cancellation; dropping a call cancels it. The filesystem implementation
//! remains usable without RoomKit. [`roomkit_owner`] names the owner under
//! which the shell, process and output adapters keep a room and agent's jobs
//! and archives.

mod bridge;
#[cfg(feature = "plans")]
mod plans;
#[cfg(feature = "plans")]
pub use plans::{
    PlanCreateToolset, PlanDeleteToolset, PlanListToolset, PlanReadToolset, PlanUpdateToolset,
};
#[cfg(feature = "docs")]
mod docs;
#[cfg(feature = "docs")]
pub use docs::{DocsListToolset, DocsReadToolset, DocsSearchToolset};
mod output;
mod process;
pub use output::{OutputForgetToolset, OutputReadToolset, ShellToolset};

pub use process::{
    ProcessReadToolset, ProcessStartToolset, ProcessStatusToolset, ProcessStopToolset,
    ProcessWriteToolset,
};

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

/// The owner key of the shell, process and output toolsets for the calls of
/// the agent `channel_id` in the room `room_id`.
///
/// Those toolsets file the jobs and archives of each call under this key:
/// compact JSON of `[room_id, channel_id]`, the key Python's `roomkit_owner`
/// also returns, so neither part can be mistaken for the other. Pass it to
/// [`ProcessManager`](roomkit_tools_process::ProcessManager) and
/// [`OutputStore`](roomkit_tools_process::OutputStore) methods to list, read,
/// stop or release them from the host.
pub fn roomkit_owner(room_id: &str, channel_id: &str) -> String {
    serde_json::Value::from(vec![room_id, channel_id]).to_string()
}

// Each adapter exposes one capability. Composing tools remains the host's
// choice; this macro only shares the RoomKit argument/result/lifecycle bridge.
macro_rules! toolset {
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

toolset!(
    /// Analyze Rust or Python syntax without executing code or following imports.
    CodeOutlineToolset, CodeOutlineTool, outline, "code_outline"
);

toolset!(
    /// The `read` tool with its host-selected root and limits.
    ///
    /// Returns both structured `data` and numbered `text`. Dropping a call
    /// cancels its token; already-issued Tokio file operations may finish in
    /// the blocking pool.
    ReadToolset, ReadTool, read, "read"
);
toolset!(
    /// The non-recursive `list` tool, independently selectable from `read`.
    ///
    /// Returns entry types, workspace-relative paths and continuation offsets.
    /// Dropping a call uses the same lifecycle as [`ReadToolset`].
    ListToolset, ListTool, list, "list"
);
toolset!(
    /// The recursive `glob` path search, independently selectable.
    ///
    /// Keeps the tool's scan and output bounds and structured pagination.
    /// Dropping a call uses the same lifecycle as [`ReadToolset`].
    GlobToolset, GlobTool, glob, "glob"
);
toolset!(
    /// The `grep` text search, independently selectable.
    ///
    /// Keeps matching lines, file paths, skipped-file counts and pagination.
    /// Dropping a call uses the same lifecycle as [`ReadToolset`].
    GrepToolset, GrepTool, grep, "grep"
);

toolset!(
    /// Create or explicitly replace text files using a host-configured `write` tool.
    ///
    /// Share a `MutationWorkspace` with `edit` to serialize mutations. Dropping
    /// a call cancels it before commit; an already-started commit may finish.
    WriteToolset, WriteTool, write, "write"
);
toolset!(
    /// Replace exact text using an independently selected `edit` tool.
    ///
    /// Preserves the tool's ambiguity checks, limits and mutation workspace.
    /// Dropping a call cancels it before commit; an already-started commit may finish.
    EditToolset, EditTool, edit, "edit"
);

toolset!(
    /// Compare two files without writing, with complete bounded hunks.
    DiffToolset, DiffTool, diff, "diff"
);
toolset!(
    /// Prepare or apply exact structured changes under a shared mutation scope.
    ///
    /// A partial application returns structured completed/applied/failure fields.
    /// The host and model must inspect these; a served call is not proof that
    /// every requested change was applied. Dropping a call cancels between effects.
    PatchToolset, PatchTool, patch, "patch"
);

toolset!(
    /// Fetch textual HTTP content under host origin rules and resource bounds.
    /// Dropping a call cancels it; issued blocking conversion may finish later.
    WebFetchToolset, WebFetchTool, fetch, "web_fetch"
);

toolset!(
    /// Search using the host-selected backend, preserving typed results and citations.
    WebSearchToolset, WebSearchTool, search, "web_search"
);

#[cfg(feature = "ocr")]
toolset!(
    /// Transcribe local images using a separately configured recognition backend.
    OcrToolset, OcrTool, ocr, "ocr"
);
