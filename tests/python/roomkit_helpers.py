"""Call RoomKit tool handlers directly with the context RoomKit's tool loop sets."""

from roomkit.tools.context import ToolCallContext, current_tool_call, tool_turn_context


def in_call(room: str, agent: str):
    """Run handlers as RoomKit runs a tool call of `agent` in `room`."""
    return tool_turn_context(
        room_id=room, actor_id="user", call=ToolCallContext(room_id=room, channel_id=agent)
    )


async def served(handler, name, arguments):
    """What `handler` answers the call of `name` with `arguments`: the text the
    model reads, and the `data` of the call's structured content, which the
    host reads.

    The handler runs within the current RoomKit tool call, or one of its own
    outside any, as RoomKit's tool loop calls it.
    """
    call = current_tool_call()
    if call is None:
        call = ToolCallContext()
        with tool_turn_context(call=call):
            text = await handler(name, arguments)
    else:
        # A copy an earlier call left must not pass for this call's.
        call.structured_content = None
        text = await handler(name, arguments)
    copy = call.structured_content
    assert isinstance(text, str), text
    assert list(copy) == ["data"], copy
    return text, copy["data"]
