//! Individually selected discovery tools and a complete list/glob/grep/read turn.

#![expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]

mod common;
mod served;

use std::sync::Arc;

use roomkit::RoomKit;
use roomkit::model::{
    ChannelId, ChannelType, EventContent, EventType, InboundMessage, Metadata, RoomId, TextContent,
    ToolCallOutcome,
};
use roomkit::testing::{RecordingTransport, RecordingTransportOptions};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{Tool, ToolEntry, ToolError, ToolResult};
use roomkit_tools_fs::{GlobTool, GrepTool, ListTool, ReadTool};
use roomkit_tools_roomkit::{Glob, Grep, List, Read};
use serde_json::{Value, json};

fn arguments(input: &Value) -> Metadata {
    serde_json::from_value(input.clone()).unwrap()
}

async fn only_this_tool<T: Tool>(tool: T, name: &str, input: Value) {
    assert_eq!(tool.definition().name, name);
    assert!(matches!(
        tool.handler("write".into(), arguments(&input)).await,
        Err(ToolError::UnservedToolCall(_))
    ));
    assert!(matches!(
        tool.handler(name.into(), arguments(&input)).await,
        Ok(ToolResult::Text(_))
    ));
}

#[tokio::test]
async fn each_adapter_exposes_only_its_own_capability_and_declines_other_calls() {
    let root = tempfile::tempdir().unwrap();
    only_this_tool(
        List::new(ListTool::new(root.path()).await.unwrap()),
        "list",
        json!({}),
    )
    .await;
    only_this_tool(
        Glob::new(GlobTool::new(root.path()).await.unwrap()),
        "glob",
        json!({"pattern":"*.txt"}),
    )
    .await;
    only_this_tool(
        Grep::new(GrepTool::new(root.path()).await.unwrap()),
        "grep",
        json!({"pattern":"Saturday"}),
    )
    .await;
}

#[tokio::test]
async fn grep_context_reaches_roomkit_without_changing_match_pagination() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("notes"), "first\nhit\nlast\n").unwrap();
    let grep = Grep::new(GrepTool::new(root.path()).await.unwrap());
    assert_eq!(
        grep.definition().parameters,
        GrepTool::definition().input_schema
    );
    let input = json!({"pattern":"hit", "context_before":1, "context_after":1, "limit":1});
    let (text, result) = served::call(&grep, "grep", arguments(&input))
        .await
        .unwrap();
    assert_eq!(
        text,
        ToolResult::Text("\"notes\"-1- first\n\"notes\":2: hit\n\"notes\"-3- last\n".into())
    );
    let found = &result["matches"][0];
    assert_eq!(found["line"], 2);
    assert_eq!(
        found["context_before"][0],
        json!({"number":1, "text":"first"})
    );
    assert_eq!(
        found["context_after"][0],
        json!({"number":3, "text":"last"})
    );
    assert!(result["next_offset"].is_null());
    assert_eq!(result["truncated"], false);
}

