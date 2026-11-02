//! Independent shell and persistent processes with bounded output and owned cleanup.
//!
//! [`ShellTool`] runs trusted commands with the host process's permissions.
//! A checked working directory is not a filesystem or network sandbox. Commands
//! receive only the environment the host selects with [`CommandEnv`]. On Unix
//! each command leads a new session, without a controlling terminal, and its
//! process group is stopped on cleanup; other platforms return an explicit
//! refusal until equivalent cleanup is implemented. No RoomKit or Python is required.

// The workspace's only unsafe code is the `setsid` call between fork and exec
// in `unix`, allowed on that one function. Every other module forbids it.
#![deny(unsafe_code)]

#[forbid(unsafe_code)]
mod capture;
#[forbid(unsafe_code)]
mod env;
#[forbid(unsafe_code)]
mod journal;
#[forbid(unsafe_code)]
mod output;
#[forbid(unsafe_code)]
mod persistent;
#[cfg(unix)]
#[forbid(unsafe_code)]
mod persistent_unix;
#[forbid(unsafe_code)]
mod process_types;
#[forbid(unsafe_code)]
mod shell;
#[cfg(unix)]
mod unix;

pub use capture::CapturedOutput;
pub use env::{CommandEnv, DEFAULT_INHERITED_ENV};
pub use output::{
    OutputArchive, OutputForgetInput, OutputForgetResult, OutputLimits, OutputPage,
    OutputReadInput, OutputStore,
};
pub use persistent::{ObservedProcess, ProcessSession};
pub use process_types::*;
pub use shell::{ShellInput, ShellLimits, ShellResult, ShellTool};

// The README's example is compiled and run as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
