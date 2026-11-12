//! Shell commands handed to a process session: started there with
//! `run_in_background`, or moved there still running at their timeout.

use roomkit_tools_core::{Output, ToolError};
use tokio_util::sync::CancellationToken;

use crate::capture::Capture;
use crate::persistent_unix::Spawned;
use crate::unix::{Foreground, render};
use crate::{CapturedOutput, ProcessSession, ProcessStatus, ShellResult};

/// Hand `command`, `foreground`'s command at its timeout, to `processes`,
/// which gives it back with the reason when it refuses it.
///
/// Once admitted, the archives of a stream whole so far are released before
/// the process opens its own: their output is all in the result, and a store
/// the shell and the session share would count them against the owner.
pub(crate) async fn take_over(
    processes: &ProcessSession,
    foreground: &Foreground,
    command: Spawned,
    (out, err): (&mut Capture, &mut Capture),
    cancel: &CancellationToken,
) -> Result<ProcessStatus, (ToolError, Box<Spawned>)> {
    let job = crate::persistent::ShellJob {
        owner: foreground.owner.as_deref(),
        command: &foreground.command,
        cwd: &foreground.cwd,
        shown_cwd: &foreground.shown_cwd,
        env: &foreground.env,
    };
    let release = || {
        if let Some((store, _)) = &foreground.outputs {
            out.discard_whole_archive(store);
            err.discard_whole_archive(store);
        }
    };
    processes.take_over(&job, command, release, cancel).await
}

/// The result of a command moved to the background as `status`'s process:
/// what it wrote until then. Its process captures the rest.
pub(crate) async fn moved(
    (mut out, mut err): (Capture, Capture),
    status: ProcessStatus,
    foreground: &Foreground,
) -> Output<ShellResult> {
    tokio::join!(out.finish_archive(), err.finish_archive());
    if let Some((store, _)) = &foreground.outputs {
        out.discard_whole_archive(store);
        err.discard_whole_archive(store);
    }
    let result = ShellResult {
        cwd: foreground.shown_cwd.clone(),
        exit_code: None,
        signal: None,
        timed_out: false,
        timeout_ms: foreground.timeout_ms,
        output_complete: false,
        stdout: out.finish(),
        stderr: err.finish(),
        process_id: Some(status.process_id),
    };
    render(result, None)
}

/// The result of a command started in the background as `status`'s process.
pub(crate) fn started(status: ProcessStatus) -> Output<ShellResult> {
    let text = format!(
        "{} started in the background; read its output with process_read\n",
        status.process_id
    );
    let none = || CapturedOutput {
        text: String::new(),
        truncated: false,
        omitted_bytes: 0,
        total_bytes: 0,
        archive: None,
    };
    let result = ShellResult {
        cwd: status.cwd,
        exit_code: None,
        signal: None,
        timed_out: false,
        timeout_ms: status.timeout_ms,
        output_complete: false,
        stdout: none(),
        stderr: none(),
        process_id: Some(status.process_id),
    };
    Output::new(result, text)
}
