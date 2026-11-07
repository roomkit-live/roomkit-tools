//! Real disk copies are independent of bounded process previews and model output.
#![cfg(unix)]

use std::num::NonZeroUsize;
use std::time::Duration;

use roomkit_tools_core::ToolError;
use roomkit_tools_process::{
    OutputForgetInput, OutputLimits, OutputReadInput, OutputStore, ProcessLimits, ProcessSession,
    ProcessStartInput, ProcessStatusInput, ProcessWriteInput, ShellInput, ShellLimits, ShellTool,
};
use tokio_util::sync::CancellationToken;

/// Shell limits that cut every output past its first byte: only an output
/// cut short is archived.
fn cutting() -> ShellLimits {
    ShellLimits {
        max_output_bytes: 1,
        ..ShellLimits::default()
    }
}

fn page(id: &str, offset: u64, max_bytes: usize) -> OutputReadInput {
    OutputReadInput {
        offset,
        max_bytes: NonZeroUsize::new(max_bytes),
        ..OutputReadInput::new(id)
    }
}

#[tokio::test]
async fn recover_full_binary_output_without_rerunning_a_command() {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let original: Vec<u8> = (0..=255).cycle().take(256 * 1024).collect();
    std::fs::write(root.path().join("payload"), &original).unwrap();
    let store = OutputStore::new(Some(storage.path()), OutputLimits::default())
        .await
        .unwrap();
    let limits = ShellLimits {
        max_output_bytes: 8,
        ..ShellLimits::default()
    };
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap()
        .with_output_store(store.clone());
    let cancel = CancellationToken::new();
    let result = shell
        .shell(
            ShellInput::new("printf run >> runs; cat payload; printf err >&2; exit 7"),
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(result.data.exit_code, Some(7));
    assert!(result.data.stdout.truncated);
    let archive = result.data.stdout.archive.unwrap();
    assert!(archive.complete, "{archive:?}");
    let id = archive.archive_id.unwrap();
    let mut recovered = Vec::new();
    loop {
        let page = store
            .read(None, page(&id, recovered.len() as u64, 4096), &cancel)
            .await
            .unwrap()
            .data;
        recovered.extend(page.bytes);
        if !page.has_more {
            break;
        }
    }
    assert_eq!(recovered, original);
    assert_eq!(std::fs::read(root.path().join("runs")).unwrap(), b"run");
    // stderr was kept whole: its copy held nothing more and is gone.
    assert!(result.data.stderr.archive.is_none());
    assert_eq!(store.list(None, &cancel).await.unwrap().len(), 1);
    assert!(store.close().await);
    assert_eq!(std::fs::read_dir(storage.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn listing_follows_creation_order_past_nine_archives() {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let store = OutputStore::new(Some(storage.path()), OutputLimits::default())
        .await
        .unwrap();
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(cutting())
        .unwrap()
        .with_output_store(store.clone());
    let cancel = CancellationToken::new();
    for _ in 0..6 {
        let both = ShellInput::new("printf ab; printf cd >&2");
        shell.shell(both, &cancel).await.unwrap();
    }
    let listed: Vec<String> = store
        .list(None, &cancel)
        .await
        .unwrap()
        .into_iter()
        .map(|archive| archive.archive_id.unwrap())
        .collect();
    let expected: Vec<String> = (1..=12).map(|n| format!("o{n}")).collect();
    assert_eq!(listed, expected);
    assert!(store.close().await);
}

#[tokio::test]
async fn no_owner_is_the_standalone_owner_of_shell_archives() {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let store = OutputStore::new(Some(storage.path()), OutputLimits::default())
        .await
        .unwrap();
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(cutting())
        .unwrap()
        .with_output_store(store.clone());
    let cancel = CancellationToken::new();
    let standalone = shell
        .shell(ShellInput::new("printf mine"), &cancel)
        .await
        .unwrap();
    let named = shell
        .shell_owned("a", ShellInput::new("printf theirs"), &cancel)
        .await
        .unwrap();
    let mine = standalone.data.stdout.archive.unwrap().archive_id.unwrap();
    let theirs = named.data.stdout.archive.unwrap().archive_id.unwrap();
    // Standalone commands file their outputs under the owner None names;
    // the empty stderr of each was kept whole, so is not archived.
    assert_eq!(store.list(None, &cancel).await.unwrap().len(), 1);
    assert_eq!(store.list(Some("a"), &cancel).await.unwrap().len(), 1);
    // A read from no offset and no size reads the stored bytes from the start.
    let page_of_mine = store.read(None, OutputReadInput::new(&mine), &cancel).await;
    assert_eq!(page_of_mine.unwrap().data.text, "mine");
    assert!(matches!(
        store.read(None, page(&theirs, 0, 64), &cancel).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(matches!(
        store.read(Some("a"), page(&mine, 0, 64), &cancel).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(store.close().await);
}

#[tokio::test]
async fn the_standalone_owner_is_apart_from_an_owner_named_default() {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let limits = OutputLimits {
        max_archives_per_owner: 2,
        ..OutputLimits::default()
    };
    let store = OutputStore::new(Some(storage.path()), limits)
        .await
        .unwrap();
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(cutting())
        .unwrap()
        .with_output_store(store.clone());
    let cancel = CancellationToken::new();
    let mine = ShellInput::new("printf mine; printf xy >&2");
    let standalone = shell.shell(mine, &cancel).await;
    let mine = standalone.unwrap().data.stdout.archive.unwrap();
    // The standalone owner's two archives fill its quota, not "default"'s.
    let named = shell
        .shell_owned(
            "default",
            ShellInput::new("printf theirs; printf yz >&2"),
            &cancel,
        )
        .await
        .unwrap();
    assert!(named.data.stdout.archive.unwrap().archive_id.is_some());
    assert_eq!(store.list(None, &cancel).await.unwrap().len(), 2);
    assert_eq!(store.list(Some("default"), &cancel).await.unwrap().len(), 2);
    let id = mine.archive_id.unwrap();
    assert!(matches!(
        store.read(Some("default"), page(&id, 0, 64), &cancel).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(store.close().await);
}

/// Set in the child process that checks the system temporary directory.
const TEMPORARY_CHILD: &str = "ROOMKIT_OUTPUT_TEMPORARY_CHILD";

#[tokio::test]
async fn no_parent_keeps_the_store_in_the_system_temporary_directory() {
    // Other processes share the system temporary directory: the check runs in
    // a child of this test whose TMPDIR is a directory of its own.
    if std::env::var_os(TEMPORARY_CHILD).is_some() {
        let temporary = std::env::temp_dir();
        let store = OutputStore::new(None, OutputLimits::default())
            .await
            .unwrap();
        let created: Vec<_> = std::fs::read_dir(&temporary)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(created.len(), 1, "{created:?}");
        assert!(created[0].to_string_lossy().starts_with("roomkit-output-"));
        assert!(store.close().await);
        assert!(!temporary.join(&created[0]).exists());
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let name = "no_parent_keeps_the_store_in_the_system_temporary_directory";
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--test-threads", "1"])
        .env("TMPDIR", temporary.path())
        .env(TEMPORARY_CHILD, "1")
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&child.stdout);
    assert!(child.status.success(), "{report}");
    assert!(report.contains("1 passed"), "{report}");
}

#[tokio::test]
async fn store_directory_is_private_to_the_host_user() {
    use std::os::unix::fs::PermissionsExt;

    let storage = tempfile::tempdir().unwrap();
    let store = OutputStore::new(Some(storage.path()), OutputLimits::default())
        .await
        .unwrap();
    let directories: Vec<_> = std::fs::read_dir(storage.path())
        .unwrap()
        .map(|entry| entry.unwrap().metadata().unwrap())
        .collect();
    assert_eq!(directories.len(), 1);
    assert!(directories[0].is_dir());
    assert_eq!(directories[0].permissions().mode() & 0o777, 0o700);
    assert!(store.close().await);
}

#[tokio::test]
async fn quotas_preserve_prefixes_and_report_copies_as_incomplete() {
    let root = tempfile::tempdir().unwrap();
    let limits = OutputLimits {
        max_stream_bytes: 5,
        max_total_bytes: 7,
        max_read_bytes: 2,
        ..OutputLimits::default()
    };
    let store = OutputStore::new(Some(root.path()), limits).await.unwrap();
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(cutting())
        .unwrap()
        .with_output_store(store.clone());
    let cancel = CancellationToken::new();
    let first = shell
        .shell(ShellInput::new("printf 0123456789"), &cancel)
        .await
        .unwrap();
    let archive = first.data.stdout.archive.unwrap();
    assert_eq!(archive.stored_bytes, 5);
    assert_eq!(archive.observed_bytes, 10);
    assert_eq!(archive.error.as_deref(), Some("stream_limit"));
    assert!(!archive.complete);
    let id = archive.archive_id.unwrap();
    assert_eq!(
        store
            .read(None, page(&id, 0, 100), &cancel)
            .await
            .unwrap()
            .data
            .bytes,
        b"01"
    );
    assert!(matches!(
        store.read(None, page(&id, 6, 1), &cancel).await,
        Err(ToolError::InvalidInput(_))
    ));
    let second = shell
        .shell(ShellInput::new("printf still-runs"), &cancel)
        .await
        .unwrap();
    assert_eq!(
        second.data.stdout.text,
        "[... 9 earlier bytes omitted ...]\ns"
    );
    assert_eq!(
        second.data.stdout.archive.unwrap().error.as_deref(),
        Some("total_limit")
    );
    store
        .forget(
            None,
            OutputForgetInput {
                archive_id: id.clone(),
            },
            &cancel,
        )
        .await
        .unwrap();
    assert!(matches!(
        store.read(None, page(&id, 0, 1), &cancel).await,
        Err(ToolError::AccessRefused(_))
    ));
    let third = shell
        .shell(ShellInput::new("printf ok"), &cancel)
        .await
        .unwrap();
    assert!(third.data.stdout.archive.unwrap().complete);
    assert!(store.close().await);
}

#[tokio::test]
async fn a_full_store_releases_its_owner_s_oldest_finished_archive() {
    let root = tempfile::tempdir().unwrap();
    // A command holds one archive per stream while it runs: two more than
    // the latest two of an owner that keeps three.
    let limits = OutputLimits {
        max_archives: 4,
        max_archives_per_owner: 3,
        ..OutputLimits::default()
    };
    let store = OutputStore::new(Some(root.path()), limits).await.unwrap();
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(cutting())
        .unwrap()
        .with_output_store(store.clone());
    let cancel = CancellationToken::new();
    let mut ids = Vec::new();
    for text in ["one", "two", "three"] {
        let input = ShellInput::new(format!("printf {text}"));
        let result = shell.shell(input, &cancel).await.unwrap();
        ids.push(result.data.stdout.archive.unwrap().archive_id.unwrap());
    }
    // The third command released the first archive: its owner keeps the
    // two latest.
    assert!(matches!(
        store.read(None, page(&ids[0], 0, 8), &cancel).await,
        Err(ToolError::AccessRefused(_))
    ));
    let text = |id: String| {
        let store = store.clone();
        let cancel = cancel.clone();
        async move {
            store
                .read(None, page(&id, 0, 8), &cancel)
                .await
                .unwrap()
                .data
                .text
        }
    };
    assert_eq!(text(ids[1].clone()).await, "two");
    assert_eq!(text(ids[2].clone()).await, "three");
    // With the store full, another owner releases its own oldest, never these.
    for text in ["mine", "more"] {
        let input = ShellInput::new(format!("printf {text}"));
        let named = shell.shell_owned("a", input, &cancel).await.unwrap();
        assert!(named.data.stdout.archive.unwrap().archive_id.is_some());
    }
    assert_eq!(store.list(None, &cancel).await.unwrap().len(), 2);
    assert_eq!(store.list(Some("a"), &cancel).await.unwrap().len(), 1);
    assert!(store.close().await);
}

#[tokio::test]
async fn archives_a_job_or_a_call_still_writes_are_never_released() {
    let root = tempfile::tempdir().unwrap();
    let limits = OutputLimits {
        max_archives_per_owner: 3,
        ..OutputLimits::default()
    };
    let store = OutputStore::new(Some(root.path()), limits).await.unwrap();
    let session = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_output_store(store.clone())
        .unwrap();
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(cutting())
        .unwrap()
        .with_output_store(store.clone());
    let cancel = CancellationToken::new();
    // A running job writes o1 and o2 for as long as it runs.
    let job = session
        .start(None, ProcessStartInput::new("cat"), &cancel)
        .await
        .unwrap();
    let job_out = job.data.stdout_archive.unwrap().archive_id.unwrap();
    // The first call's stdout takes the last slot; its stderr finds none to
    // free, the job's archives being still written.
    let first = shell
        .shell(ShellInput::new("printf ab"), &cancel)
        .await
        .unwrap();
    let first_out = first.data.stdout.archive.unwrap().archive_id.unwrap();
    // The second call frees the first call's, finished, never the job's.
    let second = shell
        .shell(ShellInput::new("printf cd"), &cancel)
        .await
        .unwrap();
    assert!(second.data.stdout.archive.unwrap().archive_id.is_some());
    assert!(
        store
            .read(None, page(&job_out, 0, 8), &cancel)
            .await
            .is_ok()
    );
    assert!(matches!(
        store.read(None, page(&first_out, 0, 8), &cancel).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(session.close().await);
    assert!(store.close().await);
}

#[tokio::test]
async fn a_call_still_running_keeps_archives_whose_streams_ended() {
    let root = tempfile::tempdir().unwrap();
    let limits = OutputLimits {
        max_archives_per_owner: 2,
        ..OutputLimits::default()
    };
    let store = OutputStore::new(Some(root.path()), limits).await.unwrap();
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(cutting())
        .unwrap()
        .with_output_store(store.clone());
    let cancel = CancellationToken::new();
    // The first call ends its streams, then runs on: its archives are done
    // being written, but the call has not returned them yet.
    let first = shell.shell(
        ShellInput::new("printf abc; exec >&- 2>&-; sleep 1"),
        &cancel,
    );
    let second = async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !store
                .list(None, &cancel)
                .await
                .unwrap()
                .iter()
                .any(|archive| archive.eof && archive.stored_bytes == 3)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        shell
            .shell(ShellInput::new("printf de"), &cancel)
            .await
            .unwrap()
    };
    let (first, second) = tokio::join!(first, second);
    let archive = first.unwrap().data.stdout.archive.unwrap();
    assert_eq!(archive.error, None, "{archive:?}");
    let id = archive.archive_id.unwrap();
    assert_eq!(
        store
            .read(None, page(&id, 0, 8), &cancel)
            .await
            .unwrap()
            .data
            .text,
        "abc"
    );
    // The second call found no archive to free while the first ran.
    let refused = second.data.stdout.archive.unwrap();
    assert_eq!(refused.error.as_deref(), Some("archive_limit"));
    assert!(store.close().await);
}

#[tokio::test]
async fn an_output_kept_whole_leaves_no_archive() {
    let root = tempfile::tempdir().unwrap();
    let store = OutputStore::new(Some(root.path()), OutputLimits::default())
        .await
        .unwrap();
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_output_store(store.clone());
    let cancel = CancellationToken::new();
    let result = shell
        .shell(ShellInput::new("printf whole; printf also >&2"), &cancel)
        .await
        .unwrap();
    assert_eq!(result.data.stdout.text, "whole");
    assert!(result.data.stdout.archive.is_none());
    assert!(result.data.stderr.archive.is_none());
    assert!(!result.text.contains("archive"), "{}", result.text);
    assert_eq!(store.list(None, &cancel).await.unwrap(), []);
    assert!(store.close().await);
}

#[tokio::test]
async fn aggregate_budget_is_shared_by_owners_and_released_explicitly() {
    let root = tempfile::tempdir().unwrap();
    let limits = OutputLimits {
        max_total_bytes: 4,
        ..OutputLimits::default()
    };
    let store = OutputStore::new(Some(root.path()), limits).await.unwrap();
    let shell = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(cutting())
        .unwrap()
        .with_output_store(store.clone());
    let cancel = CancellationToken::new();
    let first = shell
        .shell_owned("one", ShellInput::new("printf 1234"), &cancel)
        .await
        .unwrap();
    let id = first.data.stdout.archive.unwrap().archive_id.unwrap();
    let second = shell
        .shell_owned("two", ShellInput::new("printf 5678"), &cancel)
        .await
        .unwrap();
    let archive = second.data.stdout.archive.unwrap();
    assert_eq!(archive.stored_bytes, 0);
    assert_eq!(archive.error.as_deref(), Some("total_limit"));
    assert!(matches!(
        store.read(Some("two"), page(&id, 0, 1), &cancel).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(matches!(
        store
            .forget(
                Some("two"),
                OutputForgetInput {
                    archive_id: id.clone()
                },
                &cancel
            )
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    assert_eq!(store.list(Some("unknown"), &cancel).await.unwrap(), []);
    store
        .forget(Some("one"), OutputForgetInput { archive_id: id }, &cancel)
        .await
        .unwrap();
    let third = shell
        .shell_owned("two", ShellInput::new("printf 5678"), &cancel)
        .await
        .unwrap();
    assert!(third.data.stdout.archive.unwrap().complete);
    assert!(store.close().await);
}

#[tokio::test]
async fn process_archives_remain_readable_after_session_close() {
    let root = tempfile::tempdir().unwrap();
    let store = OutputStore::new(Some(root.path()), OutputLimits::default())
        .await
        .unwrap();
    let limits = ProcessLimits {
        max_output_bytes: 2,
        ..ProcessLimits::default()
    };
    let session = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap()
        .with_output_store(store.clone())
        .unwrap();
    let cancel = CancellationToken::new();
    let start = session
        .start(Some("owner"), ProcessStartInput::new("cat"), &cancel)
        .await
        .unwrap()
        .data;
    let id = start.stdout_archive.unwrap().archive_id.unwrap();
    session
        .write(
            Some("owner"),
            ProcessWriteInput {
                text: "πabc\n".into(),
                ..ProcessWriteInput::new(start.process_id.clone())
            },
            &cancel,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let result = store
                .read(Some("owner"), page(&id, 0, 1), &cancel)
                .await
                .unwrap()
                .data;
            if result.archive.stored_bytes == 6 {
                assert_eq!(result.bytes, [207]);
                assert_eq!(result.text, "�");
                assert!(!result.archive.eof);
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        store
            .forget(
                Some("owner"),
                OutputForgetInput {
                    archive_id: id.clone()
                },
                &cancel
            )
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    let status = session
        .status(
            Some("owner"),
            ProcessStatusInput {
                process_id: Some(start.process_id),
            },
            &cancel,
        )
        .await
        .unwrap();
    assert!(!status.data.processes.first().unwrap().stdin_closed);
    assert!(session.close().await);
    let result = store
        .read(Some("owner"), page(&id, 0, 100), &cancel)
        .await
        .unwrap()
        .data;
    assert_eq!(result.bytes, "πabc\n".as_bytes());
    assert!(result.archive.complete);
    assert!(store.close().await);
}

#[tokio::test]
async fn closing_store_keeps_the_process_running_and_refuses_reads() {
    let root = tempfile::tempdir().unwrap();
    let store = OutputStore::new(Some(root.path()), OutputLimits::default())
        .await
        .unwrap();
    let manager = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_output_store(store.clone())
        .unwrap();
    let cancel = CancellationToken::new();
    let start = manager
        .start(Some("owner"), ProcessStartInput::new("cat"), &cancel)
        .await
        .unwrap()
        .data;
    assert!(store.close().await);
    assert!(store.close().await);
    let sent = manager
        .write(
            Some("owner"),
            ProcessWriteInput {
                text: "still running\n".into(),
                close_stdin: true,
                ..ProcessWriteInput::new(start.process_id)
            },
            &cancel,
        )
        .await
        .unwrap();
    assert!(sent.data.complete);
    assert!(store.list(Some("owner"), &cancel).await.is_err());
    assert!(manager.close().await);
}
