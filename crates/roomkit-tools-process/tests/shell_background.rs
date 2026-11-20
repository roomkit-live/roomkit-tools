//! A shell command runs on in the background as one of a process session's
//! jobs: started there, or moved there when it reaches its timeout.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]

use std::fmt::Write as _;
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use roomkit_tools_core::ToolError;
use roomkit_tools_process::{
    CommandEnv, JobEnd, JobEnds, JobTool, ProcessEnd, ProcessLimits, ProcessReadInput,
    ProcessSession, ProcessStartInput, ProcessState, ShellInput, ShellLimits, ShellResult,
    ShellTool,
};
use tokio_util::sync::CancellationToken;

#[expect(clippy::expect_used, reason = "a missing end must fail the test")]
async fn next(ends: &mut JobEnds) -> Option<JobEnd> {
    use futures_core::Stream;
    let next = std::future::poll_fn(|context| std::pin::Pin::new(&mut *ends).poll_next(context));
    tokio::time::timeout(Duration::from_secs(5), next)
        .await
        .expect("an end or the end of the ends within five seconds")
}

async fn session(root: &std::path::Path, limits: ProcessLimits) -> ProcessSession {
    ProcessSession::new(root)
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap()
}

/// A shell attached to `processes` whose foreground commands move to the
/// background once they reach `timeout_ms`.
async fn moving_shell(
    root: &std::path::Path,
    processes: ProcessSession,
    timeout_ms: u64,
) -> ShellTool {
    ShellTool::new(root)
        .await
        .unwrap()
        .with_processes(processes)
        .with_limits(ShellLimits {
            default_timeout_ms: timeout_ms,
            background_on_timeout: true,
            ..ShellLimits::default()
        })
        .unwrap()
}

async fn run(shell: &ShellTool, owner: &str, input: ShellInput) -> (ShellResult, String) {
    let output = shell
        .shell_owned(owner, input, &CancellationToken::new())
        .await
        .unwrap();
    (output.data, output.text)
}

/// Everything `id` wrote to stdout, once it has finished.
async fn stdout(session: &ProcessSession, owner: &str, id: &str) -> String {
    let read = session
        .read(
            Some(owner),
            ProcessReadInput::new(id),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .data;
    assert_eq!(read.status.state, ProcessState::Finished);
    assert!(!read.stdout.has_more, "one page holds it all");
    read.stdout.text
}

fn background(command: &str) -> ShellInput {
    ShellInput {
        run_in_background: true,
        ..ShellInput::new(command)
    }
}

#[tokio::test]
async fn a_background_command_returns_at_once_as_a_process_of_its_owner() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("sub")).unwrap();
    let processes = session(root.path(), ProcessLimits::default()).await;
    let mut ends = processes.ends();
    let env = CommandEnv {
        env: [("GREETING".to_owned(), "hello".to_owned())].into(),
        ..CommandEnv::default()
    };
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_env(env)
        .unwrap()
        .with_processes(processes.clone());
    let started = Instant::now();
    let input = ShellInput {
        cwd: Some("sub".into()),
        ..background("sleep 0.3; printf \"$GREETING from $(basename \"$PWD\")\"; cat; exit 3")
    };
    let (result, text) = run(&shell, "agent", input).await;
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "returned at once"
    );
    let id = result.process_id.clone().unwrap();
    assert_eq!(id, "p1");
    assert_eq!(
        (result.exit_code, result.signal, result.timed_out),
        (None, None, false)
    );
    assert_eq!(result.cwd, "sub");
    assert_eq!(
        result.timeout_ms,
        ProcessLimits::default().default_timeout_ms
    );
    assert!(
        text.contains("p1") && text.contains("process_read"),
        "{text}"
    );
    // The owner's process, which another owner does not reach.
    let foreign = processes
        .read(
            Some("other"),
            ProcessReadInput::new(&id),
            &CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(foreign, Err(ToolError::AccessRefused(_))),
        "{foreign:?}"
    );
    let end = next(&mut ends).await.unwrap();
    assert_eq!(
        (end.owner.as_deref(), end.tool, end.process_id.as_deref()),
        (Some("agent"), JobTool::Process, Some("p1"))
    );
    // The shell's environment and directory; stdin closed, as in the foreground.
    assert_eq!((end.end, end.exit_code), (ProcessEnd::Exited, Some(3)));
    assert_eq!(stdout(&processes, "agent", &id).await, "hello from sub");
    assert!(processes.close().await);
}

