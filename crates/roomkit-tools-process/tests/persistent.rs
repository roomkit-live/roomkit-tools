//! Real process ownership, incremental I/O and lifecycle tests.
#![cfg(unix)]

use std::num::{NonZeroU64, NonZeroUsize};
use std::path::Path;
use std::time::{Duration, Instant};

use roomkit_tools_core::ToolError;
use roomkit_tools_process::{
    CommandEnv, OutputLimits, OutputReadInput, OutputStore, ProcessEnd, ProcessLimits,
    ProcessReadInput, ProcessSession, ProcessStartInput, ProcessState, ProcessStatus,
    ProcessStatusInput, ProcessStopInput, ProcessWriteInput,
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
            if result.data.processes.first().unwrap().state == ProcessState::Finished {
                return result.data.processes.first().unwrap().clone();
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
            .data
            .processes[0]
            .state,
        ProcessState::Running
    );
    let sent = manager
        .write(
            Some("alice"),
            ProcessWriteInput {
                text: "πxπ\nhello\n".into(),
                close_stdin: true,
                ..ProcessWriteInput::new(id.clone())
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
async fn a_cursor_past_its_stream_s_end_names_the_stream_and_that_end() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let input = ProcessStartInput::new("printf 12345; printf ab >&2");
    let id = manager
        .start(Some("a"), input, &token)
        .await
        .unwrap()
        .data
        .process_id;
    finished(&manager, "a", &id).await;
    // A model passing stdout's next_offset as both offsets.
    let both = ProcessReadInput {
        stdout_offset: 5,
        stderr_offset: 5,
        ..ProcessReadInput::new(&id)
    };
    let refused = manager.read(Some("a"), both, &token).await;
    let Err(ToolError::InvalidInput(message)) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(
        message,
        "stderr_offset 5 is past stderr's end 2; each stream has its own next_offset"
    );
    let past = ProcessReadInput {
        stdout_offset: 6,
        ..ProcessReadInput::new(&id)
    };
    let refused = manager.read(Some("a"), past, &token).await;
    let Err(ToolError::InvalidInput(message)) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(
        message,
        "stdout_offset 6 is past stdout's end 5; each stream has its own next_offset"
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
                text: "second\n".into(),
                ..ProcessWriteInput::new(id.clone())
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
        .processes
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
            .processes
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
        .stop(Some("a"), ProcessStopInput::new(id.clone()), &token)
        .await
        .unwrap()
        .data;
    assert_eq!(stopped.status.end, Some(ProcessEnd::Stopped));
    assert_eq!(stopped.status.signal, Some(9));
    assert!(!stopped.forgotten);
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
    assert!(manager.close_owner(Some("a")).await);
    assert_eq!(
        manager
            .status(Some("b"), ProcessStatusInput::default(), &token)
            .await
            .unwrap()
            .data
            .processes[0]
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

/// The identifiers of `owner`'s retained jobs, in creation order.
#[expect(clippy::unwrap_used, reason = "fixture failure must fail tests")]
async fn retained(manager: &ProcessSession, owner: &str) -> Vec<String> {
    let listed = manager
        .status(
            Some(owner),
            ProcessStatusInput::default(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    listed
        .data
        .processes
        .into_iter()
        .map(|status| status.process_id)
        .collect()
}

#[tokio::test]
async fn at_its_quota_an_owner_s_oldest_finished_job_makes_room() {
    let root = tempfile::tempdir().unwrap();
    let outputs = OutputStore::new(None, OutputLimits::default())
        .await
        .unwrap();
    let limits = ProcessLimits {
        max_processes: 3,
        max_processes_per_owner: 2,
        ..ProcessLimits::default()
    };
    let manager = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_output_store(outputs.clone())
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let token = CancellationToken::new();
    let start = |owner: &'static str, command: &'static str| {
        manager.start(Some(owner), ProcessStartInput::new(command), &token)
    };
    let first = start("a", "printf first").await.unwrap().data;
    let p1 = first.process_id;
    let archive = first.stdout_archive.unwrap().archive_id.unwrap();
    let p2 = start("a", "sleep 30").await.unwrap().data.process_id;
    finished(&manager, "a", &p1).await;
    // The finished job is released, not the running one; its archive stays.
    let p3 = start("a", "sleep 30").await.unwrap().data.process_id;
    assert_eq!(retained(&manager, "a").await, [p2, p3]);
    assert!(matches!(
        manager
            .read(Some("a"), ProcessReadInput::new(&p1), &token)
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    let page = outputs
        .read(Some("a"), OutputReadInput::new(archive), &token)
        .await
        .unwrap();
    assert_eq!(page.data.text, "first");
    // Every job of the owner runs: refused in words that say so.
    let Err(ToolError::LimitExceeded(message)) = start("a", "true").await else {
        panic!("a running job was released");
    };
    assert_eq!(
        message,
        "process quota of 2 reached: every process of this conversation still runs; \
         stop one with process_stop"
    );
    // The session is full: another owner's finished job is not released.
    let theirs = start("b", "true").await.unwrap().data.process_id;
    finished(&manager, "b", &theirs).await;
    let Err(ToolError::LimitExceeded(message)) = start("c", "true").await else {
        panic!("another owner's job was released");
    };
    assert_eq!(
        message,
        "session process quota of 3 reached: every process still runs or is another \
         conversation's"
    );
    assert!(manager.close().await);
    assert!(outputs.close().await);
}

/// A session holding two processes per owner.
#[expect(clippy::unwrap_used, reason = "fixture failure must fail tests")]
async fn holding_two(root: &Path) -> ProcessSession {
    let limits = ProcessLimits {
        max_processes_per_owner: 2,
        ..ProcessLimits::default()
    };
    ProcessSession::new(root)
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap()
}

#[tokio::test]
async fn at_its_quota_the_process_that_finished_first_makes_room() {
    let root = tempfile::tempdir().unwrap();
    let manager = holding_two(root.path()).await;
    let token = CancellationToken::new();
    let start =
        |command: &'static str| manager.start(Some("a"), ProcessStartInput::new(command), &token);
    // Started first, it ends last: the end a model was just told to read.
    let server = start("sleep 0.4; echo server crashed").await.unwrap().data;
    let build = start("echo built").await.unwrap().data;
    finished(&manager, "a", &build.process_id).await;
    finished(&manager, "a", &server.process_id).await;
    let next = start("true").await.unwrap().data.process_id;
    assert_eq!(retained(&manager, "a").await, [server.process_id, next]);
    assert!(manager.close().await);
}

/// The refusal of a read of `id` by `owner`.
#[expect(
    clippy::panic,
    reason = "a read that is not refused must fail the test"
)]
async fn refusal(manager: &ProcessSession, owner: &str, id: &str) -> String {
    let token = CancellationToken::new();
    let read = manager
        .read(Some(owner), ProcessReadInput::new(id), &token)
        .await;
    let Err(ToolError::AccessRefused(message)) = read else {
        panic!("{read:?}");
    };
    message
}

#[tokio::test]
async fn a_released_or_forgotten_process_is_named_so_to_its_owner_alone() {
    let root = tempfile::tempdir().unwrap();
    let manager = holding_two(root.path()).await;
    let token = CancellationToken::new();
    let start =
        |command: &'static str| manager.start(Some("a"), ProcessStartInput::new(command), &token);
    for _ in 0..2 {
        let id = start("true").await.unwrap().data.process_id;
        finished(&manager, "a", &id).await;
    }
    // p1 makes room for p3; p2 is forgotten.
    start("sleep 30").await.unwrap();
    let forget = ProcessStopInput {
        forget: true,
        ..ProcessStopInput::new("p2")
    };
    assert!(
        manager
            .stop(Some("a"), forget, &token)
            .await
            .unwrap()
            .data
            .forgotten
    );
    let released = |id: &str| {
        format!(
            "{id} was released: a finished process makes room at the quota or when forgotten; \
             its archives, if any, remain readable with output_read"
        )
    };
    assert_eq!(refusal(&manager, "a", "p1").await, released("p1"));
    assert_eq!(refusal(&manager, "a", "p2").await, released("p2"));
    // Another owner's identifier, or one never issued, stays unknown.
    let unknown = "unknown process in this owner scope";
    assert_eq!(refusal(&manager, "b", "p1").await, unknown);
    assert_eq!(refusal(&manager, "a", "p9").await, unknown);
    assert!(manager.close().await);
}

#[tokio::test]
async fn a_start_that_fails_to_spawn_releases_no_finished_process() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("notes.txt"), "not a directory").unwrap();
    let manager = holding_two(root.path()).await;
    let token = CancellationToken::new();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let start = manager.start(Some("a"), ProcessStartInput::new("true"), &token);
        let id = start.await.unwrap().data.process_id;
        finished(&manager, "a", &id).await;
        ids.push(id);
    }
    // The quota admits it, then the spawn fails: a file is no directory.
    let input = ProcessStartInput {
        cwd: Some("notes.txt".into()),
        ..ProcessStartInput::new("true")
    };
    let failed = manager.start(Some("a"), input, &token).await;
    assert!(matches!(failed, Err(ToolError::Io { .. })), "{failed:?}");
    assert_eq!(retained(&manager, "a").await, ids);
    assert!(manager.close().await);
}

#[tokio::test]
async fn the_standalone_owner_is_apart_from_an_owner_named_default() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        max_processes_per_owner: 1,
        ..ProcessLimits::default()
    };
    let session = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let token = CancellationToken::new();
    let mine = session
        .start(None, ProcessStartInput::new("cat"), &token)
        .await
        .unwrap()
        .data
        .process_id;
    // "default" neither sees nor reaches the standalone job, and has a quota
    // of its own.
    let theirs = session.status(Some("default"), ProcessStatusInput::default(), &token);
    assert!(theirs.await.unwrap().data.processes.is_empty());
    assert!(matches!(
        session
            .read(Some("default"), ProcessReadInput::new(&mine), &token)
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    session
        .start(Some("default"), ProcessStartInput::new("cat"), &token)
        .await
        .unwrap();
    // Closing the standalone owner leaves "default"'s job.
    assert!(session.close_owner(None).await);
    let listed = session.status(None, ProcessStatusInput::default(), &token);
    assert!(listed.await.unwrap().data.processes.is_empty());
    let theirs = session.status(Some("default"), ProcessStatusInput::default(), &token);
    assert_eq!(theirs.await.unwrap().data.processes.len(), 1);
    assert!(session.close().await);
}

#[tokio::test]
async fn no_owner_is_the_standalone_owner_with_its_own_quota() {
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
        .data
        .processes;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].process_id, id);
    // A named owner neither sees nor reaches the standalone owner's job.
    let named = session.status(Some("a"), ProcessStatusInput::default(), &token);
    assert!(named.await.unwrap().data.processes.is_empty());
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
        close_stdin: true,
        ..ProcessWriteInput::new(id.clone())
    };
    session.write(None, close, &token).await.unwrap();
    let stop = ProcessStopInput {
        forget: true,
        ..ProcessStopInput::new(id)
    };
    session.stop(None, stop, &token).await.unwrap();
    assert!(session.close().await);
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
async fn a_detached_session_leaves_its_running_processes_running() {
    let root = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let finished_id = session
        .start(Some("a"), ProcessStartInput::new("true"), &token)
        .await
        .unwrap()
        .data
        .process_id;
    session
        .start(
            Some("a"),
            ProcessStartInput::new("echo $$ > pid; exec sleep 60"),
            &token,
        )
        .await
        .unwrap();
    let pid: u32 = file(&root.path().join("pid")).await.trim().parse().unwrap();
    finished(&session, "a", &finished_id).await;
    // Detaching waits for each job to let go: a stop it replaced would have
    // killed and reaped the process before it returned.
    assert!(session.detach().await);
    assert!(session.closed());
    let listed = session
        .status(Some("a"), ProcessStatusInput { process_id: None }, &token)
        .await;
    drop(session);
    let kept = running(pid).await;
    // The process is the test's to stop, whatever the assertions say.
    std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .unwrap();
    assert!(kept, "the running process outlives its detached session");
    assert!(listed.is_err(), "a detached session is closed: {listed:?}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_detached_job_writing_to_its_pipes_runs_on() {
    let root = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let writer =
        "echo $$ > pid; i=0; while :; do echo line; i=$((i+1)); echo $i > count; sleep 0.02; done";
    session
        .start(Some("a"), ProcessStartInput::new(writer), &token)
        .await
        .unwrap();
    let pid: u32 = file(&root.path().join("pid")).await.trim().parse().unwrap();
    assert!(session.detach().await);
    drop(session);
    let count = || {
        let path = root.path().join("count");
        async move {
            tokio::fs::read_to_string(path)
                .await
                .ok()
                .and_then(|text| text.trim().parse::<u64>().ok())
        }
    };
    let at_detach = count().await.unwrap_or(0);
    // Fifty more lines, written long after the session let go of its pipes.
    let wrote_on = tokio::time::timeout(Duration::from_secs(5), async {
        while count().await.unwrap_or(0) < at_detach + 50 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_ok();
    std::process::Command::new("kill")
        .args(["-KILL", &format!("-{pid}")])
        .status()
        .unwrap();
    assert!(wrote_on, "the detached job stopped writing");
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
async fn lifetime_timeout_refuses_requests_above_the_maximum_and_retains_output() {
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
    let token = CancellationToken::new();
    let mut input = ProcessStartInput::new("printf ready; sleep 60");
    input.timeout_ms = NonZeroU64::new(60_000);
    match manager.start(Some("a"), input, &token).await {
        Err(ToolError::InvalidInput(message)) => {
            assert_eq!(message, "timeout_ms exceeds the host maximum of 200 ms");
        }
        other => panic!("{other:?}"),
    }
    let mut input = ProcessStartInput::new("printf ready; sleep 60");
    input.timeout_ms = NonZeroU64::new(200);
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
                text: "x".repeat(limits.max_write_bytes),
                close_stdin: true,
                ..ProcessWriteInput::new(id.clone())
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
    let bounded = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_limits(ProcessLimits {
            max_command_bytes: 2,
            ..ProcessLimits::default()
        })
        .unwrap();
    let refused = bounded
        .start(
            Some("a"),
            ProcessStartInput::new("touch unexpected"),
            &token,
        )
        .await;
    let Err(ToolError::LimitExceeded(message)) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(
        message,
        "command is 16 bytes, over the host limit of 2: write the script to a file, then run it"
    );
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
async fn dropping_last_session_reaps_shell_and_stops_its_child() {
    let root = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    session
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
    drop(session);
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
            .data
            .processes[0]
            .state,
        ProcessState::Running
    );
    let stopped = manager
        .stop(
            Some("a"),
            ProcessStopInput {
                forget: true,
                ..ProcessStopInput::new(id)
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
        text,
        ..ProcessWriteInput::new(id.clone())
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
                close_stdin: true,
                ..ProcessWriteInput::new(id.clone())
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
                        text: "x".repeat(65_536),
                        close_stdin: true,
                        ..ProcessWriteInput::new(write_id)
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
                .data
                .processes[0]
                .state,
            ProcessState::Running
        );
        let stopped = manager
            .stop(
                Some("a"),
                ProcessStopInput {
                    forget: true,
                    ..ProcessStopInput::new(id)
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
                text: "Hello!!\n".into(),
                ..ProcessWriteInput::new(id.clone())
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
                close_stdin: true,
                ..ProcessWriteInput::new(id.clone())
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

#[tokio::test]
async fn the_initial_directory_is_named_relative_to_the_root() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("sub")).unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let at_root = session
        .start(None, ProcessStartInput::new("true"), &token)
        .await
        .unwrap();
    assert_eq!(at_root.data.cwd, ".");
    let input = ProcessStartInput {
        cwd: Some("sub".into()),
        ..ProcessStartInput::new("true")
    };
    let below = session.start(None, input, &token).await.unwrap();
    assert_eq!(below.data.cwd, "sub");
    let listed = session.status(None, ProcessStatusInput::default(), &token);
    let cwds: Vec<_> = listed.await.unwrap().data.processes;
    assert_eq!(
        cwds.iter().map(|p| p.cwd.as_str()).collect::<Vec<_>>(),
        [".", "sub"]
    );
    assert!(session.close().await);
}

#[expect(clippy::unwrap_used, reason = "fixture failure must fail tests")]
async fn started(manager: &ProcessSession, command: &str) -> String {
    manager
        .start(
            Some("a"),
            ProcessStartInput::new(command),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .data
        .process_id
}

fn waiting(id: &str, wait_ms: u64) -> ProcessReadInput {
    ProcessReadInput {
        wait_ms: NonZeroU64::new(wait_ms),
        ..ProcessReadInput::new(id)
    }
}

/// A read with `wait_ms` returns once the process writes, and the next one
/// once it ends.
#[tokio::test]
async fn a_read_waits_for_new_output_then_for_the_end() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let id = started(&manager, "sleep 1; echo fini; sleep 1").await;
    let began = Instant::now();
    let read = manager
        .read(Some("a"), waiting(&id, 15_000), &token)
        .await
        .unwrap()
        .data;
    assert_eq!(read.stdout.text, "fini\n");
    assert!(began.elapsed() >= Duration::from_millis(900));
    assert!(began.elapsed() < Duration::from_secs(10));
    let mut next = waiting(&id, 15_000);
    next.stdout_offset = read.stdout.next_offset;
    let end = manager.read(Some("a"), next, &token).await.unwrap().data;
    assert_eq!(end.status.state, ProcessState::Finished);
    assert_eq!(end.status.exit_code, Some(0));
    assert_eq!(end.stdout.text, "");
    assert!(began.elapsed() < Duration::from_secs(10));
    // Output already past the offsets is read at once.
    let began = Instant::now();
    let again = manager.read(Some("a"), waiting(&id, 15_000), &token).await;
    assert_eq!(again.unwrap().data.stdout.text, "fini\n");
    assert!(began.elapsed() < Duration::from_secs(1));
    // An offset past the end is refused at once, not after the wait.
    let mut past = waiting(&id, 15_000);
    past.stdout_offset = 99;
    assert!(matches!(
        manager.read(Some("a"), past, &token).await,
        Err(ToolError::InvalidInput(_))
    ));
    assert!(began.elapsed() < Duration::from_secs(1));
    manager.close().await;
}

#[tokio::test]
async fn a_wait_longer_than_the_host_bound_ends_at_the_bound() {
    let root = tempfile::tempdir().unwrap();
    let limits = ProcessLimits {
        max_wait_ms: 300,
        ..ProcessLimits::default()
    };
    let manager = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let id = started(&manager, "sleep 30").await;
    let began = Instant::now();
    let read = manager
        .read(Some("a"), waiting(&id, 60_000), &CancellationToken::new())
        .await
        .unwrap()
        .data;
    assert_eq!(read.status.state, ProcessState::Running);
    assert!(began.elapsed() >= Duration::from_millis(250));
    assert!(began.elapsed() < Duration::from_secs(5));
    manager.close().await;
}

#[tokio::test]
async fn a_cancelled_call_stops_waiting() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let id = started(&manager, "sleep 30").await;
    let cancel = CancellationToken::new();
    let cancelling = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancelling.cancel();
    });
    let began = Instant::now();
    assert!(matches!(
        manager.read(Some("a"), waiting(&id, 15_000), &cancel).await,
        Err(ToolError::Cancelled)
    ));
    assert!(began.elapsed() < Duration::from_secs(5));
    manager.close().await;
}

#[tokio::test]
async fn the_wait_bound_is_positive_and_at_most_a_day() {
    let root = tempfile::tempdir().unwrap();
    for invalid in [0, 86_400_001] {
        let limits = ProcessLimits {
            max_wait_ms: invalid,
            ..ProcessLimits::default()
        };
        let session = ProcessSession::new(root.path()).await.unwrap();
        assert!(matches!(
            session.with_limits(limits),
            Err(ToolError::InvalidInput(_))
        ));
    }
}

#[tokio::test]
async fn a_missing_or_escaping_start_directory_is_refused_in_words() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let start = |cwd: &str| ProcessStartInput {
        cwd: Some(cwd.into()),
        ..ProcessStartInput::new("true")
    };
    let missing = manager.start(Some("a"), start("absent"), &token).await;
    assert!(
        matches!(&missing, Err(ToolError::InvalidInput(message))
            if message == "no such file or directory: absent"),
        "{missing:?}"
    );
    for outside in ["../absent-outside", ".."] {
        let refused = manager.start(Some("a"), start(outside), &token).await;
        assert!(
            matches!(refused, Err(ToolError::AccessRefused(_))),
            "{outside}: {refused:?}"
        );
    }
    // An empty `cwd`, as a model sends for an optional field, is the root.
    let started = manager.start(Some("a"), start(""), &token).await.unwrap();
    assert_eq!(started.data.cwd, ".");
    manager.close().await;
}
