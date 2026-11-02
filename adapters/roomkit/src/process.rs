//! Persistent processes scoped by trusted RoomKit conversation and agent IDs.

use roomkit::model::Metadata;
use roomkit_ai::{AiTool, Tool, ToolFuture};
use roomkit_tools_process::ProcessSession;

use crate::bridge::{ai_tool, current_owner, serve, tool_error};

macro_rules! process_adapter {
    ($(#[$doc:meta])* $name:ident, $definition:ident, $method:ident, $wire:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug)]
        pub struct $name {
            session: ProcessSession,
        }

        impl $name {
            /// Register one capability over a shared process session.
            ///
            /// Pass clones of the same session to the selected process
            /// adapters. The host closes that session once its agents are done.
            pub fn new(session: ProcessSession) -> Self {
                Self { session }
            }
        }

        impl Tool for $name {
            fn definition(&self) -> AiTool {
                ai_tool(ProcessSession::$definition())
            }

            fn handler(&self, name: String, arguments: Metadata) -> ToolFuture {
                let session = self.session.clone();
                // Neither owner nor room identity is accepted from model arguments.
                let owner = || current_owner("process");
                serve($wire, name, arguments, owner, move |owner, input, cancel| async move {
                    session.$method(Some(&owner), input, &cancel).await.map_err(|error| tool_error(&error))
                })
            }
        }
    };
}

process_adapter!(
    /// Start processes owned by the calling conversation and agent.
    ProcessStart, start_definition, start, "process_start"
);
process_adapter!(
    /// Inspect or list only the calling conversation/agent's retained jobs.
    ProcessStatus, status_definition, status, "process_status"
);
process_adapter!(
    /// Read independently paged stdout and stderr from an owned process.
    ProcessRead, read_definition, read, "process_read"
);
process_adapter!(
    /// Write stdin or send EOF to an owned process, with partial-effect reporting.
    ProcessWrite, write_definition, write, "process_write"
);
process_adapter!(
    /// Stop and optionally forget an owned process and its retained output.
    ProcessStop, stop_definition, stop, "process_stop"
);
