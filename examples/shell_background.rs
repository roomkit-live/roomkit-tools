//! Run shell commands on in the background as processes of a session,
//! without a model: a server started there, and a build moved there when it
//! outlived its foreground timeout. The session's ends tell both.
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
    JobEnd, JobEnds, ProcessEnd, ProcessReadInput, ProcessSession, ProcessStopInput, ShellInput,
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
    let owner = Some("example-demo");
    let session = ProcessSession::new(root.path()).await?;
    let shell = ShellTool::new(root.path())
        .await?
        .with_processes(session.clone())
        .with_limits(ShellLimits {
            default_timeout_ms: 300,
            background_on_timeout: true,
            ..ShellLimits::default()
        })?;
    // Subscribe first: an end before the subscription is not told.
    let mut ends = session.ends();

    // A server: started in the background, its process_id returned at once.
    let server = ShellInput {
        run_in_background: true,
        ..ShellInput::new("echo listening; sleep 30")
    };
    let started = shell.shell_owned("example-demo", server, &cancel).await?;
    let server_id = started
        .data
        .process_id
        .ok_or("the server was not started")?;
    print!("{}", started.text);

    // A build that outlives its 300 ms foreground timeout: moved, not stopped.
    let build = ShellInput::new("echo compiling; sleep 0.6; echo built");
    let moved = shell.shell_owned("example-demo", build, &cancel).await?;
    let build_id = moved.data.process_id.ok_or("the build was stopped")?;
    print!("{}", moved.text);

    let ended = next(&mut ends).await?;
    if ended.process_id.as_deref() != Some(build_id.as_str())
        || (ended.end, ended.exit_code) != (ProcessEnd::Exited, Some(0))
    {
        return Err("unexpected build end".into());
    }
    let rest = session
        .read(owner, ProcessReadInput::new(&build_id), &cancel)
        .await?;
    if rest.data.stdout.text != "built\n" {
        return Err("the rest of the build output is missing".into());
    }
    println!(
        "{build_id} ended after {} ms; the rest of its output: {}",
        ended.elapsed_ms,
        rest.data.stdout.text.trim_end()
    );

    let server_output = session
        .read(owner, ProcessReadInput::new(&server_id), &cancel)
        .await?;
    println!(
        "{server_id} says: {}",
        server_output.data.stdout.text.trim_end()
    );
    session
        .stop(owner, ProcessStopInput::new(&server_id), &cancel)
        .await?;
    session.close().await;
    Ok(())
}
