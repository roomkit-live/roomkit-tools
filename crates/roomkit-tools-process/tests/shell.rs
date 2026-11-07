//! Observable process, output, boundary and cleanup contracts.
#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use roomkit_tools_core::ToolError;
use roomkit_tools_process::{CommandEnv, ShellInput, ShellLimits, ShellTool};
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
    // The model sees the directory relative to the workspace, never the host's.
    assert_eq!(result.cwd, "menus");
    assert_eq!(std::env::current_dir().unwrap(), before);
    let token = CancellationToken::new();
    let (a, b) = tokio::join!(
        tool.shell(ShellInput::new("pwd"), &token),
        tool.shell(ShellInput::new("pwd"), &token)
    );
    assert_eq!(a.unwrap().data.cwd, ".");
    assert_eq!(b.unwrap().data.cwd, ".");
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
            Err(ToolError::InvalidInput(_))
        ));
    }
    let mut input = ShellInput::new("touch must-not-exist");
    input.cwd = Some(outside.path().display().to_string());
    assert!(matches!(
        tool.shell(input.clone(), &token).await,
        Err(ToolError::AccessRefused(_))
    ));
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
    input.cwd = Some("escape".into());
    assert!(matches!(
        tool.shell(input, &token).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(!outside.path().join("must-not-exist").exists());
    let mut limits = ShellLimits {
        max_command_bytes: 2,
        ..ShellLimits::default()
    };
    let bounded = tool.clone().with_limits(limits).unwrap();
    assert!(matches!(
        bounded.shell(ShellInput::new("touch x"), &token).await,
        Err(ToolError::LimitExceeded(_))
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
        Err(ToolError::Cancelled)
    ));
    assert!(!root.path().join("unexpected").exists());
}

#[tokio::test]
async fn large_outputs_keep_head_and_tail_and_continue_draining_both_pipes() {
    let root = tempfile::tempdir().unwrap();
    let limits = ShellLimits {
        max_output_bytes: 100,
        ..ShellLimits::default()
    };
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
    let limits = ShellLimits {
        max_output_bytes: 1,
        ..ShellLimits::default()
    };
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
async fn timeout_keeps_partial_output_and_refuses_a_duration_above_the_maximum() {
    let root = tempfile::tempdir().unwrap();
    let limits = ShellLimits {
        default_timeout_ms: 300,
        max_timeout_ms: 300,
        ..ShellLimits::default()
    };
    let tool = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let command = "printf started; sleep 60 & printf '%s %s' $$ $! > pids; wait";
    let mut input = ShellInput::new(command);
    input.timeout_ms = std::num::NonZeroU64::new(60_000);
    match tool.shell(input, &CancellationToken::new()).await {
        Err(ToolError::InvalidInput(message)) => {
            assert_eq!(message, "timeout_ms exceeds the host maximum of 300 ms");
        }
        other => panic!("{other:?}"),
    }
    assert!(!root.path().join("pids").exists(), "nothing ran");
    let mut input = ShellInput::new(command);
    input.timeout_ms = std::num::NonZeroU64::new(300);
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

/// Whether `id` is reaped within five seconds: its `/proc` entry is gone,
/// not only a zombie.
#[cfg(target_os = "linux")]
async fn reaped(id: u32) -> bool {
    let entry = format!("/proc/{id}");
    tokio::time::timeout(Duration::from_secs(5), async {
        while tokio::fs::try_exists(&entry).await.unwrap_or(true) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_ok()
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
            assert!(matches!(task.await.unwrap(), Err(ToolError::Cancelled)));
        }
        stopped(&ids).await;
        // A dropped call leaves the reaping to its detached worker, so the
        // shell can still be a zombie when `stopped` returns.
        assert!(reaped(ids[0]).await, "direct shell was not reaped");
    }
}

/// Process group, session and controlling terminal of a `/proc/<pid>/stat` line.
#[cfg(target_os = "linux")]
#[expect(
    clippy::unwrap_used,
    reason = "a malformed stat line must fail the test"
)]
fn session_fields(stat: &str) -> [u32; 3] {
    // After the parenthesised name: state, ppid, pgrp, session, tty_nr.
    let fields: Vec<u32> = stat
        .rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .skip(2)
        .take(3)
        .map(|field| field.parse().unwrap())
        .collect();
    fields.try_into().unwrap()
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn each_command_leads_a_new_session_without_the_host_terminal() {
    let root = tempfile::tempdir().unwrap();
    let result = execute(root.path(), "printf '%s ' $$; cat /proc/$$/stat").await;
    let (pid, stat) = result.stdout.text.split_once(' ').unwrap();
    let pid: u32 = pid.parse().unwrap();
    let [group, session, terminal] = session_fields(stat);
    let [_, host, _] = session_fields(&std::fs::read_to_string("/proc/self/stat").unwrap());
    assert_eq!(session, pid, "the shell leads its own session");
    assert_ne!(session, host);
    assert_eq!(group, pid, "group signals still target the shell's pid");
    assert_eq!(terminal, 0, "no controlling terminal");
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

/// A shell that keeps the background jobs of a command that exits on its own.
#[expect(clippy::unwrap_used, reason = "fixture failures must fail the test")]
async fn keeping(root: &Path) -> ShellTool {
    ShellTool::new(root)
        .await
        .unwrap()
        .with_limits(ShellLimits {
            keep_background_jobs: true,
            ..ShellLimits::default()
        })
        .unwrap()
}

/// Whether `id` runs, neither gone nor a zombie.
#[cfg(target_os = "linux")]
async fn running(id: u32) -> bool {
    tokio::fs::read_to_string(format!("/proc/{id}/stat"))
        .await
        .is_ok_and(|stat| {
            !stat
                .rsplit_once(") ")
                .is_some_and(|(_, rest)| rest.starts_with(['Z', 'X']))
        })
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_kept_background_job_outlives_the_command_that_started_it() {
    let root = tempfile::tempdir().unwrap();
    let result = keeping(root.path())
        .await
        .shell(
            ShellInput::new("sleep 60 > /dev/null 2>&1 & printf '%s %s' $$ $! > pids; exit 0"),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .data;
    assert_eq!(result.exit_code, Some(0));
    let ids = pids(&root.path().join("pids")).await;
    let (shell, job) = (ids[0], ids[1]);
    assert!(reaped(shell).await, "the shell itself is reaped");
    let kept = running(job).await;
    // The job is the test's to stop, whatever the assertion says.
    std::process::Command::new("kill")
        .args(["-KILL", &job.to_string()])
        .status()
        .unwrap();
    assert!(kept, "the background job runs on after the call");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_timeout_still_stops_the_jobs_a_kept_shell_started() {
    let root = tempfile::tempdir().unwrap();
    let mut input = ShellInput::new("sleep 60 & printf '%s %s' $$ $! > pids; sleep 60");
    input.timeout_ms = std::num::NonZeroU64::new(300);
    let result = keeping(root.path())
        .await
        .shell(input, &CancellationToken::new())
        .await
        .unwrap()
        .data;
    assert!(result.timed_out);
    stopped(&pids(&root.path().join("pids")).await).await;
}

#[tokio::test]
async fn host_variables_outside_the_default_list_are_not_passed() {
    // Cargo sets this in the test process; it stands for any host secret.
    assert!(std::env::var_os("CARGO_PKG_NAME").is_some());
    let root = tempfile::tempdir().unwrap();
    let result = execute(
        root.path(),
        r#"printf '%s|%s' "${CARGO_PKG_NAME-unset}" "${PATH:+path}""#,
    )
    .await;
    assert_eq!(result.stdout.text, "unset|path");
}

#[tokio::test]
async fn host_lists_inherited_variables_and_sets_explicit_values() {
    let root = tempfile::tempdir().unwrap();
    let mut env = CommandEnv {
        inherit_env: vec!["CARGO_PKG_NAME".into(), "PATH".into()],
        ..CommandEnv::default()
    };
    env.env.insert("GREETING".into(), "hello world".into());
    env.env
        .insert("CARGO_PKG_NAME".into(), "explicit wins".into());
    let tool = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_env(env)
        .unwrap();
    let result = tool
        .shell(
            ShellInput::new(
                r#"printf '%s|%s|%s|%s' "$GREETING" "$CARGO_PKG_NAME" "${PATH:+path}" "${HOME-unset}""#,
            ),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .data;
    assert_eq!(result.stdout.text, "hello world|explicit wins|path|unset");
    let mut listed = CommandEnv::default();
    listed.inherit_env.push("CARGO_PKG_NAME".into());
    let result = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_env(listed)
        .unwrap()
        .shell(
            ShellInput::new(r#"printf %s "$CARGO_PKG_NAME""#),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .data;
    assert_eq!(result.stdout.text, "roomkit-tools-process");
}

#[tokio::test]
async fn invalid_environment_names_and_values_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let tool = ShellTool::new(root.path()).await.unwrap();
    for name in ["", "A=B", "A\0B"] {
        let mut inherited = CommandEnv::default();
        inherited.inherit_env.push(name.into());
        assert!(matches!(
            tool.clone().with_env(inherited),
            Err(ToolError::InvalidInput(_))
        ));
        let mut explicit = CommandEnv::default();
        explicit.env.insert(name.into(), "value".into());
        assert!(matches!(
            tool.clone().with_env(explicit),
            Err(ToolError::InvalidInput(_))
        ));
    }
    let mut value = CommandEnv::default();
    value.env.insert("NAME".into(), "a\0b".into());
    assert!(matches!(
        tool.with_env(value),
        Err(ToolError::InvalidInput(_))
    ));
    assert_eq!(
        CommandEnv::default().inherit_env,
        [
            "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "LC_CTYPE", "TZ",
            "TMPDIR"
        ]
    );
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
