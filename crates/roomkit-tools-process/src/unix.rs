use std::fmt::Write as _;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use roomkit_tools_core::{Output, ToolError};
use rustix::process::{Pid, Signal, kill_process_group};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio_util::sync::CancellationToken;

use crate::capture::Capture;
use crate::persistent_unix::Spawned;
use crate::{CommandEnv, ShellLimits, ShellResult};

/// `/bin/sh -c command` started in `cwd` with the host-selected environment,
/// and with piped stdout and stderr.
pub(crate) fn shell_command(command: &str, cwd: &Path, env: &CommandEnv, stdin: Stdio) -> Command {
    let mut shell = Command::new("/bin/sh");
    env.apply(&mut shell);
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
) -> Result<(Child, Group), ToolError> {
    new_session(&mut command);
    let child = command
        .spawn()
        .map_err(|source| ToolError::Io { operation, source })?;
    // The group is exactly the positive child PID, never 0 or -1.
    let pid = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .filter(|id| *id > 1)
        .and_then(Pid::from_raw)
        .ok_or_else(|| ToolError::Io {
            operation: "identify process group",
            source: std::io::Error::other("missing child PID"),
        })?;
    Ok((child, Group::new(pid)))
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

/// How long a shell that refused SIGKILL is awaited before it is left running.
const REAP_GRACE: Duration = Duration::from_millis(250);

type Kill = fn(Pid, Signal) -> rustix::io::Result<()>;
type KillShell = fn(&mut Child) -> std::io::Result<()>;

/// A spawned command's process group, stopped on drop.
pub(crate) struct Group {
    pid: Option<Pid>,
    kill: Kill,
    /// SIGKILL to the shell alone; tokio's, which never signals a reaped PID.
    kill_shell: KillShell,
}

impl Group {
    fn new(pid: Pid) -> Self {
        Self {
            pid: Some(pid),
            kill: kill_process_group,
            kill_shell: Child::start_kill,
        }
    }

    /// Send SIGKILL to the shell, if still running, and reap it.
    ///
    /// A shell that refuses the signal, such as one replaced by `sudo` and
    /// running under other credentials, is awaited [`REAP_GRACE`] at most:
    /// `None` then means it runs on outside cleanup, and the runtime reaps it
    /// whenever it exits.
    pub(crate) async fn reap_shell(
        &self,
        child: &mut Child,
    ) -> Option<std::io::Result<ExitStatus>> {
        let refused = (self.kill_shell)(child)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied);
        if refused {
            tokio::time::timeout(REAP_GRACE, child.wait()).await.ok()
        } else {
            Some(child.wait().await)
        }
    }

    /// Leave every member running: the group is no longer stopped, by
    /// [`Self::stop`] or on drop. Returns its id, unless already stopped.
    pub(crate) fn release(&mut self) -> Option<Pid> {
        self.pid.take()
    }

    /// Send SIGKILL to every member at once; there is no graceful phase.
    pub(crate) fn stop(&mut self) -> std::io::Result<()> {
        if let Some(pid) = self.pid {
            match (self.kill)(pid, Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => self.pid = None,
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

fn io(operation: &'static str, source: std::io::Error) -> ToolError {
    ToolError::Io { operation, source }
}

fn missing_pipe(operation: &'static str) -> ToolError {
    io(operation, std::io::Error::other("missing pipe"))
}

/// One foreground command and the host settings it runs with.
pub(crate) struct Foreground {
    pub(crate) command: String,
    pub(crate) cwd: PathBuf,
    /// The working directory relative to the workspace, as the result names it.
    pub(crate) shown_cwd: String,
    pub(crate) env: Arc<CommandEnv>,
    pub(crate) limits: ShellLimits,
    pub(crate) timeout_ms: u64,
    /// The store and the archives' owner, `None` for the standalone one.
    pub(crate) outputs: Option<(crate::OutputStore, Option<String>)>,
    /// The caller's trusted owner, `None` for the standalone one.
    pub(crate) owner: Option<String>,
    /// Who follows the jobs the command leaves running.
    pub(crate) jobs: Arc<crate::shell_jobs::ShellJobs>,
    /// When the command started: its jobs' ends count from it.
    pub(crate) started: std::time::Instant,
    /// The session the command moves to at its timeout, if it moves.
    pub(crate) processes: Option<crate::ProcessSession>,
}

pub(crate) async fn run(
    foreground: Foreground,
    cancel: CancellationToken,
) -> Result<Output<ShellResult>, ToolError> {
    if cancel.is_cancelled() {
        return Err(ToolError::Cancelled);
    }
    let mut command = shell_command(
        &foreground.command,
        &foreground.cwd,
        &foreground.env,
        Stdio::null(),
    );
    // A command that may move to the background must outlive its handle once
    // its session lets it go; its group stops it on every other path.
    command.kill_on_drop(foreground.processes.is_none());
    let (child, group) = spawn(command, "spawn shell")?;
    supervise(child, group, foreground, &cancel).await
}

/// How the wait for a foreground command ended.
enum Waited {
    /// The shell exited on its own.
    Exited(ExitStatus),
    TimedOut,
    Cancelled,
}

/// Capture `command`'s output until it exits, times out or is cancelled. A
/// read cut short loses no byte: each one read is captured before the next
/// await, so capture can go on from the same pipes.
async fn wait(
    command: &mut Spawned,
    (out, err): (&mut Capture, &mut Capture),
    timeout_ms: u64,
    cancel: &CancellationToken,
) -> std::io::Result<Waited> {
    let Spawned {
        child,
        stdout,
        stderr,
        ..
    } = command;
    tokio::select! {
        biased;
        () = cancel.cancelled() => Ok(Waited::Cancelled),
        () = tokio::time::sleep(Duration::from_millis(timeout_ms)) => Ok(Waited::TimedOut),
        status = child.wait() => status.map(Waited::Exited),
        error = async {
            tokio::try_join!(out.drain(stdout), err.drain(stderr))?;
            // EOF is not process exit: a command can close both streams and run on.
            std::future::pending::<std::io::Result<Waited>>().await
        } => error,
    }
}

/// Capture a spawned command until it exits, times out or is cancelled, then
/// stop its group, reap it and drain what its pipes still hold. One still
/// running at its timeout moves to the background instead, when it may.
async fn supervise(
    mut child: Child,
    group: Group,
    foreground: Foreground,
    cancel: &CancellationToken,
) -> Result<Output<ShellResult>, ToolError> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| missing_pipe("open shell stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| missing_pipe("open shell stderr"))?;
    let mut command = Spawned {
        child,
        group,
        stdout,
        stderr,
        started: foreground.started,
    };
    let mut out = Capture::new(foreground.limits.max_output_bytes);
    let mut err = Capture::new(foreground.limits.max_output_bytes);
    if let Some((store, owner)) = &foreground.outputs {
        out.archive(Some(store.begin(owner.as_deref())));
        err.archive(Some(store.begin(owner.as_deref())));
    }
    let captures = (&mut out, &mut err);
    let waited = wait(&mut command, captures, foreground.timeout_ms, cancel).await;
    let mut refused = None;
    if let (Ok(Waited::TimedOut), Some(processes)) = (&waited, &foreground.processes) {
        match crate::shell_handover::take_over(processes, &foreground, command, cancel).await {
            Ok(status) => {
                return Ok(crate::shell_handover::moved((out, err), status, &foreground).await);
            }
            Err((error, back)) => {
                command = *back;
                refused = Some(error);
            }
        }
    }
    let result = stop(command, (out, err), waited, &foreground, cancel).await?;
    Ok(render(result, refused.as_ref()))
}

/// Stop the group of a command that ended its wait, unless it exited on its
/// own leaving jobs to keep, reap it and drain what its pipes still hold.
async fn stop(
    mut command: Spawned,
    (mut out, mut err): (Capture, Capture),
    waited: std::io::Result<Waited>,
    foreground: &Foreground,
    cancel: &CancellationToken,
) -> Result<ShellResult, ToolError> {
    let timed_out = matches!(waited, Ok(Waited::TimedOut));
    let cancelled = matches!(waited, Ok(Waited::Cancelled));
    // A shell that ended on its own may leave its background jobs running.
    let exited = match &waited {
        Ok(Waited::Exited(status)) => Some(*status),
        _ => None,
    };
    let kept = foreground.limits.keep_background_jobs && exited.is_some();
    let status = if kept {
        exited
    } else {
        stop_and_reap(&mut command.group, &mut command.child).await?
    };
    waited.map_err(|source| io("capture shell output", source))?;
    let (stdout, stderr) = (&mut command.stdout, &mut command.stderr);
    let output_complete = drain_rest(&mut out, &mut err, stdout, stderr).await?;
    tokio::join!(out.finish_archive(), err.finish_archive());
    // A cancelled call returns no result: its archives are all that is left.
    // Its kept jobs stop with it, the group not being released.
    if cancelled || cancel.is_cancelled() {
        return Err(ToolError::Cancelled);
    }
    if kept {
        if let Some(pgid) = command.group.release() {
            follow(foreground, pgid);
        }
        // A job still holding a pipe writes to a drainer from now on.
        if !output_complete {
            hand_off(command.stdout, command.stderr);
        }
    }
    if let Some((store, _)) = &foreground.outputs {
        out.discard_whole_archive(store);
        err.discard_whole_archive(store);
    }
    Ok(ShellResult {
        cwd: foreground.shown_cwd.clone(),
        exit_code: status.and_then(|status| status.code()),
        signal: status.and_then(|status| status.signal()),
        timed_out,
        timeout_ms: foreground.timeout_ms,
        output_complete,
        stdout: out.finish(),
        stderr: err.finish(),
        process_id: None,
    })
}

/// Follow the group `pgid` that `foreground` left running, if any job is left
/// in it, so that its end is told.
fn follow(foreground: &Foreground, pgid: Pid) {
    let end = crate::JobEnd {
        owner: foreground.owner.clone(),
        tool: crate::JobTool::Shell,
        process_id: None,
        command: foreground.command.clone(),
        cwd: foreground.shown_cwd.clone(),
        end: crate::ProcessEnd::Exited,
        exit_code: None,
        signal: None,
        elapsed_ms: 0,
    };
    foreground.jobs.follow(pgid, end, foreground.started);
}

/// Hand the pipes of a command whose jobs run on to drainers that outlive the
/// host. Dropping them instead would make a job's next write fail (EPIPE) or
/// kill it (SIGPIPE): a server logging each request would die or fail it.
pub(crate) fn hand_off(stdout: ChildStdout, stderr: ChildStderr) {
    for pipe in [stdout.into_owned_fd(), stderr.into_owned_fd()]
        .into_iter()
        .flatten()
    {
        drain_away(pipe);
    }
}

/// Read `pipe` until every writer closes it, from a shell in a session of its
/// own that outlives the host, discarding what it reads: with `cat` from the
/// host's `PATH`, or the shell's own `read` where there is none. Best effort:
/// a drainer that cannot start leaves the pipe to close.
fn drain_away(pipe: std::os::fd::OwnedFd) {
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            "command -v cat > /dev/null 2>&1 && exec cat > /dev/null; while IFS= read -r _ || [ -n \"$_\" ]; do :; done",
        ])
        .env_clear()
        .env("PATH", path)
        .stdin(Stdio::from(pipe))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    if let Ok((child, mut group)) = spawn(command, "spawn output drainer") {
        group.release();
        // Without kill_on_drop, the runtime reaps it whenever it ends.
        drop(child);
    }
}

/// Stop the whole group, which also ends background jobs after a normal exit,
/// then stop and reap the shell.
///
/// The group signal fails only when no member can be signalled, such as
/// members left running under other credentials (`sudo -b`); those stay
/// outside cleanup. A shell that runs under other credentials itself cannot be
/// stopped either: past a short grace it is left running and `None` is
/// returned, so the call still ends with its timeout or cancellation.
async fn stop_and_reap(
    group: &mut Group,
    child: &mut Child,
) -> Result<Option<ExitStatus>, ToolError> {
    drop(group.stop());
    group
        .reap_shell(child)
        .await
        .transpose()
        .map_err(|source| io("reap shell", source))
}

/// Drain the bytes still in the pipes once the group is stopped. Escaped
/// children can keep inherited pipes open, so this waits at most 250 ms and
/// returns whether both pipes reached EOF.
async fn drain_rest(
    out: &mut Capture,
    err: &mut Capture,
    stdout: &mut ChildStdout,
    stderr: &mut ChildStderr,
) -> Result<bool, ToolError> {
    let drained = tokio::time::timeout(Duration::from_millis(250), async {
        tokio::try_join!(out.drain(stdout), err.drain(stderr))
    })
    .await;
    match drained {
        Ok(result) => result
            .map(|_| true)
            .map_err(|source| io("drain shell output", source)),
        Err(_) => Ok(false),
    }
}

/// The text of `data`, with why a command was not moved to the background.
pub(crate) fn render(data: ShellResult, refused: Option<&ToolError>) -> Output<ShellResult> {
    let mut text = match &data.process_id {
        Some(id) => format!(
            "still running after {} ms: moved to the background as {id}; read the rest of its output with process_read",
            data.timeout_ms
        ),
        None => outcome(&data),
    };
    if data.stdout.text.is_empty() && data.stderr.text.is_empty() {
        text.push_str(" (no output)");
    }
    text.push('\n');
    if let Some(error) = refused {
        let _written = writeln!(text, "not moved to the background: {error}");
    }
    // Before what the command wrote, which could imitate these lines.
    crate::output::append_metadata(
        &mut text,
        data.stdout.archive.as_ref(),
        data.stderr.archive.as_ref(),
    );
    crate::output::push_stream(&mut text, "stdout:", &data.stdout.text);
    crate::output::push_stream(&mut text, "stderr:", &data.stderr.text);
    Output::new(data, text)
}

/// How a command ended, under the names of its result.
fn outcome(data: &ShellResult) -> String {
    let mut text = format!("exit_code={}", crate::output::nullable(data.exit_code));
    if let Some(signal) = data.signal {
        let _written = write!(text, ", signal={signal}");
    }
    if data.timed_out {
        text.push_str(", timed_out=true");
    }
    if !data.output_complete {
        text.push_str(", output_complete=false");
    }
    text
}

#[cfg(test)]
impl Group {
    /// The group id, for a test that stops the command itself.
    pub(crate) fn pid(&self) -> Option<Pid> {
        self.pid
    }

    /// Refuse every signal, to the group and to the shell, as members running
    /// under other credentials do. The test stops the command itself.
    pub(crate) fn refuse_all(&mut self) {
        self.kill = tests::refuse;
        self.kill_shell = |_| Err(std::io::ErrorKind::PermissionDenied.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn refuse(_: Pid, _: Signal) -> rustix::io::Result<()> {
        Err(rustix::io::Errno::PERM)
    }

    #[tokio::test]
    async fn refused_group_signal_keeps_a_finished_command_result() {
        let root = tempfile::tempdir().unwrap();
        let foreground = Foreground {
            command: "printf done; printf warning >&2; exit 3".into(),
            cwd: root.path().to_owned(),
            shown_cwd: ".".into(),
            env: Arc::default(),
            limits: ShellLimits::default(),
            timeout_ms: 10_000,
            outputs: None,
            owner: None,
            jobs: Arc::default(),
            started: std::time::Instant::now(),
            processes: None,
        };
        let command = shell_command(
            &foreground.command,
            &foreground.cwd,
            &foreground.env,
            Stdio::null(),
        );
        let (child, mut group) = spawn(command, "spawn shell").unwrap();
        // A member running under other credentials refuses the group signal.
        group.kill = refuse;
        let result = supervise(child, group, foreground, &CancellationToken::new())
            .await
            .unwrap()
            .data;
        assert_eq!(result.exit_code, Some(3));
        assert_eq!(result.stdout.text, "done");
        assert_eq!(result.stderr.text, "warning");
        assert!(result.output_complete);
    }

    /// Supervise `command` while every signal is refused, as for a shell
    /// running under other credentials, then stop it for real.
    async fn supervise_refused(
        command: &str,
        timeout_ms: u64,
        cancel: &CancellationToken,
    ) -> Result<Output<ShellResult>, ToolError> {
        let root = tempfile::tempdir().unwrap();
        let foreground = Foreground {
            command: command.into(),
            cwd: root.path().to_owned(),
            shown_cwd: ".".into(),
            env: Arc::default(),
            limits: ShellLimits::default(),
            timeout_ms,
            outputs: None,
            owner: None,
            jobs: Arc::default(),
            started: std::time::Instant::now(),
            processes: None,
        };
        let shell = shell_command(
            &foreground.command,
            &foreground.cwd,
            &foreground.env,
            Stdio::null(),
        );
        let (child, mut group) = spawn(shell, "spawn shell").unwrap();
        let pid = group.pid.unwrap();
        group.refuse_all();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            supervise(child, group, foreground, cancel),
        )
        .await;
        let _killed = kill_process_group(pid, Signal::KILL);
        result.expect("a shell refusing SIGKILL held the call")
    }

    #[tokio::test]
    async fn refused_shell_signal_still_returns_the_timeout() {
        let result = supervise_refused("sleep 30", 100, &CancellationToken::new())
            .await
            .unwrap()
            .data;
        assert!(result.timed_out);
        // No status: the shell could not be stopped and runs on outside cleanup.
        assert_eq!((result.exit_code, result.signal), (None, None));
        assert!(!result.output_complete);
    }

    #[tokio::test]
    async fn refused_shell_signal_still_returns_the_cancellation() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = supervise_refused("sleep 30", 10_000, &cancel).await;
        assert!(matches!(result, Err(ToolError::Cancelled)), "{result:?}");
    }
}
