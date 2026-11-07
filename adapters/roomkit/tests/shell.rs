//! RoomKit forwards shell results and can refuse commands before execution.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]

mod call;
mod common;

use std::sync::Arc;
use std::time::Duration;

use roomkit::RoomKit;
use roomkit::model::{ChannelId, ChannelType, InboundMessage, RoomId, TextContent};
use roomkit::testing::{RecordingTransport, RecordingTransportOptions};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiChannelOptions, ToolEntry, ToolResult};
use roomkit_tools_process::ShellTool;
use roomkit_tools_roomkit::Shell;
use serde_json::{Value, json};

#[tokio::test]
async fn real_agent_receives_shell_output_or_a_policy_refusal() {
    for refused in [false, true] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("hours"), "Saturday: 08:30-16:00\n").unwrap();
        let model = ScriptedProvider::new([
            ScriptedRound::calling(
                "",
                [(
                    "s1",
                    "shell",
                    r#"{"command":"cat hours; printf warning >&2; printf done > marker; exit 3"}"#,
                )],
            ),
            ScriptedRound::answering("The command completed."),
        ]);
        let id = ChannelId::new("assistant").unwrap();
        let shell = Shell::new(ShellTool::new(root.path()).await.unwrap());
        let agent = common::agent(&id, &model, vec![ToolEntry::Tool(Arc::new(shell))]).unwrap();
        let kit = RoomKit::new(roomkit::RoomKitOptions::default());
        if refused {
            common::refuse(&kit, &["shell"], "Commands disabled").unwrap();
        }
        let chat = RecordingTransport::new(
            ChannelId::new("chat").unwrap(),
            ChannelType::WEBSOCKET,
            RecordingTransportOptions::default(),
        );
        kit.register_channel(chat.clone()).unwrap();
        kit.register_channel(agent).unwrap();
        let conversation = RoomId::new("example").unwrap();
        common::attach(&kit, &conversation, [chat.channel_id(), &id])
            .await
            .unwrap();
        let result = kit
            .process_inbound(
                InboundMessage::new(
                    chat.channel_id().clone(),
                    "test-user",
                    TextContent::new("Run the shell command"),
                ),
                roomkit::ProcessInboundOptions {
                    room_id: Some(conversation.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(result.error.is_none());
        roomkit::testing::idle(&kit).await;
        let requests = model.requests();
        assert_eq!(requests.len(), 2);
        // The kit declares read_stored_result beside an agent's own tools, to
        // page through an outcome it stored (roomkit-rs RMK-1228, as Python's).
        assert_eq!(requests[0].tools.len(), 2);
        assert_eq!(requests[0].tools[0].name, "shell");
        assert_eq!(
            requests[0].tools[0].parameters,
            ShellTool::definition().input_schema
        );
        let text = &requests[1]
            .messages
            .iter()
            .find(|m| m.tool_result().is_some())
            .unwrap()
            .text();
        if refused {
            assert!(text.contains("Commands disabled"));
            assert!(!root.path().join("marker").exists());
        } else {
            let value: Value = serde_json::from_str(text).unwrap();
            assert_eq!(value["data"]["stdout"]["text"], "Saturday: 08:30-16:00\n");
            assert_eq!(value["data"]["stderr"]["text"], "warning");
            assert_eq!(value["data"]["exit_code"], 3);
            // The model reads the output once: in data, with no text rendering.
            assert_eq!(value.as_object().unwrap().len(), 1, "{value}");
            assert_eq!(
                std::fs::read_to_string(root.path().join("marker")).unwrap(),
                "done"
            );
        }
        kit.close().await.unwrap();
    }
}

/// Whether the process `pid` still exists.
fn alive(pid: &str) -> bool {
    std::process::Command::new("sh")
        .args(["-c", "kill -0 \"$1\" 2>/dev/null", "sh", pid])
        .status()
        .unwrap()
        .success()
}

#[tokio::test]
async fn roomkit_dropping_a_running_shell_call_kills_its_process() {
    let root = tempfile::tempdir().unwrap();
    let shell = Shell::new(ShellTool::new(root.path()).await.unwrap());
    // RoomKit drops a call past its bound per call; dropping it cancels the shell.
    let options = AiChannelOptions {
        tool_timeout: Some(Duration::from_secs(2)),
        ..Default::default()
    };
    let answer = call::turn(
        &shell,
        "example",
        "assistant",
        "shell",
        json!({"command":"printf $$ > pid; exec sleep 60"}),
        options,
    )
    .await;
    assert!(
        answer.is_none(),
        "RoomKit dropped the call before it answered: {answer:?}"
    );
    let pid = std::fs::read_to_string(root.path().join("pid")).unwrap();
    assert!(pid.parse::<u32>().is_ok(), "{pid:?}");
    tokio::time::timeout(Duration::from_secs(5), async {
        while alive(&pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn timeout_is_a_structured_result() {
    let root = tempfile::tempdir().unwrap();
    let shell = Shell::new(ShellTool::new(root.path()).await.unwrap());
    let ToolResult::Json(result) = call::run(
        &shell,
        "example",
        "assistant",
        "shell",
        json!({"command":"printf ready; sleep 60", "timeout_ms":300}),
    )
    .await
    .unwrap() else {
        panic!("expected structured result");
    };
    assert_eq!(result["data"]["timed_out"], true);
    assert_eq!(result["data"]["stdout"]["text"], "ready");
}
