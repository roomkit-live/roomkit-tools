//! Actual RoomKit turns compare, preview, apply and verify a multi-file change.
#![expect(clippy::unwrap_used, reason = "broken fixtures must fail tests")]

mod common;

use std::sync::Arc;

use roomkit::RoomKit;
use roomkit::model::{ChannelId, ChannelType, InboundMessage, Metadata, RoomId, TextContent};
use roomkit::testing::{RecordingTransport, RecordingTransportOptions};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{Tool, ToolEntry, ToolError, ToolResult};
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
    let tools: Vec<ToolEntry> = vec![
        ToolEntry::Tool(Arc::new(DiffToolset::new(
            DiffTool::new(root.path()).await.unwrap(),
        ))),
        ToolEntry::Tool(Arc::new(PatchToolset::new(PatchTool::from_workspace(
            MutationWorkspace::new(root.path()).await.unwrap(),
        )))),
    ];
    let agent = common::agent(&id, &model, tools).unwrap();
    let kit = RoomKit::new(roomkit::RoomKitOptions::default());
    if refused {
        common::refuse(&kit, &["patch"], "Patch disabled").unwrap();
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
                TextContent::new("Compare, preview, apply and verify the hours."),
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
        .filter(|message| message.tool_result().is_some())
        .map(|message| {
            let text = message.text();
            serde_json::from_str(&text).unwrap_or(Value::String(text))
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
    kit.close().await.unwrap();
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

fn arguments(input: Value) -> Metadata {
    serde_json::from_value(input).unwrap()
}

#[tokio::test]
async fn each_adapter_serves_only_its_own_tool() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("same"), "x").unwrap();
    let patch = PatchToolset::new(PatchTool::new(root.path()).await.unwrap());
    let diff = DiffToolset::new(DiffTool::new(root.path()).await.unwrap());
    let compare = json!({"old_path":"same","new_path":"same"});
    assert!(matches!(
        patch
            .handler("diff".into(), arguments(compare.clone()))
            .await,
        Err(ToolError::UnservedToolCall(_))
    ));
    assert!(matches!(
        diff.handler("patch".into(), arguments(changes())).await,
        Err(ToolError::UnservedToolCall(_))
    ));
    let Ok(ToolResult::Json(compared)) = diff.handler("diff".into(), arguments(compare)).await
    else {
        panic!("expected JSON")
    };
    assert_eq!(compared["data"]["changed"], false);
}
