//! Output identities come from trusted RoomKit conversation and agent context.

use roomkit::model::ToolCall;
use roomkit_ai::{ToolContext, ToolDefinition, ToolError, ToolOutput, Toolset};
use roomkit_tools_process::{OutputStore, ShellTool};
use tokio_util::sync::CancellationToken;

macro_rules! owned_toolset {
    ($(#[$doc:meta])* $name:ident, $tool:ident, $definition:ident, $method:ident, $wire:literal) => {
        $(#[$doc])*
        #[derive(Debug)]
        pub struct $name { tool: $tool, closed: CancellationToken }

        impl $name {
            /// Select one capability. The host closes the shared `OutputStore`
            /// after its shell calls and process managers have finished.
            pub fn new(tool: $tool) -> Self {
                Self { tool, closed: CancellationToken::new() }
            }
        }

        impl Toolset for $name {
            fn tools(&self) -> Vec<ToolDefinition> {
                let definition = $tool::$definition();
                vec![ToolDefinition::new(definition.name, definition.description, definition.input_schema)]
            }

            async fn call(&self, call: ToolCall, context: ToolContext) -> Result<ToolOutput, ToolError> {
                if call.name != $wire || self.closed.is_cancelled() {
                    return Err(ToolError::refused(concat!("Unavailable tool: ", $wire)));
                }
                let owner = serde_json::to_string(&(context.room_id, context.channel_id))
                    .map_err(|error| ToolError::raised(&error))?;
                let input = roomkit_ai::arguments(&call)?;
                let cancel = self.closed.child_token();
                let _cancel_on_drop = cancel.clone().drop_guard();
                let result = self.tool.$method(&owner, input, &cancel).await
                    .map_err(|error| super::tool_error(&error))?;
                serde_json::to_value(result).map(ToolOutput::Json).map_err(|error| ToolError::raised(&error))
            }

            async fn close(&self) { self.closed.cancel(); }
        }
    };
}

owned_toolset!(
    /// Foreground shell with optional archives scoped to the calling room and agent.
    /// Closing/dropping calls requests process cleanup, not archive deletion.
    ShellToolset, ShellTool, definition, shell_owned, "shell"
);
owned_toolset!(
    /// Read retained output owned by the current conversation and agent.
    OutputReadToolset, OutputStore, read_definition, read, "output_read"
);
owned_toolset!(
    /// Explicitly release a finished output owned by the current conversation and agent.
    OutputForgetToolset, OutputStore, forget_definition, forget, "output_forget"
);