#[tokio::test]
async fn a_background_command_needs_a_process_session() {
    let root = tempfile::tempdir().unwrap();
    let shell = ShellTool::new(root.path()).await.unwrap();
    let refused = shell
        .shell(background("true"), &CancellationToken::new())
        .await;
    let Err(ToolError::InvalidInput(message)) = refused else {
        panic!("{refused:?}");
    };
    assert!(message.contains("run_in_background"), "{message}");
    let refused = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(ShellLimits {
            background_on_timeout: true,
            ..ShellLimits::default()
        });
    assert!(
        matches!(refused, Err(ToolError::InvalidInput(_))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_background_command_lives_within_the_process_tools_limits() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        default_timeout_ms: 10_000,
        max_timeout_ms: 60_000,
        ..ProcessLimits::default()
    };
    let processes = session(root.path(), limits).await;
    let mut ends = processes.ends();
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_processes(processes.clone());
    // Above the shell's own maximum, within the process tools'.
    let long = ShellInput {
        timeout_ms: NonZeroU64::new(50_000),
        ..background("true")
    };
    assert_eq!(run(&shell, "a", long).await.0.timeout_ms, 50_000);
    assert_eq!(next(&mut ends).await.unwrap().end, ProcessEnd::Exited);
    let above = ShellInput {
        timeout_ms: NonZeroU64::new(60_001),
        ..background("true")
    };
    let refused = shell
        .shell_owned("a", above, &CancellationToken::new())
        .await;
    assert!(
        matches!(refused, Err(ToolError::InvalidInput(_))),
        "{refused:?}"
    );
    let short = ShellInput {
        timeout_ms: NonZeroU64::new(100),
        ..background("sleep 30")
    };
    run(&shell, "a", short).await;
    assert_eq!(next(&mut ends).await.unwrap().end, ProcessEnd::TimedOut);
    assert!(processes.close().await);
}

#[tokio::test]
async fn a_command_still_running_at_its_timeout_moves_to_the_background() {
    let root = tempfile::tempdir().unwrap();
    let processes = session(root.path(), ProcessLimits::default()).await;
    let mut ends = processes.ends();
    let shell = moving_shell(root.path(), processes.clone(), 300).await;
    let input = ShellInput::new("printf before; sleep 0.6; printf after; exit 4");
    let (result, text) = run(&shell, "agent", input).await;
    let id = result.process_id.clone().unwrap();
    // Not stopped: no status yet, and what it wrote so far.
    assert_eq!(
        (result.exit_code, result.signal, result.timed_out),
        (None, None, false)
    );
    assert_eq!(result.timeout_ms, 300);
    assert_eq!(result.stdout.text, "before");
    assert!(
        text.contains(&id) && text.contains("process_read"),
        "{text}"
    );
    let end = next(&mut ends).await.unwrap();
    assert_eq!((end.end, end.exit_code), (ProcessEnd::Exited, Some(4)));
    assert_eq!(end.process_id.as_deref(), Some(id.as_str()));
    // Counted from the command's start, not from the move.
    assert!(end.elapsed_ms >= 550, "{}", end.elapsed_ms);
    // The rest of its output, from the move on.
    assert_eq!(stdout(&processes, "agent", &id).await, "after");
    assert!(processes.close().await);
}

#[tokio::test]
async fn a_moved_command_lives_until_the_process_tools_maximum_from_its_start() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        max_timeout_ms: 700,
        default_timeout_ms: 700,
        ..ProcessLimits::default()
    };
    let processes = session(root.path(), limits).await;
    let mut ends = processes.ends();
    let shell = moving_shell(root.path(), processes.clone(), 300).await;
    let (result, _) = run(&shell, "a", ShellInput::new("sleep 30")).await;
    assert!(result.process_id.is_some());
    let end = next(&mut ends).await.unwrap();
    assert_eq!(end.end, ProcessEnd::TimedOut);
    assert!((650..2_000).contains(&end.elapsed_ms), "{}", end.elapsed_ms);
    let status = processes
        .status(
            Some("a"),
            roomkit_tools_process::ProcessStatusInput::default(),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .data;
    assert_eq!(status.processes[0].timeout_ms, 700);
    assert!(processes.close().await);
}

