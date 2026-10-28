use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use roomkit_tools_core::{Error, Output};
use rustix::process::{Pid, Signal, kill_process_group};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::capture::Capture;
use crate::{ShellLimits, ShellResult};

pub(crate) struct Group(pub(crate) Option<Pid>);

impl Group {
    pub(crate) fn stop(&mut self) -> std::io::Result<()> {
        if let Some(pid) = self.0 {
            match kill_process_group(pid, Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => self.0 = None,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        drop(self.stop());
    }
}

fn io(operation: &'static str, source: std::io::Error) -> Error {
    Error::Io { operation, source }
}

pub(crate) async fn run(
    command: String,
    cwd: PathBuf,
    limits: ShellLimits,
    timeout_ms: u64,
    cancel: CancellationToken,
) -> Result<Output<ShellResult>, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| io("spawn shell", source))?;
    // The new group is exactly the positive child PID, never 0 or -1.
    let pid = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .filter(|id| *id > 1)
        .and_then(Pid::from_raw)
        .ok_or_else(|| {
            io(
                "identify shell process group",
                std::io::Error::other("missing child PID"),
            )
        })?;
    let mut group = Group(Some(pid));
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io("open shell stdout", std::io::Error::other("missing pipe")))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| io("open shell stderr", std::io::Error::other("missing pipe")))?;
    let mut out = Capture::new(limits.max_output_bytes);
    let mut err = Capture::new(limits.max_output_bytes);
    let mut timed_out = false;
    let mut cancelled = false;
    let result = tokio::select! {
        biased;
        () = cancel.cancelled() => { cancelled = true; Ok(None) },
        () = tokio::time::sleep(Duration::from_millis(timeout_ms)) => { timed_out = true; Ok(None) },
        status = child.wait() => status.map(Some),
        error = async {
            tokio::try_join!(out.drain(&mut stdout), err.drain(&mut stderr))?;
            // EOF is not process exit: a command can close both streams and run on.
            std::future::pending::<std::io::Result<Option<std::process::ExitStatus>>>().await
        } => error,
    };
    // Also stop background jobs after a successful foreground shell exit.
    let stopped = group.stop();
    if stopped.is_err() {
        drop(child.start_kill());
    }
    let reaped = child.wait().await;
    stopped.map_err(|source| io("stop shell process group", source))?;
    let status = reaped.map_err(|source| io("reap shell", source))?;
    result.map_err(|source| io("capture shell output", source))?;
    // Escaped children can retain inherited pipes. Do not wait forever for EOF.
    let output_complete = match tokio::time::timeout(Duration::from_millis(250), async {
        tokio::try_join!(out.drain(&mut stdout), err.drain(&mut stderr))
    })
    .await
    {
        Ok(result) => {
            result.map_err(|source| io("drain shell output", source))?;
            true
        }
        Err(_) => false,
    };
    if cancelled || cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let data = ShellResult {
        cwd: cwd.to_string_lossy().into_owned(),
        exit_code: status.code(),
        signal: status.signal(),
        timed_out,
        timeout_ms,
        output_complete,
        stdout: out.finish(),
        stderr: err.finish(),
    };
    let text = format!(
        "exit_code={:?}, signal={:?}, timed_out={}, output_complete={}\nstdout:\n{}\nstderr:\n{}",
        data.exit_code,
        data.signal,
        data.timed_out,
        data.output_complete,
        data.stdout.text,
        data.stderr.text
    );
    Ok(Output::new(data, text))
}
