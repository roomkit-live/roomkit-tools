//! Persistent processes scoped by trusted RoomKit conversation and agent IDs.

use roomkit::model::Metadata;
use roomkit_ai::{AiTool, Tool, ToolFuture};
use roomkit_tools_process::ProcessSession;

use crate::bridge::{ai_tool, current_owner, serve, tool_error};

macro_rules! process_toolset {
    ($(#[$doc:meta])* $name:ident, $definition:ident, $method:ident, $wire:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug)]
        pub struct $name {
            manager: ProcessSession,
        }

        impl $name {
            /// Register one capability over a shared process session.
            ///
            /// Pass clones of the same session to the selected process
            /// adapters. The host closes that session once its agents are done.
            pub fn new(manager: ProcessSession) -> Self {
                Self { manager }
            }
        }

        impl Tool for $name {
            fn definition(&self) -> AiTool {
                ai_tool(ProcessSession::$definition())
            }

            fn handler(&self, name: String, arguments: Metadata) -> ToolFuture {
                let manager = self.manager.clone();
                // Neither owner nor room identity is accepted from model arguments.
                let owner = || current_owner("process");
                serve($wire, name, arguments, owner, move |owner, input, cancel| async move {
                    manager.$method(&owner, input, &cancel).await.map_err(|error| tool_error(&error))
                })
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
