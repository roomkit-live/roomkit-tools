//! Be told when background jobs end, without polling them and without a model:
//! a process, and the job a shell command left running.
#![forbid(unsafe_code)]
#![expect(
    clippy::print_stdout,
    reason = "the example prints its verified result"
)]

use std::error::Error;
use std::pin::Pin;
use std::time::Duration;

use futures_core::Stream;
use roomkit_tools_process::{
    JobEnd, JobEnds, JobTool, ProcessEnd, ProcessSession, ProcessStartInput, ShellInput,
    ShellLimits, ShellTool,
};
use tokio_util::sync::CancellationToken;

async fn next(ends: &mut JobEnds) -> Result<JobEnd, Box<dyn Error>> {
    let next = std::future::poll_fn(|context| Pin::new(&mut *ends).poll_next(context));
    tokio::time::timeout(Duration::from_secs(5), next)
        .await?
        .ok_or_else(|| "the ends stopped".into())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let cancel = CancellationToken::new();
    let session = ProcessSession::new(root.path()).await?;
    let shell = ShellTool::new(root.path())
        .await?
        .with_limits(ShellLimits {
            keep_background_jobs: true,
            ..ShellLimits::default()
        })?;
    // Subscribe first: an end before the subscription is not told.
    let mut process_ends = session.ends();
    let mut shell_ends = shell.ends();

    let build = ProcessStartInput::new("sleep 0.2; echo built; exit 3");
    session.start(Some("example-demo"), build, &cancel).await?;
    let server = ShellInput::new("(sleep 0.3; echo served > served.txt) >/dev/null 2>&1 &");
    let returned = shell.shell_owned("example-demo", server, &cancel).await?;
    if returned.data.exit_code != Some(0) {
        return Err("the shell command did not return".into());
    }

    let process = next(&mut process_ends).await?;
    let jobs = next(&mut shell_ends).await?;
    if (process.tool, process.end, process.exit_code)
        != (JobTool::Process, ProcessEnd::Exited, Some(3))
        || (jobs.tool, jobs.end, jobs.exit_code) != (JobTool::Shell, ProcessEnd::Exited, None)
        || !root.path().join("served.txt").exists()
    {
        return Err("unexpected job ends".into());
    }
    for end in [process, jobs] {
        println!(
            "{:?} `{}` ended {:?} after {} ms, exit code {:?}",
            end.tool, end.command, end.end, end.elapsed_ms, end.exit_code
        );
    }
    shell.close().await;
    session.close().await;
    Ok(())
}
