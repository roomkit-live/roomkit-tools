//! Hosts reach the jobs and archives of RoomKit tool calls through `roomkit_owner`.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "fixture errors must fail tests")]

mod call;

use std::time::Duration;

use roomkit::model::Metadata;
use roomkit_ai::{Tool, ToolError, ToolResult};
use roomkit_tools_process::{
    OutputForgetInput, OutputLimits, OutputStore, ProcessReadInput, ProcessSession, ProcessState,
    ProcessStatusInput, ProcessStopInput, ShellTool,
};
use roomkit_tools_roomkit::{
    OutputForget, OutputRead, ProcessRead, ProcessStart, ProcessStatus, ProcessStop, ProcessWrite,
    Shell, roomkit_owner,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Cases Python's `roomkit_owner` checks too, with the keys both return.
const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/roomkit_owner.json"
));

#[test]
fn roomkit_owner_returns_the_shared_fixture_keys() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_ne!(cases.len(), 0);
    for case in cases {
        let room = case["room_id"].as_str().unwrap();
        let channel = case["channel_id"].as_str().unwrap();
        let owner = roomkit_owner(room, channel);
        assert_eq!(owner, case["owner"].as_str().unwrap(), "{case}");
        assert_eq!(owner, serde_json::to_string(&(room, channel)).unwrap());
    }
}

#[expect(clippy::panic, reason = "a result other than JSON must fail the test")]
async fn start(toolset: &ProcessStart, room: &str, agent: &str, command: &str) -> Value {
    let started = call::run(
        toolset,
        room,
        agent,
        "process_start",
        json!({ "command": command }),
    )
    .await;
    let Ok(ToolResult::Json(started)) = started else {
        panic!("expected JSON, got {started:?}");
    };
    started["data"].clone()
}

/// Wait until the running job `id` of `owner` has printed `text`.
async fn wait_for_stdout(session: &ProcessSession, owner: &str, id: &str, text: &str) {
    let cancel = CancellationToken::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let page = session
                .read(Some(owner), ProcessReadInput::new(id), &cancel)
                .await
                .unwrap()
                .data;
            if page.stdout.text == text {
                assert_eq!(page.status.state, ProcessState::Running);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn host_reaches_toolset_jobs_and_archives_through_roomkit_owner() {
    let root = tempfile::tempdir().unwrap();
    let store = OutputStore::new(Some(root.path()), OutputLimits::default())
        .await
        .unwrap();
    let session = ProcessSession::new(root.path())
        .await
        .unwrap()
        .with_output_store(store.clone())
        .unwrap();
    let toolset = ProcessStart::new(session.clone());
    let started = start(&toolset, "salle \"é\"", "agent", "printf saved; sleep 60").await;
    let id = started["process_id"].as_str().unwrap();
    let owner = roomkit_owner("salle \"é\"", "agent");
    let cancel = CancellationToken::new();
    let listed = session
        .status(Some(&owner), ProcessStatusInput::default(), &cancel)
        .await
        .unwrap()
        .data;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].process_id, id);
    assert_eq!(listed[0].state, ProcessState::Running);
    // Stop kills the job at once: wait for the shell to print first, or a
    // stop before `printf` runs leaves nothing to keep.
    wait_for_stdout(&session, &owner, id, "saved").await;
    let stop = ProcessStopInput::new(id);
    let stopped = session.stop(Some(&owner), stop, &cancel).await.unwrap();
    assert_eq!(stopped.data.status.state, ProcessState::Finished);
    let page = session
        .read(Some(&owner), ProcessReadInput::new(id), &cancel)
        .await
        .unwrap();
    assert_eq!(page.data.stdout.text, "saved");
    assert!(session.close_owner(Some(&owner)).await);
    let listed = session
        .status(Some(&owner), ProcessStatusInput::default(), &cancel)
        .await
        .unwrap();
    assert_eq!(listed.data.len(), 0);
    let archives = store.list(Some(&owner), &cancel).await.unwrap();
    assert_eq!(archives.len(), 2);
    for archive in archives {
        let archive_id = archive.archive_id.unwrap();
        store
            .forget(Some(&owner), OutputForgetInput { archive_id }, &cancel)
            .await
            .unwrap();
    }
    assert_eq!(store.list(Some(&owner), &cancel).await.unwrap().len(), 0);
    assert!(session.close().await);
    assert!(store.close().await);
}

