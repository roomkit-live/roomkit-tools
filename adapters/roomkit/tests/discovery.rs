//! Individually selected discovery tools and a complete list/glob/grep/read turn.

#![expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]

use roomkit::model::{
    ChannelId, ChannelType, EventContent, EventType, InboundMessage, RoomId, ToolCall,
    ToolCallOutcome,
};
use roomkit::testing::RecordingTransport;
use roomkit::{RoomKit, ToolUse};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiAgent, ToolContext, ToolError, Toolset};
use roomkit_tools_fs::{GlobTool, GrepTool, ListTool, ReadTool};
use roomkit_tools_roomkit::{GlobToolset, GrepToolset, ListToolset, ReadToolset};
use serde_json::{Value, json};

fn context() -> ToolContext {
    ToolContext::new(
        RoomId::new("example").unwrap(),
        ChannelId::new("assistant").unwrap(),
    )
}

async fn only_this_tool<T: Toolset>(tool: T, name: &str, input: Value) {
    assert_eq!(
        tool.tools()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        [name]
    );
    let make_call = |called| {
        ToolCall::new(
            "call",
            called,
            serde_json::from_value(input.clone()).unwrap(),
        )
    };
    assert!(matches!(
        tool.call(make_call("write"), context()).await,
        Err(ToolError::Refused(_))
    ));
    tool.close().await;
    assert!(matches!(
        tool.call(make_call(name), context()).await,
        Err(ToolError::Refused(_))
    ));
}

#[tokio::test]
async fn each_toolset_exposes_only_its_own_capability_and_closes_independently() {
    let root = tempfile::tempdir().unwrap();
    only_this_tool(
        ListToolset::new(ListTool::new(root.path()).await.unwrap()),
        "list",
        json!({}),
    )
    .await;
    only_this_tool(
        GlobToolset::new(GlobTool::new(root.path()).await.unwrap()),
        "glob",
        json!({"pattern":"*.txt"}),
    )
    .await;
    only_this_tool(
        GrepToolset::new(GrepTool::new(root.path()).await.unwrap()),
        "grep",
        json!({"pattern":"Saturday"}),
    )
    .await;
}

async fn scenario(block_list: bool) -> (Vec<String>, Vec<ToolCallOutcome>) {
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
    let agent = AiAgent::new(id.clone(), model.clone())
        .with_tools(ListToolset::new(ListTool::new(root.path()).await.unwrap()))
        .unwrap()
        .with_tools(GlobToolset::new(GlobTool::new(root.path()).await.unwrap()))
        .unwrap()
        .with_tools(GrepToolset::new(GrepTool::new(root.path()).await.unwrap()))
        .unwrap()
        .with_tools(ReadToolset::new(ReadTool::new(root.path()).await.unwrap()))
        .unwrap();
    let kit = RoomKit::new();
    if block_list {
        kit.before_tool_use("deny_list", |event, _| async move {
            Ok(if event.call.name == "list" {
                ToolUse::block("Listing is disabled")
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
                "alice",
                EventContent::text("Find the hours file and read Saturday's hours."),
            ),
        )
        .await
        .unwrap();
    assert!(result.error.is_none(), "{result:?}");
    kit.idle().await;
    let requests = model.requests();
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["list", "glob", "grep", "read"]
    );
    let results = requests[4]
        .messages
        .iter()
        .filter(|message| message.tool_call_id.is_some())
        .map(|message| message.text.clone())
        .collect();
    let outcomes = chat
        .deliveries()
        .iter()
        .filter(|delivery| delivery.event.event_type == EventType::ToolCallEnd)
        .filter_map(|delivery| {
            if let EventContent::ToolCall(call) = &delivery.event.content {
                call.outcome
            } else {
                None
            }
        })
        .collect();
    assert!(
        chat.texts()
            .contains(&"On Saturday we open from 08:30 to 16:00.".to_owned())
    );
    kit.shutdown().await;
    (results, outcomes)
}

#[tokio::test]
async fn the_same_agent_discovers_searches_and_reads_a_file() {
    let (results, outcomes) = scenario(false).await;
    assert_eq!(outcomes, [ToolCallOutcome::Served; 4]);
    let data: Vec<Value> = results
        .iter()
        .map(|result| serde_json::from_str(result).unwrap())
        .collect();
    assert_eq!(data[0]["data"]["entries"][0]["path"], "info");
    assert_eq!(data[1]["data"]["entries"][0]["path"], "info/hours.txt");
    assert_eq!(data[2]["data"]["matches"][0]["line"], 2);
    assert_eq!(
        data[2]["data"]["matches"][0]["text"],
        "Saturday: 08:30-16:00"
    );
    assert_eq!(data[3]["data"]["lines"][1]["text"], "Saturday: 08:30-16:00");
}

#[tokio::test]
async fn roomkit_can_refuse_one_tool_while_leaving_the_others_available() {
    let (results, outcomes) = scenario(true).await;
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
}
