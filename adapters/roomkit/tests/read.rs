//! The adapter's JSON boundary and RoomKit's real tool loop.

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
use roomkit_tools_fs::{ReadLimits, ReadTool};
use roomkit_tools_roomkit::Read;
use serde_json::{Value, json};

fn arguments(arguments: Value) -> Metadata {
    serde_json::from_value(arguments).unwrap()
}

async fn fixture() -> (tempfile::TempDir, Read) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("hours.txt"),
        "Monday: closed\nTuesday: 08:30-18:00\n",
    )
    .unwrap();
    let tools = Read::new(ReadTool::new(root.path()).await.unwrap());
    (root, tools)
}

#[tokio::test]
async fn exposes_the_derived_schema_and_keeps_pagination_in_the_result() {
    let (_root, tools) = fixture().await;
    let definition = tools.definition();
    assert_eq!(definition.name, "read");
    assert_eq!(definition.parameters, ReadTool::definition().input_schema);
    let output = tools
        .handler(
            "read".into(),
            arguments(json!({"path":"hours.txt", "limit":1})),
        )
        .await
        .unwrap();
    let ToolResult::Json(value) = output else {
        panic!("expected structured output")
    };
    assert_eq!(value["text"], "1: Monday: closed\n");
    assert_eq!(value["data"]["truncated"], true);
    assert_eq!(value["data"]["next_offset"], 2);
}

#[tokio::test]
async fn refuses_invalid_arguments_and_declines_other_tools_even_when_called_directly() {
    let (_root, tools) = fixture().await;
    // Another tool's call is not this adapter's: RoomKit hands it to the next tool.
    assert!(matches!(
        tools
            .handler("write".into(), arguments(json!({"path":"hours.txt"})))
            .await,
        Err(ToolError::UnservedToolCall(_))
    ));
    for input in [
        json!({"path":"hours.txt", "offset":0}),
        json!({"path":"hours.txt", "unexpected":true}),
    ] {
        assert!(matches!(
            tools.handler("read".into(), arguments(input)).await,
            Err(ToolError::Refused(_))
        ));
    }
}

#[tokio::test]
async fn refuses_a_file_outside_the_configured_root() {
    let (_root, tools) = fixture().await;
    let outside = tempfile::NamedTempFile::new().unwrap();
    let result = tools
        .handler("read".into(), arguments(json!({"path":outside.path()})))
        .await;
    assert!(matches!(result, Err(ToolError::Refused(_))));
}

#[tokio::test]
async fn reports_limits_as_safe_failures_and_io_as_diagnostic_errors() {
    let (root, _) = fixture().await;
    let limits = ReadLimits {
        max_file_bytes: 1,
        ..ReadLimits::default()
    };
    let tools = Read::new(
        ReadTool::new(root.path())
            .await
            .unwrap()
            .with_limits(limits)
            .unwrap(),
    );
    let limited = tools
        .handler("read".into(), arguments(json!({"path":"hours.txt"})))
        .await;
    assert!(matches!(limited, Err(ToolError::Failed(_))));
    let missing = tools
        .handler("read".into(), arguments(json!({"path":"missing.txt"})))
        .await;
    assert!(matches!(missing, Err(ToolError::Raised { .. })));
}

/// Run the public RoomKit agent and return the outcome delivered to the chat and what
/// the second model request actually received from the tool.
async fn turn(tools: Read, arguments: &str, blocked: bool) -> (ToolCallOutcome, String) {
    let model = ScriptedProvider::new([
        ScriptedRound::calling("", [("read_hours", "read", arguments)]),
        ScriptedRound::answering("Answer after the tool result."),
    ]);
    let agent_id = ChannelId::new("assistant").unwrap();
    let agent = common::agent(&agent_id, &model, vec![ToolEntry::Tool(Arc::new(tools))]).unwrap();
    let kit = RoomKit::new(roomkit::RoomKitOptions::default());
    if blocked {
        common::refuse(&kit, &["read"], "Reading is disabled").unwrap();
    }
    let chat = RecordingTransport::new(
        ChannelId::new("chat").unwrap(),
        ChannelType::WEBSOCKET,
        RecordingTransportOptions::default(),
    );
    kit.register_channel(chat.clone()).unwrap();
    kit.register_channel(agent).unwrap();
    let room = RoomId::new("example").unwrap();
    common::attach(&kit, &room, [chat.channel_id(), &agent_id])
        .await
        .unwrap();
    let result = kit
        .process_inbound(
            InboundMessage::new(
                chat.channel_id().clone(),
                "alice",
                TextContent::new("When are you open?"),
            ),
            roomkit::ProcessInboundOptions {
                room_id: Some(room.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.error.is_none(), "{result:?}");
    roomkit::testing::idle(&kit).await;
    let deliveries = chat.deliveries();
    let outcome = deliveries
        .iter()
        .find_map(|delivery| {
            let event = &delivery.event;
            if event.r#type != EventType::ToolCallEnd {
                return None;
            }
            if let EventContent::ToolCall(row) = &event.content {
                row.outcome
            } else {
                None
            }
        })
        .unwrap();
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let result = requests[1]
        .messages
        .iter()
        .find(|message| message.tool_result().is_some())
        .unwrap()
        .text();
    assert!(
        chat.texts()
            .contains(&"Answer after the tool result.".to_owned())
    );
    kit.close().await.unwrap();
    (outcome, result)
}

#[tokio::test]
async fn the_agent_receives_file_contents_through_the_real_tool_loop() {
    let (_root, tools) = fixture().await;
    let (outcome, text) = turn(tools, r#"{"path":"hours.txt"}"#, false).await;
    assert_eq!(outcome, ToolCallOutcome::Served);
    let output: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(output["data"]["lines"][1]["text"], "Tuesday: 08:30-18:00");
    assert_eq!(output["data"]["next_offset"], Value::Null);
}

#[tokio::test]
async fn roomkit_can_block_the_call_before_the_adapter_runs() {
    let (_root, tools) = fixture().await;
    let (outcome, text) = turn(tools, r#"{"path":"hours.txt"}"#, true).await;
    // BEFORE_TOOL_USE rejects execution as Refused; Blocked is used when
    // an ON_TOOL_CALL hook withholds an already computed result.
    assert_eq!(outcome, ToolCallOutcome::Refused);
    assert!(text.contains("Reading is disabled"));
    // Had the adapter run, the model would read the file.
    assert!(!text.contains("Monday"));
}

#[tokio::test]
async fn io_diagnostics_do_not_reach_the_model() {
    let (_root, tools) = fixture().await;
    let (outcome, text) = turn(tools, r#"{"path":"private-missing-file.txt"}"#, false).await;
    assert_eq!(outcome, ToolCallOutcome::Failed);
    assert!(!text.contains("private-missing-file"));
    assert!(!text.contains("os error"));
    assert!(text.contains("Tool 'read' failed"));
}