#[tokio::test]
async fn no_output_is_lost_or_repeated_when_a_command_moves() {
    let root = tempfile::tempdir().unwrap();
    let processes = session(root.path(), ProcessLimits::default()).await;
    let mut ends = processes.ends();
    let shell = moving_shell(root.path(), processes.clone(), 200).await;
    let command =
        "i=1; while [ $i -le 300 ]; do echo $i; echo e$i >&2; i=$((i+1)); sleep 0.002; done";
    let (result, _) = run(&shell, "a", ShellInput::new(command)).await;
    let id = result.process_id.clone().unwrap();
    assert!(!result.stdout.text.is_empty(), "some lines before the move");
    next(&mut ends).await.unwrap();
    let read = processes
        .read(
            Some("a"),
            ProcessReadInput::new(&id),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .data;
    let lines = |prefix: &str| {
        (1..=300).fold(String::new(), |mut lines, i| {
            let _written = writeln!(lines, "{prefix}{i}");
            lines
        })
    };
    assert_eq!(result.stdout.text + &read.stdout.text, lines(""));
    assert_eq!(result.stderr.text + &read.stderr.text, lines("e"));
    assert!(processes.close().await);
}

#[tokio::test]
async fn a_move_the_session_refuses_stops_the_command_as_before() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        max_processes_per_owner: 1,
        ..ProcessLimits::default()
    };
    let processes = session(root.path(), limits).await;
    let cancel = CancellationToken::new();
    processes
        .start(Some("a"), ProcessStartInput::new("sleep 30"), &cancel)
        .await
        .unwrap();
    let shell = moving_shell(root.path(), processes.clone(), 200).await;
    let (result, text) = run(&shell, "a", ShellInput::new("printf before; sleep 30")).await;
    assert_eq!(result.process_id, None);
    assert!(result.timed_out);
    assert_eq!(result.signal, Some(9));
    assert_eq!(result.stdout.text, "before");
    assert!(text.contains("not moved to the background"), "{text}");
    // Another owner still has room.
    let (moved, _) = run(&shell, "b", ShellInput::new("sleep 30")).await;
    assert!(moved.process_id.is_some());
    assert!(processes.close().await);
}

#[tokio::test]
async fn finished_moved_commands_make_room_for_the_next_move() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        max_processes_per_owner: 2,
        ..ProcessLimits::default()
    };
    let processes = session(root.path(), limits).await;
    let mut ends = processes.ends();
    let shell = moving_shell(root.path(), processes.clone(), 100).await;
    let build = || ShellInput::new("sleep 0.3; echo built");
    for _ in 0..2 {
        assert!(run(&shell, "a", build()).await.0.process_id.is_some());
    }
    for _ in 0..2 {
        assert_eq!(next(&mut ends).await.unwrap().end, ProcessEnd::Exited);
    }
    // Moved, not killed: the finished records make room for it.
    let (third, text) = run(&shell, "a", build()).await;
    assert!(third.process_id.is_some(), "{text}");
    assert!(!third.timed_out, "{text}");
    assert!(processes.close().await);
}

/// A shell with a process session, a 300 ms maximum, and the option to move.
async fn bounded_shell(
    root: &std::path::Path,
    processes: ProcessSession,
    moves: bool,
) -> ShellTool {
    ShellTool::new(root)
        .await
        .unwrap()
        .with_processes(processes)
        .with_limits(ShellLimits {
            default_timeout_ms: 100,
            max_timeout_ms: 300,
            background_on_timeout: moves,
            ..ShellLimits::default()
        })
        .unwrap()
}

fn lasting(command: &str, timeout_ms: u64) -> ShellInput {
    ShellInput {
        timeout_ms: NonZeroU64::new(timeout_ms),
        ..ShellInput::new(command)
    }
}

#[tokio::test]
async fn a_timeout_above_the_maximum_is_capped_when_the_command_may_move() {
    let root = tempfile::tempdir().unwrap();
    let processes = session(root.path(), ProcessLimits::default()).await;
    let shell = bounded_shell(root.path(), processes.clone(), true).await;
    let (quick, text) = run(&shell, "a", lasting("printf done", 600_000)).await;
    assert_eq!((quick.exit_code, quick.timeout_ms), (Some(0), 300));
    assert_eq!(
        text,
        "exit_code=0\ntimeout_ms 600000 capped to the host maximum of 300 ms\nstdout:\ndone\n"
    );
    // A longer one runs until the maximum, then moves.
    let (long, text) = run(&shell, "a", lasting("sleep 30", 600_000)).await;
    assert!(long.process_id.is_some(), "{text}");
    assert_eq!(long.timeout_ms, 300);
    assert!(
        text.contains("\ntimeout_ms 600000 capped to the host maximum of 300 ms\n"),
        "{text}"
    );
    assert!(processes.close().await);
}

