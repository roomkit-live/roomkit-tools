//! Persistent processes scoped by trusted RoomKit conversation and agent IDs.

use roomkit::model::ToolCall;
use roomkit_ai::{ToolContext, ToolDefinition, ToolError, ToolOutput, Toolset};
use roomkit_tools_process::ProcessManager;
use tokio_util::sync::CancellationToken;

macro_rules! process_toolset {
    ($(#[$doc:meta])* $name:ident, $definition:ident, $method:ident, $wire:literal) => {
        $(#[$doc])*
        #[derive(Debug)]
        pub struct $name { manager: ProcessManager, closed: CancellationToken }

        impl $name {
            /// Register one capability over a shared process manager.
            ///
            /// Pass clones of the same manager to the selected process adapters.
            /// Closing any of these adapters shuts down their shared manager.
            pub fn new(manager: ProcessManager) -> Self {
                Self { manager, closed: CancellationToken::new() }
            }
        }

        impl Toolset for $name {
            fn tools(&self) -> Vec<ToolDefinition> {
                let definition = ProcessManager::$definition();
                vec![ToolDefinition::new(definition.name, definition.description, definition.input_schema)]
            }

            async fn call(&self, call: ToolCall, context: ToolContext) -> Result<ToolOutput, ToolError> {
                if call.name != $wire || self.closed.is_cancelled() {
                    return Err(ToolError::refused(concat!("Unavailable tool: ", $wire)));
                }
                // A tuple encoding avoids ambiguous string concatenation. Neither
                // owner nor room identity is accepted from model arguments.
                let owner = serde_json::to_string(&(context.room_id, context.channel_id))
                    .map_err(|error| ToolError::raised(&error))?;
                let input = roomkit_ai::arguments(&call)?;
                let cancel = self.closed.child_token();
                let _cancel_on_drop = cancel.clone().drop_guard();
                let result = self.manager.$method(&owner, input, &cancel).await
                    .map_err(|error| super::tool_error(&error))?;
                serde_json::to_value(result).map(ToolOutput::Json).map_err(|error| ToolError::raised(&error))
            }

            async fn close(&self) {
                self.closed.cancel();
                let _finished = self.manager.close().await;
            }
        }
    };
}

process_toolset!(
    /// Start processes owned by the calling conversation and agent.
    ProcessStartToolset, start_definition, start, "process_start"
);
process_toolset!(
    /// Inspect or list only the calling conversation/agent's retained jobs.
    ProcessStatusToolset, status_definition, status, "process_status"
);
process_toolset!(
    /// Read independently paged stdout and stderr from an owned process.
    ProcessReadToolset, read_definition, read, "process_read"
);
process_toolset!(
    /// Write stdin or send EOF to an owned process, with partial-effect reporting.
    ProcessWriteToolset, write_definition, write, "process_write"
);
process_toolset!(
    /// Stop and optionally forget an owned process and its retained output.
    ProcessStopToolset, stop_definition, stop, "process_stop"
);
