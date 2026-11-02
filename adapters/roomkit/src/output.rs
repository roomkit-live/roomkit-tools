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
            /// after its shell calls and process sessions have finished.
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
                let owner = || current_owner($family);
                serve($wire, name, arguments, owner, move |owner, input, cancel| async move {
                    tool.$method(Some(&owner), input, &cancel).await.map_err(|error| tool_error(&error))
                })
            }
        }
    };
}

/// Foreground shell with optional archives scoped to the calling room and agent.
///
/// With an output store, archives belong to the calling room and agent, and a
/// call outside a RoomKit tool call is refused; without one, the shell keeps
/// no archive and needs no caller, as Python's `Shell`. Dropping a call
/// requests process cleanup, not archive deletion.
#[derive(Clone, Debug)]
pub struct ShellToolset {
    tool: Arc<ShellTool>,
}

impl ShellToolset {
    /// Select the shell. The host closes its `OutputStore`, if any, after its
    /// shell calls and process sessions have finished.
    pub fn new(tool: ShellTool) -> Self {
        Self {
            tool: Arc::new(tool),
        }
    }
}

impl Tool for ShellToolset {
    fn definition(&self) -> AiTool {
        ai_tool(ShellTool::definition())
    }

    fn handler(&self, name: String, arguments: Metadata) -> ToolFuture {
        let tool = Arc::clone(&self.tool);
        let archives = tool.has_output_store();
        let owner = move || archives.then(|| current_owner("shell archive")).transpose();
        serve(
            "shell",
            name,
            arguments,
            owner,
            move |owner, input, cancel| async move {
                let output = match owner {
                    Some(owner) => tool.shell_owned(&owner, input, &cancel).await,
                    None => tool.shell(input, &cancel).await,
                };
                output.map_err(|error| tool_error(&error))
            },
        )
    }
}

owned_toolset!(
    /// Read retained output owned by the current conversation and agent.
    OutputReadToolset, OutputStore, read_definition, read, "output_read", "output"
);
owned_toolset!(
    /// Explicitly release a finished output owned by the current conversation and agent.
    OutputForgetToolset, OutputStore, forget_definition, forget, "output_forget", "output"
);
