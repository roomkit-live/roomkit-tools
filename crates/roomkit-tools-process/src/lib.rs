//! Independent shell and persistent processes with bounded output and owned cleanup.
//!
//! [`ShellTool`] runs trusted commands with the host process's permissions.
//! A checked working directory is not a filesystem or network sandbox. Unix
//! process groups are supported; other platforms return an explicit refusal
//! until equivalent cleanup is implemented. No RoomKit or Python is required.

mod capture;
mod journal;
mod persistent;
#[cfg(unix)]
mod persistent_unix;
mod process_types;
mod shell;
#[cfg(unix)]
mod unix;

pub use capture::CapturedOutput;
pub use persistent::ProcessManager;
pub use process_types::*;
pub use shell::{ShellInput, ShellLimits, ShellResult, ShellTool};
