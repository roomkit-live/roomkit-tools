//! Shared room setup using the pinned engine's public lifecycle API.

use roomkit::model::hook::{AllowOptions, BlockOptions};
use roomkit::model::{ChannelId, HookResult, RoomId};
use roomkit::{
    AttachChannelOptions, CreateRoomOptions, HookOptions, HookTrigger, RoomKit, RoomKitError,
    ToolCallEvent,
};
use roomkit_ai::testing::ScriptedProvider;
use roomkit_ai::{AiChannel, AiChannelOptions, AiError, ToolEntry};

pub async fn attach(
    kit: &RoomKit,
    room: &RoomId,
    channels: [&ChannelId; 2],
) -> Result<(), RoomKitError> {
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await?;
    for channel in channels {
        kit.attach_channel(room, channel, AttachChannelOptions::default())
            .await?;
    }
    Ok(())
}

/// The AI agent `id`, answering from `model` and declaring `tools`.
pub fn agent(
    id: &ChannelId,
    model: &ScriptedProvider,
    tools: Vec<ToolEntry>,
) -> Result<AiChannel<ScriptedProvider>, AiError> {
    let options = AiChannelOptions {
        tools,
        ..Default::default()
    };
    AiChannel::new(id.clone(), model.clone(), options)
}

/// Refuse every call of the tools `names` before it runs, saying `reason`,
/// as an integrator's `BEFORE_TOOL_USE` policy does; other calls run.
pub fn refuse(
    kit: &RoomKit,
    names: &'static [&'static str],
    reason: &'static str,
) -> Result<(), RoomKitError> {
    kit.hook(
        HookTrigger::BeforeToolUse,
        move |event: ToolCallEvent, _context| async move {
            if names.contains(&event.name.as_str()) {
                HookResult::block(reason, BlockOptions::default())
            } else {
                HookResult::allow(AllowOptions::default())
            }
        },
        HookOptions {
            name: "refuse".into(),
            ..Default::default()
        },
    )
}
