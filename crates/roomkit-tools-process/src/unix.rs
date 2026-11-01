use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use roomkit_tools_core::{Error, Output};
use rustix::process::{Pid, Signal, kill_process_group};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

use crate::capture::Capture;
use crate::{ShellLimits, ShellResult};

/// `/bin/sh -c command` started in `cwd`, with piped stdout and stderr.
pub(crate) fn shell_command(command: &str, cwd: &Path, stdin: Stdio) -> Command {
    let mut shell = Command::new("/bin/sh");
    shell
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    shell
}

/// Spawn `command` as the leader of a new session and of its process group.
///
/// The new session has no controlling terminal: a command that opens
/// `/dev/tty` fails instead of writing into the host's terminal or being
/// stopped by its job control. The group id is the child's pid.
pub(crate) fn spawn(
    mut command: Command,
    operation: &'static str,
) -> Result<(Child, Group), Error> {
    new_session(&mut command);
    let child = command
        .spawn()
        .map_err(|source| Error::Io { operation, source })?;
    // The group is exactly the positive child PID, never 0 or -1.
    let pid = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .filter(|id| *id > 1)
        .and_then(Pid::from_raw)
        .ok_or_else(|| Error::Io {
            operation: "identify process group",
            source: std::io::Error::other("missing child PID"),
        })?;
    Ok((child, Group(Some(pid))))
}

#[expect(
    unsafe_code,
    reason = "setsid must run in the child between fork and exec"
)]
fn new_session(command: &mut Command) {
    // SAFETY: the closure runs in the forked child before exec. It captures
    // nothing, allocates nothing, takes no lock and only calls setsid, which
    // is async-signal-safe; an errno converts to an I/O error without allocating.
    unsafe {
        command.pre_exec(|| rustix::process::setsid().map(drop).map_err(Into::into));
    }
}

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
    outputs: Option<(crate::OutputStore, String)>,
) -> Result<Output<ShellResult>, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let (mut child, mut group) =
        spawn(shell_command(&command, &cwd, Stdio::null()), "spawn shell")?;
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
    if let Some((store, owner)) = &outputs {
        out.archive(Some(store.begin(owner)));
        err.archive(Some(store.begin(owner)));
    }
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
    tokio::join!(out.finish_archive(), err.finish_archive());
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
    Ok(render(data))
}

fn render(data: ShellResult) -> Output<ShellResult> {
    let mut text = format!(
        "exit_code={:?}, signal={:?}, timed_out={}, output_complete={}\nstdout:\n{}\nstderr:\n{}",
        data.exit_code,
        data.signal,
        data.timed_out,
        data.output_complete,
        data.stdout.text,
        data.stderr.text
    );
    crate::output::append_metadata(
        &mut text,
        data.stdout.archive.as_ref(),
        data.stderr.archive.as_ref(),
    );
    Output::new(data, text)
}
