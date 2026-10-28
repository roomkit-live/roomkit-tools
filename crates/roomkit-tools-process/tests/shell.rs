//! Observable process, output, boundary and cleanup contracts.
#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use roomkit_tools_core::Error;
use roomkit_tools_process::{ShellInput, ShellLimits, ShellTool};
use tokio_util::sync::CancellationToken;

#[expect(clippy::unwrap_used, reason = "fixture failures must fail the test")]
async fn execute(root: &Path, command: &str) -> roomkit_tools_process::ShellResult {
    ShellTool::new(root)
        .await
        .unwrap()
        .shell(ShellInput::new(command), &CancellationToken::new())
        .await
        .unwrap()
        .data
}

#[tokio::test]
async fn shell_syntax_separate_streams_and_nonzero_exit_are_results() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("hours"), "Saturday: 08:30-16:00\n").unwrap();
    let result = execute(
        root.path(),
        "cat hours | tr a-z A-Z; printf 'warning' >&2; exit 7",
    )
    .await;
    assert_eq!(result.stdout.text, "SATURDAY: 08:30-16:00\n");
    assert_eq!(result.stderr.text, "warning");
    assert_eq!(result.exit_code, Some(7));
    assert_eq!(result.signal, None);
    assert!(!result.timed_out);
    assert!(result.output_complete);
}

#[tokio::test]
async fn cwd_is_per_call_canonical_and_does_not_change_the_host_directory() {
    let root = tempfile::tempdir().unwrap();
    let before = std::env::current_dir().unwrap();
    std::fs::create_dir(root.path().join("menus")).unwrap();
    let tool = ShellTool::new(root.path()).await.unwrap();
    let mut input = ShellInput::new("pwd");
    input.cwd = Some("menus".into());
    let result = tool
        .shell(input, &CancellationToken::new())
        .await
        .unwrap()
        .data;
    assert_eq!(
        result.stdout.text.trim(),
        root.path().join("menus").to_str().unwrap()
    );
    assert_eq!(std::env::current_dir().unwrap(), before);
    let token = CancellationToken::new();
    let (a, b) = tokio::join!(
        tool.shell(ShellInput::new("pwd"), &token),
        tool.shell(ShellInput::new("pwd"), &token)
    );
    assert_eq!(a.unwrap().data.cwd, b.unwrap().data.cwd);
}