#[tokio::test]
async fn a_timeout_above_the_maximum_is_refused_naming_run_in_background() {
    let root = tempfile::tempdir().unwrap();
    let processes = session(root.path(), ProcessLimits::default()).await;
    let shell = bounded_shell(root.path(), processes.clone(), false).await;
    let refused = shell
        .shell_owned("a", lasting("true", 301), &CancellationToken::new())
        .await;
    let Err(ToolError::InvalidInput(message)) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(
        message,
        "timeout_ms exceeds the host maximum of 300 ms; for a longer job, set run_in_background"
    );
    assert!(processes.close().await);
}

#[tokio::test]
async fn a_cancelled_command_is_never_moved() {
    let root = tempfile::tempdir().unwrap();
    let processes = session(root.path(), ProcessLimits::default()).await;
    let shell = moving_shell(root.path(), processes.clone(), 10_000).await;
    let cancel = CancellationToken::new();
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });
    let result = shell
        .shell_owned("a", ShellInput::new("sleep 30"), &cancel)
        .await;
    assert!(matches!(result, Err(ToolError::Cancelled)), "{result:?}");
    let status = processes
        .status(
            Some("a"),
            roomkit_tools_process::ProcessStatusInput::default(),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .data;
    assert!(status.processes.is_empty());
    assert!(processes.close().await);
}

/// Whether the process group `pgid` still has a member.
fn running(pgid: i32) -> bool {
    let group = rustix::process::Pid::from_raw(pgid).unwrap();
    rustix::process::test_kill_process_group(group).is_ok()
}

#[tokio::test]
async fn a_moved_command_stops_with_the_session_or_outlives_it_detached() {
    for detach in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let processes = session(root.path(), ProcessLimits::default()).await;
        let shell = moving_shell(root.path(), processes.clone(), 200).await;
        // The shell itself writes the mark once its child is done.
        let command = "echo $$; sleep 0.6; echo survived > mark";
        let (result, _) = run(&shell, "a", ShellInput::new(command)).await;
        assert!(result.process_id.is_some());
        let pgid: i32 = result.stdout.text.trim().parse().unwrap();
        assert!(running(pgid));
        if detach {
            assert!(processes.detach().await);
        } else {
            assert!(processes.close().await);
        }
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        let survived = root.path().join("mark").exists();
        assert_eq!(
            survived, detach,
            "a detached session lets it run on, a closed one stops it"
        );
    }
}

/// A moving shell that keeps the jobs of a command whose shell exits on its own.
async fn moving_and_keeping(root: &std::path::Path, processes: ProcessSession) -> ShellTool {
    ShellTool::new(root)
        .await
        .unwrap()
        .with_processes(processes)
        .with_limits(ShellLimits {
            default_timeout_ms: 200,
            keep_background_jobs: true,
            background_on_timeout: true,
            ..ShellLimits::default()
        })
        .unwrap()
}

#[tokio::test]
async fn a_moved_command_keeps_its_background_jobs_as_in_the_foreground() {
    let root = tempfile::tempdir().unwrap();
    let processes = session(root.path(), ProcessLimits::default()).await;
    let mut process_ends = processes.ends();
    let shell = moving_and_keeping(root.path(), processes.clone()).await;
    let mut shell_ends = shell.ends();
    // The job still holds the pipes and writes to them once the shell is
    // gone: its pipes go to a drainer, it is not killed.
    let command = "sleep 0.4; (sleep 1; echo late; echo survived > mark) & exit 0";
    let (result, _) = run(&shell, "a", ShellInput::new(command)).await;
    let id = result.process_id.clone().unwrap();
    let end = next(&mut process_ends).await.unwrap();
    assert_eq!(end.process_id.as_deref(), Some(id.as_str()));
    assert_eq!((end.end, end.exit_code), (ProcessEnd::Exited, Some(0)));
    // The shell tool follows the jobs and tells their end, as in the foreground.
    let kept = next(&mut shell_ends).await.unwrap();
    assert_eq!(
        (kept.tool, kept.end, kept.process_id.as_deref()),
        (JobTool::Shell, ProcessEnd::Exited, None)
    );
    assert_eq!(
        (kept.owner.as_deref(), kept.command.as_str()),
        (Some("a"), command)
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("mark")).unwrap(),
        "survived\n"
    );
    assert!(processes.close().await);
    assert!(shell.close().await);
}