/// The outcome of each call `chat` saw end, and the data it received of each
/// (null for none).
fn ended(chat: &RecordingTransport) -> (Vec<ToolCallOutcome>, Vec<Value>) {
    chat.deliveries()
        .iter()
        .filter(|delivery| delivery.event.r#type == EventType::ToolCallEnd)
        .filter_map(|delivery| {
            if let EventContent::ToolCall(call) = &delivery.event.content {
                let copy = call.structured_content.as_ref();
                let data = copy.map_or(Value::Null, |copy| copy["data"].clone());
                Some((call.outcome?, data))
            } else {
                None
            }
        })
        .unzip()
}

/// What the model read of each call, each call's outcome, and the data the
/// chat received of each (null for none).
async fn scenario(block_list: bool) -> (Vec<String>, Vec<ToolCallOutcome>, Vec<Value>) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("info")).unwrap();
    std::fs::write(
        root.path().join("info/hours.txt"),
        "Monday: closed\nSaturday: 08:30-16:00\n",
    )
    .unwrap();
    let model = ScriptedProvider::new([
        ScriptedRound::calling("", [("c1", "list", r#"{"path":"."}"#)]),
        ScriptedRound::calling("", [("c2", "glob", r#"{"pattern":"**/*.txt"}"#)]),
        ScriptedRound::calling(
            "",
            [(
                "c3",
                "grep",
                r#"{"pattern":"Saturday","include":"**/*.txt"}"#,
            )],
        ),
        ScriptedRound::calling("", [("c4", "read", r#"{"path":"info/hours.txt"}"#)]),
        ScriptedRound::answering("On Saturday we open from 08:30 to 16:00."),
    ]);
    let id = ChannelId::new("assistant").unwrap();
    let tools: Vec<ToolEntry> = vec![
        ToolEntry::Tool(Arc::new(List::new(
            ListTool::new(root.path()).await.unwrap(),
        ))),
        ToolEntry::Tool(Arc::new(Glob::new(
            GlobTool::new(root.path()).await.unwrap(),
        ))),
        ToolEntry::Tool(Arc::new(Grep::new(
            GrepTool::new(root.path()).await.unwrap(),
        ))),
        ToolEntry::Tool(Arc::new(Read::new(
            ReadTool::new(root.path()).await.unwrap(),
        ))),
    ];
    let agent = common::agent(&id, &model, tools).unwrap();
    let kit = RoomKit::new(roomkit::RoomKitOptions::default());
    if block_list {
        common::refuse(&kit, &["list"], "Listing is disabled").unwrap();
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
                TextContent::new("Find the hours file and read Saturday's hours."),
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
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        // The kit declares read_stored_result beside an agent's own tools,
        // to page through an outcome it stored (roomkit-rs RMK-1228, as Python's).
        ["list", "glob", "grep", "read", "read_stored_result"]
    );
    let results = requests[4]
        .messages
        .iter()
        .filter(|message| message.tool_result().is_some())
        .map(roomkit_ai::AiMessage::text)
        .collect();
    let (outcomes, data) = ended(&chat);
    assert!(
        chat.texts()
            .contains(&"On Saturday we open from 08:30 to 16:00.".to_owned())
    );
    kit.close().await.unwrap();
    (results, outcomes, data)
}

#[tokio::test]
async fn the_same_agent_discovers_searches_and_reads_a_file() {
    let (results, outcomes, data) = scenario(false).await;
    assert_eq!(outcomes, [ToolCallOutcome::Served; 4]);
    // The model reads each tool's text alone, which names what it found.
    assert_eq!(results[0], "1: directory \"info\"\n");
    assert_eq!(results[1], "1: file \"info/hours.txt\"\n");
    assert_eq!(results[2], "\"info/hours.txt\":2: Saturday: 08:30-16:00\n");
    assert!(
        results[3].starts_with("1: Monday: closed\n2: Saturday: 08:30-16:00\n"),
        "{}",
        results[3]
    );
    // The chat gets each call's data.
    assert_eq!(data[0]["entries"][0]["path"], "info");
    assert_eq!(data[1]["entries"][0]["path"], "info/hours.txt");
    assert_eq!(data[2]["matches"][0]["line"], 2);
    assert_eq!(data[2]["matches"][0]["text"], "Saturday: 08:30-16:00");
    assert_eq!(data[3]["lines"][1]["text"], "Saturday: 08:30-16:00");
}

#[tokio::test]
async fn roomkit_can_refuse_one_tool_while_leaving_the_others_available() {
    let (results, outcomes, data) = scenario(true).await;
    assert_eq!(
        outcomes,
        [
            ToolCallOutcome::Refused,
            ToolCallOutcome::Served,
            ToolCallOutcome::Served,
            ToolCallOutcome::Served
        ]
    );
    assert!(results[0].contains("Listing is disabled"));
    assert!(!results[0].contains("hours.txt"));
    // A refused call hands the chat no data.
    assert_eq!(data[0], Value::Null);
}
