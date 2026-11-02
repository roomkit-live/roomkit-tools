//! Real process ownership, incremental I/O and lifecycle tests.
#![cfg(unix)]

use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Duration;

use roomkit_tools_core::ToolError;
use roomkit_tools_process::{
    CommandEnv, ProcessEnd, ProcessLimits, ProcessReadInput, ProcessSession, ProcessStartInput,
    ProcessState, ProcessStatus, ProcessStatusInput, ProcessStopInput, ProcessWriteInput,
};
use tokio_util::sync::CancellationToken;

#[expect(clippy::unwrap_used, reason = "fixture failure must fail tests")]
async fn finished(manager: &ProcessSession, owner: &str, id: &str) -> ProcessStatus {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let result = manager
                .status(
                    Some(owner),
                    ProcessStatusInput {
                        process_id: Some(id.into()),
                    },
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            if result.data.first().unwrap().state == ProcessState::Finished {
                return result.data.first().unwrap().clone();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[expect(
    clippy::unwrap_used,
    reason = "fixture must produce its readiness marker"
)]
async fn file(path: &Path) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let text = tokio::fs::read_to_string(path).await.unwrap_or_default();
            if !text.is_empty() {
                return text;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn stdin_eof_output_cursors_and_nonzero_exit() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let id = manager
        .start(
            Some("alice"),
            ProcessStartInput::new("cat; printf warning >&2; exit 7"),
            &token,
        )
        .await
        .unwrap()
        .data
        .process_id;
    assert_eq!(
        manager
            .status(Some("alice"), ProcessStatusInput::default(), &token)
            .await
            .unwrap()
            .data[0]
            .state,
        ProcessState::Running
    );
    let sent = manager
        .write(
            Some("alice"),
            ProcessWriteInput {
                process_id: id.clone(),
                text: "πxπ\nhello\n".into(),
                close_stdin: true,
            },
            &token,
        )
        .await
        .unwrap()
        .data;
    assert_eq!(sent.bytes_written, 12);
    assert!(sent.complete && sent.stdin_closed);
    let status = finished(&manager, "alice", &id).await;
    assert_eq!(status.exit_code, Some(7));
    assert_eq!(status.end, Some(ProcessEnd::Exited));
    assert!(status.output_complete);
    let mut input = ProcessReadInput::new(&id);
    input.max_bytes = NonZeroUsize::new(2);
    let mut out = Vec::new();
    let mut err = Vec::new();
    loop {
        let result = manager
            .read(Some("alice"), input.clone(), &token)
            .await
            .unwrap()
            .data;
        assert_eq!(result.stdout.omitted_bytes, 0);
        assert!(result.stdout.eof);
        out.extend(result.stdout.bytes);
        err.extend(result.stderr.bytes);
        input.stdout_offset = result.stdout.next_offset;
        input.stderr_offset = result.stderr.next_offset;
        if !result.stdout.has_more && !result.stderr.has_more {
            break;
        }
    }
    assert_eq!(out, "πxπ\nhello\n".as_bytes());
    assert_eq!(err, b"warning");
    assert_eq!(
        manager
            .read(Some("alice"), input.clone(), &token)
            .await
            .unwrap()
            .data
            .stdout
            .bytes,
        Vec::<u8>::new()
    );
    input.stdout_offset += 1;
    assert!(matches!(
        manager.read(Some("alice"), input, &token).await,
        Err(ToolError::InvalidInput(_))
    ));
    assert_eq!(
        manager
            .read(Some("alice"), ProcessReadInput::new(&id), &token)
            .await
            .unwrap()
            .data
            .stdout
            .bytes,
        out
    );
    assert!(manager.close().await);
}

#[tokio::test]
async fn output_is_visible_before_exit_and_polling_does_not_duplicate_it() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let id = manager
        .start(
            Some("a"),
            ProcessStartInput::new("printf first; read -r line; printf '%s' \"$line\""),
            &token,
        )
        .await
        .unwrap()
        .data
        .process_id;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let read = manager
                .read(Some("a"), ProcessReadInput::new(&id), &token)
                .await
                .unwrap()
                .data;
            if read.stdout.text == "first" {
                assert_eq!(read.status.state, ProcessState::Running);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut input = ProcessReadInput::new(&id);
    input.stdout_offset = 5;
    assert_eq!(
        manager
            .read(Some("a"), input.clone(), &token)
            .await
            .unwrap()
            .data
            .stdout
            .bytes,
        Vec::<u8>::new()
    );
    manager
        .write(
            Some("a"),
            ProcessWriteInput {
                process_id: id.clone(),
                text: "second\n".into(),
                close_stdin: false,
            },
            &token,
        )
        .await
        .unwrap();
    finished(&manager, "a", &id).await;
    assert_eq!(
        manager
            .read(Some("a"), input, &token)
            .await
            .unwrap()
            .data
            .stdout
            .text,
        "second"
    );
    manager.close().await;
}

#[tokio::test]
async fn status_and_observation_follow_creation_order_past_nine_jobs() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        max_processes_per_owner: 11,
        ..ProcessLimits::default()
    };
    let manager = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let token = CancellationToken::new();
    for _ in 0..11 {
        manager
            .start(Some("a"), ProcessStartInput::new("true"), &token)
            .await
            .unwrap();
    }
    let expected: Vec<String> = (1..=11).map(|n| format!("p{n}")).collect();
    let listed: Vec<String> = manager
        .status(Some("a"), ProcessStatusInput::default(), &token)
        .await
        .unwrap()
        .data
        .into_iter()
        .map(|status| status.process_id)
        .collect();
    assert_eq!(listed, expected);
    let observed: Vec<String> = manager
        .observe()
        .await
        .into_iter()
        .map(|process| process.status.process_id)
        .collect();
    assert_eq!(observed, expected);
    assert!(manager.close().await);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one lifecycle exercises quota ownership, retention and release"
)]
async fn isolation_quotas_forget_and_shutdown_are_explicit() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        max_processes: 2,
        max_processes_per_owner: 1,
        ..ProcessLimits::default()
    };
    let manager = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let token = CancellationToken::new();
    let id = manager
        .start(Some("a"), ProcessStartInput::new("cat"), &token)
        .await
        .unwrap()
        .data
        .process_id;
    assert!(
        manager
            .status(Some("b"), ProcessStatusInput::default(), &token)
            .await
            .unwrap()
            .data
            .is_empty()
    );
    assert!(matches!(
        manager
            .read(Some("b"), ProcessReadInput::new(&id), &token)
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(matches!(
        manager
            .write(
                Some("b"),
                ProcessWriteInput {
                    process_id: id.clone(),
                    text: "no".into(),
                    close_stdin: true
                },
                &token
            )
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(matches!(
        manager
            .stop(
                Some("b"),
                ProcessStopInput {
                    process_id: id.clone(),
                    forget: true
                },
                &token
            )
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(matches!(
        manager
            .start(
                Some("a"),
                ProcessStartInput::new("touch unexpected"),
                &token
            )
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
    assert!(!root.path().join("unexpected").exists());
    let stopped = manager
        .stop(
            Some("a"),
            ProcessStopInput {
                process_id: id.clone(),
                forget: false,
            },
            &token,
        )
        .await
        .unwrap()
        .data;
    assert_eq!(stopped.status.end, Some(ProcessEnd::Stopped));
    assert_eq!(stopped.status.signal, Some(9));
    assert!(!stopped.forgotten);
    assert!(matches!(
        manager
            .start(Some("a"), ProcessStartInput::new("true"), &token)
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
    assert!(
        manager
            .stop(
                Some("a"),
                ProcessStopInput {
                    process_id: id.clone(),
                    forget: true
                },
                &token
            )
            .await
            .unwrap()
            .data
            .forgotten
    );
    assert!(matches!(
        manager
            .read(Some("a"), ProcessReadInput::new(&id), &token)
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    let next = manager
        .start(Some("a"), ProcessStartInput::new("cat"), &token)
        .await
        .unwrap()
        .data
        .process_id;
    assert_ne!(next, id);
    manager
        .start(Some("b"), ProcessStartInput::new("cat"), &token)
        .await
        .unwrap();
    assert!(matches!(
        manager
            .start(Some("c"), ProcessStartInput::new("true"), &token)
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
    assert!(manager.close_owner("a").await);
    assert_eq!(
        manager
            .status(Some("b"), ProcessStatusInput::default(), &token)
            .await
            .unwrap()
            .data[0]
            .state,
        ProcessState::Running
    );
    assert!(manager.close().await);
    assert!(matches!(
        manager
            .start(Some("a"), ProcessStartInput::new("true"), &token)
            .await,
        Err(ToolError::AccessRefused(_))
    ));
}

#[tokio::test]
async fn no_owner_is_one_standalone_owner_with_its_own_quota() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        max_processes: 2,
        max_processes_per_owner: 1,
        ..ProcessLimits::default()
    };
    let session = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let token = CancellationToken::new();
    let id = session
        .start(None, ProcessStartInput::new("cat"), &token)
        .await
        .unwrap()
        .data
        .process_id;
    let listed = session
        .status(None, ProcessStatusInput::default(), &token)
        .await
        .unwrap()
        .data;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].process_id, id);
    // A named owner neither sees nor reaches the standalone owner's job.
    let named = session.status(Some("a"), ProcessStatusInput::default(), &token);
    assert!(named.await.unwrap().data.is_empty());
    assert!(matches!(
        session
            .read(Some("a"), ProcessReadInput::new(&id), &token)
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    // The per-owner quota counts the standalone owner's jobs as one owner's.
    assert!(matches!(
        session
            .start(None, ProcessStartInput::new("true"), &token)
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
    session
        .start(Some("a"), ProcessStartInput::new("cat"), &token)
        .await
        .unwrap();
    session
        .read(None, ProcessReadInput::new(&id), &token)
        .await
        .unwrap();
    let close = ProcessWriteInput {
        process_id: id.clone(),
        text: String::new(),
        close_stdin: true,
    };
    session.write(None, close, &token).await.unwrap();
    let stop = ProcessStopInput {
        process_id: id,
        forget: true,
    };
    session.stop(None, stop, &token).await.unwrap();
    assert!(session.close().await);
}

#[tokio::test]
async fn closed_reports_a_requested_shutdown_to_every_clone() {
    let root = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    let clone = session.clone();
    assert!(!session.closed());
    assert!(session.close().await);
    assert!(session.closed());
    assert!(clone.closed());
}

#[tokio::test]
async fn bounded_retention_exposes_every_gap_and_keeps_draining() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        max_output_bytes: 17,
        ..ProcessLimits::default()
    };
    let manager = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let token = CancellationToken::new();
    let id = manager.start(Some("a"), ProcessStartInput::new("head -c 131072 /dev/zero; printf TAIL; head -c 131072 /dev/zero >&2; printf ERR >&2"), &token).await.unwrap().data.process_id;
    finished(&manager, "a", &id).await;
    let result = manager
        .read(Some("a"), ProcessReadInput::new(&id), &token)
        .await
        .unwrap()
        .data;
    assert_eq!(result.stdout.total_bytes, 131_076);
    assert_eq!(result.stdout.omitted_bytes, 131_076 - 17);
    assert_eq!(result.stdout.bytes.len(), 17);
    assert!(result.stdout.bytes.ends_with(b"TAIL"));
    assert_eq!(result.stderr.omitted_bytes, 131_075 - 17);
    assert!(result.stderr.bytes.ends_with(b"ERR"));
    assert!(result.status.output_complete);
    manager.close().await;
}

#[tokio::test]
async fn lifetime_timeout_caps_requests_and_retains_output() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        default_timeout_ms: 200,
        max_timeout_ms: 200,
        ..ProcessLimits::default()
    };
    let manager = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let mut input = ProcessStartInput::new("printf ready; sleep 60");
    input.timeout_ms = std::num::NonZeroU64::new(60_000);
    let token = CancellationToken::new();
    let id = manager
        .start(Some("a"), input, &token)
        .await
        .unwrap()
        .data
        .process_id;
    let status = finished(&manager, "a", &id).await;
    assert_eq!(status.end, Some(ProcessEnd::TimedOut));
    assert_eq!(status.timeout_ms, 200);
    assert_eq!(status.signal, Some(9));
    assert_eq!(
        manager
            .read(Some("a"), ProcessReadInput::new(&id), &token)
            .await
            .unwrap()
            .data
            .stdout
            .text,
        "ready"
    );
    manager.close().await;
}

#[tokio::test]
async fn blocked_stdin_reports_partial_bytes_and_does_not_prevent_stop() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        max_write_bytes: 2 * 1024 * 1024,
        write_timeout_ms: 100,
        ..ProcessLimits::default()
    };
    let manager = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let token = CancellationToken::new();
    let id = manager
        .start(Some("a"), ProcessStartInput::new("sleep 60"), &token)
        .await
        .unwrap()
        .data
        .process_id;
    let result = manager
        .write(
            Some("a"),
            ProcessWriteInput {
                process_id: id.clone(),
                text: "x".repeat(limits.max_write_bytes),
                close_stdin: true,
            },
            &token,
        )
        .await
        .unwrap()
        .data;
    assert!(!result.complete && !result.stdin_closed);
    assert!(result.bytes_written > 0 && result.bytes_written < limits.max_write_bytes);
    assert_eq!(result.error.as_deref(), Some("stdin write timed out"));
    assert!(manager.close().await);
}

#[tokio::test]
async fn pre_cancel_invalid_input_and_root_checks_prevent_spawn() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        manager
            .start(
                Some("a"),
                ProcessStartInput::new("touch unexpected"),
                &token
            )
            .await,
        Err(ToolError::Cancelled)
    ));
    let token = CancellationToken::new();
    for command in ["", "\n", "bad\0command"] {
        assert!(matches!(
            manager
                .start(Some("a"), ProcessStartInput::new(command), &token)
                .await,
            Err(ToolError::InvalidInput(_))
        ));
    }
    let mut input = ProcessStartInput::new("touch unexpected");
    input.cwd = Some(outside.path().display().to_string());
    assert!(matches!(
        manager.start(Some("a"), input, &token).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(!outside.path().join("unexpected").exists());
    assert!(!root.path().join("unexpected").exists());
    assert!(
        serde_json::from_str::<ProcessStartInput>(r#"{"command":"pwd","timeout_ms":0}"#).is_err()
    );
    assert!(
        serde_json::from_str::<ProcessReadInput>(r#"{"process_id":"p1","owner":"other"}"#).is_err()
    );
    assert_eq!(
        ProcessSession::start_definition().input_schema["additionalProperties"],
        false
    );
    manager.close().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn dropping_last_manager_reaps_shell_and_stops_its_child() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    manager
        .start(
            Some("a"),
            ProcessStartInput::new("sleep 60 & printf '%s %s' $$ $! > pids; wait"),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let pids = file(&root.path().join("pids")).await;
    let ids: Vec<&str> = pids.split_whitespace().collect();
    assert_eq!(ids.len(), 2);
    drop(manager);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut alive = false;
            for id in &ids {
                if let Ok(stat) = tokio::fs::read_to_string(format!("/proc/{id}/stat")).await {
                    alive |= !stat.rsplit_once(") ").unwrap().1.starts_with(['Z', 'X']);
                }
            }
            if !alive && !Path::new(&format!("/proc/{}", ids[0])).exists() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn closed_output_pipes_do_not_finish_a_still_running_process() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let id = manager
        .start(
            Some("a"),
            ProcessStartInput::new("exec 1>&- 2>&-; printf ready > ready; sleep 60"),
            &token,
        )
        .await
        .unwrap()
        .data
        .process_id;
    file(&root.path().join("ready")).await;
    assert_eq!(
        manager
            .status(Some("a"), ProcessStatusInput::default(), &token)
            .await
            .unwrap()
            .data[0]
            .state,
        ProcessState::Running
    );
    let stopped = manager
        .stop(
            Some("a"),
            ProcessStopInput {
                process_id: id,
                forget: true,
            },
            &token,
        )
        .await
        .unwrap()
        .data;
    assert!(stopped.forgotten && stopped.status.output_complete);
}

#[tokio::test]
async fn concurrent_stdin_calls_preserve_each_payload_and_drain_all_bytes() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let id = manager
        .start(Some("a"), ProcessStartInput::new("cat"), &token)
        .await
        .unwrap()
        .data
        .process_id;
    let a = "x".repeat(32_768);
    let b = "y".repeat(32_768);
    let write = |text: String| ProcessWriteInput {
        process_id: id.clone(),
        text,
        close_stdin: false,
    };
    let (first, second) = tokio::join!(
        manager.write(Some("a"), write(a.clone()), &token),
        manager.write(Some("a"), write(b.clone()), &token)
    );
    assert!(first.unwrap().data.complete && second.unwrap().data.complete);
    manager
        .write(
            Some("a"),
            ProcessWriteInput {
                process_id: id.clone(),
                text: String::new(),
                close_stdin: true,
            },
            &token,
        )
        .await
        .unwrap();
    assert!(finished(&manager, "a", &id).await.output_complete);
    let mut input = ProcessReadInput::new(&id);
    let mut text = String::new();
    loop {
        let page = manager
            .read(Some("a"), input.clone(), &token)
            .await
            .unwrap()
            .data;
        text.push_str(&page.stdout.text);
        input.stdout_offset = page.stdout.next_offset;
        if !page.stdout.has_more {
            break;
        }
    }
    assert!(text == format!("{a}{b}") || text == format!("{b}{a}"));
    manager.close().await;
}

#[tokio::test]
async fn cancelling_or_dropping_a_blocked_stdin_call_keeps_the_job_controllable() {
    for drop_call in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let manager = ProcessSession::new(root.path()).await.unwrap();
        let id = manager
            .start(
                Some("a"),
                ProcessStartInput::new("sleep 60"),
                &CancellationToken::new(),
            )
            .await
            .unwrap()
            .data
            .process_id;
        let writer = manager.clone();
        let token = CancellationToken::new();
        let write_token = token.clone();
        let write_id = id.clone();
        let task = tokio::spawn(async move {
            writer
                .write(
                    Some("a"),
                    ProcessWriteInput {
                        process_id: write_id,
                        text: "x".repeat(65_536),
                        close_stdin: true,
                    },
                    &write_token,
                )
                .await
        });
        tokio::task::yield_now().await;
        if drop_call {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            token.cancel();
            match task.await.unwrap() {
                Err(ToolError::Cancelled) => (),
                Ok(result) => {
                    assert!(!result.data.complete);
                    assert_eq!(result.data.error.as_deref(), Some("stdin call cancelled"));
                }
                other => panic!("unexpected write outcome: {other:?}"),
            }
        }
        assert_eq!(
            manager
                .status(
                    Some("a"),
                    ProcessStatusInput::default(),
                    &CancellationToken::new()
                )
                .await
                .unwrap()
                .data[0]
                .state,
            ProcessState::Running
        );
        let stopped = manager
            .stop(
                Some("a"),
                ProcessStopInput {
                    process_id: id,
                    forget: true,
                },
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(stopped.data.forgotten);
        manager.close().await;
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn normal_exit_stops_background_children_and_reaps_the_direct_child() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let id = manager
        .start(
            Some("a"),
            ProcessStartInput::new("sleep 60 & printf '%s %s' $$ $! > pids; exit 3"),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .data
        .process_id;
    let status = finished(&manager, "a", &id).await;
    assert_eq!(status.exit_code, Some(3));
    assert_eq!(status.end, Some(ProcessEnd::Exited));
    let pids = file(&root.path().join("pids")).await;
    let ids: Vec<&str> = pids.split_whitespace().collect();
    assert!(!Path::new(&format!("/proc/{}", ids[0])).exists());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let stat = tokio::fs::read_to_string(format!("/proc/{}", ids[1])).await;
            if match stat {
                Err(_) => true,
                Ok(s) => s.rsplit_once(") ").unwrap().1.starts_with(['Z', 'X']),
            } {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    manager.close().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn each_process_leads_a_new_session_without_the_host_terminal() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let id = manager
        .start(
            Some("a"),
            ProcessStartInput::new("printf '%s ' $$; cat /proc/$$/stat"),
            &token,
        )
        .await
        .unwrap()
        .data
        .process_id;
    finished(&manager, "a", &id).await;
    let output = manager
        .read(Some("a"), ProcessReadInput::new(&id), &token)
        .await
        .unwrap()
        .data
        .stdout
        .text;
    let (pid, stat) = output.split_once(' ').unwrap();
    // After the parenthesised name: state, ppid, pgrp, session, tty_nr.
    let fields = |stat: &str| -> Vec<String> {
        let tail = stat.rsplit_once(") ").unwrap().1;
        tail.split_whitespace()
            .skip(2)
            .take(3)
            .map(str::to_owned)
            .collect()
    };
    let child = fields(stat);
    let host = fields(&std::fs::read_to_string("/proc/self/stat").unwrap());
    assert_eq!(child[1], pid, "the shell leads its own session");
    assert_ne!(child[1], host[1]);
    assert_eq!(child[0], pid, "group signals still target the shell's pid");
    assert_eq!(child[2], "0", "no controlling terminal");
    manager.close().await;
}

#[tokio::test]
async fn persistent_processes_do_not_receive_unlisted_host_variables() {
    // Cargo sets this in the test process; it stands for any host secret.
    assert!(std::env::var_os("CARGO_PKG_NAME").is_some());
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let command = r#"printf '%s|%s' "${CARGO_PKG_NAME-unset}" "${PATH:+path}""#;
    let id = manager
        .start(Some("a"), ProcessStartInput::new(command), &token)
        .await
        .unwrap()
        .data
        .process_id;
    finished(&manager, "a", &id).await;
    let read = manager
        .read(Some("a"), ProcessReadInput::new(&id), &token)
        .await
        .unwrap()
        .data;
    assert_eq!(read.stdout.text, "unset|path");
    manager.close().await;
}

#[tokio::test]
async fn persistent_environment_is_host_configured_before_start() {
    let root = tempfile::tempdir().unwrap();
    let mut env = CommandEnv::default();
    env.inherit_env.push("CARGO_PKG_NAME".into());
    env.env.insert("GREETING".into(), "hello".into());
    let manager = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_env(env.clone())
        .unwrap();
    let token = CancellationToken::new();
    let command = r#"printf '%s|%s' "$CARGO_PKG_NAME" "$GREETING""#;
    let id = manager
        .start(Some("a"), ProcessStartInput::new(command), &token)
        .await
        .unwrap()
        .data
        .process_id;
    finished(&manager, "a", &id).await;
    let read = manager
        .read(Some("a"), ProcessReadInput::new(&id), &token)
        .await
        .unwrap()
        .data;
    assert_eq!(read.stdout.text, "roomkit-tools-process|hello");
    assert!(matches!(
        manager.clone().with_env(env.clone()),
        Err(ToolError::AccessRefused(_))
    ));
    manager.close().await;
    let mut invalid = env;
    invalid.inherit_env.push("A=B".into());
    assert!(matches!(
        ProcessSession::new(root.path())
            .await
            .unwrap()
            .with_env(invalid),
        Err(ToolError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn reading_cat_preserves_stdin_and_reports_newline_and_normal_eof_exactly() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let id = manager
        .start(Some("a"), ProcessStartInput::new("cat"), &token)
        .await
        .unwrap()
        .data
        .process_id;
    let sent = manager
        .write(
            Some("a"),
            ProcessWriteInput {
                process_id: id.clone(),
                text: "Hello!!\n".into(),
                close_stdin: false,
            },
            &token,
        )
        .await
        .unwrap()
        .data;
    assert_eq!(sent.bytes_written, 8);
    assert!(sent.complete && !sent.stdin_closed);
    let output = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let read = manager
                .read(Some("a"), ProcessReadInput::new(&id), &token)
                .await
                .unwrap()
                .data;
            if read.stdout.total_bytes == 8 {
                return read;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(output.stdout.bytes, b"Hello!!\n");
    assert_eq!(output.stdout.text, "Hello!!\n");
    assert_eq!(output.stdout.next_offset, 8);
    assert!(!output.stdout.eof);
    assert_eq!(output.status.state, ProcessState::Running);
    assert!(!output.status.stdin_closed);
    assert_eq!(output.status.exit_code, None);
    assert_eq!(output.status.signal, None);
    let mut next = ProcessReadInput::new(&id);
    next.stdout_offset = output.stdout.next_offset;
    let empty = manager.read(Some("a"), next, &token).await.unwrap().data;
    assert_eq!(empty.stdout.bytes, b"");
    assert_eq!(empty.status.state, ProcessState::Running);
    assert!(!empty.status.stdin_closed);
    manager
        .write(
            Some("a"),
            ProcessWriteInput {
                process_id: id.clone(),
                text: String::new(),
                close_stdin: true,
            },
            &token,
        )
        .await
        .unwrap();
    let status = finished(&manager, "a", &id).await;
    assert_eq!(status.end, Some(ProcessEnd::Exited));
    assert_eq!(status.exit_code, Some(0));
    assert_eq!(status.signal, None);
    assert!(status.stdin_closed && status.output_complete);
    assert_eq!(status.stdout_bytes, 8);
    manager.close().await;
}
