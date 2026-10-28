//! Actual RoomKit turns compare, preview, apply and verify a multi-file change.
#![expect(clippy::unwrap_used, reason = "broken fixtures must fail tests")]

use roomkit::model::{ChannelId, ChannelType, EventContent, InboundMessage, RoomId, ToolCall};
use roomkit::testing::RecordingTransport;
use roomkit::{RoomKit, ToolUse};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiAgent, ToolContext, ToolError, Toolset};
use roomkit_tools_fs::{DiffTool, MutationWorkspace, PatchTool};
use roomkit_tools_roomkit::{DiffToolset, PatchToolset};
use serde_json::{Value, json};

fn changes() -> Value {
    json!({"changes":[
        {"op":"update","path":"hours","old_content":"Saturday: 16:00\n","new_content":"Saturday: 17:00\n"},
        {"op":"add","path":"notice","content":"New hours\n"},
        {"op":"move","from":"orders","to":"archived-orders","old_content":"43 | Taylor\n"},
        {"op":"delete","path":"obsolete","old_content":"old\n"}
    ]})
}

fn script() -> [ScriptedRound; 5] {
    let preview = changes().to_string();
    let mut apply = changes();
    apply["dry_run"] = json!(false);
    let apply = apply.to_string();
    [
        ScriptedRound::calling(
            "",
            [(
                "d1",
                "diff",
                r#"{"old_path":"hours","new_path":"proposed"}"#,
            )],
        ),
        ScriptedRound::calling("", [("p1", "patch", preview.as_str())]),
        ScriptedRound::calling("", [("p2", "patch", apply.as_str())]),
        ScriptedRound::calling(
            "",
            [(
                "d2",
                "diff",
                r#"{"old_path":"hours","new_path":"proposed"}"#,
            )],
        ),
        ScriptedRound::answering("The requested tool sequence is complete."),
    ]
}

async fn scenario(refused: bool) -> Vec<Value> {
    let root = tempfile::tempdir().unwrap();
    for (path, value) in [
        ("hours", "Saturday: 16:00\n"),
        ("proposed", "Saturday: 17:00\n"),
        ("orders", "43 | Taylor\n"),
        ("obsolete", "old\n"),
    ] {
        std::fs::write(root.path().join(path), value).unwrap();
    }
    let model = ScriptedProvider::new(script());
    let id = ChannelId::new("assistant").unwrap();
    let agent = AiAgent::new(id.clone(), model.clone())
        .with_tools(DiffToolset::new(DiffTool::new(root.path()).await.unwrap()))
        .unwrap()
        .with_tools(PatchToolset::new(PatchTool::from_workspace(
            MutationWorkspace::new(root.path()).await.unwrap(),
        )))
        .unwrap();
    let kit = RoomKit::new();
    if refused {
        kit.before_tool_use("deny_patch", |event, _| async move {
            Ok(if event.call.name == "patch" {
                ToolUse::block("Patch disabled")
            } else {
                ToolUse::Allow
            })
        })
        .unwrap();
    }
    let chat = RecordingTransport::new(ChannelId::new("chat").unwrap(), ChannelType::WEBSOCKET);
    kit.register_channel(chat.clone()).unwrap();
    kit.register_channel(agent).unwrap();
    let conversation = RoomId::new("example").unwrap();
    kit.create_room(&conversation).await.unwrap();
    for channel in [chat.id(), &id] {
        kit.attach(&conversation, channel).await.unwrap();
    }
    let result = kit
        .process_inbound_in(
            &conversation,
            InboundMessage::new(
                chat.id().clone(),
                "test-user",
                EventContent::text("Compare, preview, apply and verify the hours."),
            ),
        )
        .await
        .unwrap();
    assert!(result.error.is_none());
    kit.idle().await;
    let requests = model.requests();
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["diff", "patch"]
    );
    assert_eq!(
        requests[0].tools[0].parameters,
        DiffTool::definition().input_schema
    );
    assert_eq!(
        requests[0].tools[1].parameters,
        PatchTool::definition().input_schema
    );
    let outputs = requests[4]
        .messages
        .iter()
        .filter(|message| message.tool_call_id.is_some())
        .map(|message| {
            serde_json::from_str(&message.text)
                .unwrap_or_else(|_| Value::String(message.text.clone()))
        })
        .collect();
    assert_eq!(
        std::fs::read_to_string(root.path().join("hours")).unwrap(),
        if refused {
            "Saturday: 16:00\n"
        } else {
            "Saturday: 17:00\n"
        }
    );
    assert_eq!(root.path().join("notice").exists(), !refused);
    assert_eq!(root.path().join("obsolete").exists(), refused);
    assert_eq!(root.path().join("orders").exists(), refused);
    assert_eq!(root.path().join("archived-orders").exists(), !refused);
    kit.shutdown().await;
    outputs
}

#[tokio::test]
async fn agent_receives_real_hunks_preview_application_and_final_equality() {
    let outputs = scenario(false).await;
    assert_eq!(outputs[0]["data"]["additions"], 1);
    assert_eq!(outputs[1]["data"]["dry_run"], true);
    assert_eq!(outputs[1]["data"]["applied"], json!([]));
    assert_eq!(outputs[2]["data"]["completed"], true);
    assert_eq!(outputs[2]["data"]["applied"].as_array().unwrap().len(), 5);
    assert_eq!(outputs[3]["data"]["changed"], false);
}

#[tokio::test]
async fn roomkit_can_refuse_patch_while_allowing_diff() {
    let outputs = scenario(true).await;
    assert!(outputs[1].to_string().contains("Patch disabled"));
    assert!(outputs[2].to_string().contains("Patch disabled"));
    assert_eq!(outputs[3]["data"]["changed"], true);
}

#[tokio::test]
async fn closed_adapters_refuse_calls_independently() {
    let root = tempfile::tempdir().unwrap();
    let patch = PatchToolset::new(PatchTool::new(root.path()).await.unwrap());
    patch.close().await;
    let call = ToolCall::new("p", "patch", serde_json::from_value(changes()).unwrap());
    let context = ToolContext::new(
        RoomId::new("example").unwrap(),
        ChannelId::new("assistant").unwrap(),
    );
    assert!(matches!(
        patch.call(call, context).await,
        Err(ToolError::Refused(_))
    ));
    std::fs::write(root.path().join("same"), "x").unwrap();
    let diff = DiffToolset::new(DiffTool::new(root.path()).await.unwrap());
    let call = ToolCall::new(
        "d",
        "diff",
        serde_json::from_value(json!({"old_path":"same","new_path":"same"})).unwrap(),
    );
    let context = ToolContext::new(
        RoomId::new("example").unwrap(),
        ChannelId::new("assistant").unwrap(),
    );
    assert!(diff.call(call.clone(), context.clone()).await.is_ok());
    diff.close().await;
    assert!(matches!(
        diff.call(call, context).await,
        Err(ToolError::Refused(_))
    ));
}
