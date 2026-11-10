//! Process tools use trusted room/agent ownership and real Rust subprocesses.
#![cfg(unix)]

mod call;
mod common;

use std::sync::Arc;

use roomkit::RoomKit;
use roomkit::model::{ChannelId, ChannelType, InboundMessage, RoomId, TextContent};
use roomkit::testing::{RecordingTransport, RecordingTransportOptions};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{ToolEntry, ToolError, ToolResult};
use roomkit_tools_process::ProcessSession;
use roomkit_tools_roomkit::{ProcessRead, ProcessStart, ProcessStatus, ProcessStop, ProcessWrite};
use serde_json::{Value, json};

/// The JSON a served call answered.
#[expect(clippy::panic, reason = "an answer other than JSON must fail the test")]
fn json_answer(answer: Result<ToolResult, ToolError>) -> Value {
    let Ok(ToolResult::Json(value)) = answer else {
        panic!("expected JSON, got {answer:?}");
    };
    value
}

#[tokio::test]
async fn individual_adapters_share_only_the_trusted_room_and_agent_scope() {
    let fixture = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(fixture.path()).await.unwrap();
    let start = ProcessStart::new(session.clone());
    let read = ProcessRead::new(session.clone());
    let write = ProcessWrite::new(session.clone());
    let status = ProcessStatus::new(session.clone());
    let stop = ProcessStop::new(session.clone());
    let (room, agent) = ("example", "assistant");
    let started = json_answer(
        call::run(
            &start,
            room,
            agent,
            "process_start",
            json!({"command":"cat"}),
        )
        .await,
    );
    let id = started["data"]["process_id"].as_str().unwrap();
    for (other_room, other_agent) in [("other-room", "assistant"), ("example", "other-agent")] {
        let process = json!({"process_id":id});
        assert!(matches!(
            call::run(
                &read,
                other_room,
                other_agent,
                "process_read",
                process.clone()
            )
            .await,
            Err(ToolError::Refused(_))
        ));
        assert!(matches!(
            call::run(
                &write,
                other_room,
                other_agent,
                "process_write",
                json!({"process_id":id,"text":"bad"})
            )
            .await,
            Err(ToolError::Refused(_))
        ));
        assert!(matches!(
            call::run(&stop, other_room, other_agent, "process_stop", process).await,
            Err(ToolError::Refused(_))
        ));
        let list = json_answer(
            call::run(
                &status,
                other_room,
                other_agent,
                "process_status",
                json!({}),
            )
            .await,
        );
        assert_eq!(list["data"], json!({"processes": []}));
    }
    let sent = json_answer(
        call::run(
            &write,
            room,
            agent,
            "process_write",
            json!({"process_id":id,"text":"order 43\n","close_stdin":true}),
        )
        .await,
    );
    assert_eq!(sent["data"]["complete"], true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let output = json_answer(
                call::run(&read, room, agent, "process_read", json!({"process_id":id})).await,
            );
            if output["data"]["status"]["state"] == "finished" {
                return output;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(output["data"]["stdout"]["text"], "order 43\n");
    assert_eq!(output["data"]["status"]["exit_code"], 0);
    let stopped = json_answer(
        call::run(
            &stop,
            room,
            agent,
            "process_stop",
            json!({"process_id":id,"forget":true}),
        )
        .await,
    );
    assert_eq!(stopped["data"]["forgotten"], true);
    // The host, not the adapters, closes the shared session.
    assert!(session.close().await);
}

#[tokio::test]
async fn agent_can_start_send_inspect_stop_and_read_a_process_across_calls() {
    let fixture = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(fixture.path()).await.unwrap();
    let model = ScriptedProvider::new([
        ScriptedRound::calling("", [("p1", "process_start", r#"{"command":"cat"}"#)]),
        ScriptedRound::calling(
            "",
            [(
                "p2",
                "process_write",
                r#"{"process_id":"p1","text":"order 43\n"}"#,
            )],
        ),
        ScriptedRound::calling("", [("p3", "process_status", "{}")]),
        ScriptedRound::answering("The process is running."),
        ScriptedRound::calling("", [("p4", "process_stop", r#"{"process_id":"p1"}"#)]),
        ScriptedRound::calling("", [("p5", "process_read", r#"{"process_id":"p1"}"#)]),
        ScriptedRound::answering("The process is stopped."),
    ]);
    let id = ChannelId::new("assistant").unwrap();
    let tools: Vec<ToolEntry> = vec![
        ToolEntry::Tool(Arc::new(ProcessStart::new(session.clone()))),
        ToolEntry::Tool(Arc::new(ProcessStatus::new(session.clone()))),
        ToolEntry::Tool(Arc::new(ProcessWrite::new(session.clone()))),
        ToolEntry::Tool(Arc::new(ProcessRead::new(session.clone()))),
        ToolEntry::Tool(Arc::new(ProcessStop::new(session.clone()))),
    ];
    let agent = common::agent(&id, &model, tools).unwrap();
    let kit = RoomKit::new(roomkit::RoomKitOptions::default());
    let chat = RecordingTransport::new(
        ChannelId::new("chat").unwrap(),
        ChannelType::WEBSOCKET,
        RecordingTransportOptions::default(),
    );
    kit.register_channel(chat.clone()).unwrap();
    kit.register_channel(agent).unwrap();
    let room = RoomId::new("example").unwrap();
    common::attach(&kit, &room, [chat.channel_id(), &id])
        .await
        .unwrap();
    for request in [
        "Start the process and send order 43",
        "Stop it and read its output",
    ] {
        let result = kit
            .process_inbound(
                InboundMessage::new(
                    chat.channel_id().clone(),
                    "test-user",
                    TextContent::new(request),
                ),
                roomkit::ProcessInboundOptions {
                    room_id: Some(room.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(result.error.is_none());
        roomkit::testing::idle(&kit).await;
    }
    let requests = model.requests();
    assert_eq!(requests.len(), 7);
    // The kit declares read_stored_result beside an agent's own tools, to
    // page through an outcome it stored (roomkit-rs RMK-1228, as Python's).
    assert_eq!(requests[0].tools.len(), 6);
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .find(|t| t.name == "process_start")
            .unwrap()
            .parameters,
        ProcessSession::start_definition().input_schema
    );
    let result_of = |request: usize, call: &str| -> Value {
        let message = requests[request]
            .messages
            .iter()
            .find(|m| {
                m.tool_result()
                    .is_some_and(|result| result.tool_call_id == call)
            })
            .unwrap();
        serde_json::from_str(&message.text()).unwrap()
    };
    let stopped = result_of(5, "p4");
    assert_eq!(stopped["data"]["status"]["state"], "finished");
    let read = result_of(6, "p5");
    assert_eq!(read["data"]["status"]["state"], "finished");
    kit.close().await.unwrap();
    assert!(session.close().await);
}

#[tokio::test]
async fn roomkit_policy_can_refuse_spawn_before_any_command_effect() {
    let fixture = tempfile::tempdir().unwrap();
    let model = ScriptedProvider::new([
        ScriptedRound::calling(
            "",
            [(
                "spawn",
                "process_start",
                r#"{"command":"touch unexpected"}"#,
            )],
        ),
        ScriptedRound::answering("The command was refused."),
    ]);
    let session = ProcessSession::new(fixture.path()).await.unwrap();
    let agent_id = ChannelId::new("assistant").unwrap();
    let start = ToolEntry::Tool(Arc::new(ProcessStart::new(session.clone())));
    let agent = common::agent(&agent_id, &model, vec![start]).unwrap();
    let kit = RoomKit::new(roomkit::RoomKitOptions::default());
    common::refuse(&kit, &["process_start"], "Processes disabled").unwrap();
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
    kit.process_inbound(
        InboundMessage::new(
            chat.channel_id().clone(),
            "test-user",
            TextContent::new("Run the command"),
        ),
        roomkit::ProcessInboundOptions {
            room_id: Some(room.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    roomkit::testing::idle(&kit).await;
    assert!(!fixture.path().join("unexpected").exists());
    assert!(
        model.requests()[1]
            .messages
            .iter()
            .any(|m| m.tool_result().is_some() && m.text().contains("Processes disabled"))
    );
    kit.close().await.unwrap();
    assert!(session.close().await);
}

/// An agent awaits a process in one `process_read` with `wait_ms`, and reads
/// what it wrote.
#[tokio::test]
async fn an_agent_awaits_a_process_in_one_read() {
    let fixture = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(fixture.path()).await.unwrap();
    let model = ScriptedProvider::new([
        ScriptedRound::calling(
            "",
            [("p1", "process_start", r#"{"command":"sleep 1; echo fini"}"#)],
        ),
        ScriptedRound::calling(
            "",
            [(
                "p2",
                "process_read",
                r#"{"process_id":"p1","wait_ms":15000}"#,
            )],
        ),
        ScriptedRound::answering("The build printed fini."),
    ]);
    let id = ChannelId::new("assistant").unwrap();
    let tools: Vec<ToolEntry> = vec![
        ToolEntry::Tool(Arc::new(ProcessStart::new(session.clone()))),
        ToolEntry::Tool(Arc::new(ProcessRead::new(session.clone()))),
    ];
    let agent = common::agent(&id, &model, tools).unwrap();
    let kit = RoomKit::new(roomkit::RoomKitOptions::default());
    let chat = RecordingTransport::new(
        ChannelId::new("chat").unwrap(),
        ChannelType::WEBSOCKET,
        RecordingTransportOptions::default(),
    );
    kit.register_channel(chat.clone()).unwrap();
    kit.register_channel(agent).unwrap();
    let room = RoomId::new("example").unwrap();
    common::attach(&kit, &room, [chat.channel_id(), &id])
        .await
        .unwrap();
    let result = kit
        .process_inbound(
            InboundMessage::new(
                chat.channel_id().clone(),
                "test-user",
                TextContent::new("Run the build and tell me what it prints"),
            ),
            roomkit::ProcessInboundOptions {
                room_id: Some(room.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.error.is_none());
    roomkit::testing::idle(&kit).await;
    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    let read = requests[2]
        .messages
        .iter()
        .find(|m| m.tool_result().is_some_and(|r| r.tool_call_id == "p2"))
        .unwrap();
    let read: Value = serde_json::from_str(&read.text()).unwrap();
    assert_eq!(read["data"]["stdout"]["text"], "fini\n");
    kit.close().await.unwrap();
    assert!(session.close().await);
}
