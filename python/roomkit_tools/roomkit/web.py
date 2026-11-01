"""RoomKit tools that fetch and search the web."""

from __future__ import annotations

from typing import Any

from ..search import SearchBackend, WebSearchTool
from ..web import WebFetchTool
from ._adapter import _ToolAdapter


class WebFetch(_ToolAdapter):
    """Select HTTP fetching independently, with host-configured origin rules.

    Keyword arguments are the same as WebFetchTool. Construction performs no I/O.
    """

    _tool_type = WebFetchTool

    def __init__(self, **configuration: Any) -> None:
        self._tool = WebFetchTool(**configuration)

    async def _get_tool(self) -> WebFetchTool:
        return self._tool


class WebSearch(_ToolAdapter):
    """Pass WebSearch(backend=ExaSearch()) directly to RoomKit's tools list."""

    _tool_type = WebSearchTool

    def __init__(self, *, backend: SearchBackend, **configuration: Any) -> None:
        self._tool = WebSearchTool(backend=backend, **configuration)

    async def _get_tool(self) -> WebSearchTool:
        return self._tool
