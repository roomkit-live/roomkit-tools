//! Shared room setup using the pinned engine's public lifecycle API.

use roomkit::model::{ChannelId, RoomId};
use roomkit::{AttachChannelOptions, CreateRoomOptions, RoomKit, RoomKitError};

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
