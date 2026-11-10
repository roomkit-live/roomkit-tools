use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use roomkit_tools_core::ToolError;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, ChildStderr, ChildStdout};
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::journal::Journal;
use crate::persistent::{Entry, Record};
use crate::unix::Group;
use crate::{CommandEnv, ProcessEnd, ProcessLimits, ProcessState, ProcessStatus};

/// A command to start for an owner, in its initial directory and environment.
pub(crate) struct Launch<'a> {
    pub(crate) owner: Option<&'a str>,
    pub(crate) command: &'a str,
    pub(crate) cwd: &'a Path,
    pub(crate) env: &'a CommandEnv,
}

pub(crate) fn launch(
    request: &Launch<'_>,
    status: ProcessStatus,
    limits: ProcessLimits,
    stop: CancellationToken,
    detach: CancellationToken,
    outputs: Option<&crate::OutputStore>,
) -> Result<(Arc<Entry>, ProcessStatus), ToolError> {
    let mut shell =
        crate::unix::shell_command(request.command, request.cwd, request.env, Stdio::piped());
    // The group stops every member on drop, the shell included; a detached
    // job's shell must outlive its dropped handle.
    shell.kill_on_drop(false);
    let spawned = crate::unix::spawn(shell, "spawn persistent process")?;
    adopt(spawned, request, status, limits, (stop, detach), outputs)
}

/// Record a spawned command, then capture and stop it in its own task.
fn adopt(
    (mut child, group): (Child, Group),
    request: &Launch<'_>,
    mut status: ProcessStatus,
    limits: ProcessLimits,
    (stop, detach): (CancellationToken, CancellationToken),
    outputs: Option<&crate::OutputStore>,
) -> Result<(Arc<Entry>, ProcessStatus), ToolError> {
    let Launch { owner, command, .. } = *request;
    let stdout = child.stdout.take().ok_or_else(missing_pipe)?;
    let stderr = child.stderr.take().ok_or_else(missing_pipe)?;
    let stdin = child.stdin.take().ok_or_else(missing_pipe)?;
    let (done, receiver) = watch::channel(false);
    let stdout_archive = outputs.map(|store| store.begin(owner));
    let stderr_archive = outputs.map(|store| store.begin(owner));
    let archive_timeout = outputs.map_or(0, crate::OutputStore::write_timeout_ms);
    status.stdout_archive = stdout_archive.as_ref().map(crate::output::Sink::snapshot);
    status.stderr_archive = stderr_archive.as_ref().map(crate::output::Sink::snapshot);
    let entry = Arc::new(Entry {
        owner: owner.map(Into::into),
        command: command.into(),
        started: std::time::Instant::now(),
        finished: std::sync::OnceLock::new(),
        record: Arc::new(Mutex::new(Record {
            status: status.clone(),
            stdout: Journal::new(limits.max_output_bytes),
            stderr: Journal::new(limits.max_output_bytes),
            stdout_archive: stdout_archive.as_ref().map(crate::output::Sink::view),
            stderr_archive: stderr_archive.as_ref().map(crate::output::Sink::view),
            output: watch::channel(()).0,
        })),
        stdin: Mutex::new(Some(stdin)),
        stop,
        detach,
        done: receiver,
    });
    tokio::spawn(run(
        child,
        group,
        stdout,
        stderr,
        Arc::clone(&entry),
        done,
        (stdout_archive, stderr_archive, archive_timeout),
    ));
    Ok((entry, status))
}

fn missing_pipe() -> ToolError {
    ToolError::Io {
        operation: "open process pipe",
        source: std::io::Error::other("missing pipe"),
    }
}

async fn drain(
    stream: &mut (impl AsyncRead + Unpin),
    record: &Mutex<Record>,
    stdout: bool,
    mut archive: Option<crate::output::Sink>,
) -> std::io::Result<()> {
    let mut bytes = vec![0; 8192];
    loop {
        if let Some(archive) = &mut archive {
            archive.flush_pending().await;
        }
        let count = stream.read(&mut bytes).await?;
        let mut record = record.lock().await;
        let journal = if stdout {
            &mut record.stdout
        } else {
            &mut record.stderr
        };
        if count == 0 {
            journal.eof = true;
            if let Some(archive) = &mut archive {
                archive.end(true);
            }
            record.output.send_replace(());
            return Ok(());
        }
        if let Some(chunk) = bytes.get(..count) {
            journal.push(chunk)?;
            if let Some(archive) = &mut archive {
                archive.push(chunk);
            }
        }
        record.output.send_replace(());
    }
}

