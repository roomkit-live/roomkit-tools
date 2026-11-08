//! What a model reads of a command's result: its text alone must do.

#![expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]

use std::num::{NonZeroU64, NonZeroUsize};
use std::path::Path;
use std::time::Duration;

use roomkit_tools_process::{
    OutputForgetInput, OutputLimits, OutputReadInput, OutputStore, ProcessReadInput,
    ProcessSession, ProcessStartInput, ProcessStatusInput, ProcessStopInput, ShellInput,
    ShellLimits, ShellTool,
};
use tokio_util::sync::CancellationToken;

async fn text_of(command: &str) -> String {
    let root = tempfile::tempdir().unwrap();
    ShellTool::new(root.path())
        .await
        .unwrap()
        .shell(ShellInput::new(command), &CancellationToken::new())
        .await
        .unwrap()
        .text
}

async fn store(limits: OutputLimits) -> OutputStore {
    OutputStore::new(None::<&Path>, limits).await.unwrap()
}

/// A shell cutting its streams after four bytes, archiving them in `store`.
async fn cutting(root: &Path, store: &OutputStore) -> ShellTool {
    let limits = ShellLimits {
        max_output_bytes: 4,
        ..ShellLimits::default()
    };
    ShellTool::new(root)
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap()
        .with_output_store(store.clone())
}

#[tokio::test]
async fn a_command_reads_as_how_it_ended_and_the_streams_it_wrote() {
    assert_eq!(
        text_of("echo hi; echo warn >&2").await,
        "exit_code=0\nstdout:\nhi\nstderr:\nwarn\n"
    );
    assert_eq!(
        text_of("echo nope; exit 3").await,
        "exit_code=3\nstdout:\nnope\n"
    );
    assert_eq!(
        text_of("printf partial").await,
        "exit_code=0\nstdout:\npartial\n"
    );
}

#[tokio::test]
async fn a_silent_command_says_so_and_a_killed_one_names_its_signal() {
    assert_eq!(text_of("true").await, "exit_code=0 (no output)\n");
    assert_eq!(
        text_of("kill -9 $$").await,
        "exit_code=null, signal=9 (no output)\n"
    );
}

#[tokio::test]
async fn a_timeout_and_pipes_left_open_are_said() {
    let root = tempfile::tempdir().unwrap();
    let tool = ShellTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let mut slow = ShellInput::new("sleep 5");
    slow.timeout_ms = Some(NonZeroU64::new(50).unwrap());
    let timed_out = tool.shell(slow, &cancel).await.unwrap();
    assert_eq!(
        timed_out.text,
        format!(
            "exit_code=null, signal={}, timed_out=true (no output)\n",
            timed_out.data.signal.unwrap()
        )
    );
    let keeping = ShellLimits {
        keep_background_jobs: true,
        ..ShellLimits::default()
    };
    let open = ShellTool::new(root.path())
        .await
        .unwrap()
        .with_limits(keeping)
        .unwrap()
        .shell(ShellInput::new("sleep 1 & echo hi"), &cancel)
        .await
        .unwrap();
    assert_eq!(
        open.text,
        "exit_code=0, output_complete=false\nstdout:\nhi\n"
    );
}

