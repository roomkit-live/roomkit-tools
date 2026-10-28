"""Optional adapters for RoomKit Python's public Tool protocol."""

from __future__ import annotations

import json
from typing import Any

from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError

from . import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    ReadTool,
    UnsupportedResourceError,
)


class ReadToolAdapter:
    """Register one configured Rust read tool with a RoomKit AI channel.

    RoomKit owns schema validation, policies, hooks and turn cancellation.
    Cancelling a handler propagates to the native call. I/O errors are raised
    normally for RoomKit to report safely; only explicit refusals and resource
    failures expose their public messages. The adapter starts no background job.
    """

    def __init__(self, tool: ReadTool) -> None:
        self._tool = tool

    @property
    def definition(self) -> dict[str, Any]:
        """Expose the unchanged Rust input schema under RoomKit's parameters key."""
        definition = self._tool.definition()
        return {
            "name": definition["name"],
            "description": definition["description"],
            "parameters": definition["input_schema"],
        }

    async def handler(self, name: str, arguments: dict[str, Any]) -> str:
        """Return both structured data and rendered text as a JSON tool result."""
        if name != "read":
            raise UnservedToolCallError(name)
        try:
            result = await self._tool.call(arguments)
        except (InvalidInputError, AccessRefusedError) as error:
            raise ToolRefusedError(str(error)) from error
        except (LimitExceededError, UnsupportedResourceError) as error:
            raise ToolFailedError(str(error)) from error
        return json.dumps(result, ensure_ascii=False)
