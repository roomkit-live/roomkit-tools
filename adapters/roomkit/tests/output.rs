//! Archive access follows the same trusted room/agent identity as shell/processes.
#![cfg(unix)]

use roomkit::model::{ChannelId, RoomId, ToolCall};
use roomkit_ai::{ToolContext, ToolError, ToolOutput, Toolset};
use roomkit_tools_process::{OutputLimits, OutputStore, ShellTool};
use roomkit_tools_roomkit::{OutputForgetToolset, OutputReadToolset, ShellToolset};
use serde_json::{Value, json};

#[expect(clippy::unwrap_used, reason = "test context must be valid")]
fn context(room: &str, agent: &str) -> ToolContext {
    ToolContext::new(RoomId::new(room).unwrap(), ChannelId::new(agent).unwrap())
}

#[expect(clippy::unwrap_used, reason = "test arguments must be valid")]
fn call(name: &str, args: Value) -> ToolCall {
    ToolCall::new("call", name, serde_json::from_value(args).unwrap())
}

#[tokio::test]
async fn archived_shell_output_uses_room_and_agent_scope() {
    let root = tempfile::tempdir().unwrap();
    let store = OutputStore::new(root.path(), OutputLimits::default())
        .await
        .unwrap();
    let shell = ShellToolset::new(
        ShellTool::new(root.path())
            .await
            .unwrap()
            .with_output_store(store.clone()),
    );
    let read = OutputReadToolset::new(store.clone());
    let forget = OutputForgetToolset::new(store.clone());
    let ToolOutput::Json(result) = shell
        .call(
            call("shell", json!({"command":"printf saved"})),
            context("one", "agent"),
        )
        .await
        .unwrap()
    else {
        panic!("expected JSON");
    };
    let aid = result["data"]["stdout"]["archive"]["archive_id"]
        .as_str()
        .unwrap();
    for (room, agent) in [("two", "agent"), ("one", "another")] {
        assert!(matches!(
            read.call(
                call("output_read", json!({"archive_id":aid})),
                context(room, agent)
            )
            .await,
            Err(ToolError::Refused(_))
        ));
        assert!(matches!(
            forget
                .call(
                    call("output_forget", json!({"archive_id":aid})),
                    context(room, agent)
                )
                .await,
            Err(ToolError::Refused(_))
        ));
    }
    let ToolOutput::Json(page) = read
        .call(
            call("output_read", json!({"archive_id":aid})),
            context("one", "agent"),
        )
        .await
        .unwrap()
    else {
        panic!("expected JSON");
    };
    assert_eq!(page["data"]["text"], "saved");
    shell.close().await;
    // Closing a selected adapter does not discard the shared store's logs.
    assert!(
        read.call(
            call("output_read", json!({"archive_id":aid})),
            context("one", "agent")
        )
        .await
        .is_ok()
    );
    forget
        .call(
            call("output_forget", json!({"archive_id":aid})),
            context("one", "agent"),
        )
        .await
        .unwrap();
    assert!(matches!(
        read.call(
            call("output_read", json!({"archive_id":aid})),
            context("one", "agent")
        )
        .await,
        Err(ToolError::Refused(_))
    ));
    read.close().await;
    forget.close().await;
    assert!(store.close().await);
}
