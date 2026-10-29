"""Optional adapters for RoomKit Python's public Tool protocol."""

from __future__ import annotations

import json
import os
from typing import Any, ClassVar

from ._native import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    UnsupportedResourceError,
)
from .fs import GlobTool, GrepTool, ListTool, ReadTool


class _ToolAdapter:
    _tool_type: ClassVar[type[ReadTool | ListTool | GlobTool | GrepTool]]

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
        from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError

        if name != self.definition["name"]:
            raise UnservedToolCallError(name)
        try:
            tool = await self._get_tool()
            result = await tool.call(arguments)
        except (InvalidInputError, AccessRefusedError) as error:
            raise ToolRefusedError(str(error)) from error
        except (LimitExceededError, UnsupportedResourceError) as error:
            raise ToolFailedError(str(error)) from error
        return json.dumps(result, ensure_ascii=False)

    async def _get_tool(self) -> ReadTool | ListTool | GlobTool | GrepTool:
        raise NotImplementedError


class ReadToolAdapter(_ToolAdapter):
    """Register an already-created ReadTool with a RoomKit AI channel.

    RoomKit owns schema validation, policies, hooks and turn cancellation.
    I/O errors propagate for RoomKit to report safely; explicit refusals and
    resource failures expose their public messages. No background job is started.
    """

    _tool_type = ReadTool

    def __init__(self, tool: ReadTool) -> None:
        self._tool = tool

    async def _get_tool(self) -> ReadTool:
        return self._tool


class _ConfiguredTool(_ToolAdapter):
    def _configure(self, root: str | os.PathLike[str], **limits: int | None) -> None:
        path = os.fspath(root)
        if not path:
            raise InvalidInputError("root must not be empty")
        self._root = os.path.abspath(path)
        self._limits = limits
        self._resolved: ReadTool | ListTool | GlobTool | GrepTool | None = None

    async def _get_tool(self) -> ReadTool | ListTool | GlobTool | GrepTool:
        if self._resolved is None:
            # Each first caller owns its initialization: cancellation cannot
            # poison another call, and no task/lock is tied to an event loop.
            tool = await self._tool_type.create(self._root, **self._limits)
            self._resolved = tool
        return self._resolved


class Read(_ConfiguredTool):
    """A Rust read tool ready to pass directly to RoomKit's tools list.

    Construction captures an absolute root without opening files. The first
    handler call resolves it and validates limits asynchronously in Rust.
    Initialization failures can be retried. RoomKit is imported only by the
    handler. Use ReadTool for the standalone dictionary-returning API.
    """

    _tool_type = ReadTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        max_file_bytes: int | None = None,
        max_output_bytes: int | None = None,
        max_lines: int | None = None,
    ) -> None:
        self._configure(
            root,
            max_file_bytes=max_file_bytes,
            max_output_bytes=max_output_bytes,
            max_lines=max_lines,
        )


class List(_ConfiguredTool):
    """The Rust list tool, selectable directly in RoomKit's tools list.

    The root and limits are resolved asynchronously on first use, as for Read.
    See ListTool for search behavior and default bounds. Each instance owns
    its configuration; registering it does not expose other tools.
    """

    _tool_type = ListTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        max_entries: int | None = None,
        max_output_bytes: int | None = None,
        max_scan_entries: int | None = None,
    ) -> None:
        self._configure(
            root,
            max_entries=max_entries,
            max_output_bytes=max_output_bytes,
            max_scan_entries=max_scan_entries,
        )


class Glob(_ConfiguredTool):
    """The Rust glob tool, selectable directly in RoomKit's tools list.

    The root and limits are resolved asynchronously on first use, as for Read.
    See GlobTool for search behavior and default bounds. Each instance owns
    its configuration; registering it does not expose other tools.
    """

    _tool_type = GlobTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        max_matches: int | None = None,
        max_output_bytes: int | None = None,
        max_scan_entries: int | None = None,
        max_depth: int | None = None,
    ) -> None:
        self._configure(
            root,
            max_matches=max_matches,
            max_output_bytes=max_output_bytes,
            max_scan_entries=max_scan_entries,
            max_depth=max_depth,
        )


class Grep(_ConfiguredTool):
    """The Rust grep tool, selectable directly in RoomKit's tools list.

    The root and limits are resolved asynchronously on first use, as for Read.
    See GrepTool for search behavior and default bounds. Each instance owns
    its configuration; registering it does not expose other tools.
    """

    _tool_type = GrepTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        max_matches: int | None = None,
        max_output_bytes: int | None = None,
        max_scan_entries: int | None = None,
        max_depth: int | None = None,
        max_file_bytes: int | None = None,
        max_total_bytes: int | None = None,
        max_context_lines: int | None = None,
    ) -> None:
        self._configure(
            root,
            max_matches=max_matches,
            max_output_bytes=max_output_bytes,
            max_scan_entries=max_scan_entries,
            max_depth=max_depth,
            max_file_bytes=max_file_bytes,
            max_total_bytes=max_total_bytes,
            max_context_lines=max_context_lines,
        )
