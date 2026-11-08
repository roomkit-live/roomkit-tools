//! Actual RoomKit turns create, add, edit and remove file-backed orders.

#![expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]

mod common;

use std::sync::Arc;

use roomkit::RoomKit;
use roomkit::model::{
    ChannelId, ChannelType, EventContent, EventType, InboundMessage, Metadata, RoomId, TextContent,
    ToolCallOutcome,
};
use roomkit::testing::{RecordingTransport, RecordingTransportOptions};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{Tool, ToolEntry, ToolError, ToolResult};
use roomkit_tools_fs::{EditTool, ReadTool, Workspace, WriteTool};
use roomkit_tools_roomkit::{Edit, Read, Write};
use serde_json::{Value, json};

fn script() -> [ScriptedRound; 6] {
    [
        ScriptedRound::calling(
            "",
            [(
                "c1",
                "write",
                r#"{"path":"orders.txt","content":"Orders\n43 | Taylor | 2 notebooks\n"}"#,
            )],
        ),
        ScriptedRound::calling(
            "",
            [(
                "c2",
                "edit",
                r#"{"path":"orders.txt","old_text":"Orders\n","new_text":"Orders\n44 | Alice | 1 pen\n"}"#,
            )],
        ),
        ScriptedRound::calling(
            "",
            [(
                "c3",
                "edit",
                r#"{"path":"orders.txt","old_text":"43 | Taylor | 2 notebooks","new_text":"43 | Taylor | 3 notebooks"}"#,
            )],
        ),
        ScriptedRound::calling(
            "",
            [(
                "c4",
                "edit",
                r#"{"path":"orders.txt","old_text":"44 | Alice | 1 pen\n","new_text":""}"#,
            )],
        ),
        ScriptedRound::calling("", [("c5", "read", r#"{"path":"orders.txt"}"#)]),
        ScriptedRound::answering("Order 43 is for three notebooks; order 44 was removed."),
    ]
}

async fn scenario(block_mutations: bool) -> (Vec<Value>, Vec<ToolCallOutcome>, Option<String>) {
    let root = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(root.path()).await.unwrap();
    let model = ScriptedProvider::new(script());
    let id = ChannelId::new("assistant").unwrap();
    let tools: Vec<ToolEntry> = vec![
        ToolEntry::Tool(Arc::new(Write::new(WriteTool::from_workspace(
            workspace.clone(),
        )))),
        ToolEntry::Tool(Arc::new(Edit::new(EditTool::from_workspace(workspace)))),
        ToolEntry::Tool(Arc::new(Read::new(
            ReadTool::new(root.path()).await.unwrap(),
        ))),
    ];
    let agent = common::agent(&id, &model, tools).unwrap();
    let kit = RoomKit::new(roomkit::RoomKitOptions::default());
    if block_mutations {
        common::refuse(&kit, &["write", "edit"], "File mutations are disabled").unwrap();
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
                "alice",
                TextContent::new(
                    "Create, add, update and remove sample customer orders, then read the file.",
                ),
            ),
            roomkit::ProcessInboundOptions {
                room_id: Some(conversation.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.error.is_none(), "{result:?}");
    roomkit::testing::idle(&kit).await;
    let requests = model.requests();
    assert_eq!(requests.len(), 6);
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        // The kit declares read_stored_result beside an agent's own tools,
        // to page through an outcome it stored (roomkit-rs RMK-1228, as Python's).
        ["write", "edit", "read", "read_stored_result"]
    );
    let results = requests[5]
        .messages
        .iter()
        .filter(|message| message.tool_result().is_some())
        .map(|message| {
            let text = message.text();
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        })
        .collect();
    let outcomes = chat
        .deliveries()
        .iter()
        .filter(|delivery| delivery.event.r#type == EventType::ToolCallEnd)
        .filter_map(|delivery| {
            if let EventContent::ToolCall(call) = &delivery.event.content {
                call.outcome
            } else {
                None
            }
        })
        .collect();
    let contents = std::fs::read_to_string(root.path().join("orders.txt")).ok();
    assert_eq!(
        std::fs::read_dir(root.path()).unwrap().count(),
        usize::from(!block_mutations)
    );
    kit.close().await.unwrap();
    (results, outcomes, contents)
}

#[tokio::test]
async fn agent_mutations_reach_the_file_and_the_model_receives_the_real_results() {
    let (results, outcomes, contents) = scenario(false).await;
    assert_eq!(outcomes, [ToolCallOutcome::Served; 5]);
    assert_eq!(
        contents.as_deref(),
        Some("Orders\n43 | Taylor | 3 notebooks\n")
    );
    assert_eq!(results[0]["data"]["created"], true);
    for result in &results[1..4] {
        assert_eq!(result["data"]["replacements"], 1);
        assert_eq!(result["data"]["changed"], true);
    }
    let read = results[4]["text"].as_str().unwrap();
    assert!(
        read.starts_with("1: Orders\n2: 43 | Taylor | 3 notebooks\n[revision=sha256:"),
        "{read}"
    );
}

#[tokio::test]
async fn roomkit_can_refuse_mutations_before_any_file_is_created() {
    let (results, outcomes, contents) = scenario(true).await;
    assert_eq!(&outcomes[..4], [ToolCallOutcome::Refused; 4]);
    assert!(contents.is_none());
    for result in &results[..4] {
        assert!(result.to_string().contains("File mutations are disabled"));
    }
}

fn arguments(input: Value) -> Metadata {
    serde_json::from_value(input).unwrap()
}

#[tokio::test]
async fn revision_round_trip_refuses_stale_calls_and_accepts_a_fresh_read() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("notes");
    std::fs::write(&path, "count=1\nowner=Alex\n").unwrap();
    let read = Read::new(ReadTool::new(root.path()).await.unwrap());
    let workspace = Workspace::new(root.path()).await.unwrap();
    let edit = Edit::new(EditTool::from_workspace(workspace.clone()));
    let write = Write::new(WriteTool::from_workspace(workspace));
    let ToolResult::Json(original) = read
        .handler("read".into(), arguments(json!({"path":"notes", "limit":1})))
        .await
        .unwrap()
    else {
        panic!("expected JSON")
    };
    let revision = &original["data"]["revision"];
    assert!(revision.as_str().unwrap().starts_with("sha256:"));
    assert!(
        edit.definition().parameters["properties"]
            .get("expected_revision")
            .is_some()
    );
    std::fs::write(&path, "count=1\nowner=Drew\n").unwrap();
    let stale_edit = json!({"path":"notes", "old_text":"count=1", "new_text":"count=2", "expected_revision":revision});
    assert!(matches!(
        edit.handler("edit".into(), arguments(stale_edit.clone()))
            .await,
        Err(ToolError::Refused(_))
    ));
    let stale_write = json!({"path":"notes", "content":"replaced", "overwrite":true, "expected_revision":revision});
    assert!(matches!(
        write.handler("write".into(), arguments(stale_write)).await,
        Err(ToolError::Refused(_))
    ));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "count=1\nowner=Drew\n"
    );
    let ToolResult::Json(current) = read
        .handler("read".into(), arguments(json!({"path":"notes"})))
        .await
        .unwrap()
    else {
        panic!("expected JSON")
    };
    let mut fresh = stale_edit;
    fresh["expected_revision"] = current["data"]["revision"].clone();
    let ToolResult::Json(changed) = edit.handler("edit".into(), arguments(fresh)).await.unwrap()
    else {
        panic!("expected JSON")
    };
    assert_eq!(changed["data"]["changed"], true);
    assert_ne!(changed["data"]["revision"], current["data"]["revision"]);
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "count=2\nowner=Drew\n"
    );
}

#[tokio::test]
async fn adapters_keep_overwrite_and_ambiguity_refusals_and_decline_other_tools() {
    let root = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(root.path()).await.unwrap();
    let write = Write::new(WriteTool::from_workspace(workspace.clone()));
    let edit = Edit::new(EditTool::from_workspace(workspace));
    assert_eq!(write.definition().name, "write");
    assert_eq!(edit.definition().name, "edit");
    let make_write = || arguments(json!({"path":"orders", "content":"a a"}));
    assert!(matches!(
        write.handler("write".into(), make_write()).await,
        Ok(ToolResult::Json(_))
    ));
    assert!(matches!(
        write.handler("write".into(), make_write()).await,
        Err(ToolError::Refused(_))
    ));
    assert!(matches!(
        edit.handler(
            "edit".into(),
            arguments(json!({"path":"orders", "old_text":"a", "new_text":"b"}))
        )
        .await,
        Err(ToolError::Refused(_))
    ));
    edit.handler(
        "edit".into(),
        arguments(json!({"path":"orders", "old_text":"a", "new_text":"b", "replace_all":true})),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("orders")).unwrap(),
        "b b"
    );
    assert!(matches!(
        edit.handler("write".into(), make_write()).await,
        Err(ToolError::UnservedToolCall(_))
    ));
    assert!(matches!(
        write
            .handler(
                "edit".into(),
                arguments(json!({"path":"orders", "old_text":"b b", "new_text":""}))
            )
            .await,
        Err(ToolError::UnservedToolCall(_))
    ));
    assert_eq!(
        std::fs::read_to_string(root.path().join("orders")).unwrap(),
        "b b"
    );
}
