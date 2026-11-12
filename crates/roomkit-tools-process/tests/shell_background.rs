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
