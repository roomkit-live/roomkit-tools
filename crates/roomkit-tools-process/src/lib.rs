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

/// A working directory below `root` as the model sees it: relative to the
/// workspace, `.` for the root. A directory outside the root is refused,
/// never shown as the host's path.
fn relative_cwd(
    root: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<String, roomkit_tools_core::ToolError> {
    let relative = cwd.strip_prefix(root).map_err(|_| {
        roomkit_tools_core::ToolError::AccessRefused("working directory is outside the root".into())
    })?;
    let relative = relative.to_string_lossy();
    Ok(if relative.is_empty() {
        ".".into()
    } else {
        relative.replace(std::path::MAIN_SEPARATOR, "/")
    })
}

#[cfg(test)]
mod relative_cwd_tests {
    use std::path::Path;

    #[test]
    fn a_directory_outside_the_root_is_refused_never_shown() {
        let root = Path::new("/srv/workspace");
        assert_eq!(super::relative_cwd(root, root).unwrap(), ".");
        assert_eq!(
            super::relative_cwd(root, &root.join("src/bin")).unwrap(),
            "src/bin"
        );
        assert!(matches!(
            super::relative_cwd(root, Path::new("/home/someone/project")),
            Err(roomkit_tools_core::ToolError::AccessRefused(_))
        ));
    }
}

// The README's example is compiled and run as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
