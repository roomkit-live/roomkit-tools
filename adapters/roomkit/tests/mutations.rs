//! Actual RoomKit turns create, add, edit and remove file-backed orders.

#![expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]

use roomkit::model::{
    ChannelId, ChannelType, EventContent, EventType, InboundMessage, RoomId, ToolCall,
    ToolCallOutcome,
};
use roomkit::testing::RecordingTransport;
use roomkit::{RoomKit, ToolUse};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiAgent, ToolContext, ToolError, ToolOutput, Toolset};
use roomkit_tools_fs::{EditTool, MutationWorkspace, ReadTool, WriteTool};
use roomkit_tools_roomkit::{EditToolset, ReadToolset, WriteToolset};
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
    let workspace = MutationWorkspace::new(root.path()).await.unwrap();
    let model = ScriptedProvider::new(script());
    let id = ChannelId::new("assistant").unwrap();
    let agent = AiAgent::new(id.clone(), model.clone())
        .with_tools(WriteToolset::new(WriteTool::from_workspace(
            workspace.clone(),
        )))
        .unwrap()
        .with_tools(EditToolset::new(EditTool::from_workspace(workspace)))
        .unwrap()
        .with_tools(ReadToolset::new(ReadTool::new(root.path()).await.unwrap()))
        .unwrap();
    let kit = RoomKit::new();
    if block_mutations {
        kit.before_tool_use("deny_mutation", |event, _| async move {
            Ok(if matches!(event.call.name.as_str(), "write" | "edit") {
                ToolUse::block("File mutations are disabled")
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
                EventContent::text(
                    "Create, add, update and remove sample customer orders, then read the file.",
                ),
            ),
        )
        .await
        .unwrap();
    assert!(result.error.is_none(), "{result:?}");
    kit.idle().await;
    let requests = model.requests();
    assert_eq!(requests.len(), 6);
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["write", "edit", "read"]
    );
    let results = requests[5]
        .messages
        .iter()
        .filter(|message| message.tool_call_id.is_some())
        .map(|message| {
            serde_json::from_str(&message.text)
                .unwrap_or_else(|_| Value::String(message.text.clone()))
        })
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
    let contents = std::fs::read_to_string(root.path().join("orders.txt")).ok();
    assert_eq!(
        std::fs::read_dir(root.path()).unwrap().count(),
        usize::from(!block_mutations)
    );
    kit.shutdown().await;
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
    assert_eq!(
        results[4]["text"],
        "1: Orders\n2: 43 | Taylor | 3 notebooks\n"
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

fn context() -> ToolContext {
    ToolContext::new(
        RoomId::new("example").unwrap(),
        ChannelId::new("assistant").unwrap(),
    )
}

fn call(name: &str, input: Value) -> ToolCall {
    ToolCall::new("call", name, serde_json::from_value(input).unwrap())
}

#[tokio::test]
async fn revision_round_trip_refuses_stale_calls_and_accepts_a_fresh_read() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("notes");
    std::fs::write(&path, "count=1\nowner=Alex\n").unwrap();
    let read = ReadToolset::new(ReadTool::new(root.path()).await.unwrap());
    let workspace = MutationWorkspace::new(root.path()).await.unwrap();
    let edit = EditToolset::new(EditTool::from_workspace(workspace.clone()));
    let write = WriteToolset::new(WriteTool::from_workspace(workspace));
    let ToolOutput::Json(original) = read
        .call(call("read", json!({"path":"notes", "limit":1})), context())
        .await
        .unwrap()
    else {
        panic!("expected JSON")
    };
    let revision = &original["data"]["revision"];
    assert!(revision.as_str().unwrap().starts_with("sha256:"));
    assert!(
        edit.tools()[0].parameters["properties"]
            .get("expected_revision")
            .is_some()
    );
    std::fs::write(&path, "count=1\nowner=Drew\n").unwrap();
    let stale_edit = json!({"path":"notes", "old_text":"count=1", "new_text":"count=2", "expected_revision":revision});
    assert!(matches!(
        edit.call(call("edit", stale_edit.clone()), context()).await,
        Err(ToolError::Refused(_))
    ));
    assert!(matches!(write.call(call("write", json!({"path":"notes", "content":"replaced", "overwrite":true, "expected_revision":revision})), context()).await, Err(ToolError::Refused(_))));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "count=1\nowner=Drew\n"
    );
    let ToolOutput::Json(current) = read
        .call(call("read", json!({"path":"notes"})), context())
        .await
        .unwrap()
    else {
        panic!("expected JSON")
    };
    let mut fresh = stale_edit;
    fresh["expected_revision"] = current["data"]["revision"].clone();
    let ToolOutput::Json(changed) = edit.call(call("edit", fresh), context()).await.unwrap() else {
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
async fn adapters_keep_overwrite_and_ambiguity_refusals_and_close_independently() {
    let root = tempfile::tempdir().unwrap();
    let workspace = MutationWorkspace::new(root.path()).await.unwrap();
    let write = WriteToolset::new(WriteTool::from_workspace(workspace.clone()));
    let edit = EditToolset::new(EditTool::from_workspace(workspace));
    assert_eq!(write.tools()[0].name, "write");
    assert_eq!(edit.tools()[0].name, "edit");
    let make_write = || call("write", json!({"path":"orders", "content":"a a"}));
    assert!(matches!(
        write.call(make_write(), context()).await.unwrap(),
        ToolOutput::Json(_)
    ));
    assert!(matches!(
        write.call(make_write(), context()).await,
        Err(ToolError::Refused(_))
    ));
    assert!(matches!(
        edit.call(
            call(
                "edit",
                json!({"path":"orders", "old_text":"a", "new_text":"b"})
            ),
            context()
        )
        .await,
        Err(ToolError::Refused(_))
    ));
    write.close().await;
    assert!(matches!(
        write.call(make_write(), context()).await,
        Err(ToolError::Refused(_))
    ));
    edit.call(
        call(
            "edit",
            json!({"path":"orders", "old_text":"a", "new_text":"b", "replace_all":true}),
        ),
        context(),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("orders")).unwrap(),
        "b b"
    );
    assert!(matches!(
        edit.call(make_write(), context()).await,
        Err(ToolError::Refused(_))
    ));
    edit.close().await;
    assert!(matches!(
        edit.call(
            call(
                "edit",
                json!({"path":"orders", "old_text":"b b", "new_text":""})
            ),
            context()
        )
        .await,
        Err(ToolError::Refused(_))
    ));
}