async fn run(
    mut child: Child,
    mut group: Group,
    stdout: ChildStdout,
    stderr: ChildStderr,
    entry: Arc<Entry>,
    done: watch::Sender<bool>,
    archives: (
        Option<crate::output::Sink>,
        Option<crate::output::Sink>,
        u64,
    ),
) {
    let (stdout_archive, stderr_archive, archive_timeout) = archives;
    let timeout = entry.record.lock().await.status.timeout_ms;
    let record = Arc::clone(&entry.record);
    let (capture, capture_failed, release) =
        spawn_capture(stdout, stderr, record, (stdout_archive, stderr_archive));
    let (end, execution) = tokio::select! {
        biased;
        // First: closing the session after detaching it stops nothing.
        () = entry.detach.cancelled() => {
            return leave_running(child, group, (capture, release), &entry, done).await;
        }
        () = entry.stop.cancelled() => (ProcessEnd::Stopped, Ok(())),
        () = tokio::time::sleep(Duration::from_millis(timeout)) => (ProcessEnd::TimedOut, Ok(())),
        status = child.wait() => (ProcessEnd::Exited, status.map(|_| ())),
        () = capture_failed.cancelled() => (ProcessEnd::Failed, Err(std::io::Error::other("capture failed"))),
    };
    // Release any pending stdin writer and clean background children on every exit.
    entry.stop.cancel();
    let stopped = group.stop();
    entry.stdin.lock().await.take();
    let reaped = group.reap_shell(&mut child).await;
    let drained = finish_capture(capture).await;
    settle_archives(&entry, archive_timeout).await;
    let error = failure(&execution, &stopped, reaped.as_ref(), drained);
    publish(&entry, end, reaped, drained, error).await;
    done.send_replace(true);
}

/// Let a detached job go, still running: its group is no longer stopped,
/// its stdin closes, its output pipes go to drainers that outlive the host,
/// and its shell, spawned without `kill_on_drop`, is left to the runtime to
/// reap whenever it exits.
async fn leave_running(
    child: Child,
    mut group: Group,
    (capture, release): (JoinHandle<Captured>, CancellationToken),
    entry: &Entry,
    done: watch::Sender<bool>,
) {
    group.release();
    entry.stdin.lock().await.take();
    release.cancel();
    if let Ok(Captured::Released(stdout, stderr)) = capture.await {
        crate::unix::hand_off(stdout, stderr);
    }
    drop(child);
    done.send_replace(true);
}

/// How a capture task ended: both pipes drained to their end, or given back
/// when its job was let go.
enum Captured {
    Drained(std::io::Result<((), ())>),
    Released(ChildStdout, ChildStderr),
}

/// Capture both streams in their own task, which outlives the wait so that
/// stopping never discards bytes already read while their append awaits the
/// record lock. The first token is cancelled when capture fails; cancelling
/// the second makes the task give the pipes back.
fn spawn_capture(
    mut stdout: ChildStdout,
    mut stderr: ChildStderr,
    record: Arc<Mutex<Record>>,
    (stdout_archive, stderr_archive): (Option<crate::output::Sink>, Option<crate::output::Sink>),
) -> (JoinHandle<Captured>, CancellationToken, CancellationToken) {
    let capture_failed = CancellationToken::new();
    let failed = capture_failed.clone();
    let release = CancellationToken::new();
    let released = release.clone();
    let capture = tokio::spawn(async move {
        let drained = tokio::select! {
            biased;
            () = released.cancelled() => None,
            result = async {
                tokio::try_join!(
                    drain(&mut stdout, &record, true, stdout_archive),
                    drain(&mut stderr, &record, false, stderr_archive)
                )
            } => Some(result),
        };
        let Some(result) = drained else {
            return Captured::Released(stdout, stderr);
        };
        if result.is_err() {
            failed.cancel();
        }
        Captured::Drained(result)
    });
    (capture, capture_failed, release)
}

/// How output capture ended once the process was stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drained {
    /// Both pipes reached EOF.
    Complete,
    /// Reading a pipe failed.
    Failed,
    /// A pipe was still open after the drain deadline.
    CutShort,
}