#[tokio::test]
async fn validates_inputs_limits_and_initial_cwd_before_spawning() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let tool = ShellTool::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    for command in ["", " \n", "bad\0command"] {
        assert!(matches!(
            tool.shell(ShellInput::new(command), &token).await,
            Err(Error::InvalidInput(_))
        ));
    }
    let mut input = ShellInput::new("touch must-not-exist");
    input.cwd = Some(outside.path().display().to_string());
    assert!(matches!(
        tool.shell(input.clone(), &token).await,
        Err(Error::Refused(_))
    ));
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
    input.cwd = Some("escape".into());
    assert!(matches!(
        tool.shell(input, &token).await,
        Err(Error::Refused(_))
    ));
    assert!(!outside.path().join("must-not-exist").exists());
    let mut limits = ShellLimits::default();
    limits.max_command_bytes = 2;
    let bounded = tool.clone().with_limits(limits).unwrap();
    assert!(matches!(
        bounded.shell(ShellInput::new("touch x"), &token).await,
        Err(Error::LimitExceeded(_))
    ));
    limits.max_output_bytes = 0;
    assert!(tool.with_limits(limits).is_err());
    assert!(serde_json::from_str::<ShellInput>(r#"{"command":"pwd","timeout_ms":0}"#).is_err());
    assert!(serde_json::from_str::<ShellInput>(r#"{"command":"pwd","sandbox":true}"#).is_err());
    assert_eq!(
        ShellTool::definition().input_schema["additionalProperties"],
        false
    );
}

#[tokio::test]
async fn cancelled_before_start_has_no_side_effect() {
    let root = tempfile::tempdir().unwrap();
    let token = CancellationToken::new();
    token.cancel();
    let tool = ShellTool::new(root.path()).await.unwrap();
    assert!(matches!(
        tool.shell(ShellInput::new("touch unexpected"), &token)
            .await,
        Err(Error::Cancelled)
    ));
    assert!(!root.path().join("unexpected").exists());
}

#[tokio::test]
async fn large_outputs_keep_head_and_tail_and_continue_draining_both_pipes() {
    let root = tempfile::tempdir().unwrap();
    let mut limits = ShellLimits::default();
    limits.max_output_bytes = 100;
    let tool = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let result = tool.shell(ShellInput::new("printf HEAD; head -c 262144 /dev/zero; printf TAIL; printf ERR >&2; head -c 262144 /dev/zero >&2; printf END >&2"), &CancellationToken::new()).await.unwrap().data;
    assert_eq!(result.exit_code, Some(0));
    assert!(result.output_complete);
    assert!(result.stdout.text.starts_with("HEAD"));
    assert!(result.stdout.text.ends_with("TAIL"));
    assert!(result.stderr.text.starts_with("ERR"));
    assert!(result.stderr.text.ends_with("END"));
    for (stream, bytes) in [(&result.stdout, 262_152), (&result.stderr, 262_150)] {
        assert_eq!(stream.total_bytes, bytes);
        assert_eq!(stream.omitted_bytes, bytes - 100);
        assert!(stream.truncated);
    }
}

#[tokio::test]
async fn utf8_binary_and_single_byte_budget_have_explicit_lossy_views() {
    let root = tempfile::tempdir().unwrap();
    let result = execute(root.path(), "printf 'πxπ'; printf '\\377' >&2").await;
    assert_eq!(result.stdout.text, "πxπ");
    assert_eq!(result.stdout.total_bytes, 5);
    assert_eq!(result.stderr.text, "\u{fffd}");
    assert_eq!(result.stderr.total_bytes, 1);
    let mut limits = ShellLimits::default();
    limits.max_output_bytes = 1;
    let tool = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let result = tool
        .shell(ShellInput::new("printf abc"), &CancellationToken::new())
        .await
        .unwrap()
        .data;
    assert!(result.stdout.text.starts_with('a'));
    assert_eq!(result.stdout.omitted_bytes, 2);
}

#[tokio::test]
async fn stdin_is_eof_and_signal_exit_is_distinct_from_an_exit_code() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(execute(root.path(), "cat").await.exit_code, Some(0));
    let result = execute(root.path(), "kill -TERM $$").await;
    assert_eq!(result.exit_code, None);
    assert_eq!(result.signal, Some(15));
    assert!(!result.timed_out);
}

#[tokio::test]
async fn timeout_keeps_partial_output_and_clamps_the_requested_duration() {
    let root = tempfile::tempdir().unwrap();
    let mut limits = ShellLimits::default();
    limits.default_timeout_ms = 300;
    limits.max_timeout_ms = 300;
    let tool = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let mut input = ShellInput::new("printf started; sleep 60 & printf '%s %s' $$ $! > pids; wait");
    input.timeout_ms = std::num::NonZeroU64::new(60_000);
    let result = tool
        .shell(input, &CancellationToken::new())
        .await
        .unwrap()
        .data;
    assert!(result.timed_out);
    assert_eq!(result.timeout_ms, 300);
    assert_eq!(result.stdout.text, "started");
    assert_eq!(result.signal, Some(9));
    assert!(result.output_complete);
    #[cfg(target_os = "linux")]
    stopped(&pids(&root.path().join("pids")).await).await;
}

#[tokio::test]
async fn closing_pipes_does_not_turn_a_running_process_into_success() {
    let root = tempfile::tempdir().unwrap();
    let mut input = ShellInput::new("exec 1>&- 2>&-; sleep 60");
    input.timeout_ms = std::num::NonZeroU64::new(100);
    let result = ShellTool::new(root.path())
        .await
        .unwrap()
        .shell(input, &CancellationToken::new())
        .await
        .unwrap();
    assert!(result.data.timed_out);
}

#[cfg(target_os = "linux")]
#[expect(clippy::unwrap_used, reason = "fixture readiness must be observed")]
async fn pids(path: &Path) -> Vec<u32> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let contents = tokio::fs::read_to_string(path).await.unwrap_or_default();
            let ids: Vec<u32> = contents
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            if ids.len() == 2 {
                return ids;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[cfg(target_os = "linux")]
#[expect(clippy::unwrap_used, reason = "cleanup failure must fail the test")]
async fn stopped(ids: &[u32]) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut alive = false;
            for id in ids {
                if let Ok(stat) = tokio::fs::read_to_string(format!("/proc/{id}/stat")).await {
                    // Orphans are reaped by the system's init; a zombie cannot run.
                    alive |= !stat.rsplit_once(") ").unwrap().1.starts_with(['Z', 'X']);
                }
            }
            if !alive {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancellation_and_dropped_call_stop_the_shell_and_its_background_child() {
    for drop_call in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let tool = ShellTool::new(root.path()).await.unwrap();
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let task = tokio::spawn(async move {
            tool.shell(
                ShellInput::new("sleep 60 & printf '%s %s' $$ $! > pids; wait"),
                &worker_token,
            )
            .await
        });
        let ids = pids(&root.path().join("pids")).await;
        if drop_call {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            token.cancel();
            assert!(matches!(task.await.unwrap(), Err(Error::Cancelled)));
        }
        stopped(&ids).await;
        assert!(
            !Path::new(&format!("/proc/{}", ids[0])).exists(),
            "direct shell was not reaped"
        );
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn successful_shell_exit_also_terminates_its_background_jobs() {
    let root = tempfile::tempdir().unwrap();
    let result = execute(
        root.path(),
        "sleep 60 & printf '%s %s' $$ $! > pids; exit 0",
    )
    .await;
    assert_eq!(result.exit_code, Some(0));
    assert!(result.output_complete);
    stopped(&pids(&root.path().join("pids")).await).await;
}

#[tokio::test]
async fn cwd_checks_are_not_a_sandbox() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(parent.path().join("outside"), "host permissions apply").unwrap();
    assert_eq!(
        execute(&root, "cat ../outside").await.stdout.text,
        "host permissions apply"
    );
}
