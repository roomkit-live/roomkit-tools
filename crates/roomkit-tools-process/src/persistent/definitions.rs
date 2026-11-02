//! The model-facing definitions of the persistent process tools.

use roomkit_tools_core::Definition;

use super::ProcessSession;
use crate::{
    ProcessReadInput, ProcessReadResult, ProcessStartInput, ProcessStatus, ProcessStatusInput,
    ProcessStopInput, ProcessStopResult, ProcessWriteInput, ProcessWriteResult,
};

impl ProcessSession {
    /// Derived schema for starting a process.
    pub fn start_definition() -> Definition {
        Definition::new::<ProcessStartInput, ProcessStatus>(
            "process_start",
            "Start /bin/sh -c with open stdin and a bounded lifetime. Returns immediately with process_id. Use process_read for output, process_write for input, process_stop for cleanup. Processes belong to this conversation; host permissions apply, no sandbox or PTY.",
        )
    }
    /// Derived schema for inspecting or listing owned processes.
    pub fn status_definition() -> Definition {
        Definition::new::<ProcessStatusInput, Vec<ProcessStatus>>(
            "process_status",
            "Inspect process_id or omit it to list this conversation's processes, including retained completed jobs. Read-only: does not write, close stdin or stop jobs. State finished means cleanup and capture ended. Report the actual end, exit_code, signal and error: exit_code 0 with signal null means normal successful exit, not signal termination.",
        )
    }
    /// Derived schema for cursor-based output reading.
    pub fn read_definition() -> Definition {
        Definition::new::<ProcessReadInput, ProcessReadResult>(
            "process_read",
            "Read available stdout and stderr without writing input, closing stdin or stopping the process. Call this tool directly to read a running job; no preparatory process_write or EOF is needed. Use independent zero-based byte offsets, initially 0, then each next_offset. Reading does not consume output. Empty output means no new bytes yet, not completion. omitted_bytes reports eviction. Each stream's bytes and total_bytes are authoritative; the outer rendered text contains formatting not produced by the process.",
        )
    }
    /// Derived schema for writing or closing stdin.
    pub fn write_definition() -> Definition {
        Definition::new::<ProcessWriteInput, ProcessWriteResult>(
            "process_write",
            "Send requested text to process stdin, or close stdin when EOF was requested. Encode an actual newline as the JSON escape \\n when requested; no newline is added automatically. Keep close_stdin false unless closing input is intended: EOF may terminate the process. Do not call this tool to read or poll output; use process_read directly. Inspect complete and bytes_written, since input may be partial on timeout/cancellation; never blindly replay it. If stdin_closed is already true, no further close is needed.",
        )
    }
    /// Derived schema for terminating and optionally forgetting a process.
    pub fn stop_definition() -> Definition {
        Definition::new::<ProcessStopInput, ProcessStopResult>(
            "process_stop",
            "Stop an owned process group immediately with SIGKILL: no graceful shutdown, so the command cannot save state or clean up. Awaits bounded cleanup. Set forget=true to release the completed process record and all retained output. Check status.state and forgotten. Repeated stop of a retained completed job is safe.",
        )
    }
}
