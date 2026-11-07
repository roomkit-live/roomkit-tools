//! The model-facing definitions of the persistent process tools.

use roomkit_tools_core::Definition;

use super::ProcessSession;
use crate::{
    ProcessReadInput, ProcessReadResult, ProcessStartInput, ProcessStatus, ProcessStatusInput,
    ProcessStatusResult, ProcessStopInput, ProcessStopResult, ProcessWriteInput,
    ProcessWriteResult,
};

impl ProcessSession {
    /// Derived schema for starting a process.
    pub fn start_definition() -> Definition {
        Definition::new::<ProcessStartInput, ProcessStatus>(
            "process_start",
            "Start a background command with /bin/sh -c and open stdin; returns process_id at once. Use it with process_read, process_write and process_stop. Pipes, not a PTY. Not a sandbox.",
        )
    }
    /// Derived schema for inspecting or listing owned processes.
    pub fn status_definition() -> Definition {
        Definition::new::<ProcessStatusInput, ProcessStatusResult>(
            "process_status",
            "Show one process, or list this conversation's processes, finished ones included. Read-only. finished means cleanup and capture ended. Report end, exit_code, signal and error as returned: exit_code 0 with signal null is a normal exit.",
        )
    }
    /// Derived schema for cursor-based output reading.
    pub fn read_definition() -> Definition {
        Definition::new::<ProcessReadInput, ProcessReadResult>(
            "process_read",
            "Read a process's stdout and stderr without side effects; no process_write or EOF is needed first. Pass each stream's next_offset as its offset (start at 0); reading does not consume output. Empty means no new bytes yet, not exit. omitted_bytes counts evicted bytes.",
        )
    }
    /// Derived schema for writing or closing stdin.
    pub fn write_definition() -> Definition {
        Definition::new::<ProcessWriteInput, ProcessWriteResult>(
            "process_write",
            "Write text to a process's stdin, optionally closing it. Not needed to read output: use process_read. Input may be partial on timeout: check complete and bytes_written before any resend.",
        )
    }
    /// Derived schema for terminating and optionally forgetting a process.
    pub fn stop_definition() -> Definition {
        Definition::new::<ProcessStopInput, ProcessStopResult>(
            "process_stop",
            "Kill a process group with SIGKILL (no graceful shutdown) and wait for cleanup. Safe to repeat on a finished process. Check status.state and forgotten.",
        )
    }
}
