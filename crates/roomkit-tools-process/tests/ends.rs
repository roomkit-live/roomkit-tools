//! A host is told when a background job ends, without polling it.
#![cfg(unix)]

use std::num::NonZeroU64;
use std::time::Duration;

use roomkit_tools_process::{
    JobEnd, JobEnds, JobTool, ProcessEnd, ProcessSession, ProcessStartInput, ProcessStopInput,
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

#[expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]
async fn start(session: &ProcessSession, owner: &str, input: ProcessStartInput) -> String {
    session
        .start(Some(owner), input, &CancellationToken::new())
        .await
        .unwrap()
        .data
        .process_id
}

#[tokio::test]
async fn an_exit_is_told_once_with_its_owner_command_and_code() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("sub")).unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    let mut ends = session.ends();
    let input = ProcessStartInput {
        cwd: Some("sub".into()),
        ..ProcessStartInput::new("sleep 0.2; exit 3")
    };
    let id = start(&session, "room-agent", input).await;
    let end = next(&mut ends).await.unwrap();
    assert_eq!(
        end,
        JobEnd {
            owner: Some("room-agent".into()),
            tool: JobTool::Process,
            process_id: Some(id),
            command: "sleep 0.2; exit 3".into(),
            cwd: "sub".into(),
            end: ProcessEnd::Exited,
            exit_code: Some(3),
            signal: None,
            elapsed_ms: end.elapsed_ms,
        }
    );
    // Counted from the spawn, a little after the shell started.
    assert!(end.elapsed_ms >= 150, "{}", end.elapsed_ms);
    assert!(session.close().await);
    assert_eq!(next(&mut ends).await, None, "told once, then the ends stop");
}

#[tokio::test]
async fn a_timeout_and_a_stop_are_told_as_such_to_every_subscriber() {
    let root = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    let mut first = session.ends();
    let mut second = session.ends();
    let timed = ProcessStartInput {
        timeout_ms: NonZeroU64::new(100),
        ..ProcessStartInput::new("sleep 30")
    };
    start(&session, "a", timed).await;
    let ended = next(&mut first).await.unwrap();
    assert_eq!(ended.end, ProcessEnd::TimedOut);
    assert_eq!(next(&mut second).await.unwrap(), ended);
    let stopped = start(&session, "a", ProcessStartInput::new("sleep 30")).await;
    session
        .stop(
            Some("a"),
            ProcessStopInput::new(stopped.clone()),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let ended = next(&mut first).await.unwrap();
    assert_eq!(
        (ended.process_id.as_deref(), ended.end),
        (Some(stopped.as_str()), ProcessEnd::Stopped)
    );
    assert_eq!(next(&mut second).await.unwrap(), ended);
}

#[tokio::test]
async fn an_end_before_the_subscription_is_not_told() {
    let root = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    let mut early = session.ends();
    start(&session, "a", ProcessStartInput::new("true")).await;
    next(&mut early).await.unwrap();
    let mut late = session.ends();
    start(&session, "a", ProcessStartInput::new("exit 1")).await;
    assert_eq!(next(&mut late).await.unwrap().exit_code, Some(1));
}

#[tokio::test]
async fn a_detached_job_is_let_go_and_not_told() {
    let root = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    let mut ends = session.ends();
    let id = start(&session, "a", ProcessStartInput::new("sleep 30")).await;
    assert!(session.detach().await);
    assert_eq!(next(&mut ends).await, None, "{id} was let go, not ended");
}

#[tokio::test]
async fn closing_tells_the_stops_it_makes_then_ends() {
    let root = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    let mut ends = session.ends();
    start(&session, "a", ProcessStartInput::new("sleep 30")).await;
    assert!(session.close().await);
    assert_eq!(next(&mut ends).await.unwrap().end, ProcessEnd::Stopped);
    assert_eq!(next(&mut ends).await, None);
    assert_eq!(
        next(&mut session.ends()).await,
        None,
        "closed: nothing to tell"
    );
}

mod shell {
    use std::time::Duration;

    use roomkit_tools_process::{JobEnd, JobTool, ProcessEnd, ShellInput, ShellLimits, ShellTool};
    use tokio_util::sync::CancellationToken;

    use super::next;

    #[expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]
    async fn keeping(root: &std::path::Path, keep_background_jobs: bool) -> ShellTool {
        ShellTool::new(root)
            .await
            .unwrap()
            .with_limits(ShellLimits {
                keep_background_jobs,
                ..ShellLimits::default()
            })
            .unwrap()
    }

    #[expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]
    async fn run(shell: &ShellTool, command: &str) {
        let result = shell
            .shell_owned(
                "room-agent",
                ShellInput::new(command),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.data.exit_code, Some(0), "{}", result.text);
    }

    #[tokio::test]
    async fn a_command_s_jobs_are_told_once_they_all_ended() {
        let root = tempfile::tempdir().unwrap();
        let shell = keeping(root.path(), true).await;
        let mut ends = shell.ends();
        let command = "sleep 0.3 >/dev/null 2>&1 & sleep 0.8 >/dev/null 2>&1 & echo started";
        run(&shell, command).await;
        let end = next(&mut ends).await.unwrap();
        assert_eq!(
            end,
            JobEnd {
                owner: Some("room-agent".into()),
                tool: JobTool::Shell,
                process_id: None,
                command: command.into(),
                cwd: ".".into(),
                end: ProcessEnd::Exited,
                exit_code: None,
                signal: None,
                elapsed_ms: end.elapsed_ms,
            }
        );
        assert!(
            end.elapsed_ms >= 800,
            "told after the last job: {}",
            end.elapsed_ms
        );
        assert!(shell.close().await);
        assert_eq!(next(&mut ends).await, None, "one end per command");
    }

    #[tokio::test]
    async fn a_command_that_left_no_job_or_was_cleaned_up_is_not_told() {
        let root = tempfile::tempdir().unwrap();
        let kept = keeping(root.path(), true).await;
        let mut kept_ends = kept.ends();
        run(&kept, "true").await;
        let stopped = keeping(root.path(), false).await;
        let mut stopped_ends = stopped.ends();
        run(&stopped, "sleep 0.3 >/dev/null 2>&1 &").await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(kept.close().await && stopped.close().await);
        assert_eq!(next(&mut kept_ends).await, None);
        assert_eq!(next(&mut stopped_ends).await, None);
    }

    #[tokio::test]
    async fn closing_stops_the_jobs_tells_them_then_refuses_commands() {
        let root = tempfile::tempdir().unwrap();
        let shell = keeping(root.path(), true).await;
        let mut ends = shell.ends();
        run(&shell, "sleep 30 >/dev/null 2>&1 &").await;
        let clone = shell.clone();
        assert!(clone.close().await);
        assert_eq!(next(&mut ends).await.unwrap().end, ProcessEnd::Stopped);
        assert_eq!(next(&mut ends).await, None);
        let refused = shell
            .shell(ShellInput::new("true"), &CancellationToken::new())
            .await;
        assert!(
            matches!(
                refused,
                Err(roomkit_tools_core::ToolError::AccessRefused(_))
            ),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn detaching_lets_the_jobs_run_on_untold() {
        let root = tempfile::tempdir().unwrap();
        let shell = keeping(root.path(), true).await;
        let mut ends = shell.ends();
        run(&shell, "(sleep 0.5; touch left) >/dev/null 2>&1 &").await;
        assert!(shell.detach().await);
        assert_eq!(next(&mut ends).await, None);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(root.path().join("left").exists(), "the job ran on");
    }
}
