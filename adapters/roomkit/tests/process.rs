//! Process tools use trusted room/agent ownership and real Rust subprocesses.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "fixture errors must fail tests")]

use roomkit::model::{ChannelId, ChannelType, EventContent, InboundMessage, RoomId, ToolCall};
use roomkit::testing::RecordingTransport;
use roomkit::{RoomKit, ToolUse};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiAgent, ToolContext, ToolError, ToolOutput, Toolset};
use roomkit_tools_process::ProcessManager;
use roomkit_tools_roomkit::{
    ProcessReadToolset, ProcessStartToolset, ProcessStatusToolset, ProcessStopToolset,
    ProcessWriteToolset,
};
use serde_json::{Value, json};

fn context(room: &str, agent: &str) -> ToolContext {
    ToolContext::new(RoomId::new(room).unwrap(), ChannelId::new(agent).unwrap())
}

fn call(name: &str, value: Value) -> ToolCall {
    ToolCall::new("call", name, serde_json::from_value(value).unwrap())
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scoped lifecycle exercises all five independently registered adapters"
)]
async fn individual_adapters_share_only_the_trusted_room_and_agent_scope() {
    let fixture = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new(fixture.path()).await.unwrap();
    let start = ProcessStartToolset::new(manager.clone());
    let read = ProcessReadToolset::new(manager.clone());
    let write = ProcessWriteToolset::new(manager.clone());
    let status = ProcessStatusToolset::new(manager.clone());
    let stop = ProcessStopToolset::new(manager);
    let ToolOutput::Json(started) = start
        .call(
            call("process_start", json!({"command":"cat"})),
            context("example", "assistant"),
        )
        .await
        .unwrap()
    else {
        panic!("expected JSON");
    };
    let id = started["data"]["process_id"].as_str().unwrap();
    for (room, agent) in [("other-room", "assistant"), ("example", "other-agent")] {
        assert!(matches!(
            read.call(
                call("process_read", json!({"process_id":id})),
                context(room, agent)
            )
            .await,
            Err(ToolError::Refused(_))
        ));
        assert!(matches!(
            write
                .call(
                    call("process_write", json!({"process_id":id,"text":"bad"})),
                    context(room, agent)
                )
                .await,
            Err(ToolError::Refused(_))
        ));
        assert!(matches!(
            stop.call(
                call("process_stop", json!({"process_id":id})),
                context(room, agent)
            )
            .await,
            Err(ToolError::Refused(_))
        ));
        let ToolOutput::Json(list) = status
            .call(call("process_status", json!({})), context(room, agent))
            .await
            .unwrap()
        else {
            panic!("expected JSON");
        };
        assert_eq!(list["data"], json!([]));
    }
    let ToolOutput::Json(sent) = write
        .call(
            call(
                "process_write",
                json!({"process_id":id,"text":"order 43\n","close_stdin":true}),
            ),
            context("example", "assistant"),
        )
        .await
        .unwrap()
    else {
        panic!("expected JSON");
    };
    assert_eq!(sent["data"]["complete"], true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let ToolOutput::Json(output) = read
                .call(
                    call("process_read", json!({"process_id":id})),
                    context("example", "assistant"),
                )
                .await
                .unwrap()
            else {
                panic!("expected JSON");
            };
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
    let ToolOutput::Json(stopped) = stop
        .call(
            call("process_stop", json!({"process_id":id,"forget":true})),
            context("example", "assistant"),
        )
        .await
        .unwrap()
    else {
        panic!("expected JSON");
    };
    assert_eq!(stopped["data"]["forgotten"], true);
    start.close().await;
    assert!(matches!(
        start
            .call(
                call("process_start", json!({"command":"cat"})),
                context("example", "assistant")
            )
            .await,
        Err(ToolError::Refused(_))
    ));
}

#[tokio::test]
async fn agent_can_start_send_inspect_stop_and_read_a_process_across_calls() {
    let fixture = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new(fixture.path()).await.unwrap();
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
    let agent = AiAgent::new(id.clone(), model.clone())
        .with_tools(ProcessStartToolset::new(manager.clone()))
        .unwrap()
        .with_tools(ProcessStatusToolset::new(manager.clone()))
        .unwrap()
        .with_tools(ProcessWriteToolset::new(manager.clone()))
        .unwrap()
        .with_tools(ProcessReadToolset::new(manager.clone()))
        .unwrap()
        .with_tools(ProcessStopToolset::new(manager))
        .unwrap();
    let kit = RoomKit::new();
    let chat = RecordingTransport::new(ChannelId::new("chat").unwrap(), ChannelType::WEBSOCKET);
    kit.register_channel(chat.clone()).unwrap();
    kit.register_channel(agent).unwrap();
    let room = RoomId::new("example").unwrap();
    kit.create_room(&room).await.unwrap();
    for channel in [chat.id(), &id] {
        kit.attach(&room, channel).await.unwrap();
    }
    for request in [
        "Start the process and send order 43",
        "Stop it and read its output",
    ] {
        let result = kit
            .process_inbound_in(
                &room,
                InboundMessage::new(chat.id().clone(), "test-user", EventContent::text(request)),
            )
            .await
            .unwrap();
        assert!(result.error.is_none());
        kit.idle().await;
    }
    let requests = model.requests();
    assert_eq!(requests.len(), 7);
    assert_eq!(requests[0].tools.len(), 5);
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .find(|t| t.name == "process_start")
            .unwrap()
            .parameters,
        ProcessManager::start_definition().input_schema
    );
    let stopped: Value = serde_json::from_str(
        &requests[5]
            .messages
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("p4"))
            .unwrap()
            .text,
    )
    .unwrap();
    assert_eq!(stopped["data"]["status"]["state"], "finished");
    let read: Value = serde_json::from_str(
        &requests[6]
            .messages
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("p5"))
            .unwrap()
            .text,
    )
    .unwrap();
    assert_eq!(read["data"]["status"]["state"], "finished");
    kit.shutdown().await;
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
    let agent_id = ChannelId::new("assistant").unwrap();
    let agent = AiAgent::new(agent_id.clone(), model.clone())
        .with_tools(ProcessStartToolset::new(
            ProcessManager::new(fixture.path()).await.unwrap(),
        ))
        .unwrap();
    let kit = RoomKit::new();
    kit.before_tool_use("deny_processes", |_, _| async {
        Ok(ToolUse::block("Processes disabled"))
    })
    .unwrap();
    let chat = RecordingTransport::new(ChannelId::new("chat").unwrap(), ChannelType::WEBSOCKET);
    kit.register_channel(chat.clone()).unwrap();
    kit.register_channel(agent).unwrap();
    let room = RoomId::new("example").unwrap();
    kit.create_room(&room).await.unwrap();
    for channel in [chat.id(), &agent_id] {
        kit.attach(&room, channel).await.unwrap();
    }
    kit.process_inbound_in(
        &room,
        InboundMessage::new(
            chat.id().clone(),
            "test-user",
            EventContent::text("Run the command"),
        ),
    )
    .await
    .unwrap();
    kit.idle().await;
    assert!(!fixture.path().join("unexpected").exists());
    assert!(
        model.requests()[1]
            .messages
            .iter()
            .any(|m| m.text.contains("Processes disabled"))
    );
    kit.shutdown().await;
}
