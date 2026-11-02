//! A host can follow actual lifecycle changes without model calls or polling.
#![cfg(unix)]

use roomkit_tools_process::{
    ProcessSession, ProcessStartInput, ProcessState, ProcessStopInput, ProcessWriteInput,
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn observation_tracks_owners_natural_exit_stable_duration_and_forgetting() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessSession::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let mut changes = manager.subscribe();
    assert!(manager.observe().await.is_empty());
    let process = manager
        .start(
            Some("trusted-owner"),
            ProcessStartInput::new("cat"),
            &cancel,
        )
        .await
        .unwrap()
        .data;
    tokio::time::timeout(Duration::from_secs(2), changes.changed())
        .await
        .unwrap()
        .unwrap();
    let observed = manager.observe().await;
    assert_eq!(observed[0].owner, "trusted-owner");
    assert_eq!(observed[0].command, "cat");
    assert_eq!(observed[0].status.state, ProcessState::Running);
    manager
        .write(
            Some("trusted-owner"),
            ProcessWriteInput {
                process_id: process.process_id.clone(),
                text: "hello\n".into(),
                close_stdin: false,
            },
            &cancel,
        )
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), changes.changed())
            .await
            .is_err(),
        "Output alone must not drive UI redraws"
    );
    manager
        .write(
            Some("trusted-owner"),
            ProcessWriteInput {
                process_id: process.process_id.clone(),
                text: String::new(),
                close_stdin: true,
            },
            &cancel,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), changes.changed())
        .await
        .unwrap()
        .unwrap();
    let finished = manager.observe().await;
    assert_eq!(finished[0].status.state, ProcessState::Finished);
    assert_eq!(finished[0].status.exit_code, Some(0));
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(manager.observe().await[0].elapsed, finished[0].elapsed);
    manager
        .stop(
            Some("trusted-owner"),
            ProcessStopInput {
                process_id: process.process_id,
                forget: true,
            },
            &cancel,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), changes.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(manager.observe().await.is_empty());
    assert!(manager.close().await);
}
