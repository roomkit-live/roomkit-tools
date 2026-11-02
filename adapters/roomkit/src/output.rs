//! Output identities come from trusted RoomKit conversation and agent context.

use std::sync::Arc;

use roomkit::model::Metadata;
use roomkit_ai::{AiTool, Tool, ToolFuture};
use roomkit_tools_process::{OutputStore, ShellTool};

use crate::bridge::{ai_tool, current_owner, serve, tool_error};

macro_rules! owned_toolset {
    ($(#[$doc:meta])* $name:ident, $tool:ident, $definition:ident, $method:ident, $wire:literal, $family:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug)]
        pub struct $name {
            tool: Arc<$tool>,
        }

        impl $name {
            /// Select one capability. The host closes the shared `OutputStore`
            /// after its shell calls and process managers have finished.
            pub fn new(tool: $tool) -> Self {
                Self { tool: Arc::new(tool) }
            }
        }

        impl Tool for $name {
            fn definition(&self) -> AiTool {
                ai_tool($tool::$definition())
            }

            fn handler(&self, name: String, arguments: Metadata) -> ToolFuture {
                let tool = Arc::clone(&self.tool);
                serve($wire, name, arguments, move |input, cancel| async move {
                    let owner = current_owner($family)?;
                    tool.$method(&owner, input, &cancel).await.map_err(|error| tool_error(&error))
                })
            }
        }
    };
}

owned_toolset!(
    /// Foreground shell with optional archives scoped to the calling room and agent.
    /// Dropping a call requests process cleanup, not archive deletion.
    ShellToolset, ShellTool, definition, shell_owned, "shell", "shell"
);
owned_toolset!(
    /// Read retained output owned by the current conversation and agent.
    OutputReadToolset, OutputStore, read_definition, read, "output_read", "output"
);
owned_toolset!(
    /// Explicitly release a finished output owned by the current conversation and agent.
    OutputForgetToolset, OutputStore, forget_definition, forget, "output_forget", "output"
);
