//! Hosts reach the jobs and archives of RoomKit tool calls through `roomkit_owner`.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "fixture errors must fail tests")]

use roomkit::model::{ChannelId, RoomId, ToolCall};
use roomkit_ai::{ToolContext, ToolOutput, Toolset};
use roomkit_tools_process::{
    OutputForgetInput, OutputLimits, OutputStore, ProcessManager, ProcessReadInput, ProcessState,
    ProcessStatusInput, ProcessStopInput,
};
use roomkit_tools_roomkit::{ProcessStartToolset, roomkit_owner};
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
async fn start(toolset: &ProcessStartToolset, room: &str, agent: &str, command: &str) -> Value {
    let context = ToolContext::new(RoomId::new(room).unwrap(), ChannelId::new(agent).unwrap());
    let call = ToolCall::new(
        "call",
        "process_start",
        serde_json::from_value(json!({ "command": command })).unwrap(),
    );
    let ToolOutput::Json(started) = toolset.call(call, context).await.unwrap() else {
        panic!("expected JSON");
    };
    started["data"].clone()
}

#[tokio::test]
async fn host_reaches_toolset_jobs_and_archives_through_roomkit_owner() {
    let root = tempfile::tempdir().unwrap();
    let store = OutputStore::new(root.path(), OutputLimits::default())
        .await
        .unwrap();
    let manager = ProcessManager::new(root.path())
        .await
        .unwrap()
        .with_output_store(store.clone())
        .unwrap();
    let toolset = ProcessStartToolset::new(manager.clone());
    let started = start(&toolset, "salle \"é\"", "agent", "printf saved; sleep 60").await;
    let id = started["process_id"].as_str().unwrap();
    let owner = roomkit_owner("salle \"é\"", "agent");
    let cancel = CancellationToken::new();
    let listed = manager
        .status(&owner, ProcessStatusInput::default(), &cancel)
        .await
        .unwrap()
        .data;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].process_id, id);
    assert_eq!(listed[0].state, ProcessState::Running);
    let stop = ProcessStopInput {
        process_id: id.into(),
        forget: false,
    };
    let stopped = manager.stop(&owner, stop, &cancel).await.unwrap();
    assert_eq!(stopped.data.status.state, ProcessState::Finished);
    let page = manager
        .read(&owner, ProcessReadInput::new(id), &cancel)
        .await
        .unwrap();
    assert_eq!(page.data.stdout.text, "saved");
    assert!(manager.close_owner(&owner).await);
    let listed = manager
        .status(&owner, ProcessStatusInput::default(), &cancel)
        .await
        .unwrap();
    assert_eq!(listed.data.len(), 0);
    let archives = store.list(&owner, &cancel).unwrap();
    assert_eq!(archives.len(), 2);
    for archive in archives {
        let archive_id = archive.archive_id.unwrap();
        store
            .forget(&owner, OutputForgetInput { archive_id }, &cancel)
            .await
            .unwrap();
    }
    assert_eq!(store.list(&owner, &cancel).unwrap().len(), 0);
    assert!(manager.close().await);
    assert!(store.close().await);
}
