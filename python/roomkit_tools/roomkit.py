"""Optional adapters for RoomKit Python's public Tool protocol."""

from __future__ import annotations

import json
import os
from typing import Any

from ._native import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    UnsupportedResourceError,
)
from .fs import ReadTool


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
        definition = ReadTool.definition()
        return {
            "name": definition["name"],
            "description": definition["description"],
            "parameters": definition["input_schema"],
        }

    async def handler(self, name: str, arguments: dict[str, Any]) -> str:
        """Return both structured data and rendered text as a JSON tool result."""
        from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError

        if name != "read":
            raise UnservedToolCallError(name)
        try:
            tool = await self._get_tool()
            result = await tool.call(arguments)
        except (InvalidInputError, AccessRefusedError) as error:
            raise ToolRefusedError(str(error)) from error
        except (LimitExceededError, UnsupportedResourceError) as error:
            raise ToolFailedError(str(error)) from error
        return json.dumps(result, ensure_ascii=False)

    async def _get_tool(self) -> ReadTool:
        return self._tool


class Read(ReadToolAdapter):
    """A Rust read tool ready to pass directly to RoomKit's ``tools`` list.

    ``Read(root="./workspace")`` captures an absolute root without opening files.
    The first handler call resolves it and validates limits asynchronously in
    Rust. Initialization failures leave the object available for another call.
    RoomKit is an optional dependency, imported only when a handler is called.
    Use ``ReadTool`` for the standalone dictionary-returning Python API.
    """

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        max_file_bytes: int | None = None,
        max_output_bytes: int | None = None,
        max_lines: int | None = None,
    ) -> None:
        path = os.fspath(root)
        if not path:
            raise InvalidInputError("root must not be empty")
        self._root = os.path.abspath(path)
        self._max_file_bytes = max_file_bytes
        self._max_output_bytes = max_output_bytes
        self._max_lines = max_lines
        self._resolved: ReadTool | None = None

    async def _get_tool(self) -> ReadTool:
        if self._resolved is None:
            # Each first caller owns its initialization: cancellation cannot
            # poison another call, and no task/lock is tied to an event loop.
            tool = await ReadTool.create(
                self._root,
                max_file_bytes=self._max_file_bytes,
                max_output_bytes=self._max_output_bytes,
                max_lines=self._max_lines,
            )
            self._resolved = tool
        return self._resolved
