//! Individually selectable tools for RoomKit Rust.
//!
//! Register each of [`ReadToolset`], [`ListToolset`], [`GlobToolset`] and
//! [`GrepToolset`], [`WriteToolset`], [`EditToolset`] and [`ShellToolset`] independently with `AiAgent::with_tools` or
//! `OpenAiAgent::with_tools`. RoomKit retains argument validation, policies,
//! hooks, timeouts and turn cancellation. The filesystem implementation
//! remains usable without RoomKit.

mod process;

pub use process::{
    ProcessReadToolset, ProcessStartToolset, ProcessStatusToolset, ProcessStopToolset,
    ProcessWriteToolset,
};

use roomkit::model::ToolCall;
use roomkit_ai::{ToolContext, ToolDefinition, ToolError, ToolOutput, Toolset};
use roomkit_tools_core::Error;
use roomkit_tools_fs::{
    DiffTool, EditTool, GlobTool, GrepTool, ListTool, PatchTool, ReadTool, WriteTool,
};
use roomkit_tools_process::ShellTool;
use tokio_util::sync::CancellationToken;

// Each adapter exposes one capability. Composing toolsets remains the host's
// choice; this macro only shares the RoomKit argument/result/lifecycle bridge.
macro_rules! toolset {
    ($(#[$doc:meta])* $name:ident, $tool:ident, $method:ident, $wire:literal) => {
        $(#[$doc])*
        #[derive(Debug)]
        pub struct $name {
            tool: $tool,
            closed: CancellationToken,
        }

        impl $name {
            /// Adapt a configured tool without expanding its root or limits.
            pub fn new(tool: $tool) -> Self {
                Self { tool, closed: CancellationToken::new() }
            }
        }

        impl Toolset for $name {
            fn tools(&self) -> Vec<ToolDefinition> {
                let definition = $tool::definition();
                vec![ToolDefinition::new(definition.name, definition.description, definition.input_schema)]
            }

            async fn call(&self, call: ToolCall, _context: ToolContext) -> Result<ToolOutput, ToolError> {
                if call.name != $wire {
                    return Err(ToolError::refused(concat!("This toolset only serves '", $wire, "'.")));
                }
                if self.closed.is_cancelled() {
                    return Err(ToolError::refused(concat!("The ", $wire, " toolset is closed.")));
                }
                let input = roomkit_ai::arguments(&call)?;
                let cancel = self.closed.child_token();
                let _cancel_on_drop = cancel.clone().drop_guard();
                let output = self.tool.$method(input, &cancel).await.map_err(|error| tool_error(&error))?;
                serde_json::to_value(output).map(ToolOutput::Json).map_err(|error| ToolError::raised(&error))
            }

            async fn close(&self) { self.closed.cancel(); }
        }
    };
}

toolset!(
    /// The `read` tool with its host-selected root and limits.
    ///
    /// Returns both structured `data` and numbered `text`. Closing cancels
    /// pending calls and refuses new ones. Dropping a call cancels its token;
    /// already-issued Tokio file operations may finish in the blocking pool.
    ReadToolset, ReadTool, read, "read"
);
toolset!(
    /// The non-recursive `list` tool, independently selectable from `read`.
    ///
    /// Returns entry types, workspace-relative paths and continuation offsets.
    /// Closing or dropping a call uses the same lifecycle as [`ReadToolset`].
    ListToolset, ListTool, list, "list"
);
toolset!(
    /// The recursive `glob` path search, independently selectable.
    ///
    /// Keeps the tool's scan and output bounds and structured pagination.
    /// Closing or dropping a call uses the same lifecycle as [`ReadToolset`].
    GlobToolset, GlobTool, glob, "glob"
);
toolset!(
    /// The `grep` text search, independently selectable.
    ///
    /// Keeps matching lines, file paths, skipped-file counts and pagination.
    /// Closing or dropping a call uses the same lifecycle as [`ReadToolset`].
    GrepToolset, GrepTool, grep, "grep"
);

toolset!(
    /// Create or explicitly replace text files using a host-configured `write` tool.
    ///
    /// Share a `MutationWorkspace` with `edit` to serialize mutations. Closing
    /// cancels calls before commit; an already-started commit may finish.
    WriteToolset, WriteTool, write, "write"
);
toolset!(
    /// Replace exact text using an independently selected `edit` tool.
    ///
    /// Preserves the tool's ambiguity checks, limits and mutation workspace.
    /// Closing cancels calls before commit; an already-started commit may finish.
    EditToolset, EditTool, edit, "edit"
);

toolset!(
    /// Foreground shell execution with host-selected limits and initial root.
    ///
    /// Commands inherit host permissions; this adapter is not a sandbox. Closing
    /// or dropping a call requests process-group cleanup in the owned worker.
    ShellToolset, ShellTool, shell, "shell"
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
    /// every requested change was applied. Closing cancels between effects.
    PatchToolset, PatchTool, patch, "patch"
);

fn tool_error(error: &Error) -> ToolError {
    match error {
        Error::InvalidInput(_) | Error::Refused(_) => ToolError::refused(error.to_string()),
        Error::LimitExceeded(_) | Error::Unsupported(_) | Error::Cancelled => {
            ToolError::failed(error.to_string())
        }
        // Diagnostic causes reach RoomKit's observers, never the model.
        _ => ToolError::raised(error),
    }
}
