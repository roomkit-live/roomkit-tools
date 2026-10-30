//! RoomKit forwards shell results and can refuse commands before execution.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]

mod common;

use roomkit::model::{ChannelId, ChannelType, InboundMessage, RoomId, TextContent, ToolCall};
use roomkit::testing::RecordingTransport;
use roomkit::{RoomKit, ToolUse};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiAgent, ToolContext, ToolError, ToolOutput, Toolset};
use roomkit_tools_process::ShellTool;
use roomkit_tools_roomkit::ShellToolset;
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
        let agent = AiAgent::new(id.clone(), model.clone())
            .with_tools(ShellToolset::new(
                ShellTool::new(root.path()).await.unwrap(),
            ))
            .unwrap();
        let kit = RoomKit::new(roomkit::RoomKitOptions::default());
        if refused {
            kit.before_tool_use("deny_shell", |_, _| async {
                Ok(ToolUse::block("Commands disabled"))
            })
            .unwrap();
        }
        let chat = RecordingTransport::new(ChannelId::new("chat").unwrap(), ChannelType::WEBSOCKET);
        kit.register_channel(chat.clone()).unwrap();
        kit.register_channel(agent).unwrap();
        let conversation = RoomId::new("example").unwrap();
        common::attach(&kit, &conversation, [chat.id(), &id])
            .await
            .unwrap();
        let result = kit
            .process_inbound(
                InboundMessage::new(
                    chat.id().clone(),
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
        assert_eq!(requests[0].tools.len(), 1);
        assert_eq!(requests[0].tools[0].name, "shell");
        assert_eq!(
            requests[0].tools[0].parameters,
            ShellTool::definition().input_schema
        );
        let text = &requests[1]
            .messages
            .iter()
            .find(|m| m.tool_call_id.is_some())
            .unwrap()
            .text;
        if refused {
            assert!(text.contains("Commands disabled"));
            assert!(!root.path().join("marker").exists());
        } else {
            let value: Value = serde_json::from_str(text).unwrap();
            assert_eq!(value["data"]["stdout"]["text"], "Saturday: 08:30-16:00\n");
            assert_eq!(value["data"]["stderr"]["text"], "warning");
            assert_eq!(value["data"]["exit_code"], 3);
            assert_eq!(
                std::fs::read_to_string(root.path().join("marker")).unwrap(),
                "done"
            );
        }
        kit.close().await.unwrap();
    }
}

fn context() -> ToolContext {
    ToolContext::new(
        RoomId::new("example").unwrap(),
        ChannelId::new("assistant").unwrap(),
    )
}

#[tokio::test]
async fn closing_a_toolset_cancels_an_already_running_shell() {
    let root = tempfile::tempdir().unwrap();
    let toolset = std::sync::Arc::new(ShellToolset::new(
        ShellTool::new(root.path()).await.unwrap(),
    ));
    let running = std::sync::Arc::clone(&toolset);
    let task = tokio::spawn(async move {
        running
            .call(
                ToolCall::new(
                    "s1",
                    "shell",
                    serde_json::from_value(json!({"command":"printf ready > ready; sleep 60"}))
                        .unwrap(),
                ),
                context(),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while tokio::fs::read_to_string(root.path().join("ready"))
            .await
            .unwrap_or_default()
            != "ready"
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    toolset.close().await;
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap(),
        Err(ToolError::Failed(_))
    ));
}

#[tokio::test]
async fn timeout_is_a_structured_result_and_close_refuses_new_calls() {
    let root = tempfile::tempdir().unwrap();
    let toolset = ShellToolset::new(ShellTool::new(root.path()).await.unwrap());
    let call = || {
        ToolCall::new(
            "s1",
            "shell",
            serde_json::from_value(json!({"command":"printf ready; sleep 60", "timeout_ms":300}))
                .unwrap(),
        )
    };
    let ToolOutput::Json(result) = toolset.call(call(), context()).await.unwrap() else {
        panic!("expected structured result");
    };
    assert_eq!(result["data"]["timed_out"], true);
    assert_eq!(result["data"]["stdout"]["text"], "ready");
    toolset.close().await;
    assert!(matches!(
        toolset.call(call(), context()).await,
        Err(ToolError::Refused(_))
    ));
}