/// The refusal `tool` answers a call of `name` with `arguments` made
/// outside any RoomKit tool call.
#[expect(
    clippy::panic,
    reason = "an answer other than a refusal must fail the test"
)]
async fn refusal_outside_roomkit(tool: &impl Tool, name: &str, arguments: Value) -> String {
    let arguments: Metadata = serde_json::from_value(arguments).unwrap();
    match tool.handler(name.into(), arguments).await {
        Err(ToolError::Refused(refusal)) => refusal,
        other => panic!("{name}: expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn owner_scoped_adapters_refuse_calls_outside_a_roomkit_tool_call() {
    let root = tempfile::tempdir().unwrap();
    let store = OutputStore::new(Some(root.path()), OutputLimits::default())
        .await
        .unwrap();
    let session = ProcessSession::new(root.path()).await.unwrap();
    let shell = ShellTool::new(root.path()).await.unwrap();
    let archiving = Shell::new(shell.with_output_store(store.clone()));
    let shell_refusal = "shell archive tools require a conversation and an acting agent";
    assert_eq!(
        refusal_outside_roomkit(&archiving, "shell", json!({"command":"touch shell-ran"})).await,
        shell_refusal
    );
    // The caller is checked before the arguments, as Python's adapter does:
    // invalid arguments get the same refusal here and below.
    assert_eq!(
        refusal_outside_roomkit(&archiving, "shell", json!({"bogus":1})).await,
        shell_refusal
    );
    let output_refusal = "output tools require a conversation and an acting agent";
    let archive = json!({"archive_id":"a1"});
    let read = OutputRead::new(store.clone());
    let forget = OutputForget::new(store.clone());
    assert_eq!(
        refusal_outside_roomkit(&read, "output_read", archive.clone()).await,
        output_refusal
    );
    assert_eq!(
        refusal_outside_roomkit(&forget, "output_forget", archive).await,
        output_refusal
    );
    assert_eq!(
        refusal_outside_roomkit(&read, "output_read", json!({})).await,
        output_refusal
    );
    let process_refusal = "process tools require a conversation and an acting agent";
    let process = json!({"process_id":"p1"});
    let start = ProcessStart::new(session.clone());
    let status = ProcessStatus::new(session.clone());
    let read = ProcessRead::new(session.clone());
    let write = ProcessWrite::new(session.clone());
    let stop = ProcessStop::new(session.clone());
    for refusal in [
        refusal_outside_roomkit(&read, "process_read", json!({})).await,
        refusal_outside_roomkit(
            &start,
            "process_start",
            json!({"command":"touch process-ran"}),
        )
        .await,
        refusal_outside_roomkit(&status, "process_status", json!({})).await,
        refusal_outside_roomkit(&read, "process_read", process.clone()).await,
        refusal_outside_roomkit(
            &write,
            "process_write",
            json!({"process_id":"p1","text":"order 43\n"}),
        )
        .await,
        refusal_outside_roomkit(&stop, "process_stop", process).await,
    ] {
        assert_eq!(refusal, process_refusal);
    }
    // Nothing ran: the refusal comes before any effect.
    assert!(!root.path().join("shell-ran").exists());
    assert!(!root.path().join("process-ran").exists());
    assert!(session.close().await);
    assert!(store.close().await);
}

/// A shell keeping no archive needs no caller, as Python's `Shell`: it runs
/// outside a RoomKit tool call.
#[tokio::test]
async fn a_shell_without_archives_runs_outside_a_roomkit_tool_call() {
    let root = tempfile::tempdir().unwrap();
    let shell = Shell::new(ShellTool::new(root.path()).await.unwrap());
    let arguments: Metadata = serde_json::from_value(json!({"command":"printf ran"})).unwrap();
    let Ok(ToolResult::Json(result)) = shell.handler("shell".into(), arguments).await else {
        unreachable!("the shell answers outside RoomKit");
    };
    assert_eq!(result["data"]["stdout"]["text"], "ran");
}
