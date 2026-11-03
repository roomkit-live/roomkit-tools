//! One tool call through a real kit.
//!
//! Owner-scoped adapters take the room and the acting agent from RoomKit's
//! current tool call, which only RoomKit sets around a call it runs. These
//! helpers run one call of an agent in a room, as an integrator's kit would,
//! and return what the adapter's handler answered.

use std::sync::{Arc, Mutex, PoisonError};

use roomkit::model::{ChannelId, ChannelType, InboundMessage, Metadata, RoomId, TextContent};
use roomkit::testing::{RecordingTransport, RecordingTransportOptions};
use roomkit::{
    AttachChannelOptions, CreateRoomOptions, ProcessInboundOptions, RoomKit, RoomKitOptions,
};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{
    AiChannel, AiChannelOptions, AiTool, Tool, ToolEntry, ToolError, ToolFuture, ToolResult,
};
use serde_json::Value;

/// What a tool's handler answered.
pub type Answer = Result<ToolResult, ToolError>;

/// `tool`, keeping a copy of what its handler answers.
struct Recording<T> {
    tool: T,
    answer: Arc<Mutex<Option<Answer>>>,
}

impl<T: Tool> Tool for Recording<T> {
    fn definition(&self) -> AiTool {
        self.tool.definition()
    }

    fn handler(&self, name: String, arguments: Metadata) -> ToolFuture {
        let call = self.tool.handler(name, arguments);
        let answer = Arc::clone(&self.answer);
        Box::pin(async move {
            let answered = call.await;
            *answer.lock().unwrap_or_else(PoisonError::into_inner) = Some(answered.clone());
            answered
        })
    }
}

/// What `tool`'s handler answered the call of `name` with `arguments`, made
/// by the agent `agent` in the room `room` through a real kit.
#[expect(
    clippy::expect_used,
    reason = "a call RoomKit never served must fail the test"
)]
pub async fn run<T: Tool + Clone>(
    tool: &T,
    room: &str,
    agent: &str,
    name: &str,
    arguments: Value,
) -> Answer {
    turn(
        tool,
        room,
        agent,
        name,
        arguments,
        AiChannelOptions::default(),
    )
    .await
    .expect("RoomKit handed the call to the adapter")
}

/// The call of `name` with `arguments`, made by the agent `agent` in the
/// room `room`, in a kit whose AI channel takes `options` and serves the
/// call with `tool`: the agent's model calls the tool once, then answers.
///
/// Returns what the adapter's handler answered; `None` when RoomKit never
/// let it answer: it refused the call first, or dropped the running call.
#[expect(clippy::expect_used, reason = "a broken fixture must fail the test")]
pub async fn turn<T: Tool + Clone>(
    tool: &T,
    room: &str,
    agent: &str,
    name: &str,
    arguments: Value,
    options: AiChannelOptions,
) -> Option<Answer> {
    let answer = Arc::new(Mutex::new(None));
    let recording = Recording {
        tool: tool.clone(),
        answer: Arc::clone(&answer),
    };
    let arguments = arguments.to_string();
    let model = ScriptedProvider::new([
        ScriptedRound::calling("", [("call", name, arguments.as_str())]),
        ScriptedRound::answering("Done"),
    ]);
    let options = AiChannelOptions {
        tools: vec![ToolEntry::Tool(Arc::new(recording))],
        ..options
    };
    let agent_id = ChannelId::new(agent).expect("valid agent id");
    let channel = AiChannel::new(agent_id.clone(), model.clone(), options).expect("valid channel");
    let kit = RoomKit::new(RoomKitOptions::default());
    kit.register_channel(channel).expect("agent registers");
    ask(&kit, &RoomId::new(room).expect("valid room id"), &agent_id).await;
    kit.close().await.expect("kit closes");
    assert_eq!(
        model.requests().len(),
        2,
        "the model calls the tool, then answers"
    );
    answer.lock().unwrap_or_else(PoisonError::into_inner).take()
}

/// Send one chat message into `room`, where the registered agent `agent`
/// answers it, then wait until `kit` runs nothing more.
#[expect(clippy::expect_used, reason = "a broken fixture must fail the test")]
pub async fn ask(kit: &RoomKit, room: &RoomId, agent: &ChannelId) {
    let chat = RecordingTransport::new(
        ChannelId::new("chat").expect("valid chat id"),
        ChannelType::WEBSOCKET,
        RecordingTransportOptions::default(),
    );
    kit.register_channel(chat.clone()).expect("chat registers");
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await
    .expect("room is created");
    for channel in [chat.channel_id(), agent] {
        kit.attach_channel(room, channel, AttachChannelOptions::default())
            .await
            .expect("room attaches the channel");
    }
    let message = InboundMessage::new(
        chat.channel_id().clone(),
        "alice",
        TextContent::new("Use the tool."),
    );
    let options = ProcessInboundOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    };
    let result = kit
        .process_inbound(message, options)
        .await
        .expect("inbound message is processed");
    assert!(result.error.is_none(), "{result:?}");
    roomkit::testing::idle(kit).await;
}
