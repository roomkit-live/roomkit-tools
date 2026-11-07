//! Archive access follows the same trusted room/agent identity as shell/processes.
#![cfg(unix)]

mod call;

use roomkit_ai::{ToolError, ToolResult};
use roomkit_tools_process::{OutputLimits, OutputStore, ShellLimits, ShellTool};
use roomkit_tools_roomkit::{OutputForget, OutputRead, Shell};
use serde_json::json;

#[tokio::test]
async fn archived_shell_output_uses_room_and_agent_scope() {
    let root = tempfile::tempdir().unwrap();
    let store = OutputStore::new(Some(root.path()), OutputLimits::default())
        .await
        .unwrap();
    // Only an output cut short is archived: cut every one past a byte.
    let cutting = ShellLimits {
        max_output_bytes: 1,
        ..ShellLimits::default()
    };
    let shell = Shell::new(
        ShellTool::new(root.path())
            .await
            .unwrap()
            .with_limits(cutting)
            .unwrap()
            .with_output_store(store.clone()),
    );
    let read = OutputRead::new(store.clone());
    let forget = OutputForget::new(store.clone());
    let ToolResult::Json(result) = call::run(
        &shell,
        "one",
        "agent",
        "shell",
        json!({"command":"printf saved"}),
    )
    .await
    .unwrap() else {
        panic!("expected JSON");
    };
    let aid = result["data"]["stdout"]["archive"]["archive_id"]
        .as_str()
        .unwrap();
    for (room, agent) in [("two", "agent"), ("one", "another")] {
        assert!(matches!(
            call::run(&read, room, agent, "output_read", json!({"archive_id":aid})).await,
            Err(ToolError::Refused(_))
        ));
        assert!(matches!(
            call::run(
                &forget,
                room,
                agent,
                "output_forget",
                json!({"archive_id":aid})
            )
            .await,
            Err(ToolError::Refused(_))
        ));
    }
    let ToolResult::Json(page) = call::run(
        &read,
        "one",
        "agent",
        "output_read",
        json!({"archive_id":aid}),
    )
    .await
    .unwrap() else {
        panic!("expected JSON");
    };
    assert_eq!(page["data"]["text"], "saved");
    call::run(
        &forget,
        "one",
        "agent",
        "output_forget",
        json!({"archive_id":aid}),
    )
    .await
    .unwrap();
    assert!(matches!(
        call::run(
            &read,
            "one",
            "agent",
            "output_read",
            json!({"archive_id":aid})
        )
        .await,
        Err(ToolError::Refused(_))
    ));
    assert!(store.close().await);
}
