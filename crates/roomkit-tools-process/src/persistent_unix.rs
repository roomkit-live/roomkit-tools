use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use roomkit_tools_core::Error;
use rustix::process::Pid;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

use crate::journal::Journal;
use crate::persistent::{Entry, Record};
use crate::unix::Group;
use crate::{ProcessEnd, ProcessLimits, ProcessState, ProcessStatus};

pub(crate) fn launch(
    owner: &str,
    command: &str,
    cwd: &Path,
    mut status: ProcessStatus,
    limits: ProcessLimits,
    stop: CancellationToken,
    outputs: Option<&crate::OutputStore>,
) -> Result<(Arc<Entry>, ProcessStatus), Error> {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| Error::Io {
            operation: "spawn persistent process",
            source,
        })?;
    let pid = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .filter(|id| *id > 1)
        .and_then(Pid::from_raw)
        .ok_or_else(|| Error::Io {
            operation: "identify persistent process group",
            source: std::io::Error::other("missing child PID"),
        })?;
    let group = Group(Some(pid));
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
        owner: owner.into(),
        record: Arc::new(Mutex::new(Record {
            status: status.clone(),
            stdout: Journal::new(limits.max_output_bytes),
            stderr: Journal::new(limits.max_output_bytes),
            stdout_archive: stdout_archive.as_ref().map(crate::output::Sink::view),
            stderr_archive: stderr_archive.as_ref().map(crate::output::Sink::view),
        })),
        stdin: Mutex::new(Some(stdin)),
        stop,
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

fn missing_pipe() -> Error {
    Error::Io {
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
            return Ok(());
        }
        if let Some(chunk) = bytes.get(..count) {
            journal.push(chunk)?;
            if let Some(archive) = &mut archive {
                archive.push(chunk);
            }
        }
    }
}

async fn run(
    mut child: Child,
    mut group: Group,
    mut stdout: ChildStdout,
    mut stderr: ChildStderr,
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
    // Capture outlives the wait/select, so stopping never discards bytes already
    // read while their append is awaiting the record lock.
    let record = Arc::clone(&entry.record);
    let capture_failed = CancellationToken::new();
    let failed = capture_failed.clone();
    let mut capture = tokio::spawn(async move {
        let result = tokio::try_join!(
            drain(&mut stdout, &record, true, stdout_archive),
            drain(&mut stderr, &record, false, stderr_archive)
        );
        if result.is_err() {
            failed.cancel();
        }
        result
    });
    let (mut end, execution) = tokio::select! {
        biased;
        () = entry.stop.cancelled() => (ProcessEnd::Stopped, Ok(())),
        () = tokio::time::sleep(Duration::from_millis(timeout)) => (ProcessEnd::TimedOut, Ok(())),
        status = child.wait() => (ProcessEnd::Exited, status.map(|_| ())),
        () = capture_failed.cancelled() => (ProcessEnd::Failed, Err(std::io::Error::other("capture failed"))),
    };
    // Release any pending stdin writer and clean background children on every exit.
    entry.stop.cancel();
    let stopped = group.stop();
    if stopped.is_err() {
        drop(child.start_kill());
    }
    entry.stdin.lock().await.take();
    let reaped = child.wait().await;
    let drained = tokio::time::timeout(Duration::from_millis(250), &mut capture).await;
    if drained.is_err() {
        capture.abort();
        let _joined = capture.await;
    }
    let (out_archive, err_archive) = {
        let record = entry.record.lock().await;
        (record.stdout_archive.clone(), record.stderr_archive.clone())
    };
    tokio::join!(
        async {
            if let Some(view) = out_archive {
                view.wait(archive_timeout).await;
            }
        },
        async {
            if let Some(view) = err_archive {
                view.wait(archive_timeout).await;
            }
        }
    );
    let error = if execution.is_err() {
        Some("process execution or capture failed")
    } else if stopped.is_err() {
        Some("process group cleanup failed")
    } else if reaped.is_err() {
        Some("process reaping failed")
    } else if matches!(drained, Ok(Err(_) | Ok(Err(_)))) {
        Some("process output drain failed")
    } else {
        None
    };
    if error.is_some() {
        end = ProcessEnd::Failed;
    }
    let mut record = entry.record.lock().await;
    record.status.state = ProcessState::Finished;
    record.status.end = Some(end);
    record.status.exit_code = reaped
        .as_ref()
        .ok()
        .and_then(std::process::ExitStatus::code);
    record.status.signal = reaped.as_ref().ok().and_then(ExitStatusExt::signal);
    record.status.output_complete =
        matches!(drained, Ok(Ok(Ok(_)))) && record.stdout.eof && record.stderr.eof;
    record.status.stdin_closed = true;
    record.status.error = error.map(str::to_owned);
    drop(record);
    done.send_replace(true);
}
