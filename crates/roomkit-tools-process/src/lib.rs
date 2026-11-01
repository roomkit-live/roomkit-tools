//! Independent shell and persistent processes with bounded output and owned cleanup.
//!
//! [`ShellTool`] runs trusted commands with the host process's permissions.
//! A checked working directory is not a filesystem or network sandbox. Commands
//! receive only the environment the host selects with [`CommandEnv`]. On Unix
//! each command leads a new session, without a controlling terminal, and its
//! process group is stopped on cleanup; other platforms return an explicit
//! refusal until equivalent cleanup is implemented. No RoomKit or Python is required.

mod capture;
mod env;
mod journal;
mod output;
mod persistent;
#[cfg(unix)]
mod persistent_unix;
mod process_types;
mod shell;
#[cfg(unix)]
mod unix;

pub use capture::CapturedOutput;
pub use env::{CommandEnv, DEFAULT_INHERITED_ENV};
pub use output::{
    OutputArchive, OutputForgetInput, OutputForgetResult, OutputLimits, OutputPage,
    OutputReadInput, OutputStore,
};
pub use persistent::{ObservedProcess, ProcessManager};
pub use process_types::*;
pub use shell::{ShellInput, ShellLimits, ShellResult, ShellTool};
