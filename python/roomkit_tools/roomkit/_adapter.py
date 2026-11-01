"""The shared RoomKit Tool bridge: schema exposure, error mapping and lazy setup."""

from __future__ import annotations

import json
import os
from typing import Any, ClassVar

from .._native import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    UnsupportedResourceError,
)
from ..code import CodeOutlineTool
from ..fs import DiffTool, EditTool, GlobTool, GrepTool, ListTool, PatchTool, ReadTool, WriteTool
from ..image import ViewImageTool
from ..ocr import OcrTool
from ..process import ShellTool
from ..search import WebSearchTool
from ..web import WebFetchTool

type _BoundTool = (
    CodeOutlineTool
    | ViewImageTool
    | ReadTool
    | ListTool
    | GlobTool
    | GrepTool
    | WriteTool
    | EditTool
    | DiffTool
    | PatchTool
    | ShellTool
    | WebFetchTool
    | WebSearchTool
    | OcrTool
)


class _ToolAdapter:
    _tool_type: ClassVar[type[_BoundTool]]

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
            result = await self._call(arguments)
        except (InvalidInputError, AccessRefusedError) as error:
            raise ToolRefusedError(str(error)) from error
        except (LimitExceededError, UnsupportedResourceError) as error:
            raise ToolFailedError(str(error)) from error
        return json.dumps(result, ensure_ascii=False)

    async def _call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        tool = await self._get_tool()
        return await tool.call(arguments)

    async def _get_tool(self) -> _BoundTool:
        raise NotImplementedError


class _ConfiguredTool(_ToolAdapter):
    def _configure(self, root: str | os.PathLike[str], **limits: int | None) -> None:
        path = os.fspath(root)
        if not path:
            raise InvalidInputError("root must not be empty")
        self._root = os.path.abspath(path)
        self._limits = limits
        self._resolved: _BoundTool | None = None

    async def _get_tool(self) -> _BoundTool:
        if self._resolved is None:
            # Each first caller owns its initialization: cancellation cannot
            # poison another call, and no task/lock is tied to an event loop.
            tool = await self._tool_type.create(self._root, **self._limits)
            self._resolved = tool
        return self._resolved