#[tokio::test]
async fn a_moved_command_s_kept_jobs_stop_with_the_shell_or_outlive_it_detached() {
    for detach in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let processes = session(root.path(), ProcessLimits::default()).await;
        let mut process_ends = processes.ends();
        let shell = moving_and_keeping(root.path(), processes.clone()).await;
        let mut shell_ends = shell.ends();
        let command = "echo $$; sleep 0.4; sleep 60 > /dev/null 2>&1 & exit 0";
        let (result, _) = run(&shell, "a", ShellInput::new(command)).await;
        assert!(result.process_id.is_some());
        let pgid: i32 = result.stdout.text.trim().parse().unwrap();
        assert_eq!(
            next(&mut process_ends).await.unwrap().end,
            ProcessEnd::Exited
        );
        assert!(
            running(pgid),
            "the job runs on once the moved command's shell exits"
        );
        if detach {
            assert!(shell.detach().await);
            assert_eq!(next(&mut shell_ends).await, None, "let go untold");
        } else {
            assert!(shell.close().await);
            let stopped = next(&mut shell_ends).await.unwrap();
            assert_eq!(
                (stopped.tool, stopped.end),
                (JobTool::Shell, ProcessEnd::Stopped)
            );
        }
        let outlived = running(pgid);
        // The job is the test's to stop, whatever the assertions say.
        let _killed = rustix::process::kill_process_group(
            rustix::process::Pid::from_raw(pgid).unwrap(),
            rustix::process::Signal::KILL,
        );
        assert_eq!(outlived, detach, "close stops it, detach lets it go");
        assert!(processes.close().await);
    }
}

#[tokio::test]
async fn a_move_closes_the_shell_s_copies_whole_and_says_where_reads_start() {
    let root = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let outputs = roomkit_tools_process::OutputStore::new(
        Some(parent.path()),
        roomkit_tools_process::OutputLimits::default(),
    )
    .await
    .unwrap();
    let processes = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_output_store(outputs.clone())
        .unwrap();
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_processes(processes.clone())
        .with_limits(ShellLimits {
            max_output_bytes: 64,
            default_timeout_ms: 300,
            background_on_timeout: true,
            ..ShellLimits::default()
        })
        .unwrap()
        .with_output_store(outputs.clone());
    // 201 bytes, more than the result keeps, then the move.
    let command = "head -c 200 /dev/zero | tr '\\0' a; echo; sleep 0.6; printf after";
    let (result, text) = run(&shell, "a", ShellInput::new(command)).await;
    let id = result.process_id.clone().unwrap();
    let archive = result.stdout.archive.clone().unwrap();
    // Every byte until the move is stored: the copy is whole, not cut short.
    assert_eq!((archive.observed_bytes, archive.stored_bytes), (201, 201));
    assert_eq!((archive.complete, archive.error), (true, None));
    let archive_id = archive.archive_id.unwrap();
    let moved = format!(
        "still running after 300 ms: moved to the background as {id}; process_read reads \
         what it writes after the move, each stream from offset 0\n\
         [stdout archive {archive_id}: 201 of 201 bytes stored; output_read reads it]\n"
    );
    assert!(text.starts_with(&moved), "{text}");
    assert!(processes.close().await);
    assert!(outputs.close().await);
}

#[tokio::test]
async fn a_moved_command_keeps_its_archives_within_a_shared_store_s_quota() {
    let root = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let limits = roomkit_tools_process::OutputLimits {
        max_archives_per_owner: 2,
        ..roomkit_tools_process::OutputLimits::default()
    };
    let outputs = roomkit_tools_process::OutputStore::new(Some(parent.path()), limits)
        .await
        .unwrap();
    let processes = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_output_store(outputs.clone())
        .unwrap();
    let shell = moving_shell(root.path(), processes.clone(), 200)
        .await
        .with_output_store(outputs.clone());
    let command = "printf before; sleep 0.4; printf after";
    let (result, _) = run(&shell, "a", ShellInput::new(command)).await;
    let id = result.process_id.clone().unwrap();
    // Whole until the move, the shell's copies are released: the process's
    // take the owner's two archives.
    assert_eq!(result.stdout.archive, None);
    let input = roomkit_tools_process::ProcessStatusInput {
        process_id: Some(id),
    };
    let status = processes
        .status(Some("a"), input, &CancellationToken::new())
        .await
        .unwrap()
        .data;
    let archive = status.processes[0].stdout_archive.clone().unwrap();
    assert!(archive.archive_id.is_some(), "{archive:?}");
    assert!(processes.close().await);
    outputs.close().await;
}