/// Let capture drain what the pipes still hold. Escaped children can keep
/// inherited pipes open, so this waits at most 250 ms.
async fn finish_capture(mut capture: JoinHandle<Captured>) -> Drained {
    match tokio::time::timeout(Duration::from_millis(250), &mut capture).await {
        Ok(Ok(Captured::Drained(Ok(_)))) => Drained::Complete,
        Ok(Ok(Captured::Drained(Err(_))) | Err(_)) => Drained::Failed,
        // Only a detached job's capture gives its pipes back, never here.
        Ok(Ok(Captured::Released(..))) => Drained::CutShort,
        Err(_) => {
            capture.abort();
            let _joined = capture.await;
            Drained::CutShort
        }
    }
}

/// Wait for both archives to settle, each up to the store's write deadline.
async fn settle_archives(entry: &Entry, timeout_ms: u64) {
    let (out_archive, err_archive) = {
        let record = entry.record.lock().await;
        (record.stdout_archive.clone(), record.stderr_archive.clone())
    };
    tokio::join!(
        async {
            if let Some(view) = out_archive {
                view.wait(timeout_ms).await;
            }
        },
        async {
            if let Some(view) = err_archive {
                view.wait(timeout_ms).await;
            }
        }
    );
}

/// The safe error a stopped job reports, if any, most severe first. A shell
/// that could not be stopped, as under other credentials, keeps running
/// outside cleanup.
fn failure(
    execution: &std::io::Result<()>,
    stopped: &std::io::Result<()>,
    reaped: Option<&std::io::Result<ExitStatus>>,
    drained: Drained,
) -> Option<&'static str> {
    if reaped.is_none() {
        Some("process could not be stopped")
    } else if execution.is_err() {
        Some("process execution or capture failed")
    } else if stopped.is_err() {
        Some("process group cleanup failed")
    } else if reaped.is_some_and(Result::is_err) {
        Some("process reaping failed")
    } else if drained == Drained::Failed {
        Some("process output drain failed")
    } else {
        None
    }
}

/// Publish the final status of a stopped job; any error makes it `failed`.
async fn publish(
    entry: &Entry,
    end: ProcessEnd,
    reaped: Option<std::io::Result<ExitStatus>>,
    drained: Drained,
    error: Option<&str>,
) {
    let status = reaped.and_then(Result::ok);
    let mut record = entry.record.lock().await;
    let _ = entry.finished.set(entry.started.elapsed());
    record.status.state = ProcessState::Finished;
    record.status.end = Some(if error.is_some() {
        ProcessEnd::Failed
    } else {
        end
    });
    record.status.exit_code = status.and_then(|status| status.code());
    record.status.signal = status.and_then(|status| status.signal());
    record.status.output_complete =
        drained == Drained::Complete && record.stdout.eof && record.stderr.eof;
    record.status.stdin_closed = true;
    record.status.error = error.map(str::to_owned);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::process::{Signal, kill_process_group};

    #[tokio::test]
    async fn refused_stop_finishes_the_job_as_failed() {
        let root = tempfile::tempdir().unwrap();
        let env = CommandEnv::default();
        let request = Launch {
            owner: Some("a"),
            command: "sleep 30",
            cwd: root.path(),
            env: &env,
        };
        let shell = crate::unix::shell_command(request.command, request.cwd, &env, Stdio::piped());
        let (child, mut group) = crate::unix::spawn(shell, "spawn").unwrap();
        let pid = group.pid().unwrap();
        // A shell under other credentials refuses every signal.
        group.refuse_all();
        let status = crate::persistent::initial_status("p1".into(), ".".into(), 60_000);
        let limits = ProcessLimits::default();
        let stop = CancellationToken::new();
        let (entry, _) = adopt(
            (child, group),
            &request,
            status,
            limits,
            (stop, CancellationToken::new()),
            None,
        )
        .unwrap();
        entry.stop.cancel();
        let mut done = entry.done.clone();
        let finished =
            tokio::time::timeout(Duration::from_secs(5), done.wait_for(|done| *done)).await;
        let _killed = kill_process_group(pid, Signal::KILL);
        assert!(
            finished.is_ok(),
            "a shell refusing SIGKILL kept the job running"
        );
        let status = entry.record.lock().await.status();
        assert_eq!(status.state, ProcessState::Finished);
        assert_eq!(status.end, Some(ProcessEnd::Failed));
        assert_eq!(
            status.error.as_deref(),
            Some("process could not be stopped")
        );
        assert_eq!((status.exit_code, status.signal), (None, None));
    }
}