#[tokio::test]
async fn a_stream_cut_short_names_its_archive_before_what_the_command_wrote() {
    let root = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let store = store(OutputLimits::default()).await;
    let shell = cutting(root.path(), &store).await;
    let cut = shell
        .shell(ShellInput::new("seq 1 50"), &cancel)
        .await
        .unwrap();
    let archive = cut.data.stdout.archive.as_ref().unwrap();
    let id = archive.archive_id.clone().unwrap();
    assert_eq!(
        cut.text,
        format!(
            "exit_code=0\n[stdout archive {id}: 141 of 141 bytes stored; output_read reads it]\nstdout:\n{}",
            cut.data.stdout.text
        )
    );

    let mut input = OutputReadInput::new(id.clone());
    input.max_bytes = NonZeroUsize::new(4);
    let page = store.read(None, input, &cancel).await.unwrap();
    assert_eq!(
        page.text,
        format!("{id} bytes 0..4, next_offset=4:\n1\n2\n")
    );
    let mut rest = OutputReadInput::new(id.clone());
    rest.offset = 139;
    let last = store.read(None, rest, &cancel).await.unwrap();
    assert_eq!(last.text, format!("{id} bytes 139..141:\n0\n"));
    let forgotten = store
        .forget(
            None,
            OutputForgetInput {
                archive_id: id.clone(),
            },
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(forgotten.text, format!("{id} forgotten\n"));
    assert!(store.close().await);
}

#[tokio::test]
async fn a_partial_archive_says_so_and_one_never_stored_says_why() {
    let root = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let limits = OutputLimits {
        max_stream_bytes: 4,
        ..OutputLimits::default()
    };
    let partial = store(limits).await;
    let shell = cutting(root.path(), &partial).await;
    let cut = shell
        .shell(ShellInput::new("seq 1 50"), &cancel)
        .await
        .unwrap();
    let archive = cut.data.stdout.archive.as_ref().unwrap();
    let id = archive.archive_id.clone().unwrap();
    let error = archive.error.clone().unwrap();
    assert!(
        cut.text.contains(&format!(
            "[stdout archive {id}: 4 of 141 bytes stored, complete=false, error={error:?}; output_read reads it]\n"
        )),
        "{}",
        cut.text
    );
    let page = partial
        .read(None, OutputReadInput::new(id.clone()), &cancel)
        .await
        .unwrap();
    assert_eq!(
        page.text,
        format!("{id} bytes 0..4, complete=false, error={error:?}:\n1\n2\n")
    );
    assert!(partial.close().await);

    let full = store(OutputLimits {
        max_archives: 1,
        max_archives_per_owner: 1,
        ..OutputLimits::default()
    })
    .await;
    let shell = cutting(root.path(), &full).await;
    // One call's two streams need two archives: the second one gets none.
    let both = shell
        .shell(ShellInput::new("seq 1 50; seq 1 50 >&2"), &cancel)
        .await
        .unwrap();
    let refused = both.data.stderr.archive.as_ref().unwrap();
    assert!(refused.archive_id.is_none(), "{refused:?}");
    let reason = refused.error.clone().unwrap();
    assert!(
        both.text
            .contains(&format!("[stderr: nothing was stored, error={reason:?}]\n")),
        "{}",
        both.text
    );
    assert!(full.close().await);

    let spent = store(OutputLimits {
        max_total_bytes: 4,
        ..OutputLimits::default()
    })
    .await;
    let shell = cutting(root.path(), &spent).await;
    // The first stream takes the store's room: the second has an id, no bytes.
    let both = shell
        .shell(
            ShellInput::new("seq 1 50; sleep 0.1; seq 1 50 >&2"),
            &cancel,
        )
        .await
        .unwrap();
    let empty = both.data.stderr.archive.as_ref().unwrap();
    assert!(
        empty.archive_id.is_some() && empty.stored_bytes == 0,
        "{empty:?}"
    );
    let reason = empty.error.clone().unwrap();
    assert!(
        both.text
            .contains(&format!("[stderr: nothing was stored, error={reason:?}]\n")),
        "{}",
        both.text
    );
    assert!(spent.close().await);
}

#[tokio::test]
async fn a_process_reads_under_the_names_its_tools_describe() {
    let root = tempfile::tempdir().unwrap();
    let store = store(OutputLimits::default()).await;
    let session = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_output_store(store.clone())
        .unwrap();
    let token = CancellationToken::new();
    let none = session
        .status(None, ProcessStatusInput::default(), &token)
        .await
        .unwrap();
    assert_eq!(none.text, "no processes\n");
    let started = session
        .start(None, ProcessStartInput::new("echo up; sleep 30"), &token)
        .await
        .unwrap();
    let id = started.data.process_id.clone();
    // A stream with nothing stored has nothing to read back.
    assert_eq!(
        started.text,
        format!("{id} started; use process_read for output\n")
    );

    let mut read = session
        .read(None, ProcessReadInput::new(id.clone()), &token)
        .await
        .unwrap();
    for _ in 0..200 {
        let stored = read
            .data
            .status
            .stdout_archive
            .as_ref()
            .unwrap()
            .stored_bytes;
        if read.data.stdout.next_offset > 0 && stored > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        read = session
            .read(None, ProcessReadInput::new(id.clone()), &token)
            .await
            .unwrap();
    }
    let archive = read.data.status.stdout_archive.as_ref().unwrap();
    let archive_id = archive.archive_id.clone().unwrap();
    assert_eq!(
        read.text,
        format!(
            "{id}: state=running\n[stdout archive {archive_id}: 3 of 3 bytes stored, still being written; output_read reads it]\nstdout (next_offset=3):\nup\nstderr (next_offset=0):\n"
        )
    );
    let status = session
        .status(None, ProcessStatusInput::default(), &token)
        .await
        .unwrap();
    assert!(
        status.text.starts_with(&format!("{id}: state=running\n")),
        "{}",
        status.text
    );

    let stopped = session
        .stop(None, ProcessStopInput::new(id.clone()), &token)
        .await
        .unwrap();
    assert!(
        stopped.text.starts_with(&format!(
            "{id}: state=finished, end=stopped, exit_code=null, signal=9, forgotten=false\n"
        )),
        "{}",
        stopped.text
    );
    session.close().await;
    store.close().await;
}

#[tokio::test]
async fn a_process_whose_pipes_stay_open_says_so() {
    let root = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let id = session
        .start(
            None,
            ProcessStartInput::new("setsid sleep 3 & sleep 0.1; echo hi"),
            &token,
        )
        .await
        .unwrap()
        .data
        .process_id;
    let finished = roomkit_tools_process::ProcessState::Finished;
    let mut status = session
        .status(None, ProcessStatusInput::default(), &token)
        .await
        .unwrap();
    for _ in 0..1000 {
        if status.data.processes[0].state == finished {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        status = session
            .status(None, ProcessStatusInput::default(), &token)
            .await
            .unwrap();
    }
    assert_eq!(
        status.text,
        format!(
            "{id}: state=finished, end=exited, exit_code=0, signal=null, output_complete=false\n"
        )
    );
    session.close().await;
}

#[tokio::test]
async fn a_page_of_a_process_says_when_more_remains() {
    let root = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    let token = CancellationToken::new();
    let id = session
        .start(None, ProcessStartInput::new("printf abcdefgh"), &token)
        .await
        .unwrap()
        .data
        .process_id;
    let mut input = ProcessReadInput::new(id.clone());
    input.max_bytes = NonZeroUsize::new(3);
    let mut page = session.read(None, input.clone(), &token).await.unwrap();
    for _ in 0..200 {
        if !page
            .data
            .status
            .state
            .eq(&roomkit_tools_process::ProcessState::Running)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        page = session.read(None, input.clone(), &token).await.unwrap();
    }
    assert_eq!(
        page.text,
        format!(
            "{id}: state=finished, end=exited, exit_code=0, signal=null\nstdout (next_offset=3, has_more=true):\nabc\nstderr (next_offset=0):\n"
        )
    );
    session.close().await;
}
