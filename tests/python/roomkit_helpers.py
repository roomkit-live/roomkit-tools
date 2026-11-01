"""Call RoomKit tool handlers directly with the context RoomKit's tool loop sets."""

from roomkit.tools.context import ToolCallContext, tool_turn_context


def in_call(room: str, agent: str):
    """Run handlers as RoomKit runs a tool call of `agent` in `room`."""
    return tool_turn_context(
        room_id=room, actor_id="user", call=ToolCallContext(room_id=room, channel_id=agent)
    )
