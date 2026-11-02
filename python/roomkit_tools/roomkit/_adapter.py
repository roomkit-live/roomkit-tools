"""The shared RoomKit Tool bridge: schema exposure, error mapping and trusted callers."""

from __future__ import annotations

import json
from collections.abc import Awaitable
from typing import Any, ClassVar

from .._native import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    UnsupportedResourceError,
)


class _ToolAdapter[T]:
    """A RoomKit tool over a built tool `T`, the class's `_tool_type`."""

    _tool_type: ClassVar[type[Any]]

    def __init__(self, tool: T) -> None:
        if not isinstance(tool, self._tool_type):
            raise TypeError(f"{type(self).__name__} requires a {self._tool_type.__name__}")
        self._tool = tool

    @property
    def definition(self) -> dict[str, Any]:
        """Expose the unchanged Rust input schema under RoomKit's parameters key."""
        definition = self._tool_type.definition()
        return {
            "name": definition["name"],
            "description": definition["description"],
            "parameters": definition["input_schema"],
        }

    async def handler(self, name: str, arguments: dict[str, Any]) -> str:
        """Return data and text, preserving refusal, failure and cancellation."""
        from roomkit.core.exceptions import UnservedToolCallError

        if name != self.definition["name"]:
            raise UnservedToolCallError(name)
        result = await _mapped(self._call(arguments))
        return json.dumps(result, ensure_ascii=False)

    async def _call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        return await self._tool.call(arguments)


async def _mapped[T](call: Awaitable[T]) -> T:
    """Explicit refusals and resource failures as RoomKit's, with their public
    messages; I/O and other tool errors propagate for RoomKit to report."""
    from roomkit.core.exceptions import ToolFailedError, ToolRefusedError

    try:
        return await call
    except (InvalidInputError, AccessRefusedError) as error:
        raise ToolRefusedError(str(error)) from error
    except (LimitExceededError, UnsupportedResourceError) as error:
        raise ToolFailedError(str(error)) from error


def _trusted_caller(tools: str) -> tuple[str, str]:
    """The room and acting agent of the current RoomKit tool call.

    Model arguments never select them. Outside a RoomKit tool call, refuse.
    """
    from roomkit.core.exceptions import ToolRefusedError
    from roomkit.tools.context import current_tool_call, current_tool_room_id

    room, call = current_tool_room_id(), current_tool_call()
    actor = call.channel_id if call is not None else None
    if not room or not actor:
        raise ToolRefusedError(f"{tools} tools require a conversation and an acting agent")
    return room, actor


def _structured(result: dict[str, Any]) -> str:
    """Hand RoomKit's current call the structured result; return its text."""
    from roomkit.tools.context import current_tool_call

    context = current_tool_call()
    if context is not None:
        context.structured_content = result
    return json.dumps(result, ensure_ascii=False)
