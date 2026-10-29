//! The adapter's JSON boundary and RoomKit's real tool loop.

#![expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]

mod common;

use roomkit::model::{
    ChannelId, ChannelType, EventContent, EventType, InboundMessage, RoomId, ToolCall,
    ToolCallOutcome,
};
use roomkit::testing::RecordingTransport;
use roomkit::{RoomKit, ToolUse};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiAgent, ToolContext, ToolError, ToolOutput, Toolset};
use roomkit_tools_fs::{ReadLimits, ReadTool};
use roomkit_tools_roomkit::ReadToolset;
use serde_json::{Value, json};

fn context() -> ToolContext {
    ToolContext::new(
        RoomId::new("example").unwrap(),
        ChannelId::new("assistant").unwrap(),
    )
}

fn call(name: &str, arguments: Value) -> ToolCall {
    ToolCall::new(
        "read_hours",
        name,
        serde_json::from_value(arguments).unwrap(),
    )
}

async fn fixture() -> (tempfile::TempDir, ReadToolset) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("hours.txt"),
        "Monday: closed\nTuesday: 08:30-18:00\n",
    )
    .unwrap();
    let tools = ReadToolset::new(ReadTool::new(root.path()).await.unwrap());
    (root, tools)
}

#[tokio::test]
async fn exposes_the_derived_schema_and_keeps_pagination_in_the_result() {
    let (_root, tools) = fixture().await;
    let definitions = tools.tools();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name, "read");
    assert_eq!(
        definitions[0].parameters,
        ReadTool::definition().input_schema
    );
    let output = tools
        .call(
            call("read", json!({"path":"hours.txt", "limit":1})),
            context(),
        )
        .await
        .unwrap();
    let ToolOutput::Json(value) = output else {
        panic!("expected structured output")
    };
    assert_eq!(value["text"], "1: Monday: closed\n");
    assert_eq!(value["data"]["truncated"], true);
    assert_eq!(value["data"]["next_offset"], 2);
}

#[tokio::test]
async fn refuses_invalid_arguments_and_unknown_tools_even_when_called_directly() {
    let (_root, tools) = fixture().await;
    for call in [
        call("write", json!({"path":"hours.txt"})),
        call("read", json!({"path":"hours.txt", "offset":0})),
        call("read", json!({"path":"hours.txt", "unexpected":true})),
    ] {
        assert!(matches!(
            tools.call(call, context()).await,
            Err(ToolError::Refused(_))
        ));
    }
}

#[tokio::test]
async fn refuses_a_file_outside_the_configured_root() {
    let (_root, tools) = fixture().await;
    let outside = tempfile::NamedTempFile::new().unwrap();
    let result = tools
        .call(call("read", json!({"path":outside.path()})), context())
        .await;
    assert!(matches!(result, Err(ToolError::Refused(_))));
}

#[tokio::test]
async fn reports_limits_as_safe_failures_and_io_as_diagnostic_errors() {
    let (root, _) = fixture().await;
    let mut limits = ReadLimits::default();
    limits.max_file_bytes = 1;
    let tools = ReadToolset::new(
        ReadTool::new(root.path())
            .await
            .unwrap()
            .with_limits(limits)
            .unwrap(),
    );
    let limited = tools
        .call(call("read", json!({"path":"hours.txt"})), context())
        .await;
    assert!(matches!(limited, Err(ToolError::Failed(_))));
    let missing = tools
        .call(call("read", json!({"path":"missing.txt"})), context())
        .await;
    assert!(matches!(missing, Err(ToolError::Raised { .. })));
}

#[tokio::test]
async fn closing_is_idempotent_and_prevents_subsequent_calls() {
    let (_root, tools) = fixture().await;
    tools.close().await;
    tools.close().await;
    let result = tools
        .call(call("read", json!({"path":"hours.txt"})), context())
        .await;
    assert!(matches!(result, Err(ToolError::Refused(_))));
}

/// Run the public RoomKit agent and return the outcome delivered to the chat and what
/// the second model request actually received from the tool.
async fn turn(tools: ReadToolset, arguments: &str, blocked: bool) -> (ToolCallOutcome, String) {
    let model = ScriptedProvider::new([
        ScriptedRound::calling("", [("read_hours", "read", arguments)]),
        ScriptedRound::answering("Answer after the tool result."),
    ]);
    let agent_id = ChannelId::new("assistant").unwrap();
    let agent = AiAgent::new(agent_id.clone(), model.clone())
        .with_tools(tools)
        .unwrap();
    let kit = RoomKit::new(roomkit::RoomKitOptions::default());
    if blocked {
        kit.before_tool_use("deny_read", |_, _| async {
            Ok(ToolUse::block("Reading is disabled"))
        })
        .unwrap();
    }
    let chat = RecordingTransport::new(ChannelId::new("chat").unwrap(), ChannelType::WEBSOCKET);
    kit.register_channel(chat.clone()).unwrap();
    kit.register_channel(agent).unwrap();
    let room = RoomId::new("example").unwrap();
    common::attach(&kit, &room, [chat.id(), &agent_id])
        .await
        .unwrap();
    let result = kit
        .process_inbound(
            InboundMessage::new(
                chat.id().clone(),
                "alice",
                EventContent::text("When are you open?"),
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
            if event.event_type != EventType::ToolCallEnd {
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
        .find(|message| message.tool_call_id.is_some())
        .unwrap()
        .text
        .clone();
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
    // This closed toolset would refuse execution; the hook must run first.
    tools.close().await;
    let (outcome, text) = turn(tools, r#"{"path":"hours.txt"}"#, true).await;
    // BEFORE_TOOL_USE rejects execution as Refused; Blocked is used when
    // an ON_TOOL_CALL hook withholds an already computed result.
    assert_eq!(outcome, ToolCallOutcome::Refused);
    assert!(text.contains("Reading is disabled"));
    assert!(!text.contains("closed"));
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
